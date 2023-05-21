use std::collections::VecDeque;
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
use rubato::{InterpolationParameters, Resampler, SincFixedOut};

type Ringbuf<T> = LocalRb<T, Vec<MaybeUninit<T>>>;

pub struct AudioReader {
    probe_result: ProbeResult,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    resampler: SincFixedOut<f32>,
    encoder: Encoder,

    source_buffer: Vec<VecDeque<f32>>,

    position: usize, // we might not actually need this for anything now?
    finished: bool,

    sample_rate: u32,
    channel_count: usize,
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

        let mut decoder = symphonia::default::get_codecs()
            .make(
                &probe_result
                    .format
                    .default_track()
                    .expect("uhhhh")
                    .codec_params,
                &DecoderOptions::default(),
            )
            .unwrap();

        // read 1 packet to check the sample rate and channel count
        // TODO: there HAS to be a better way than opening the file twice
        let (sample_rate, channel_count) = {
            let src2 = std::fs::File::open(path)?;
            let mss2 = MediaSourceStream::new(Box::new(src2), Default::default());

            let mut temp_probe = symphonia::default::get_probe()
                .format(
                    &hint,
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

            (spec.rate, spec.channels.count())
        };

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

        let encoder =
            opus::Encoder::new(48000, opus::Channels::Stereo, opus::Application::Audio).unwrap();
        //encoder.set_bitrate(opus::Bitrate::Bits(256)).unwrap();

        Ok(Self {
            probe_result,
            decoder,
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
        println!("read_packet() called");

        match self.probe_result.format.next_packet() {
            Ok(packet) => {
                let decoded = self.decoder.decode(&packet).unwrap();
                let spec = *decoded.spec();

                if decoded.frames() > 0 {
                    let mut sample_buffer: SampleBuffer<f32> =
                        SampleBuffer::new(decoded.frames() as u64, spec);

                    sample_buffer.copy_interleaved_ref(decoded);

                    let samples = sample_buffer.samples();

                    println!("packet had {:?}", samples.len());

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
                    println!("filling with 0s!!!");
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
            println!("resampling");
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

        println!("resampled {:?} interleaved samples", samples.len(),);

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
