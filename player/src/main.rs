use fltk::app;
use fltk::window::DoubleWindow;
use futures::StreamExt;
use protocol::network::FrameStream;
use protocol::{AudioData, AuthenticateRequest, GetInfo, Message, Track};
use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::path::PathBuf;
use tokio::sync::mpsc::UnboundedSender;

use tokio::time::timeout;
use tokio::{net::TcpStream, sync::mpsc};

mod audio;
use audio::AudioReader;

mod gui;
use fltk::prelude::{BrowserExt, ValuatorExt, WidgetBase, WidgetExt, WindowExt};

struct Connection {
    stream: FrameStream,
    my_id: String,

    playing: Option<Track>,
    queue: VecDeque<Track>,
    connected_users: HashMap<String, String>,

    // we only need to use this once, when we first connect
    // if we see the start message -> we are in sync
    // if we see a frame without start message -> we need to catch up first
    is_synced: bool,

    // network messages
    message_rx: mpsc::UnboundedReceiver<Message>,
    message_tx: mpsc::UnboundedSender<Message>,

    // transmit audio to playback subsystem
    audio_tx: std::sync::mpsc::Sender<AudioData>,

    // get status feedback from audio playing
    audio_status_rx: mpsc::UnboundedReceiver<AudioStatus>,

    // controls transmitting thread
    transmit_tx: mpsc::UnboundedSender<TransmitCommand>,

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

        let (message_tx, message_rx) = mpsc::unbounded_channel::<Message>();

        let (audio_tx, audio_rx) = std::sync::mpsc::channel::<AudioData>();
        let audio_status_rx = Self::run_audio(audio_rx);

        let (transmit_tx, transmit_rx) = mpsc::unbounded_channel::<TransmitCommand>();
        Self::run_transmit(audio_tx.clone(), message_tx.clone(), transmit_rx);

        let (ui_tx, ui_rx) = mpsc::unbounded_channel::<UIEvent>();

        stream
            .send(&Message::GetInfo(GetInfo::Queue))
            .await
            .unwrap();

