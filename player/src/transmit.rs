use std::collections::VecDeque;
use std::error::Error;

use opus::Encoder;
use rubato::{InterpolationParameters, Resampler, SincFixedOut};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, StandardTagKey};
use symphonia::core::probe::{Hint, ProbeResult};
use tokio::time::Interval;

use protocol::{AudioData, Message};
use protocol::{AudioFrame, TrackMetadata};

pub struct AudioInfoReader {
    probe_result: ProbeResult,
    decoder: Box<dyn Decoder>,
}

impl AudioInfoReader {
    pub fn load(path: &str) -> Result<Self, Box<dyn Error>> {
        let src = std::fs::File::open(path)?;

        let mss = MediaSourceStream::new(Box::new(src), Default::default());

        let hint = Hint::new();

        let probe_result = symphonia::default::get_probe()
            .format(
                &hint,
                mss,
                &FormatOptions {
                    enable_gapless: true,
                    ..FormatOptions::default()
                },
                &MetadataOptions::default(),
            )
            .unwrap();

        let decoder = symphonia::default::get_codecs()
            .make(
                &probe_result
                    .format
                    .default_track()
                    .expect("uhhhh")
                    .codec_params,
                &DecoderOptions::default(),
            )
            .unwrap();

        Ok(AudioInfoReader {
            probe_result,
            decoder,
        })
    }

    pub fn read_info(&mut self) -> Result<(u32, usize, TrackMetadata), Box<dyn Error>> {
        // read 1 packet to check the sample rate and channel count
        let packet = self.probe_result.format.next_packet()?;
        let decoded = self.decoder.decode(&packet)?;
        let spec = *decoded.spec();

        let duration = self
            .probe_result
            .format
            .default_track()
            .unwrap()
            .codec_params
            .n_frames
            .unwrap_or(0);

        // seek back to start since we consumed packets
        self.probe_result.format.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time: symphonia::core::units::Time::new(0, 0.0),
                track_id: None,
            },
        )?;
        self.decoder.reset();

        // read track metadata
        let mut track_md = TrackMetadata::default();

        track_md.duration = (duration as f64 / spec.rate as f64) as usize;

        let mut md = self.probe_result.format.metadata();
        if let Some(current) = md.skip_to_latest() {
            let tags = current.tags();
            Self::fill_metadata(&mut track_md, tags);
        } else if let Some(mut md) = self.probe_result.metadata.get() {
            let tags = md.skip_to_latest().unwrap().tags();
            Self::fill_metadata(&mut track_md, tags);
        } else {
            println!("really no metadata to speak of...");
        }

        Ok((spec.rate, spec.channels.count(), track_md))
    }

    fn fill_metadata(md: &mut TrackMetadata, tags: &[symphonia::core::meta::Tag]) {
        for tag in tags {
            match tag.std_key {
                Some(StandardTagKey::TrackTitle) => {
                    md.title = Some(tag.value.to_string());
                }
                Some(StandardTagKey::Album) => {
                    md.album = Some(tag.value.to_string());
                }
                Some(StandardTagKey::Artist) => {
                    md.artist = Some(tag.value.to_string());
                }
                Some(StandardTagKey::TrackNumber) => {
                    md.track_no = Some(tag.value.to_string());
                }
                Some(StandardTagKey::AlbumArtist) => {
                    md.album_artist = Some(tag.value.to_string());
                }

                _ => {}
            }
        }
    }
}

pub struct AudioReader {
    inner: AudioInfoReader,

    resampler: SincFixedOut<f32>,
    encoder: Encoder,

    source_buffer: Vec<VecDeque<f32>>,

    position: usize,
    finished: bool,

    sample_rate: u32,
    channel_count: usize,
}

