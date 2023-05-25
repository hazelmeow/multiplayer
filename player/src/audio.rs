use std::mem::MaybeUninit;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, Stream};
use opus::Decoder;
use ringbuf::{LocalRb, Rb};

type Ringbuf<T> = LocalRb<T, Vec<MaybeUninit<T>>>;

const PLAYER_BUFFER_SIZE: usize = 48000 * 10; // 10s of samples?

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
            buffer: Arc::new(Mutex::new(Ringbuf::<f32>::new(PLAYER_BUFFER_SIZE))),
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
