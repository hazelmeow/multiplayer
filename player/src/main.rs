use futures::StreamExt;
use protocol::network::FrameStream;
use protocol::{AudioFrame, AuthenticateRequest, GetInfo, Message, PlayingState};
use std::collections::HashMap;
use std::error::Error;

use tokio::time::timeout;
use tokio::{net::TcpStream, sync::mpsc};

mod audio;
use audio::AudioReader;

mod gui;
use fltk::prelude::{InputExt, WidgetExt, BrowserExt};

struct Connection {
    stream: FrameStream,
    my_id: String,
    
    play_state: PlayState,
    connected_users: HashMap<String, String>,
    // network messages
    message_rx: mpsc::UnboundedReceiver<Message>,
    message_tx: mpsc::UnboundedSender<Message>,

    // transmit audio to playback subsystem
    audio_tx: std::sync::mpsc::Sender<AudioData>,

    // get status feedback from audio playing
    audio_status_rx: mpsc::UnboundedReceiver<AudioStatus>,

    // from ui to logic
    ui_tx: mpsc::UnboundedSender<UIEvent>,
    ui_rx: mpsc::UnboundedReceiver<UIEvent>,

    // from logic to ui
    ui_sender: fltk::app::Sender<UIEvent>,
}
impl Connection {
    async fn create(
        addr: &str,
        id: &String,
        ui_sender: fltk::app::Sender<UIEvent>,
    ) -> Result<Self, Box<dyn Error>> {
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

        let audio_status_rx = Self::run_audio(audio_rx);

        let (ui_tx, mut ui_rx) = mpsc::unbounded_channel::<UIEvent>();

        /*
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
        }); */

        stream
            .send(&Message::GetInfo(GetInfo::QueueList))
            .await
            .unwrap();

        Ok(Self {
            stream,
            my_id: id.to_string(),
            play_state: PlayState::Stopped,
            connected_users: HashMap::new(),
            message_rx,
            message_tx,
            audio_tx,
            audio_status_rx,
            ui_rx,
            ui_tx,
            ui_sender,
        })
    }
    fn run_audio(
        audio_rx: std::sync::mpsc::Receiver<AudioData>,
    ) -> mpsc::UnboundedReceiver<AudioStatus> {
        // audio player thread

        // status channel
        let (tx, rx) = mpsc::unbounded_channel::<AudioStatus>();

        std::thread::spawn(move || {
            let mut p = audio::Player::new();

            let mut wants_play = false;

            while let Ok(data) = audio_rx.recv() {
                match data {
                    AudioData::Frame(frame) => {
                        if frame.frame % 10 == 0 {
                            //println!("player thread got frame {}", frame.frame);
                            tx.send(AudioStatus::Elapsed(p.get_seconds_elapsed()))
                                .unwrap();
                        }
                        p.receive(frame.data);
                    }
                    AudioData::Start => {
                        wants_play = true;
                    }
                    AudioData::StartLate(frame_id) => {
                        wants_play = true;
                        p.fake_frames_received(frame_id);
                    }
                    AudioData::Stop => {
                        p.pause();
                    }
                    AudioData::Finish => {
                        while !p.finish() {
                            println!("finishing.....");
                            std::thread::sleep(std::time::Duration::from_millis(20));
                        }
                        p.pause();
                        tx.send(AudioStatus::Finished).unwrap();
                    }
                    AudioData::Shutdown => break,
                    AudioData::Clear => {
                        tx.send(AudioStatus::Elapsed(0)).unwrap();
                        p.clear();
                    }
                }
                if p.ready() && wants_play {
                    p.play();
                    wants_play = false;
                    tx.send(AudioStatus::DoneBuffering).unwrap();
                }
            }
        });
        return rx;
    }
    async fn xmit_test(&mut self, path: String) -> bool {
        /* let track = protocol::Track {
            path: "blah".to_string(),
            owner: self.my_id.clone(),
            queue_position: 0,
        };

        self.stream.send(&Message::QueuePush(track)).await.unwrap();
        self.stream
            .send(&Message::GetInfo(GetInfo::QueueList))
            .await
            .unwrap(); */

        // temporarily don't hear our own frames
        let audio_tx_2 = self.audio_tx.clone();
        let message_tx_2 = self.message_tx.clone();

        let mut t: AudioReader = match AudioReader::load(&path) {
            Ok(t) => t,
            Err(_) => return false,
        };

        self.play_state = PlayState::Transmitting;

        tokio::spawn(async move {
            audio_tx_2.send(AudioData::Clear).unwrap();
            audio_tx_2.send(AudioData::Start).unwrap();

            message_tx_2
                .send(Message::PlayingState(PlayingState::Playing))
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

            audio_tx_2.send(AudioData::Finish).unwrap();
            message_tx_2
                .send(Message::PlayingState(PlayingState::Stopped))
                .unwrap();
        });
        return true
    }

    // lazy helper function
    fn ui_status(&mut self, text: &str) {
        self.ui_sender.send(
            UIEvent::Update(UIUpdateEvent::Status(text.to_string()))
        );
    }

    fn ui_status_default(&mut self) {
        let line = format!("Connected to {}", self.stream.get_addr().unwrap());
        self.ui_status(&line);
    }

