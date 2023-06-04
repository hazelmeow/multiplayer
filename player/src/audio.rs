use std::mem::MaybeUninit;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, Stream};
use opus::Decoder;
use ringbuf::{LocalRb, Rb};

use protocol::AudioData;

use crate::gui::visualizer::calculate_visualizer;
use crate::AudioStatus;

type Ringbuf<T> = LocalRb<T, Vec<MaybeUninit<T>>>;

const PLAYER_BUFFER_SIZE: usize = 48000 * 10; // 10s of samples?

pub struct Player {
    decoder: Decoder,
    buffer: Arc<Mutex<Ringbuf<f32>>>,
    stream: Option<Stream>,
    frames_received: usize,
    volume: Arc<Mutex<f32>>,
}
impl Player {
    pub fn new() -> Self {
        Self {
            decoder: Decoder::new(48000, opus::Channels::Stereo).unwrap(),
            buffer: Arc::new(Mutex::new(Ringbuf::<f32>::new(PLAYER_BUFFER_SIZE))),
            stream: None,
            frames_received: 0,
            volume: Arc::new(Mutex::new(1.0)),
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
                move |data, info| write_audio::<f32>(data, info, &buffer_handle, &volume_handle),
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
    pub fn get_visualizer_buffer(&mut self) -> Option<[f32; 4096]> {
        let buf = self.buffer.lock().unwrap();
        let copied = buf.as_slices();

        let mut sbuf = [0.0; 4096];
        if copied.0.len() > sbuf.len() {
            for (i, s) in copied.0.iter().step_by(2).enumerate() {
                if i >= sbuf.len() {
                    break;
                }
                let summed = s + copied.0[i + 1] / 2.0;
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
fn write_audio<T: Sample>(
    out_data: &mut [f32],
    _: &cpal::OutputCallbackInfo,
    buffer_mutex: &Arc<Mutex<Ringbuf<f32>>>,
    volume_mutex: &Arc<Mutex<f32>>,
) {
    let mut buffer = buffer_mutex.lock().unwrap();
    let vol = volume_mutex.lock().unwrap();
    if out_data.len() < buffer.len() {
        buffer.pop_slice(out_data);

        for sample in out_data.iter_mut() {
            *sample = *sample * *vol; // lol
        }
    } else {
        // uhhhh
        println!("write_audio: buffer underrun!! (but no panic)");
    }
    if buffer.len() < 12000 {
        // TODO: actually fix this......
        println!("write_audio: buffer has {} left", buffer.len());
    }
}

#[derive(Debug, Clone)]
pub enum AudioCommand {
    AudioData(AudioData),
    StartLate(usize),
    Clear,
    Volume(f32),
    Shutdown,
}

pub type AudioTx = std::sync::mpsc::Sender<AudioCommand>;
pub type AudioRx = std::sync::mpsc::Receiver<AudioCommand>;
pub type AudioStatusTx = tokio::sync::mpsc::UnboundedSender<AudioStatus>;
pub type AudioStatusRx = tokio::sync::mpsc::UnboundedReceiver<AudioStatus>;

pub struct AudioThreadHandle {
    tx: AudioTx,
}

impl AudioThreadHandle {
    pub fn send(
        &mut self,
        cmd: AudioCommand,
    ) -> Result<(), std::sync::mpsc::SendError<AudioCommand>> {
        self.tx.send(cmd)
    }
}

pub struct AudioThread {
    rx: AudioRx,
    tx: AudioStatusTx,
    p: Player,
    wants_play: bool,
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
            let mut t = AudioThread::new(audio_rx, audio_status_tx);
            t.run();
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
        }
    }

    fn run(&mut self) {
        while let Ok(data) = self.rx.recv() {
            match data {
                AudioCommand::AudioData(d) => match d {
                    AudioData::Frame(frame) => {
                        if frame.frame % 10 == 0 {
                            let _ = self
                                .tx
                                .send(AudioStatus::Elapsed(self.p.get_seconds_elapsed()));
                            let _ = self.tx.send(AudioStatus::Buffer(self.p.buffer_status()));
                        }

                        self.p.receive(frame.data);
                        if let Some(samples) = self.p.get_visualizer_buffer() {
                            let bars = calculate_visualizer(&samples);
                            let _ = self.tx.send(AudioStatus::Visualizer(bars));
                        }
                    }
                    AudioData::Start => {
                        self.wants_play = true;
                        self.p.fake_frames_received(0);
                        let _ = self.tx.send(AudioStatus::Buffering(true));
                    }
                    AudioData::Finish => {
                        while !self.p.finish() {
                            std::thread::sleep(std::time::Duration::from_millis(20));
                        }
                        self.p.pause();
                        let _ = self.tx.send(AudioStatus::Finished);
                    }
                    AudioData::Resume => {
                        self.p.resume();
                    }
                },
                AudioCommand::StartLate(frame_id) => {
                    self.wants_play = true;
                    self.p.fake_frames_received(frame_id);
                }

                AudioCommand::Clear => {
                    let _ = self.tx.send(AudioStatus::Elapsed(0));
                    self.p.clear();
                }
                AudioCommand::Volume(val) => {
                    self.p.volume(val);
                }
                AudioCommand::Shutdown => {
                    break;
                }
            }
            if self.p.is_ready() && self.wants_play {
                self.wants_play = false;

                let _ = self.tx.send(AudioStatus::Buffering(false));

                if !self.p.is_started() {
                    self.p.start()
                } else {
                    self.p.resume()
                }
            }
        }
    }
}
