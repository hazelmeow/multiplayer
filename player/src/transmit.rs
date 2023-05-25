use protocol::{AudioData, Message};
use tokio::time::Interval;

use crate::audio::AudioReader;

#[derive(Debug, Clone)]
pub enum TransmitCommand {
    Start(String),
    Stop,
}

//temp
type MessageTx = tokio::sync::mpsc::UnboundedSender<Message>;
type AudioTx = std::sync::mpsc::Sender<AudioData>;

type TransmitTx = tokio::sync::mpsc::UnboundedSender<TransmitCommand>;
type TransmitRx = tokio::sync::mpsc::UnboundedReceiver<TransmitCommand>;

pub struct TransmitThreadHandle {
    tx: TransmitTx,
    // join_handle: JoinHandle<()>,
}

impl TransmitThreadHandle {
    pub fn send(
        &mut self,
        cmd: TransmitCommand,
    ) -> Result<(), tokio::sync::mpsc::error::SendError<TransmitCommand>> {
        self.tx.send(cmd)
    }
}

pub struct TransmitThread {
    rx: TransmitRx,
    message_tx: MessageTx,
    audio_tx: AudioTx,

    audio_reader: Option<AudioReader>,
    interval: Interval,
}

impl TransmitThread {
    // pass in the channel parts because we need to create all the channels before spawning threads
    // since they depend on each other...
    pub fn spawn(
        transmit_tx: TransmitTx,
        transmit_rx: TransmitRx,
        message_tx: &MessageTx,
        audio_tx: &AudioTx,
    ) -> TransmitThreadHandle {
        let message_tx = message_tx.to_owned();
        let audio_tx = audio_tx.to_owned();

        let _join_handle = tokio::spawn(async move {
            let mut t = TransmitThread::new(transmit_rx, message_tx, audio_tx);
            t.run().await;
        });

        TransmitThreadHandle {
            tx: transmit_tx,
            // join_handle,
        }
    }

    fn new(rx: TransmitRx, message_tx: MessageTx, audio_tx: AudioTx) -> Self {
        // ok let's actually do the math for this
        // each frame is 960 samples
        // at 48k that means it's 20ms per frame
        // SO we need to send a frame at least every 20ms.
        // i think.................
        // LOL OK it's two frames idk why maybe because it's stereo interleaved??????
        let interval = tokio::time::interval(tokio::time::Duration::from_millis(200));

        TransmitThread {
            rx,
            message_tx,
            audio_tx,

            audio_reader: None,
            interval,
        }
    }

    fn send_both(&self, data: AudioData) {
        self.audio_tx.send(data.clone()).unwrap();
        self.message_tx.send(Message::AudioData(data)).unwrap();
    }

    async fn run(&mut self) {
        loop {
            tokio::select! {
                Some(cmd) = self.rx.recv() => {
                    self.handle_command(cmd)
                }

                // tick loop
                _ = self.interval.tick() => {
                    self.handle_tick();
                }
            }
        }
    }

    fn handle_command(&mut self, cmd: TransmitCommand) {
        match cmd {
            TransmitCommand::Start(path) => {
                if self.audio_reader.is_some() {
                    eprintln!("TransmitCommand::Start failed, already transmitting?");
                    return;
                }

                if let Ok(r) = AudioReader::load(&path) {
                    self.audio_reader = Some(r);

                    self.send_both(AudioData::Start);
                } else {
                    // failed to load file, need to skip it somehow?
                }
            }
            TransmitCommand::Stop => {
                self.send_both(AudioData::Finish);
                if let Some(t) = self.audio_reader.as_mut() {
                    t.set_finished();
                }
            }
        }
    }

    fn handle_tick(&mut self) {
        // TODO: only run the timer when we need it?
        if let Some(t) = self.audio_reader.as_mut() {
            for _ in 0..20 {
                // if ran out in the middle of this "tick" of 20 frames
                if t.finished() {
                    break;
                };

                let f = t.encode_frame();
                if let Ok(frame) = f {
                    // borrow checker doesn't like self.send_both() here since we already borrow self or something
                    let data = AudioData::Frame(frame);
                    self.audio_tx.send(data.clone()).unwrap();
                    self.message_tx.send(Message::AudioData(data)).unwrap();
                } else {
                    break; // we're done i guess
                }
            }

            // we encoded the entire file
            if t.finished() {
                self.message_tx
                    .send(Message::AudioData(AudioData::Finish))
                    .unwrap();

                self.audio_reader = None;
            };
        }
    }
}
