use std::error::Error;
use std::mem::MaybeUninit;
use std::sync::{Arc, Mutex};

use opus::{Decoder, Encoder};
use protocol::AudioFrame;
use ringbuf::{LocalRb, Rb};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::{Hint, ProbeResult};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, Stream};
use rubato::{FftFixedIn, Resampler};

type Ringbuf<T> = LocalRb<T, Vec<MaybeUninit<T>>>;

pub struct AudioReader {
    probe_result: ProbeResult,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    encoder: Encoder,

    buffer: Ringbuf<f32>,

    position: usize, // we might not actually need this for anything now?
    finished: bool,

    // not read until the first packet
    sample_rate: Option<u32>,
    channel_count: Option<usize>,
}

impl AudioReader {
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

        let encoder =
            opus::Encoder::new(48000, opus::Channels::Stereo, opus::Application::Audio).unwrap();
        //encoder.set_bitrate(opus::Bitrate::Bits(256)).unwrap();

        Ok(Self {
            probe_result,
            decoder,
            encoder,

            buffer: Ringbuf::new(16384),

            position: 0,
            finished: false,

            sample_rate: None,
            channel_count: None,
        })
    }

    // Ok(true) if there's more to read
    fn read_packets(&mut self) -> Result<bool, Box<dyn Error>> {
        println!("read_packet() called");

        let mut has_more = true;
        let mut channel_samples: Vec<Vec<f32>> = vec![Vec::new(); self.channel_count.unwrap_or(0)];
        let mut sample_count = 0;

        loop {
            match self.probe_result.format.next_packet() {
                Ok(packet) => {
                    let decoded = self.decoder.decode(&packet).unwrap();
                    let spec = *decoded.spec();

                    if self.sample_rate == None {
                        self.sample_rate = Some(spec.rate);
                    }
                    if self.channel_count == None {
                        self.channel_count = Some(spec.channels.count());

                        // we probably also need to allocate channel_samples now since this is the 1st packet we're processing
                        channel_samples = vec![Vec::new(); spec.channels.count()];
                    }

                    if decoded.frames() > 0 {
                        let mut sample_buffer: SampleBuffer<f32> =
                            SampleBuffer::new(decoded.frames() as u64, spec);

                        sample_buffer.copy_interleaved_ref(decoded);

                        let samples = sample_buffer.samples();

                        println!("packet had {:?}", samples.len());
                        sample_count += samples.len();

                        for frame in samples.chunks(spec.channels.count()) {
                            for (chan, sample) in frame.iter().enumerate() {
                                channel_samples[chan].push(*sample)
                            }
                        }
                    } else {
                        eprintln!("Empty packet encountered while loading song!");
                    }
                }
                Err(SymphoniaError::IoError(_)) => {
                    // no more packets
                    has_more = false;
                    break;
                }
                Err(e) => return Err("failed to parse track".into()),
            }

            // we should stop after a reasonable number of samples
            // (a reasonable number --> nobody knows)
            // we probably want more than 960 so we can resample enough at once (does that matter?)
            if sample_count >= 2048 {
                break;
            }
        }

        println!("resampling....");

        // resample to standard 48000 if needed
        let samples_correct_rate = if self.sample_rate.unwrap() != 48000 {
            let l = channel_samples[0].len();
            let mut resampler =
                FftFixedIn::<f32>::new(self.sample_rate.unwrap() as usize, 48000, l, 100, 2)
                    .unwrap();

            resampler.process(&channel_samples, None).unwrap()
        } else {
            channel_samples
        };

        println!("interleaving....");

        // now we have to interleave it since we had to use the resampling thing
        let samples: Vec<f32> = samples_correct_rate[0]
            .chunks(1)
            .zip(samples_correct_rate[1].chunks(1))
            .flat_map(|(a, b)| a.into_iter().chain(b))
            .copied()
            .collect();

        println!(
            "copying {:?} samples to the buffer which has {:?} free",
            samples.len(),
            self.buffer.free_len()
        );

        for sample in samples.iter() {
            self.buffer.push(*sample).unwrap();
        }

        Ok(has_more)
    }

    pub fn encode_frame(&mut self) -> Result<AudioFrame, ()> {
        const PCM_LENGTH: usize = 960;

        // we don't have enough samples buffered to encode an opus frame, read more packets
        if self.buffer.len() < PCM_LENGTH {
            if let Ok(has_more) = self.read_packets() {
                if !has_more {
                    // no more packets, fill with 0s until we have a multiple of 960 frames
                    while self.buffer.len() % 960 != 0 {
                        self.buffer.push(0.0).unwrap();
                    }

                    self.finished = true;
                }
            } else {
                // unrecoverable error?
                return Err(());
            }
        }

        let mut pcm = [0.0; PCM_LENGTH];

        self.buffer.pop_slice(&mut pcm);

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
}

pub struct Player {
    decoder: Decoder,
    buffer: Arc<Mutex<Ringbuf<f32>>>,
    stream: Option<Stream>,
}
impl Player {
    pub fn new() -> Self {
        Self {
            decoder: Decoder::new(48000, opus::Channels::Stereo).unwrap(),
            buffer: Arc::new(Mutex::new(Ringbuf::<f32>::new(48000 * 10))), // 10s??
            stream: None,
        }
    }

    // decode opus data when received and buffer the samples
    pub fn receive(&mut self, frame: Vec<u8>) {
        // allocate and decode into here
        let mut pcm = vec![0.0; 960];
        self.decoder.decode_float(&frame, &mut pcm, false).unwrap();

        let mut buffer = self.buffer.lock().unwrap();
        if buffer.len() < buffer.free_len() {
            // copy to ringbuffer
            buffer.push_slice(&pcm);
        } else {
            // otherwise just discard i guess
            println!("player buffer full... discarding :/")
        }
    }

    pub fn play(&mut self) {
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
        self.stream.as_mut().unwrap().pause().unwrap();
    }

    pub fn resume(&mut self) {
        assert!(self.stream.is_some());
        self.stream.as_mut().unwrap().play().unwrap();
    }

    pub fn clear(&mut self) {
        self.buffer.lock().unwrap().clear();
    }

    pub fn ready(&self) -> bool {
        let buffer = self.buffer.lock().unwrap();
        buffer.len() > 48000 * 2 // 2s...
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
    if buffer.len() < 48000 * 2 - 1000 {
        println!("write_audio: buffer has {} left", buffer.len());
    }
}
