use futures::StreamExt;
use protocol::network::FrameStream;
use protocol::{AudioFrame, AuthenticateRequest, GetInfo, Message, PlaybackState, PlayingState};
use std::error::Error;
use std::io;
use tokio::time::timeout;
use tokio::{net::TcpStream, sync::mpsc};

mod audio;
use audio::AudioReader;
struct Connection {
    stream: FrameStream,
    my_id: String,

    play_state: PlayState,

    // network messages
    message_rx: mpsc::UnboundedReceiver<Message>,
    message_tx: mpsc::UnboundedSender<Message>,

    // transmit audio to playback subsystem
    audio_tx: std::sync::mpsc::Sender<AudioData>,

    // receive user input from keyboard
    ui_rx: mpsc::UnboundedReceiver<UIEvent>,
}
impl Connection {
    async fn create(addr: &str, id: &String) -> Result<Self, Box<dyn Error>> {
        let tcp_stream = TcpStream::connect(addr).await?;
        let mut stream = FrameStream::new(tcp_stream);

        // handshake
        stream
            .send(&Message::Handshake("meow".to_string()))
            .await
            .unwrap();

        let hs_timeout = std::time::Duration::from_millis(1000);
        if let Ok(hsr) = timeout(hs_timeout, stream.get_inner().next()).await {
            match hsr {
                Some(Ok(r)) => {
                    let response: Message = bincode::deserialize(&r).unwrap();
                    if let Message::Handshake(r) = response {
                        if r != "nyaa" {
                            return Err("invalid handshake".into());
                        }
                    }
                }
                Some(Err(_)) | None => {
                    return Err("handshake failed".into());
                }
            }
        } else {
            return Err("handshake timed out".into());
        };

        // handshake okay, send authentication
        let args: Vec<String> = std::env::args().collect();
        let my_id = &args[1];
        println!("connecting as {:?}", my_id);

        stream
            .send(&Message::Authenticate(AuthenticateRequest {
                id: my_id.clone(),
                name: my_id.clone().repeat(5), // TODO temp
            }))
            .await
            .unwrap();

        let (message_tx, mut message_rx) = mpsc::unbounded_channel::<Message>();

        let (audio_tx, mut audio_rx) = std::sync::mpsc::channel::<AudioData>();

        Self::run_audio(audio_rx);

        let (ui_tx, mut ui_rx) = mpsc::unbounded_channel::<UIEvent>();

        std::thread::spawn(move || loop {
            let mut buf = String::new();
            io::stdin().read_line(&mut buf).unwrap();

            let line = buf.trim();
            let r = if line == "q" {
                Some(UIEvent::Quit)
            } else if line == "gq" {
                Some(UIEvent::GetInfo(GetInfo::QueueList))
            } else if line == "aq" {
                Some(UIEvent::TestAddQueue)
            } else if line.starts_with("p ") {
                let p = line.split_ascii_whitespace().nth(1);
                if let Some(path) = p {
                    Some(UIEvent::TestPlay(path.to_string()))
                } else {
                    None
                }
            } else if line.starts_with("c ") {
                let mut c = line
                    .split_ascii_whitespace()
                    .into_iter()
                    .collect::<Vec<&str>>();
                if c.len() < 2 {
                    None
                } else {
                    c.remove(0);
                    let t = c.join(" ");
                    Some(UIEvent::Test(t.to_string()))
                }
            } else {
                println!("?");
                None
            };

            if let Some(x) = r {
                ui_tx.send(x).unwrap();
            }
        });

        stream
            .send(&Message::GetInfo(GetInfo::QueueList))
            .await
            .unwrap();

        Ok(Self {
            stream,
            my_id: id.to_string(),
            play_state: PlayState::Stopped,
            message_rx,
            message_tx,
            audio_tx,
            ui_rx,
        })
    }
    fn run_audio(audio_rx: std::sync::mpsc::Receiver<AudioData>) {
        // audio player thread
        std::thread::spawn(move || {
            let mut p = audio::Player::new();

            let mut wants_play = false;

            while let Ok(data) = audio_rx.recv() {
                match data {
                    AudioData::Frame(frame) => {
                        if frame.frame % 10 == 0 {
                            println!("player thread got frame {}", frame.frame);
                        }
                        p.receive(frame.data);
                    }
                    AudioData::Start => {
                        wants_play = true;
                    }
                    AudioData::Stop => {
                        p.pause();
                    }
                    AudioData::Shutdown => break,
                    AudioData::Clear => {
                        p.clear();
                    }
                }
                if p.ready() && wants_play {
                    p.play();
                    wants_play = false;
                }
            }
        });
    }
    async fn xmit_test(&mut self, path: String) {
        let track = protocol::Track {
            path: "blah".to_string(),
            owner: self.my_id.clone(),
            queue_position: 0,
        };

        self.stream.send(&Message::QueuePush(track)).await.unwrap();
        self.stream
            .send(&Message::GetInfo(GetInfo::QueueList))
            .await
            .unwrap();

        // temporarily don't hear our own frames
        let audio_tx_2 = self.audio_tx.clone();
        let message_tx_2 = self.message_tx.clone();

        self.play_state = PlayState::Transmitting;
        tokio::spawn(async move {
            let mut t: AudioReader = match AudioReader::load(&path) {
                Ok(t) => t,
                Err(_) => return,
            };

            audio_tx_2.send(AudioData::Clear).unwrap();
            audio_tx_2.send(AudioData::Start).unwrap();

            message_tx_2
                .send(Message::PlaybackState(PlaybackState {
                    state: PlayingState::Playing,
                }))
                .unwrap();

            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(200));
            loop {
                // ok let's actually do the math for this
                // each frame is 960 samples
                // at 48k that means it's 20ms per frame
                // SO we need to send a frame at least every 20ms.
                // i think.................
                // LOL OK it's two frames idk why maybe because it's stereo interleaved??????
                interval.tick().await;

                for _ in 0..20 {
                    // stop in the middle of this "tick" of 20 frames
                    if t.finished() {
                        break;
                    };

                    let f = t.encode_frame();
                    if let Ok(frame) = f {
                        audio_tx_2.send(AudioData::Frame(frame.clone())).unwrap();
                        message_tx_2.send(Message::AudioFrame(frame)).unwrap();
                    } else {
                        break; // we're done i guess
                    }
                }

                // we encoded the entire file
                if t.finished() {
                    break;
                };
            }

            // TODO: should be "End"? and should finish playing without stopping immediately
            audio_tx_2.send(AudioData::Stop).unwrap();
            message_tx_2
                .send(Message::PlaybackState(PlaybackState {
                    state: PlayingState::Stopped,
                }))
                .unwrap();
        });
    }
    async fn main_loop(&mut self) {
        loop {
            tokio::select! {
                // something to send
                Some(msg) = self.message_rx.recv() => {
                    // temporary hack, we need another channel for this stuff
                    if let Message::PlaybackState(state) = &msg {
                        if state.state == PlayingState::Stopped {
                            self.play_state = PlayState::Stopped;
                        }
                    }

                    self.stream.send(&msg).await.unwrap();
                },

                // tcp message
                result = self.stream.get_inner().next() => match result {
                    Some(Ok(bytes)) => {
                        let msg: Message =
                            bincode::deserialize(&bytes).expect("failed to deserialize message");

                        match msg {
                            Message::AudioFrame(f) => {
                                if self.play_state == PlayState::Receiving {
                                    self.audio_tx.send(AudioData::Frame(f)).unwrap();
                                } else {
                                    println!("got audio frame when not receiving?!");
                                }
                            },
                            Message::PlaybackState(s) => {
                                match s.state {
                                    PlayingState::Playing => {
                                        self.play_state = PlayState::Receiving;
                                        self.audio_tx.send(AudioData::Start).unwrap();
                                    },
                                    PlayingState::Stopped => {
                                        self.play_state = PlayState::Stopped;
                                        self.audio_tx.send(AudioData::Stop).unwrap();
                                    },
                                    _ => {}
                                }
                            }

                            _ => {
                                println!("received message: {:?}", msg);
                            }
                        }
                    }

                    Some(Err(e)) => {
                        println!("error occurred while processing message: {:?}", e)
                    }

                    // socket disconnected
                    None => break,
                },

                // some ui event
                Some(event) = self.ui_rx.recv() => {
                    match event {
                        UIEvent::Test(text) => {
                            self.message_tx.send(Message::Text(text)).unwrap();
                        },
                        UIEvent::GetInfo(i) => {
                            self.message_tx.send(Message::GetInfo(i)).unwrap();
                        },
                        UIEvent::TestAddQueue => {
                            let track = protocol::Track {
                                path: "blah".to_string(),
                                owner: self.my_id.clone(), // temp
                                queue_position: 0,
                            };
                            self.message_tx.send(Message::QueuePush(track)).unwrap();
                            self.message_tx.send(Message::GetInfo(GetInfo::QueueList)).unwrap();
                            // and uhh put it in idk
                        },
                        UIEvent::TestPlay(path) => {
                            if self.play_state == PlayState::Stopped {
                                println!("{:?}", self.play_state);
                                self.xmit_test(path).await;
                            }
                        },
                        UIEvent::Quit => break,
                    }
                }

            }
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let my_id = &args[1];

    let mut conn = Connection::create("127.0.0.1:8080", my_id).await.unwrap();
    conn.main_loop().await;

    println!("disconnected");

    Ok(())
}

// only temporary
#[derive(Debug)]
enum UIEvent {
    Test(String),
    GetInfo(GetInfo),
    TestAddQueue,
    TestPlay(String),
    Quit,
}

struct QueueItem {
    track: protocol::Track,
    player_id: usize,
}
enum AudioData {
    Start,
    Frame(AudioFrame),
    Stop,
    Clear,
    Shutdown,
}

#[derive(Debug, PartialEq)]
enum PlayState {
    Stopped,
    Receiving,
    Transmitting,
}
