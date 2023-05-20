use std::sync::Arc;

use opus::{Decoder, Encoder};
use protocol::AudioFrame;
use ringbuf::{HeapRb, Rb};
use symphonia::core::audio::{SampleBuffer};
use symphonia::core::codecs::{DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Data, PlayStreamError, Sample, SampleFormat};
use rubato::{Resampler, FftFixedIn};
pub struct Track {
    pub samples: Arc<Vec<f32>>,
    pub sample_rate: u32,
    pub channel_count: usize,
    pub position: usize,
    encoder: Encoder,
}

impl Track {
    pub fn load(path: &str) -> Result<Self, ()> {
        let src = std::fs::File::open(path).expect("failed to open media");

        let mss = MediaSourceStream::new(Box::new(src), Default::default());

        let mut hint = Hint::new();
        //hint.with_extension("mp3");

        let mut samples_not_interleaved: Option<Vec<Vec<f32>>> = None;
        let mut sample_rate: u32 = 0;
        let mut channel_count: usize = 0;

        let mut probe_result = symphonia::default::get_probe()
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

        loop {
            match probe_result.format.next_packet() {
                Ok(packet) => {
                    let decoded = decoder.decode(&packet).unwrap();
                    let spec = *decoded.spec();
                    let song_samples = match &mut samples_not_interleaved {
                        Some(s) => Some(s),
                        None => {
                            samples_not_interleaved = Some(vec![Vec::new(); spec.channels.count()]);
                            sample_rate = spec.rate;
                            channel_count = spec.channels.count();
                            samples_not_interleaved.as_mut()
                        }
                    }
                    .unwrap();

                    if decoded.frames() > 0 {
                        let mut samples: SampleBuffer<f32> =
                            SampleBuffer::new(decoded.frames() as u64, spec);

                        samples.copy_interleaved_ref(decoded);
                        for frame in samples.samples().chunks(spec.channels.count()) {
							for (chan, sample) in frame.iter().enumerate() {
								song_samples[chan].push(*sample)
							}
						}
                    } else {
                        eprintln!("Empty packet encountered while loading song!");
                    }
                }
                Err(SymphoniaError::IoError(_)) => break,
                Err(e) => return Err(()),
            }
        }

        // resample to standard 48000 if needed
        let samples_correct_rate = if sample_rate != 48000 {

            let l = samples_not_interleaved.as_ref().unwrap()[0].len().clone();
            let mut resampler = FftFixedIn::<f32>::new(
                sample_rate as usize,
                48000,
                l,
                100,
                2
            ).unwrap();

            resampler.process(&samples_not_interleaved.unwrap(), None).unwrap()
        } else {
            samples_not_interleaved.unwrap()
        };

        // now we have to interleave it since we had to use the resampling thing
        let samples: Vec<f32> = samples_correct_rate[0].chunks(1)
            .zip(samples_correct_rate[1].chunks(1))
            .flat_map(|(a, b)| a.into_iter().chain(b))
            .copied()
            .collect();

        // it would be really sick to do all that at once (while reading it)
        // but since we read like.. 2 samples at a time idk how
        // the way it is it's just too slow for anything realistically-sized

        let mut encoder = opus::Encoder::new(48000, opus::Channels::Stereo, opus::Application::Audio).unwrap();
        //encoder.set_bitrate(opus::Bitrate::Bits(256)).unwrap();
        Ok(Self {
            samples: Arc::new(samples),
            sample_rate,
            channel_count,
            position: 0,
            encoder,
        })
    }

    pub fn encode_frame(&mut self) -> AudioFrame {
        const pcm_length: usize = 960;
        let mut pcm = [0.0; pcm_length];

        for i in 0..pcm_length {
            pcm[i] = self.samples[i + pcm_length * self.position];
        }

        let x = self.encoder.encode_vec_float(&pcm, 256).unwrap();
        self.position += 1;

        AudioFrame {
            frame: self.position as u32,
            data: x,
        }
    }
}

pub struct Player {
    decoder: Decoder,
    buffer: HeapRb<Vec<u8>>,
}
impl Player {
    pub fn new() -> Self {
        Self {
            decoder: Decoder::new(48000, opus::Channels::Stereo).unwrap(),
            buffer:HeapRb::<Vec<u8>>::new(1000),
        }
    }
    pub fn receive(&mut self, data: Vec<u8>) {
        self.buffer.push(data).unwrap();
    }
    pub fn decode_frame(&mut self) -> [f32; 960] {
        let mut pcm = [0.0; 960];
        let frame = self.buffer.pop().unwrap();
        let x = self.decoder.decode_float(&frame, &mut pcm, false).unwrap();
        pcm
    }

    pub fn debug_export(&mut self) {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create("blah.wav", spec).unwrap();
        loop {
            let frame = self.decode_frame();
            for s in frame {
                writer.write_sample(s).unwrap();
            }
        }
    }
/* 
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
            .with_max_sample_rate();

        let err_fn = |err| eprintln!("an error occurred on the output audio stream: {}", err);
        let sample_format = supported_config.sample_format();
        let config = supported_config.into();

        let stream = device.build_output_stream(
                &config,
                move |data, info| Self::write_audio::<f32>(data, info, &self.buffer),
                err_fn,
                None
            ).unwrap();

        println!("Starting audio stream");
        stream.play().unwrap();
    }

    fn write_audio<T: Sample>(data: &mut [f32], _: &cpal::OutputCallbackInfo, rb_audio: &HeapRb<Vec<u8>>) {
        println!("Len {}", data.len());
        // here you'd basically just 
    } */
}
