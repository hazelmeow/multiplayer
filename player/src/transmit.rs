use std::collections::VecDeque;
use std::error::Error;
use std::io::Cursor;
use std::path::PathBuf;

use image::imageops::FilterType;
use opus::Encoder;
use rubato::{InterpolationParameters, Resampler, SincFixedOut};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, StandardTagKey};
use symphonia::core::probe::{Hint, ProbeResult};
use tokio::sync::mpsc;
use tokio::time::Interval;

use protocol::{AudioData, TrackArt};
use protocol::{AudioFrame, TrackMetadata};

use crate::audio::AudioTx;
use crate::AudioCommand;

pub struct AudioInfoReader {
    probe_result: ProbeResult,
    decoder: Box<dyn Decoder>,
    filename: String,
}

impl AudioInfoReader {
    pub fn load(path: &PathBuf) -> Result<Self, Box<dyn Error>> {
        let filename = path
            .file_name()
            .unwrap_or_default()
            .to_os_string()
            .into_string()
            .unwrap_or_default();
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
            filename,
        })
    }

    pub fn read_info(&mut self) -> Result<(u32, usize, TrackMetadata), Box<dyn Error>> {
        // read 1 packet to check the sample rate and channel count

        let decoded = loop {
            let p = self.probe_result.format.next_packet();
            match p {
                Ok(packet) => {
                    match self.decoder.decode(&packet) {
                        Ok(decoded) => break decoded,
                        Err(_) => {
                            // failed to decode, keep trying
                            continue;
                        }
                    }
                }
                Err(SymphoniaError::IoError(_)) => {
                    // out of packets
                    return Err("couldn't read any packets so we gave up :/".into());
                }
                Err(e) => {
                    return Err(e.into());
                }
            }
        };
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
        track_md.duration = (duration as f64 / spec.rate as f64) as u32;

        let mut format_md = self.probe_result.format.metadata();
        if let Some(rev) = format_md.skip_to_latest() {
            let tags = rev.tags();
            let visuals = rev.visuals();

            Self::fill_metadata(&mut track_md, tags, visuals);
        } else if let Some(mut md) = self.probe_result.metadata.get() {
            let rev = md.skip_to_latest().unwrap();
            let tags = rev.tags();
            let visuals = rev.visuals();

            Self::fill_metadata(&mut track_md, tags, visuals);
        } else {
            println!("really no metadata to speak of...");
        }

        if track_md.title.is_none() {
            track_md.title = Some(self.filename.clone());
        }

        Ok((spec.rate, spec.channels.count(), track_md))
    }

    fn fill_metadata(
        md: &mut TrackMetadata,
        tags: &[symphonia::core::meta::Tag],
        visuals: &[symphonia::core::meta::Visual],
    ) {
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

        // default albumartist to artist if necessary & possible
        if md.album_artist.is_none() {
            if let Some(artist) = &md.artist {
                md.album_artist = Some(artist.clone());
            }
        }

        // just assume there's only one visual in the file?
        if let Some(v) = visuals.first() {
            match v.media_type.as_str() {
                "image/png" | "image/jpg" | "image/jpeg" => {
                    let reader = image::io::Reader::new(Cursor::new(v.data.clone()))
                        .with_guessed_format()
                        .expect("cursor io never fails");

                    if let Ok(image) = reader.decode() {
                        let resized = image.resize(29, 29, FilterType::Nearest);
                        let mut buf: Vec<u8> = vec![];
                        let mut jpeg_encoder = image::codecs::jpeg::JpegEncoder::new(&mut buf);
                        jpeg_encoder.encode_image(&resized).unwrap();
                        md.art = Some(TrackArt::Jpeg(buf))
                    } else {
                        md.art = None
                    }
                }

                _ => println!("unhandled media_type for visual: {}", v.media_type),
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
    pub fn load(path: &PathBuf) -> Result<Self, Box<dyn Error>> {
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
                let decoded = self.inner.decoder.decode(&packet)?;
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
            .zip({
                if self.channel_count == 1 {
                    samples_correct_rate[0].chunks(1)
                } else {
                    samples_correct_rate[1].chunks(1)
                }
            })
            .flat_map(|(a, b)| a.into_iter().chain(b))
            .copied()
            .collect();

        Ok(samples)
    }

    pub fn encode_frame(&mut self) -> Result<AudioFrame, ()> {
        const PCM_LENGTH: usize = 960;

        let mut pcm = [0.0; PCM_LENGTH];

        match self.resample_more() {
            Ok(resampled) => {
                for i in 0..PCM_LENGTH {
                    pcm[i] = resampled[i];
                }
            }
            Err(e) => {
                println!("failed to encode frame: {:?}", e);
            }
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
    Start(PathBuf),
    Stop,
    Shutdown,
}

pub type TransmitTx = mpsc::UnboundedSender<TransmitCommand>;
pub type TransmitRx = mpsc::UnboundedReceiver<TransmitCommand>;

type NetworkAudioTx = mpsc::UnboundedSender<AudioData>;

#[derive(Clone)]
pub struct TransmitThreadHandle {
    tx: TransmitTx,
}

impl TransmitThreadHandle {
    pub fn send(
        &mut self,
        cmd: TransmitCommand,
    ) -> Result<(), mpsc::error::SendError<TransmitCommand>> {
        self.tx.send(cmd)
    }
}

pub struct TransmitThread {
    rx: TransmitRx,
    network_audio_tx: NetworkAudioTx,
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
        network_audio_tx: NetworkAudioTx,
        audio_tx: &AudioTx,
    ) -> TransmitThreadHandle {
        let network_audio_tx = network_audio_tx.to_owned();
        let audio_tx = audio_tx.to_owned();

        tokio::spawn(async move {
            let mut t = TransmitThread::new(transmit_rx, network_audio_tx, audio_tx);
            t.run().await;
        });

        TransmitThreadHandle { tx: transmit_tx }
    }

    fn new(rx: TransmitRx, network_audio_tx: NetworkAudioTx, audio_tx: AudioTx) -> Self {
        // ok let's actually do the math for this
        // each frame is 960 samples
        // at 48k that means it's 20ms per frame
        // SO we need to send a frame at least every 20ms.
        // i think.................
        // LOL OK it's two frames idk why maybe because it's stereo interleaved??????
        let interval = tokio::time::interval(tokio::time::Duration::from_millis(100));

        TransmitThread {
            rx,
            network_audio_tx,
            audio_tx,

            audio_reader: None,
            interval,
        }
    }

    fn send_both(&self, data: AudioData) {
        self.audio_tx
            .send(AudioCommand::AudioData(data.clone()))
            .unwrap();
        self.network_audio_tx.send(data).unwrap();
    }

    async fn run(&mut self) {
        loop {
            tokio::select! {
                Some(cmd) = self.rx.recv() => {
                    let should_continue = self.handle_command(cmd);
                    if !should_continue { break }
                },

                _ = self.interval.tick() => self.handle_tick(),
            }
        }
    }

    fn handle_command(&mut self, cmd: TransmitCommand) -> bool {
        match cmd {
            TransmitCommand::Start(path) => {
                if self.audio_reader.is_some() {
                    eprintln!("TransmitCommand::Start failed, already transmitting?");
                    return true;
                }

                match AudioReader::load(&path) {
                    Ok(r) => {
                        self.audio_reader = Some(r);

                        self.send_both(AudioData::Start);
                    }
                    Err(e) => {
                        println!("failed to load {:?} while transmitting: {e}", path)
                    }
                }
            }
            TransmitCommand::Stop => {
                // see the other comment for why we don't send it directly to the audio thread
                self.network_audio_tx.send(AudioData::Finish).unwrap();

                self.audio_reader = None;
            }
            TransmitCommand::Shutdown => return false,
        }

        // don't end loop
        return true;
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
                    self.audio_tx
                        .send(AudioCommand::AudioData(data.clone()))
                        .unwrap();
                    self.network_audio_tx.send(data).unwrap();
                } else {
                    break; // we're done i guess
                }
            }

            // we encoded the entire file
            if t.finished() {
                // don't send AudioData::Finish to ourselves BECAUSE
                // we only want the audio thread to get finish if there's nothing to play
                // the server checks if something new is queued and intercepts the Finish message
                self.network_audio_tx.send(AudioData::Finish).unwrap();

                self.audio_reader = None;
            };
        }
    }
}
