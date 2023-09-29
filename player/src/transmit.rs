use std::collections::VecDeque;
use std::error::Error;
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

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

use protocol::{AudioData, Message, TrackArt};
use protocol::{AudioFrame, TrackMetadata};

use crate::audio::AudioThreadHandle;
use crate::connection::NetworkCommand;
use crate::AudioCommand;

pub struct AudioInfoReader {
    probe_result: ProbeResult,
    decoder: Box<dyn Decoder>,
    filename: String,
    path: PathBuf,
}

impl AudioInfoReader {
    pub fn load(path: &PathBuf) -> Result<Self, Box<dyn Error + Send + Sync>> {
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
            path: path.to_owned(),
        })
    }

    pub fn read_info(
        &mut self,
    ) -> Result<(u32, usize, TrackMetadata), Box<dyn Error + Send + Sync>> {
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

        let num_frames = self
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
        let mut track_md = TrackMetadata {
            duration: num_frames as f32 / spec.rate as f32,
            ..Default::default()
        };

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

        // lyrics
        // lazily search for lrc file assuming it's the same name and stuff
        let cwd = self.path.parent().unwrap();
        let lyrics_path = if cwd.join("lyrics").is_dir() {
            cwd.join("lyrics")
                .join(self.path.file_name().unwrap())
                .with_extension("lrc")
        } else {
            self.path.with_extension("lrc")
        };
        if lyrics_path.exists() {
            // epic
            println!("has lyrics!");
            let mut buffer = String::new();
            let mut f = File::open(&lyrics_path).unwrap();
            f.read_to_string(&mut buffer).unwrap();
            let lines: Vec<(usize, String)> = buffer
                .lines()
                .map(|l| crate::lrc::parse_line(l).unwrap())
                .collect();
            track_md.lyrics = Some(protocol::Lyrics { lines });
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

    position: u32,
    finished: bool,

    sample_rate: u32,
    channel_count: usize,
}

impl AudioReader {
    pub fn load(path: &PathBuf) -> Result<Self, Box<dyn Error + Send + Sync>> {
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
                48000_f64 / sample_rate as f64,
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
            480
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
            .flat_map(|(a, b)| a.iter().chain(b))
            .copied()
            .collect();

        Ok(samples)
    }

    pub fn encode_frame(&mut self) -> Result<AudioFrame, ()> {
        const PCM_LENGTH: usize = 960;

        let mut pcm = [0.0; PCM_LENGTH];

        match self.resample_more() {
            Ok(resampled) => pcm[..PCM_LENGTH].copy_from_slice(&resampled[..PCM_LENGTH]),
            Err(e) => {
                println!("failed to encode frame: {:?}", e);
                // TODO: do something with this error
            }
        }

        let opus_data = self.encoder.encode_vec_float(&pcm, 256).unwrap();
        self.position += 1;

        Ok(AudioFrame {
            frame: self.position,
            data: opus_data,
        })
    }

    pub fn seek_to(&mut self, secs: f32) -> Result<(), Box<dyn Error>> {
        self.inner.probe_result.format.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time: symphonia::core::units::Time::from(secs),
                track_id: None,
            },
        )?;
        self.inner.decoder.reset();

        let samples = secs * 48000.0;
        let position = samples / 480.0;
        self.position = position as u32;

        Ok(())
    }

    pub fn finished(&self) -> bool {
        self.finished
    }
}

#[derive(Debug, Clone)]
pub enum TransmitCommand {
    Stop,
    PauseState(bool),
    SeekTo(f32),
}

pub type TransmitTx = mpsc::UnboundedSender<TransmitCommand>;
pub type TransmitRx = mpsc::UnboundedReceiver<TransmitCommand>;

#[derive(Debug)]
pub struct TransmitThreadHandle {
    pub track_id: u32,

    tx: TransmitTx,
}

impl TransmitThreadHandle {
    pub fn send(
        &self,
        cmd: TransmitCommand,
    ) -> Result<(), mpsc::error::SendError<TransmitCommand>> {
        self.tx.send(cmd)
    }
}

pub struct TransmitThread {
    track_id: u32,
    paused: bool,

