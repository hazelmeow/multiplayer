use fltk::app;
use fltk::image::JpegImage;
use fltk::prelude::{BrowserExt, ImageExt, ValuatorExt, WidgetBase, WidgetExt, WindowExt};
use fltk::window::DoubleWindow;
use futures::StreamExt;
use key::Key;
use protocol::network::FrameStream;
use protocol::{AudioData, AuthenticateRequest, GetInfo, Message, Track, TrackArt};
use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::path::PathBuf;
use tokio::sync::mpsc::UnboundedSender;
use tokio::time::timeout;
use tokio::{net::TcpStream, sync::mpsc};

mod audio;
mod key;

mod transmit;
use transmit::{AudioInfoReader, TransmitCommand, TransmitThread, TransmitThreadHandle};

mod gui;

struct Connection {
    stream: FrameStream,
    my_id: String,

    key: Key,

    playing: Option<Track>,
    queue: VecDeque<Track>,
    connected_users: HashMap<String, String>,

    // we only need to use this once, when we first connect
    // if we see the start message -> we are in sync
    // if we see a frame without start message -> we need to catch up first
    is_synced: bool,

    transmit: TransmitThreadHandle,

    // network messages
    message_rx: mpsc::UnboundedReceiver<Message>,
    message_tx: mpsc::UnboundedSender<Message>,

    // transmit audio to playback subsystem
    audio_tx: std::sync::mpsc::Sender<AudioEngineData>,

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
        let key = Key::load().expect("failed to load key?");

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

        let (audio_tx, audio_rx) = std::sync::mpsc::channel::<AudioEngineData>();
        let audio_status_rx = Self::run_audio(audio_rx);

        let (transmit_tx, transmit_rx) = tokio::sync::mpsc::unbounded_channel::<TransmitCommand>();
        let transmit = TransmitThread::spawn(transmit_tx, transmit_rx, &message_tx, &audio_tx);

        let (ui_tx, ui_rx) = mpsc::unbounded_channel::<UIEvent>();

        stream
            .send(&Message::GetInfo(GetInfo::Queue))
            .await
            .unwrap();

        Ok(Self {
            stream,
            my_id: id.to_string(),

            key,

            playing: None,
            queue: VecDeque::new(),
            connected_users: HashMap::new(),

            is_synced: false,

            transmit,

            message_rx,
            message_tx,
            audio_tx,
            audio_status_rx,
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
        audio_rx: std::sync::mpsc::Receiver<AudioEngineData>,
    ) -> mpsc::UnboundedReceiver<AudioStatus> {
        // audio player thread

        // status channel
        let (tx, rx) = mpsc::unbounded_channel::<AudioStatus>();

        std::thread::spawn(move || {
            let mut p = audio::Player::new();

            let mut wants_play = false;

            while let Ok(data) = audio_rx.recv() {
                match data {
                    AudioEngineData::AudioData(d) => match d {
                        AudioData::Frame(frame) => {
                            if frame.frame % 10 == 0 {
                                tx.send(AudioStatus::Elapsed(p.get_seconds_elapsed()))
                                    .unwrap();
                                tx.send(AudioStatus::Buffer(p.buffer_status())).unwrap();
                            }

                            p.receive(frame.data);
                            if let Some(samples) = p.get_visualizer_buffer() {
                                let bars = audio::calculate_visualizer(&samples);
                                tx.send(AudioStatus::Visualizer(bars)).unwrap();
                            }
                        }
                        AudioData::Start => {
                            wants_play = true;
                            p.fake_frames_received(0);
                            tx.send(AudioStatus::Buffering).unwrap();
                        }
                        AudioData::Finish => {
                            while !p.finish() {
                                std::thread::sleep(std::time::Duration::from_millis(20));
                            }
                            p.pause();
                            tx.send(AudioStatus::Finished).unwrap();
                        }
                        AudioData::Resume => {
                            p.resume();
                        }
                    },
                    AudioEngineData::StartLate(frame_id) => {
                        wants_play = true;
                        p.fake_frames_received(frame_id);
                    }

                    AudioEngineData::Clear => {
                        tx.send(AudioStatus::Elapsed(0)).unwrap();
                        p.clear();
                    }
                    AudioEngineData::Volume(val) => {
                        p.volume(val);
                    }
                    AudioEngineData::Shutdown => break,
                }
                if p.is_ready() && wants_play {
                    wants_play = false;

                    tx.send(AudioStatus::DoneBuffering).unwrap();

                    if !p.is_started() {
                        p.start()
                    } else {
                        p.resume()
                    }
                }
            }
        });
        return rx;
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

        let mut ui_interval = tokio::time::interval(tokio::time::Duration::from_millis(50));

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
                                PlayState::Empty => unreachable!("done buffering when playstate empty"),
                            }
                        }
                        AudioStatus::Finished => {
                            self.ui_status_default();
                        }
                        AudioStatus::Visualizer(bars) => {
                            self.ui_sender.send(UIEvent::Update(UIUpdateEvent::Visualizer(bars)))
                        }
                        AudioStatus::Buffer(val) => {
                            self.ui_sender.send(UIEvent::Update(UIUpdateEvent::Buffer(val)))
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
                                                let path = self.key.decrypt_path(t.path).unwrap();
                                                self.transmit.send(TransmitCommand::Start(path)).unwrap();
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
                                            self.audio_tx.send(AudioEngineData::StartLate(f.frame as usize)).unwrap();
                                        }