        Ok(Self {
            stream,
            my_id: id.to_string(),

            playing: None,
            queue: VecDeque::new(),
            connected_users: HashMap::new(),

            is_synced: false,

            message_rx,
            message_tx,
            audio_tx,
            audio_status_rx,
            transmit_tx,
            ui_rx,
            ui_tx,
            ui_sender,
        })
    }

    // determine PlayState from playing/queue state
    fn play_state(&self) -> PlayState {
        if let Some(t) = &self.playing {
            if t.owner == self.my_id {
                PlayState::Transmitting
            } else {
                PlayState::Receiving
            }
        } else {
            PlayState::Empty
        }
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
                            tx.send(AudioStatus::Elapsed(p.get_seconds_elapsed()))
                                .unwrap();
                        }

                        p.receive(frame.data);
                    }
                    AudioData::Start => {
                        wants_play = true;
                        p.fake_frames_received(0);
                        tx.send(AudioStatus::Buffering).unwrap();
                    }
                    AudioData::StartLate(frame_id) => {
                        wants_play = true;
                        p.fake_frames_received(frame_id);
                    }
                    AudioData::Stop => {
                        p.pause();
                    }
                    AudioData::Resume => {
                        p.resume();
                    }
                    AudioData::Finish => {
                        println!("finishing.....");
                        while !p.finish() {
                            std::thread::sleep(std::time::Duration::from_millis(20));
                        }
                        p.pause();
                        tx.send(AudioStatus::Finished).unwrap();
                    }
                    AudioData::Clear => {
                        tx.send(AudioStatus::Elapsed(0)).unwrap();
                        p.clear();
                    }
                    AudioData::Shutdown => break,
                }
                if p.is_ready() && wants_play {
                    wants_play = false;

                    tx.send(AudioStatus::DoneBuffering).unwrap();

                    if !p.is_started() {
                        p.start()
                    } else {
                        p.resume()
                    }
                } else if !p.is_ready() && wants_play && p.is_started() {
                    tx.send(AudioStatus::DoneBuffering).unwrap();
                }
            }
        });
        return rx;
    }

    fn run_transmit(
        audio_tx: std::sync::mpsc::Sender<AudioData>,
        message_tx: tokio::sync::mpsc::UnboundedSender<Message>,
        mut rx: tokio::sync::mpsc::UnboundedReceiver<TransmitCommand>,
    ) {
        tokio::spawn(async move {
            // helper to send to audio thread and other clients
            let message_tx_2 = message_tx.clone();
            let send_audio_data = move |data: AudioData| {
                audio_tx.send(data.clone()).unwrap();
                message_tx.send(Message::AudioData(data)).unwrap();
            };

            let mut audio_reader: Option<AudioReader> = None;

            // ok let's actually do the math for this
            // each frame is 960 samples
            // at 48k that means it's 20ms per frame
            // SO we need to send a frame at least every 20ms.
            // i think.................
            // LOL OK it's two frames idk why maybe because it's stereo interleaved??????
            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(200));

            loop {
                tokio::select! {
                    Some(cmd) = rx.recv() => {
                        match cmd {
                            TransmitCommand::Start(path) => {
                                if audio_reader.is_some() {
                                    eprintln!("TransmitCommand::Start failed, already transmitting?");
                                    return;
                                }

                                if let Ok(r) = AudioReader::load(&path) {
                                    audio_reader = Some(r);

                                    send_audio_data(AudioData::Start);
                                } else {
                                    // failed to load file, need to skip it somehow?
                                }
                            }
                            TransmitCommand::Stop => {
                                send_audio_data(AudioData::Finish);
                                if let Some(t) = audio_reader.as_mut() {
                                    t.set_finished();
                                }
                            }
                        }
                    }

                    // tick loop
                    _ = interval.tick() => {
                        // TODO: only run the timer when we need it?
                        if let Some(t) = audio_reader.as_mut() {
                            for _ in 0..20 {
                                // if ran out in the middle of this "tick" of 20 frames
                                if t.finished() {
                                    break;
                                };

                                let f = t.encode_frame();
                                if let Ok(frame) = f {
                                    send_audio_data(AudioData::Frame(frame));
                                } else {
                                    break; // we're done i guess
                                }
                            }

                            // we encoded the entire file
                            if t.finished() {
                                message_tx_2.send(Message::AudioData(AudioData::Finish)).unwrap();

                                audio_reader = None;
                            };
                        }
                    }
                }
            }
        });
    }

    // lazy helper function
    fn ui_status(&mut self, text: &str) {
        self.ui_sender
            .send(UIEvent::Update(UIUpdateEvent::Status(text.to_string())));
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
                    self.stream.send(&msg).await.unwrap();
                },

                // something from audio
                Some(msg) = self.audio_status_rx.recv() => {
                    //dbg!(&msg);
                    match msg {
                        AudioStatus::Elapsed(secs) => {
                            let total = match &self.playing {
                                Some(t) => {
                                    t.metadata.duration
                                },
                                None => 0
                            };
                            self.ui_sender.send(UIEvent::Update(UIUpdateEvent::SetTime(secs, total)));
                        }
                        AudioStatus::Buffering => {
                            self.ui_status(
                                "Buffering...",
                            );
                        }
                        AudioStatus::DoneBuffering => {
                            match self.play_state() {
                                PlayState::Transmitting => {
                                    self.ui_status(
                                        "Playing (transmitting)",
                                    );
                                }
                                PlayState::Receiving => {
                                    if let Some(t) = &self.playing {
                                        let owner = t.owner.to_owned();
                                        self.ui_status(format!("Playing (receiving from {:?})", owner).as_str());
                                    } else {
                                        //??
                                        self.ui_status_default();
                                    }
                                },
                                PlayState::Empty => unreachable!(),
                            }
                        }
                        AudioStatus::Finished => {
                            self.ui_status_default();
                        }
                    }
                },

                // tcp message
                result = self.stream.get_inner().next() => match result {
                    Some(Ok(bytes)) => {
                        let msg: Message =
                            bincode::deserialize(&bytes).expect("failed to deserialize message");

                        match msg {
                            Message::Info(
                                info
                            ) => {
                                match info {
                                    protocol::Info::Queue(queue) => {
                                        // TODO
                                        println!("got queue: {:?}", queue);
                                        self.ui_sender.send(UIEvent::Update(UIUpdateEvent::UpdateQueue(self.playing.clone(), queue.clone())));
                                        self.queue = queue;
                                    },
                                    protocol::Info::Playing(playing) => {
                                        // TODO
                                        println!("got playing: {:?}", playing);

                                        self.playing = playing.clone();

                                        if let Some(t) = playing {
                                            if t.owner == self.my_id {
                                                self.transmit_tx.send(TransmitCommand::Start(t.path)).unwrap();
                                            }
                                        }
                                    },
                                    protocol::Info::ConnectedUsers(list) => {
                                        self.connected_users = list;
                                        self.ui_sender.send(UIEvent::Update(UIUpdateEvent::UpdateUserList(self.connected_users.clone())));
                                    }
                                    _ => {}
                                }
                            }
                            Message::AudioData(data) => {
                                match data {
                                    protocol::AudioData::Frame(f) => {
                                        if !self.is_synced {
                                            // we're late.... try and catch up will you
                                            self.is_synced = true;
                                            self.audio_tx.send(AudioData::StartLate(f.frame as usize)).unwrap();
                                        }

                                        self.audio_tx.send(AudioData::Frame(f)).unwrap();
                                    }
                                    protocol::AudioData::Start => {
                                        self.is_synced = true;
                                        self.audio_tx.send(data).unwrap();
                                    }
                                    _ => {
                                        // forward to audio thread
                                        self.audio_tx.send(data).unwrap();
                                    }
                                }

                            },

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
                        UIEvent::Play(path) => {
                            // temporary api lol
                            // can open?
                            // TODO: this blocks and fucks everything up if it takes too long to read....
                            self.ui_status("Loading file...");
                            let file = path.into_os_string().into_string().unwrap();
                            if let Ok(f) = std::fs::File::open(&file) {
                                let (_, mut decoder) = AudioReader::new_decoder(f);
                                if let Ok((_, _, _, metadata)) = AudioReader::load_info(&file, &mut decoder, true) {
                                    let track = protocol::Track {
                                        path: file,
                                        owner: self.my_id.clone(),
                                        metadata
                                    };
                                    self.message_tx.send(Message::QueuePush(track)).unwrap();
                                }
                            }

                        }
                        UIEvent::Pause => {

                        }
                        UIEvent::Stop => {
                            // send network message to stop?
                            match self.play_state() {
                                PlayState::Transmitting => {
                                    self.transmit_tx.send(TransmitCommand::Stop).unwrap();
                                }
                                _ => {}
                            }
                        }
                        UIEvent::Test(text) => {
                            self.message_tx.send(Message::Text(text)).unwrap();
                        },
                        UIEvent::GetInfo(i) => {
                            self.message_tx.send(Message::GetInfo(i)).unwrap();
                        },
                        UIEvent::Quit => break,
                        _ => {}
                    }
                }

            }
        }
    }
}