impl AudioReader {
    pub fn load(path: &str) -> Result<Self, Box<dyn Error>> {
        let mut info_reader = AudioInfoReader::load(path)?;

        let (sample_rate, channel_count, _) = info_reader.read_info()?;

        let resampler = {
            let params = InterpolationParameters {
                sinc_len: 256,
                f_cutoff: 0.95,
                interpolation: rubato::InterpolationType::Linear,
                oversampling_factor: 256,
                window: rubato::WindowFunction::BlackmanHarris2,
            };
            SincFixedOut::<f32>::new(
                48000 as f64 / sample_rate as f64,
                1.0,
                params,
                480,
                channel_count,
            )
            .unwrap()
        };

        let mut encoder =
            opus::Encoder::new(48000, opus::Channels::Stereo, opus::Application::Audio).unwrap();
        encoder.set_bitrate(opus::Bitrate::Bits(256000)).unwrap();

        Ok(Self {
            inner: info_reader,

            resampler,
            encoder,

            source_buffer: vec![VecDeque::new(); channel_count],

            position: 0,
            finished: false,

            sample_rate,
            channel_count,
        })
    }

    // sets self.finished if the end was reached
    fn read_more(&mut self) -> Result<(), Box<dyn Error>> {
        match self.inner.probe_result.format.next_packet() {
            Ok(packet) => {
                let decoded = self.inner.decoder.decode(&packet).unwrap();
                let spec = *decoded.spec();

                if decoded.frames() > 0 {
                    let mut sample_buffer: SampleBuffer<f32> =
                        SampleBuffer::new(decoded.frames() as u64, spec);

                    sample_buffer.copy_interleaved_ref(decoded);

                    let samples = sample_buffer.samples();

                    for frame in samples.chunks(spec.channels.count()) {
                        for (chan, sample) in frame.iter().enumerate() {
                            self.source_buffer[chan].push_back(*sample)
                        }
                    }
                } else {
                    eprintln!("Empty packet encountered while loading song!");
                }
            }
            Err(SymphoniaError::IoError(_)) => {
                // no more packets
                self.finished = true;
            }
            Err(e) => return Err("failed to parse track".into()),
        }

        Ok(())
    }

    fn resample_more(&mut self) -> Result<Vec<f32>, Box<dyn Error>> {
        let samples_needed = if self.sample_rate != 48000 {
            self.resampler.input_frames_next()
        } else {
            480 as usize
        };

        while self.source_buffer[0].len() < samples_needed {
            if self.finished {
                // if no more packets to read, pad with 0s until we reach input_frames_next
                for chan in self.source_buffer.iter_mut() {
                    //println!("filling with 0s!!!");
                    chan.push_back(0.0);
                }
            } else {
                self.read_more()?;
            }
        }

        let mut chunk_samples: Vec<Vec<f32>> = vec![Vec::new(); self.channel_count];

        for (chan_idx, chan) in self.source_buffer.iter_mut().enumerate() {
            for _ in 0..samples_needed {
                // maybe there's a better way to do this?
                chunk_samples[chan_idx].push(chan.pop_front().unwrap());
            }
        }

        // resample to standard 48000 if needed
        let samples_correct_rate = if self.sample_rate != 48000 {
            self.resampler.process(&chunk_samples, None).unwrap()
        } else {
            chunk_samples
        };

        // now we have to interleave it since we had to use the resampling thing
        let samples: Vec<f32> = samples_correct_rate[0]
            .chunks(1)
            .zip(samples_correct_rate[1].chunks(1))
            .flat_map(|(a, b)| a.into_iter().chain(b))
            .copied()
            .collect();

        Ok(samples)
    }

    pub fn encode_frame(&mut self) -> Result<AudioFrame, ()> {
        const PCM_LENGTH: usize = 960;

        let mut pcm = [0.0; PCM_LENGTH];

        let resampled = self.resample_more().unwrap();
        for i in 0..PCM_LENGTH {
            pcm[i] = resampled[i];
        }

        let x = self.encoder.encode_vec_float(&pcm, 256).unwrap();
        self.position += 1;

        Ok(AudioFrame {
            frame: self.position as u32,
            data: x,
        })
    }

    pub fn finished(&self) -> bool {
        self.finished
    }

    pub fn set_finished(&mut self) {
        self.finished = true;
    }
}

#[derive(Debug, Clone)]
pub enum TransmitCommand {
    Start(String),
    Stop,
}