    audio_reader: AudioReader,
    interval: Interval,

    // receive TransmitCommands
    rx: TransmitRx,

    // send AudioData to network and self
    network_tx: mpsc::UnboundedSender<NetworkCommand>,
    audio: AudioThreadHandle,

    // interrupt own loop
    exit_tx: mpsc::UnboundedSender<()>,
    exit_rx: mpsc::UnboundedReceiver<()>,
}

impl TransmitThread {
    // pass in the channel parts because we need to create all the channels before spawning threads
    // since they depend on each other...
    pub fn spawn(
        track_id: u32,
        track_path: PathBuf,
        network_tx: mpsc::UnboundedSender<NetworkCommand>,
        audio: AudioThreadHandle,
    ) -> TransmitThreadHandle {
        let (transmit_tx, transmit_rx) = mpsc::unbounded_channel();

        tokio::spawn(async move {
            let thread = TransmitThread::new(track_id, track_path, transmit_rx, network_tx, audio);
            match thread {
                Ok(mut t) => {
                    t.run().await;
                }
                Err(e) => {
                    eprintln!("error while spawning transmit thread: {:?}", e);
                }
            }
        });

        TransmitThreadHandle {
            track_id,

            tx: transmit_tx,
        }
    }

    fn new(
        track_id: u32,
        track_path: PathBuf,
        rx: TransmitRx,
        network_tx: mpsc::UnboundedSender<NetworkCommand>,
        audio: AudioThreadHandle,
    ) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let (exit_tx, exit_rx) = mpsc::unbounded_channel();

        let audio_reader = AudioReader::load(&track_path)?;

        // ok let's actually do the math for this
        // each frame is 960 samples
        // at 48k that means it's 20ms per frame
        // SO we need to send a frame at least every 20ms.
        // i think.................
        // LOL OK it's two frames idk why maybe because it's stereo interleaved??????
        //
        // CLARIFICATION : 1 frame is 960 samples but that's for 2 channels so 480 samples per channel per frame
        // 480 samples / 48k sample rate = 10ms per frame
        let interval = tokio::time::interval(tokio::time::Duration::from_millis(10));

        let t = TransmitThread {
            track_id,
            paused: false,

            audio_reader,
            interval,

            rx,

            network_tx,
            audio,

            exit_tx,
            exit_rx,
        };

        // TODO ?
        t.send_both(AudioData::Start);

        Ok(t)
    }

    fn send_both(&self, data: AudioData) {
        self.audio
            .send(AudioCommand::AudioData(data.clone()))
            .unwrap();
        self.network_tx
            .send(NetworkCommand::Plain(Message::AudioData(data)))
            .unwrap();
    }

    async fn run(&mut self) {
        loop {
            tokio::select! {
                result = self.rx.recv() => {
                    match result {
                        Some(cmd) => self.handle_command(cmd),

                        // all txs were dropped
                        None => break,
                    }
                },

                _ = self.exit_rx.recv() => {
                    break;
                }

                _ = self.interval.tick() => self.handle_tick(),
            }
        }

        println!("transmit thread for {} exiting", self.track_id);
    }

    // returns true if we should continue looping
    fn handle_command(&mut self, cmd: TransmitCommand) {
        match cmd {
            TransmitCommand::Stop => {
                let _ = self.exit_tx.send(());
                self.paused = true;
            }
            TransmitCommand::PauseState(p) => {
                self.paused = p;
            }
            TransmitCommand::SeekTo(secs) => {
                if let Err(e) = self.audio_reader.seek_to(secs) {
                    println!("failed to seek: {e:?}");
                }
            }
        }
    }

    fn handle_tick(&mut self) {
        if self.paused {
            return;
        }

        // we encoded the entire file
        if self.audio_reader.finished() {
            // send to network and ourselves
            self.send_both(AudioData::Finish);

            // shut down this transmitter
            let _ = self.exit_tx.send(());

            return;
        };

        if let Ok(frame) = self.audio_reader.encode_frame() {
            self.send_both(AudioData::Frame(frame));
        } else {
            // we're done i guess? or there was some other encoding error
            // idk we should probably handle this
        }
    }
}
