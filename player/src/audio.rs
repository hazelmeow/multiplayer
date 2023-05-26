use std::mem::MaybeUninit;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, Stream};
use opus::Decoder;
use ringbuf::{LocalRb, Rb};

use spectrum_analyzer;
use spectrum_analyzer::scaling::{divide_by_N_sqrt, scale_to_zero_to_one, scale_20_times_log10};

type Ringbuf<T> = LocalRb<T, Vec<MaybeUninit<T>>>;

const PLAYER_BUFFER_SIZE: usize = 48000 * 10; // 10s of samples?

pub struct Player {
    decoder: Decoder,
    buffer: Arc<Mutex<Ringbuf<f32>>>,
    stream: Option<Stream>,
    pub frames_received: usize,
    visualizer_buffer: Arc<Mutex<Vec<[f32; 480 * 5]>>>, // :3
}
impl Player {
    pub fn new() -> Self {
        Self {
            decoder: Decoder::new(48000, opus::Channels::Stereo).unwrap(),
            buffer: Arc::new(Mutex::new(Ringbuf::<f32>::new(PLAYER_BUFFER_SIZE))),
            stream: None,
            frames_received: 0,
            visualizer_buffer: Arc::new(Mutex::new(vec![])),
        }
    }

    // decode opus data when received and buffer the samples
    pub fn receive(&mut self, frame: Vec<u8>) {
        // allocate and decode into here
        let mut pcm = [0.0; 960];
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

        let chunk = self.frames_received % 5;
        let mut buf = self.visualizer_buffer.lock().unwrap();
        let bl = buf.len();

        if chunk == 0 || bl == 0 {
            buf.push([0.0;480*5]);
        }

        let bl = buf.len();

        for (i, s) in pcm.iter().step_by(2).enumerate() {
            let summed = s + pcm[i + 1] / 2.0;
            buf[bl-1][chunk + i] = summed;
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
    pub fn get_visualizer_buffer(&mut self) -> Arc<Mutex<Vec<[f32; 480 * 5]>>> {
        assert!(self.frames_received % 5 == 0);
        self.visualizer_buffer.clone()
    }
    pub fn buffer_status(&self) -> u8 {
        let buffer = self.buffer.lock().unwrap();
        match buffer.len() / 12000 {
            // quarter-seconds of buffer
            // ...not the most linear scale of all time
            0 => 0,       // empty
            1..=2 => 1,   // 0.25 to 0.5s
            3..=4 => 2,   // 0.75 to 1s
            5..=6 => 3,   // 1.25 to 1.5s
            7..=12 => 4,  // 1.75 to 3s
            13..=19 => 5, // 3.25 to 4.75s
            20..=24 => 6, // 5 to 6s
            _ => 7,       // and beyond
        }
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

pub fn calculate_visualizer(samples: &[f32; 2400]) -> [u8; 14] {
    let samples_truncated = &samples[..2048];
    let hamming_window = spectrum_analyzer::windows::hann_window(samples_truncated);
    let spectrum = spectrum_analyzer::samples_fft_to_spectrum(
        &hamming_window,
        48000,
        spectrum_analyzer::FrequencyLimit::All,
        Some(&scale_to_zero_to_one),
    )
    .unwrap();

    let mut bars_float = [0.0; 14];
    let min_freq = 20.0;
    let max_freq = 20000.0;

    for (fr, fr_val) in spectrum.data().iter() {
        let fr = fr.val();
        if fr < 25.0 {
            continue;
        }

        let index = (-5.25062 * fr.log(0.06) - 7.56997).round().min(13.0) as usize;
        //println!("{}, {}, {}", index, fr);
        let lmao = if fr < 200.0 {
            3.0
        } else if fr < 500.0 {
            2.0
        } else {
            1.0
        };
        bars_float[index] += fr_val.val() * lmao;
    }
    //println!("{:?}", bars_float);
    let bars: [u8; 14] = bars_float
        .iter()
        .map(|&num| (num as f32 / 2.0).round().min(8.0) as u8)
        .collect::<Vec<u8>>()
        .try_into()
        .unwrap();
    //println!("{:?}", bars);
    bars
}
