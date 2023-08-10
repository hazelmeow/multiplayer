use std::mem::MaybeUninit;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;
use opus::Decoder;
use ringbuf::ring_buffer::RbBase;
use ringbuf::{LocalRb, Rb};
use tokio::sync::mpsc;
use tokio::time::{Instant, Interval};

use protocol::AudioData;

use crate::gui::visualizer::calculate_visualizer;
use crate::AudioStatus;

const PLAYER_BUFFER_SIZE: usize = 48000 * 10; // 10s of samples?

type Ringbuf<T> = LocalRb<T, Vec<MaybeUninit<T>>>;

pub struct Player {
    decoder: Decoder,
    buffer: Arc<Mutex<Ringbuf<f32>>>,
    stream: Option<Stream>,
    volume: Arc<Mutex<f32>>,
    last_frame: Option<u32>,
}
impl Player {
    pub fn new() -> Self {
        Self {
            decoder: Decoder::new(48000, opus::Channels::Stereo).unwrap(),
            buffer: Arc::new(Mutex::new(Ringbuf::<f32>::new(PLAYER_BUFFER_SIZE))),
            stream: None,
            last_frame: None,
            volume: Arc::new(Mutex::new(1.0)),
        }
    }

    // decode opus data when received and buffer the samples
    pub fn receive(&mut self, frame_data: Vec<u8>, frame_idx: u32) {
        self.last_frame = Some(frame_idx);

        // allocate and decode into here
        let mut pcm = [0.0; 960];
        self.decoder
            .decode_float(&frame_data, &mut pcm, false)
            .unwrap();

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
        let volume_handle = self.volume.clone();

        let stream = device
            .build_output_stream(
                &config,
                move |data, info| write_audio(data, info, &buffer_handle, &volume_handle),
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
        self.buffer.lock().unwrap().clear();
        self.last_frame = None;
    }

    pub fn is_empty(&mut self) -> bool {
        let buffer = self.buffer.lock().unwrap();
        buffer.is_empty()
    }

    pub fn is_started(&self) -> bool {
        self.stream.is_some()
    }

    pub fn is_ready(&self) -> bool {
        const SECONDS: usize = 1;

        let buffer = self.buffer.lock().unwrap();
        buffer.len() > 48000 * 2 * SECONDS // 1s of stereo 48k
    }
    pub fn get_seconds_elapsed(&self) -> u32 {
        const SECONDS: u32 = 1;

        // 960 = 2 channels * 480 samples per frame
        let samples_received = self.last_frame.unwrap_or_default() * 480;
        let current_sample = samples_received.saturating_sub(48000 * SECONDS);
        current_sample.max(0) / 48000
    }
    pub fn get_visualizer_buffer(&mut self) -> Option<[f32; 4096]> {
        let buf = self.buffer.lock().unwrap();

        let mut sbuf = [0.0; 4096];
        if buf.len() > sbuf.len() {
            for (i, s) in buf.iter().step_by(2).enumerate() {
                if i >= sbuf.len() {
                    break;
                }
                let summed = s + buf.iter().nth(i + 1).unwrap() / 2.0;
                sbuf[i] = summed;
            }
            Some(sbuf)
        } else {
            None
        }
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
    pub fn volume(&mut self, vol: f32) {
        let mut v = self.volume.lock().unwrap();
        *v = vol;
    }
}

// callback when the audio output needs more data
fn write_audio(
    out_data: &mut [f32],
    _: &cpal::OutputCallbackInfo,
    buffer_mutex: &Arc<Mutex<Ringbuf<f32>>>,
    volume_mutex: &Arc<Mutex<f32>>,
) {
    let mut buffer = buffer_mutex.lock().unwrap();
    let vol = volume_mutex.lock().unwrap();

    let fill_zeros = out_data.len().saturating_sub(buffer.len());
    if fill_zeros > 0 {
        println!("write_audio: buffer underrun, filling with zeros");

        buffer.push_slice(&vec![0.0; fill_zeros]);
    }

    buffer.pop_slice(out_data);

    for sample in out_data.iter_mut() {
        *sample = *sample * *vol; // lol
    }

    if buffer.len() < 12000 {
        // TODO: actually fix this......
        println!("write_audio: buffer has {} left", buffer.len());
    }
}

#[derive(Debug, Clone)]
pub enum AudioCommand {
    AudioData(AudioData),
    Clear,
    Volume(f32),
    Pause(bool),
    Shutdown,
}

pub type AudioTx = mpsc::UnboundedSender<AudioCommand>;
pub type AudioRx = mpsc::UnboundedReceiver<AudioCommand>;
pub type AudioStatusTx = mpsc::UnboundedSender<AudioStatus>;
pub type AudioStatusRx = mpsc::UnboundedReceiver<AudioStatus>;

#[derive(Clone)]
pub struct AudioThreadHandle {
    tx: AudioTx,
}

impl AudioThreadHandle {
    pub fn send(&self, cmd: AudioCommand) -> Result<(), mpsc::error::SendError<AudioCommand>> {
        self.tx.send(cmd)
    }
}

pub struct AudioThread {
    rx: AudioRx,
    tx: AudioStatusTx,

    p: Player,

    wants_play: bool,
    paused: bool,

    finish_interval: Option<Interval>,
}

async fn maybe_interval_tick(maybe_interval: &mut Option<Interval>) -> Option<Instant> {
    match maybe_interval {
        Some(interval) => Some(interval.tick().await),
        None => None,
    }
}

impl AudioThread {
    // pass in all the channel parts again
    pub fn spawn(
        audio_tx: AudioTx,
        audio_rx: AudioRx,
        audio_status_tx: &AudioStatusTx,
    ) -> AudioThreadHandle {
        let audio_status_tx = audio_status_tx.to_owned();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .unwrap();

            let mut t = AudioThread::new(audio_rx, audio_status_tx);

            rt.block_on(t.run());
        });

        AudioThreadHandle { tx: audio_tx }
    }