    async fn main_loop(&mut self) {
        self.ui_status_default();
        loop {
            tokio::select! {
                // something to send
                Some(msg) = self.message_rx.recv() => {
                    // temporary hack, we need another channel for this stuff
                    if let Message::PlayingState(state) = &msg {
                        if *state == PlayingState::Stopped {
                            self.play_state = PlayState::Stopped;
                        }
                    }

                    self.stream.send(&msg).await.unwrap();
                },

                Some(msg) = self.audio_status_rx.recv() => match msg {
                    AudioStatus::Elapsed(secs) => {
                        let line = format!("{:02}:{:02}/00:00", secs / 60, secs % 60);
                        self.ui_sender.send(UIEvent::Update(UIUpdateEvent::SetTime(line)));
                    }
                    AudioStatus::DoneBuffering => {
                        self.ui_status(
                            match self.play_state {
                                PlayState::Transmitting => "Playing (transmitting)",
                                PlayState::Receiving => "Playing (receiving from ...)",
                                PlayState::Stopped => unreachable!(),

                            }
                        );
                    }
                    AudioStatus::Finished => {
                        self.ui_status_default();
                    }
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
                                    // we're late.... try and catch up will you
                                    self.play_state = PlayState::Receiving;
                                    self.audio_tx.send(AudioData::StartLate(f.frame as usize)).unwrap();
                                    self.audio_tx.send(AudioData::Frame(f)).unwrap();
                                }
                            },
                            Message::PlayingState(s) => {
                                match s {
                                    PlayingState::Playing => {
                                        self.play_state = PlayState::Receiving;
                                        self.audio_tx.send(AudioData::Start).unwrap();
                                    },
                                    PlayingState::Stopped => {
                                        self.play_state = PlayState::Stopped;
                                        self.audio_tx.send(AudioData::Finish).unwrap();
                                    },
                                    _ => {}
                                }
                            }
                            Message::ConnectedUsers(list) => {
                                self.connected_users = list;
                                self.ui_sender.send(UIEvent::Update(UIUpdateEvent::UpdateUserList(self.connected_users.clone())));
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
                        UIEvent::Play(file) => {
                            // temporary api lol
                            if self.play_state == PlayState::Stopped {
                                // check what we're supposed to do?
                                // really shouldn't the server keep track of this stuff.....

                                // for now just send
                                if self.xmit_test(file).await {
                                    self.ui_status("Buffering...");
                                }
                            } else {
                                // no-op.. either already transmitting or receiving
                            }
                        }
                        UIEvent::Pause => {

                        }
                        UIEvent::Stop => {
                            match self.play_state {
                                PlayState::Receiving => {
                                    // just stop caring about audioframes
                                    self.play_state = PlayState::Stopped;
                                },
                                PlayState::Transmitting => {

                                },
                                PlayState::Stopped => {},
                            };

                        }
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
                        UIEvent::Quit => break,
                        _ => {}
                    }
                }

            }
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let my_id = args[1].clone();

    let (sender, receiver) = fltk::app::channel();

    let connection = Connection::create("127.0.0.1:8080", &my_id.clone(), sender.clone())
        .await
        .unwrap();
    let mut ui_state = UIState { connection };

    let ui_tx = ui_state.connection.ui_tx.clone();

    std::thread::spawn(move || {
        let app = fltk::app::App::default();
        let widgets = fltk_theme::WidgetTheme::new(fltk_theme::ThemeType::Classic);
        widgets.apply();

        let theme = fltk_theme::ColorTheme::new(fltk_theme::color_themes::GRAY_THEME);
        theme.apply();

        let mut gui = gui::UserInterface::make_window(sender.clone());
        gui.main_win.show();
        gui.main_win.emit(sender, UIEvent::Quit);

        while app.wait() {
            if let Some(msg) = receiver.recv() {
                println!("got event from app: {:?}", msg);
                ui_tx.send(msg.clone()).unwrap();
                // only deal with ui-relevant stuff here
                match msg {
                    UIEvent::Update(evt) => match evt {
                        UIUpdateEvent::SetTime(val) => {
                            gui.lbl_time.set_label(&val);
                        }
                        UIUpdateEvent::Status(val) => {
                            gui.status_field.set_label(&val);
                        }
                        UIUpdateEvent::UpdateUserList(val) => {
                            gui.users.clear();
                            for (id, name) in val.iter() {
                                let line = if id == &my_id {
                                    format!("@b* you")
                                } else {
                                    format!("* {} ({})", name, id)
                                };
                                gui.users.add(&line);
                            }
                        }
                        _ => {
                            dbg!(evt);
                        }
                    },
                    UIEvent::BtnPlay => {
                        ui_tx.send(UIEvent::Play(gui.temp_input.value())).unwrap();
                    }
                    UIEvent::Quit => break,
                    UIEvent::Test(s) => {
                        dbg!(gui.temp_input.value());
                    }
                    _ => {}
                }
            }
        }
    });

    ui_state.connection.main_loop().await;

    println!("disconnected");

    Ok(())
}

// only temporary
#[derive(Debug, Clone)]
pub enum UIEvent {
    BtnPlay, // bleh
    Update(UIUpdateEvent),
    Play(String),
    Stop,
    Pause,
    Test(String),
    GetInfo(GetInfo),
    TestAddQueue,
    Quit,
}

#[derive(Debug, Clone)]
pub enum UIUpdateEvent {
    SetTime(String),
    UpdateUserList(HashMap<String, String>),
    Status(String),
}

struct UIState {
    connection: Connection,
}

struct QueueItem {
    track: protocol::Track,
    player_id: usize,
}
enum AudioData {
    Start,
    StartLate(usize),
    Frame(AudioFrame),
    Stop,
    Finish,
    Clear,
    Shutdown,
}

#[derive(Debug, Clone)]
enum AudioStatus {
    Elapsed(usize),
    DoneBuffering,
    Finished
}

#[derive(Debug, PartialEq)]
enum PlayState {
    Stopped,
    Receiving,
    Transmitting,
}
