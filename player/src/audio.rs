use std::collections::VecDeque;
use std::error::Error;
use std::mem::MaybeUninit;
use std::sync::{Arc, Mutex};

use opus::{Decoder, Encoder};
use protocol::{AudioFrame, TrackMetadata};
use ringbuf::{LocalRb, Rb};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, StandardTagKey, Metadata};
use symphonia::core::probe::{Hint, ProbeResult};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, Stream};
use rubato::{InterpolationParameters, Resampler, SincFixedOut};

type Ringbuf<T> = LocalRb<T, Vec<MaybeUninit<T>>>;

pub struct AudioReader {
    probe_result: ProbeResult,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    resampler: SincFixedOut<f32>,
    encoder: Encoder,

    source_buffer: Vec<VecDeque<f32>>,

    position: usize,
    duration: u64,
    finished: bool,

    sample_rate: u32,
    channel_count: usize,
}

impl AudioReader {
    pub fn load(path: &str) -> Result<Self, Box<dyn Error>> {
        let src = std::fs::File::open(path)?;

        let (mut probe_result, mut decoder) = Self::new_decoder(src);

        let (sample_rate, channel_count, duration, _) = Self::load_info(path, &mut decoder)?;

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
            probe_result,
            decoder,
            resampler,
            encoder,

            source_buffer: vec![VecDeque::new(); channel_count],

            position: 0,
            duration,
            finished: false,

            sample_rate,
            channel_count,
        })
    }

    pub fn load_info(
        path: &str,
        decoder: &mut Box<dyn symphonia::core::codecs::Decoder>,
    ) -> Result<(u32, usize, u64, TrackMetadata), Box<dyn Error>> {
        // read 1 packet to check the sample rate and channel count
        // TODO: there HAS to be a better way than opening the file twice
        // TODO: there is.. it's ur job to clean all of this up now :3 (sorry)
        let src2 = std::fs::File::open(path)?;
        let mss2 = MediaSourceStream::new(Box::new(src2), Default::default());

        let mut temp_probe = symphonia::default::get_probe()
            .format(
                &Hint::new(),
                mss2,
                &FormatOptions {
                    enable_gapless: true,
                    ..FormatOptions::default()
                },
                &MetadataOptions::default(),
            )
            .unwrap();

        let packet = temp_probe.format.next_packet().unwrap();
        let decoded = decoder.decode(&packet).unwrap();
        let spec = *decoded.spec();

        // TODO: maybe cache this somewhere? idk
        let mut duration = 0;
        while let Ok(packet) = temp_probe.format.next_packet() {
            duration += packet.dur;
        }

        let mut track_md = TrackMetadata::default();

        track_md.duration = (duration as f64 / spec.rate as f64) as usize;

        let mut md = temp_probe.format.metadata();
        if let Some(current) = md.skip_to_latest() {
            dbg!(current.tags());
            dbg!(current.vendor_data());
    
            let tags = current.tags();
            Self::fill_metadata(&mut track_md, tags);
        } else if let Some(mut md) = temp_probe.metadata.get() {
            let tags = md.skip_to_latest().unwrap().tags();
            Self::fill_metadata(&mut track_md, tags);
        }
         else {
            println!("really no metadata to speak of...");
        }
        

        Ok((spec.rate, spec.channels.count(), duration, track_md))
    }

    // hazel's favorite calling pattern
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

    pub fn new_decoder(
        src: std::fs::File,
    ) -> (ProbeResult, Box<dyn symphonia::core::codecs::Decoder>) {
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

        (probe_result, decoder)
    }

    // sets self.finished if the end was reached
    fn read_more(&mut self) -> Result<(), Box<dyn Error>> {
        match self.probe_result.format.next_packet() {
            Ok(packet) => {
                let decoded = self.decoder.decode(&packet).unwrap();
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

pub struct Player {
    decoder: Decoder,
    buffer: Arc<Mutex<Ringbuf<f32>>>,
    stream: Option<Stream>,
    frames_received: usize,
}
impl Player {
    pub fn new() -> Self {
        Self {
            decoder: Decoder::new(48000, opus::Channels::Stereo).unwrap(),
            buffer: Arc::new(Mutex::new(Ringbuf::<f32>::new(48000 * 10))), // 10s??
            stream: None,
            frames_received: 0,
        }
    }

    // decode opus data when received and buffer the samples
    pub fn receive(&mut self, frame: Vec<u8>) {
        // allocate and decode into here
        let mut pcm = vec![0.0; 960];
        self.decoder.decode_float(&frame, &mut pcm, false).unwrap();
        self.frames_received += 1;

        let mut buffer = self.buffer.lock().unwrap();
        if buffer.len() < buffer.free_len() {
            // copy to ringbuffer
            buffer.push_slice(&pcm);
        } else {
            // otherwise just discard i guess
            println!("player buffer full... discarding :/")
        }
    }

    pub fn start(&mut self) {
        assert!(self.stream.is_none());

        println!("Initialising local audio...");
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .expect("no output device available");

        let mut supported_configs_range = device
            .supported_output_configs()
            .expect("error while querying configs");
        let supported_config = supported_configs_range
            .next()
            .expect("no supported config?!")
            .with_sample_rate(cpal::SampleRate(48000));
        // .with_max_sample_rate(); //???

        let err_fn = |err| eprintln!("an error occurred on the output audio stream: {}", err);
        let sample_format = supported_config.sample_format();
        println!("sample format is {:?}", sample_format); //?? do we care about other sample formats
        let config = supported_config.into();

        let buffer_handle = self.buffer.clone();

        let stream = device
            .build_output_stream(
                &config,
                move |data, info| write_audio::<f32>(data, info, &buffer_handle),
                err_fn,
                None,
            )
            .unwrap();

        stream.play().unwrap();
        println!("Starting audio stream");

        // dont let it be dropped
        self.stream = Some(stream);
    }

    pub fn pause(&mut self) {
        assert!(self.stream.is_some());
        println!("pausing stream");
        self.stream.as_mut().unwrap().pause().unwrap();
    }

    pub fn resume(&mut self) {
        assert!(self.stream.is_some());
        self.stream.as_mut().unwrap().play().unwrap();
    }

    pub fn clear(&mut self) {
        println!("clearing playback buffer");
        self.buffer.lock().unwrap().clear();
        self.frames_received = 0;
    }

    pub fn finish(&mut self) -> bool {
        {
            let buffer = self.buffer.lock().unwrap();
            if buffer.len() > 1024 {
                // we will lose a tiny bit but
                return false;
            }
        }
        self.pause();
        true
    }

    pub fn is_started(&self) -> bool {
        self.stream.is_some()
    }

    pub fn is_ready(&self) -> bool {
        let buffer = self.buffer.lock().unwrap();
        buffer.len() > 48000 * 2 // 2s...
    }
    pub fn get_seconds_elapsed(&self) -> usize {
        self.frames_received * 480 / 48000
    }
    pub fn fake_frames_received(&mut self, frames: usize) {
        self.frames_received = frames;
    }
}

// callback when the audio output needs more data
fn write_audio<T: Sample>(
    out_data: &mut [f32],
    _: &cpal::OutputCallbackInfo,
    buffer_mutex: &Arc<Mutex<Ringbuf<f32>>>,
) {
    let mut buffer = buffer_mutex.lock().unwrap();
    if out_data.len() < buffer.len() {
        buffer.pop_slice(out_data);

        for sample in out_data.iter_mut() {
            *sample = *sample * 0.1;
        }
    } else {
        // uhhhh
        println!("write_audio: buffer underrun!! (but no panic)");
    }
    if buffer.len() < 48000 * 2 - 10000 {
        println!("write_audio: buffer has {} left", buffer.len());
    }
}