    fn new(rx: AudioRx, tx: AudioStatusTx) -> Self {
        let player = Player::new();

        AudioThread {
            rx,
            tx,

            p: player,

            wants_play: false,
            paused: false,

            finish_interval: None,
        }
    }

    async fn run(&mut self) {
        loop {
            tokio::select! {
                Some(command) = self.rx.recv() => {
                    match command {
                        AudioCommand::Shutdown => {
                            break;
                        }
                        _ => self.handle_command(command).await,
                    }

                    if self.p.is_ready() && self.wants_play && !self.paused {
                        self.wants_play = false;

                        let _ = self.tx.send(AudioStatus::Buffering(false));

                        if !self.p.is_started() {
                            self.p.start()
                        } else {
                            self.p.resume()
                        }
                    }
                }

                Some(_) = maybe_interval_tick(&mut self.finish_interval) => {
                    // fake tick the frame count at the same rate
                    if let Some(prev) = self.p.last_frame {
                        self.p.last_frame = Some(prev + 1);
                    }

                    let _ = self
                        .tx
                        .send(AudioStatus::Elapsed(self.p.get_seconds_elapsed()));
                    let _ = self.tx.send(AudioStatus::Buffer(self.p.buffer_status()));

                    if self.p.is_empty() {
                        self.finish_interval = None;

                        self.p.pause();

                        let _ = self.tx.send(AudioStatus::Finished);
                    }
                }

                else => {
                    break
                }
            }
        }
    }

    async fn handle_command(&mut self, data: AudioCommand) {
        match data {
            AudioCommand::AudioData(d) => match d {
                AudioData::Frame(frame) => {
                    if frame.frame % 10 == 0 {
                        let _ = self
                            .tx
                            .send(AudioStatus::Elapsed(self.p.get_seconds_elapsed()));
                        let _ = self.tx.send(AudioStatus::Buffer(self.p.buffer_status()));
                    }

                    self.p.receive(frame.data, frame.frame);

                    if let Some(samples) = self.p.get_visualizer_buffer() {
                        let bars = calculate_visualizer(&samples);
                        let _ = self.tx.send(AudioStatus::Visualizer(bars));
                    }
                }
                AudioData::Start => {
                    self.wants_play = true;
                    self.finish_interval = None;

                    // if we don't have enough buffer yet, wait
                    // otherwise we had extra so we can just keep playing i guess?
                    // TODO: maybe this threshold should be lower so we can catch up only when we're actually low instead of every time we get a little low??
                    if !self.p.is_ready() {
                        let _ = self.tx.send(AudioStatus::Buffering(true));

                        // if we're continuing from another song, pause until we have enough
                        if self.p.is_started() {
                            self.p.pause();
                        }
                    }
                }

                // start an interval to finish playing stuff
                AudioData::Finish => {
                    let mut interval = tokio::time::interval(Duration::from_millis(10));
                    // consume the first immediate tick
                    interval.tick().await;
                    self.finish_interval = Some(interval);
                }
            },
            AudioCommand::Clear => {
                let _ = self.tx.send(AudioStatus::Elapsed(0));
                self.p.clear();
                self.p.pause();
            }
            AudioCommand::Volume(val) => {
                self.p.volume(val);
            }
            AudioCommand::Pause(paused) => {
                self.paused = paused;
                if paused {
                    if self.p.is_started() {
                        self.p.pause();
                    }
                } else {
                    self.wants_play = true;
                }
            }
            AudioCommand::Shutdown => unreachable!(),
        }
    }
}