                                        self.audio_tx.send(AudioEngineData::AudioData(AudioData::Frame(f))).unwrap();
                                    }
                                    protocol::AudioData::Start => {
                                        self.is_synced = true;
                                        self.audio_tx.send(AudioEngineData::AudioData(data)).unwrap();
                                    }
                                    _ => {
                                        // forward to audio thread
                                        self.audio_tx.send(AudioEngineData::AudioData(data)).unwrap();
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

                            if let Ok(mut reader) = AudioInfoReader::load(&file) {
                                if let Ok((_, _, metadata)) = reader.read_info() {
                                    let track = protocol::Track {
                                        path: self.key.encrypt_path(file).unwrap(),
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
                                    self.transmit.send(TransmitCommand::Stop).unwrap();
                                }
                                _ => {
                                    println!("pointless stop button use");
                                }
                            }
                        }
                        UIEvent::VolumeSlider(pos) => {
                            self.audio_tx.send(AudioEngineData::Volume(pos)).unwrap();
                        }
                        UIEvent::Test(text) => {
                            self.message_tx.send(Message::Text(text)).unwrap();
                        },
                        UIEvent::GetInfo(i) => {
                            self.message_tx.send(Message::GetInfo(i)).unwrap();
                        },
                        UIEvent::VolumeUp => {
                            self.ui_sender.send(UIEvent::Update(UIUpdateEvent::VolumeUp));
                        }
                        UIEvent::VolumeDown => {
                            self.ui_sender.send(UIEvent::Update(UIUpdateEvent::VolumeDown));
                        }
                        UIEvent::Quit => break,
                        _ => {}
                    }
                }

                _ = ui_interval.tick() => {
                    self.ui_sender.send(UIEvent::Update(UIUpdateEvent::Periodic));
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
                    for path in app::event_text().split("\n") {
                        let path = path.trim().replace("file://", "");
                        let path = std::path::PathBuf::from(&path);

                        if path.exists() {
                            tx.send(UIEvent::Play(path.clone())).unwrap();
                        }
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
            fltk::enums::Event::MouseWheel => {
                let dy = app::event_dy();
                match dy {
                    // uhhhhhhhh these are the opposite of what you'd think (???)
                    app::MouseWheel::Up => {
                        tx.send(UIEvent::VolumeDown).unwrap();
                        true
                    }
                    app::MouseWheel::Down => {
                        tx.send(UIEvent::VolumeUp).unwrap();
                        true
                    }
                    _ => false,
                }
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

        // TODO: we'd probably have to communicate this somehow
        //       for now just pretend
        gui.bitrate_bar.update_bitrate(256);

        // blehhhh this is all such a mess
        gui.volume_slider.set_value(0.5);
        ui_tx
            .send(UIEvent::VolumeSlider(gui::MainWindow::volume_scale(0.5)))
            .unwrap();

        gui.lbl_title.set_text(&"hi, welcome >.<".to_string());

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
                        UIUpdateEvent::Visualizer(bars) => {
                            gui.visualizer.update_values(bars);
                        }
                        UIUpdateEvent::Buffer(val) => {
                            gui.bitrate_bar.update_buffer_level(val);
                        }
                        UIUpdateEvent::Bitrate(val) => {
                            gui.bitrate_bar.update_bitrate(val);
                        }
                        UIUpdateEvent::Status(val) => {
                            gui.status_field.set_label(&val);
                        }
                        UIUpdateEvent::Periodic => {
                            if gui.lbl_title.waited_long_enough() {
                                gui.lbl_title.nudge(-4);
                            }
                        }
                        UIUpdateEvent::VolumeUp => {
                            gui.volume_slider
                                .set_value((gui.volume_slider.value() + 0.02).min(1.));
                            ui_tx
                                .send(UIEvent::VolumeSlider(gui::MainWindow::volume_scale(
                                    gui.volume_slider.value(),
                                )))
                                .unwrap();
                        }
                        UIUpdateEvent::VolumeDown => {
                            gui.volume_slider
                                .set_value((gui.volume_slider.value() - 0.02).max(0.));
                            ui_tx
                                .send(UIEvent::VolumeSlider(gui::MainWindow::volume_scale(
                                    gui.volume_slider.value(),
                                )))
                                .unwrap();
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
                                "U{:0>2} Q{:0>2}",
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
                                gui.lbl_title.set_text(
                                    track
                                        .metadata
                                        .title
                                        .as_ref()
                                        .unwrap_or(&"[no title]".to_string()),
                                );
                                gui.lbl_title.zero();

                                gui.lbl_data1.set_label(
                                    track
                                        .metadata
                                        .artist
                                        .as_ref()
                                        .unwrap_or(&"[unknown artist]".to_string()),
                                );

                                if let Some(art) = track.metadata.art {
                                    if let Err(err) = match art {
                                        TrackArt::Jpeg(data) => JpegImage::from_data(&data)
                                            .and_then(|img| {
                                                gui.art_frame.set_image(Some(img));
                                                gui.art_frame.redraw();
                                                Ok(())
                                            }),
                                    } {
                                        eprintln!("failed to load image: {:?}", err);
                                    }
                                } else {
                                    gui.art_frame.set_image(None::<JpegImage>); // hm that's silly
                                    gui.art_frame.redraw();
                                }
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
                                "U{:0>2} Q{:0>2}",
                                gui.users.size(),
                                queue_gui.queue_browser.size(),
                            ));
                        }
                        _ => {
                            dbg!(evt);
                        }
                    },
                    UIEvent::BtnPlay => {
                        // play button behaviour:
                        // if stopped and queue is empty, ask for file
                        // if stopped and queue has songs, tell it to play
                        // if already playing, do nothing
                    }
                    UIEvent::BtnStop => {
                        ui_tx.send(UIEvent::Stop).unwrap();
                    }
                    UIEvent::BtnPause => {}
                    UIEvent::BtnNext => {}
                    UIEvent::BtnPrev => {}

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
    BtnNext,
    BtnPrev,
    BtnQueue,
    VolumeSlider(f32),
    VolumeUp,
    VolumeDown,
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
    Visualizer([u8; 14]),
    Buffer(u8),
    Bitrate(usize),
    VolumeUp,
    VolumeDown,
    Periodic,
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
    Visualizer([u8; 14]),
    Buffer(u8),
}

#[derive(Debug, Clone)]
pub enum AudioEngineData {
    AudioData(AudioData),
    StartLate(usize),
    Clear,
    Volume(f32),
    Shutdown,
}

#[derive(Debug, PartialEq)]
enum PlayState {
    Transmitting,
    Receiving,
    Empty,
}

fn min_secs(secs: usize) -> String {
    format!("{:02}:{:02}", secs / 60, secs % 60)
}