//temp
type MessageTx = tokio::sync::mpsc::UnboundedSender<Message>;
type AudioTx = std::sync::mpsc::Sender<AudioData>;

type TransmitTx = tokio::sync::mpsc::UnboundedSender<TransmitCommand>;
type TransmitRx = tokio::sync::mpsc::UnboundedReceiver<TransmitCommand>;

pub struct TransmitThreadHandle {
    tx: TransmitTx,
    // join_handle: JoinHandle<()>,
}

impl TransmitThreadHandle {
    pub fn send(
        &mut self,
        cmd: TransmitCommand,
    ) -> Result<(), tokio::sync::mpsc::error::SendError<TransmitCommand>> {
        self.tx.send(cmd)
    }
}

pub struct TransmitThread {
    rx: TransmitRx,
    message_tx: MessageTx,
    audio_tx: AudioTx,

    audio_reader: Option<AudioReader>,
    interval: Interval,
}

impl TransmitThread {
    // pass in the channel parts because we need to create all the channels before spawning threads
    // since they depend on each other...
    pub fn spawn(
        transmit_tx: TransmitTx,
        transmit_rx: TransmitRx,
        message_tx: &MessageTx,
        audio_tx: &AudioTx,
    ) -> TransmitThreadHandle {
        let message_tx = message_tx.to_owned();
        let audio_tx = audio_tx.to_owned();

        let _join_handle = tokio::spawn(async move {
            let mut t = TransmitThread::new(transmit_rx, message_tx, audio_tx);
            t.run().await;
        });

        TransmitThreadHandle {
            tx: transmit_tx,
            // join_handle,
        }
    }

    fn new(rx: TransmitRx, message_tx: MessageTx, audio_tx: AudioTx) -> Self {
        // ok let's actually do the math for this
        // each frame is 960 samples
        // at 48k that means it's 20ms per frame
        // SO we need to send a frame at least every 20ms.
        // i think.................
        // LOL OK it's two frames idk why maybe because it's stereo interleaved??????
        let interval = tokio::time::interval(tokio::time::Duration::from_millis(100));

        TransmitThread {
            rx,
            message_tx,
            audio_tx,

            audio_reader: None,
            interval,
        }
    }

    fn send_both(&self, data: AudioData) {
        self.audio_tx.send(data.clone()).unwrap();
        self.message_tx.send(Message::AudioData(data)).unwrap();
    }

    async fn run(&mut self) {
        loop {
            tokio::select! {
                Some(cmd) = self.rx.recv() => self.handle_command(cmd),

                _ = self.interval.tick() => self.handle_tick(),
            }
        }
    }

    fn handle_command(&mut self, cmd: TransmitCommand) {
        match cmd {
            TransmitCommand::Start(path) => {
                if self.audio_reader.is_some() {
                    eprintln!("TransmitCommand::Start failed, already transmitting?");
                    return;
                }

                if let Ok(r) = AudioReader::load(&path) {
                    self.audio_reader = Some(r);

                    self.send_both(AudioData::Start);
                } else {
                    // failed to load file, need to skip it somehow?
                }
            }
            TransmitCommand::Stop => {
                self.send_both(AudioData::Finish);
                if let Some(t) = self.audio_reader.as_mut() {
                    t.set_finished();
                }
            }
        }
    }

    fn handle_tick(&mut self) {
        // TODO: only run the timer when we need it?
        if let Some(t) = self.audio_reader.as_mut() {
            for _ in 0..10 {
                // if ran out in the middle of this "tick" of 20 frames
                if t.finished() {
                    break;
                };

                let f = t.encode_frame();
                if let Ok(frame) = f {
                    // borrow checker doesn't like self.send_both() here since we already borrow self or something
                    let data = AudioData::Frame(frame);
                    self.audio_tx.send(data.clone()).unwrap();
                    self.message_tx.send(Message::AudioData(data)).unwrap();
                } else {
                    break; // we're done i guess
                }
            }

            // we encoded the entire file
            if t.finished() {
                self.message_tx
                    .send(Message::AudioData(AudioData::Finish))
                    .unwrap();

                self.audio_reader = None;
            };
        }
    }
}