#[tokio::main(flavor = "multi_thread")]
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

        let mut gui = gui::MainWindow::make_window(sender.clone());
        let mut queue_gui = gui::QueueWindow::make_window(sender.clone());

        // stuff for cool custom titlebars
        gui.main_win.set_border(false);
        gui.main_win.show();
        gui.fix_taskbar_after_show();

        gui.main_win.emit(sender, UIEvent::Quit);

        let mut x = 0;
        let mut y = 0;
        let mut dnd = false;
        let mut released = false;

        let mut mover = move |w: &mut DoubleWindow, ev, tx: UnboundedSender<UIEvent>| match ev {
            fltk::enums::Event::Push => {
                let coords = app::event_coords();
                x = coords.0;
                y = coords.1;
                true
            }
            fltk::enums::Event::Drag => {
                //if y < 20 {
                w.set_pos(app::event_x_root() - x, app::event_y_root() - y);
                true
                //} else {
                //    false
                //}
            }
            fltk::enums::Event::DndEnter => {
                println!("blah");
                dnd = true;
                true
            }
            fltk::enums::Event::DndDrag => true,
            fltk::enums::Event::DndRelease => {
                released = true;
                true
            }
            fltk::enums::Event::Paste => {
                if dnd && released {
                    let path = app::event_text();
                    let path = path.trim();
                    let path = path.replace("file://", "");
                    let path = std::path::PathBuf::from(&path);
                    if path.exists() {
                        // we use a timeout to avoid pasting the path into the buffer
                        app::add_timeout3(0.0, {
                            move |_| {
                                println!("got dropped file {:?}", path);
                                tx.send(UIEvent::Play(path.clone())).unwrap();
                            }
                        });
                    }
                    dnd = false;
                    released = false;
                    true
                } else {
                    println!("paste");
                    false
                }
            }
            fltk::enums::Event::DndLeave => {
                dnd = false;
                released = false;
                true
            }
            _ => false,
        };

        // this is REALLY silly but we need to make sure to make enough
        // of these things so the closure won't complain

        let ui_tx2 = ui_tx.clone();

        gui.main_win.handle(move |w, ev| {
            let ui_tx3 = ui_tx2.clone();
            mover(w, ev, ui_tx3)
        });

        let ui_tx2 = ui_tx.clone(); // reusing the same name,

        queue_gui.main_win.handle(move |w, ev| {
            let ui_tx3 = ui_tx2.clone();
            mover(w, ev, ui_tx3)
        });

        while app.wait() {
            if let Some(msg) = receiver.recv() {
                //println!("got event from app: {:?}", msg);
                ui_tx.send(msg.clone()).unwrap();
                // only deal with ui-relevant stuff here
                match msg {
                    UIEvent::Update(evt) => match evt {
                        UIUpdateEvent::SetTime(elapsed, total) => {
                            // TODO: switch between elapsed and remaining on there
                            gui.lbl_time.set_label(&min_secs(elapsed));
                            let progress = elapsed as f64 / total as f64;
                            gui.seek_bar.set_value(progress);
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
                            gui.status_right_display.set_label(&format!(
                                "U{} Q{}",
                                gui.users.size(),
                                queue_gui.queue_browser.size(),
                            ));
                        }
                        UIUpdateEvent::UpdateQueue(current, queue) => {
                            queue_gui.queue_browser.clear();
                            // TODO: metadata stuff here is TOO LONG AND ANNOYING
                            if let Some(track) = current {
                                let line = format!(
                                    "@b{}\t[{}] {}",
                                    track.owner,
                                    min_secs(track.metadata.duration),
                                    track
                                        .metadata
                                        .title
                                        .as_ref()
                                        .unwrap_or(&"[no title]".to_string())
                                );
                                queue_gui.queue_browser.add(&line);
                                // also update display

                                gui.lbl_title.set_label(
                                    track
                                        .metadata
                                        .title
                                        .as_ref()
                                        .unwrap_or(&"[no title]".to_string()),
                                );
                                gui.lbl_data1.set_label(
                                    track
                                        .metadata
                                        .artist
                                        .as_ref()
                                        .unwrap_or(&"[unknown artist]".to_string()),
                                )
                            }
                            for track in queue {
                                let line = format!(
                                    "{}\t[{}] {}",
                                    track.owner,
                                    min_secs(track.metadata.duration),
                                    track.metadata.title.unwrap_or("[no title]".to_string())
                                );
                                queue_gui.queue_browser.add(&line);
                            }
                            gui.status_right_display.set_label(&format!(
                                "U{} Q{}",
                                gui.users.size(),
                                queue_gui.queue_browser.size(),
                            ));
                        }
                        _ => {
                            dbg!(evt);
                        }
                    },
                    UIEvent::BtnPlay => {}
                    UIEvent::BtnStop => {
                        ui_tx.send(UIEvent::Stop).unwrap();
                    }
                    UIEvent::BtnQueue => {
                        queue_gui.main_win.show();
                        //ui_tx.send(UIEvent::Update(UIUpdateEvent::UpdateQueue(self.queue.clone())));
                    }
                    UIEvent::HideQueue => {
                        queue_gui.main_win.hide();
                    }
                    UIEvent::Quit => break,
                    UIEvent::Test(s) => {
                        //dbg!(gui.temp_input.value());
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
    BtnStop,
    BtnPause,
    BtnQueue,
    Update(UIUpdateEvent),
    Play(PathBuf),
    Stop,
    Pause,
    Test(String),
    GetInfo(GetInfo),
    HideQueue,
    Quit,
}

#[derive(Debug, Clone)]
pub enum UIUpdateEvent {
    SetTime(usize, usize),
    UpdateUserList(HashMap<String, String>),
    UpdateQueue(Option<Track>, VecDeque<Track>),
    Status(String),
}

struct UIState {
    connection: Connection,
}

#[derive(Debug, Clone)]
enum AudioStatus {
    Elapsed(usize),
    Buffering,
    DoneBuffering,
    Finished,
}

#[derive(Debug, PartialEq)]
enum PlayState {
    Transmitting,
    Receiving,
    Empty,
}

#[derive(Debug, Clone)]
enum TransmitCommand {
    Start(String),
    Stop,
}

fn min_secs(secs: usize) -> String {
    format!("{:02}:{:02}", secs / 60, secs % 60)
}
