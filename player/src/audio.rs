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
use symphonia::core::probe::Hint;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Data, PlayStreamError, Sample, SampleFormat, Stream};
use rubato::{FftFixedIn, Resampler};

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
            let mut resampler =
                FftFixedIn::<f32>::new(sample_rate as usize, 48000, l, 100, 2).unwrap();

            resampler
                .process(&samples_not_interleaved.unwrap(), None)
                .unwrap()
        } else {
            samples_not_interleaved.unwrap()
        };

        // now we have to interleave it since we had to use the resampling thing
        let samples: Vec<f32> = samples_correct_rate[0]
            .chunks(1)
            .zip(samples_correct_rate[1].chunks(1))
            .flat_map(|(a, b)| a.into_iter().chain(b))
            .copied()
            .collect();

        // it would be really sick to do all that at once (while reading it)
        // but since we read like.. 2 samples at a time idk how
        // the way it is it's just too slow for anything realistically-sized

        let mut encoder =
            opus::Encoder::new(48000, opus::Channels::Stereo, opus::Application::Audio).unwrap();
        //encoder.set_bitrate(opus::Bitrate::Bits(256)).unwrap();
        Ok(Self {
            samples: Arc::new(samples),
            sample_rate,
            channel_count,
            position: 0,
            encoder,
        })
    }

    pub fn encode_frame(&mut self) -> Result<AudioFrame, ()> {
        const pcm_length: usize = 960;

        if pcm_length * (self.position + 1) >= self.samples.len() {
            println!(
                "encode_frame was about to run out of samples at pos {}",
                self.position
            );
            return Err(());
        }

        let mut pcm = [0.0; pcm_length];

        for i in 0..pcm_length {
            pcm[i] = self.samples[i + pcm_length * self.position];
        }

        let x = self.encoder.encode_vec_float(&pcm, 256).unwrap();
        self.position += 1;

        Ok(AudioFrame {
            frame: self.position as u32,
            data: x,
        })
    }
}

type Ringbuf = LocalRb<f32, Vec<MaybeUninit<f32>>>;

pub struct Player {
    decoder: Decoder,
    buffer: Arc<Mutex<Ringbuf>>,
    stream: Option<Stream>,
}
impl Player {
    pub fn new() -> Self {
        Self {
            decoder: Decoder::new(48000, opus::Channels::Stereo).unwrap(),
            buffer: Arc::new(Mutex::new(Ringbuf::new(48000 * 10))), // 10s??
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
    buffer_mutex: &Arc<Mutex<Ringbuf>>,
) {
    let mut buffer = buffer_mutex.lock().unwrap();
    if out_data.len() < buffer.len() {
        buffer.pop_slice(out_data);
    } else {
        // uhhhh
        println!("write_audio: buffer underrun!! (but no panic)");
    }
    if buffer.len() < 48000 * 2 - 1000 {
        println!("write_audio: buffer has {} left", buffer.len());
    }
}
