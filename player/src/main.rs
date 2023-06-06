use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::error::Error;

use audio::{AudioStatusRx, AudioThreadHandle};
use futures::future::join_all;
use futures::{SinkExt, StreamExt};
use gui::UIThreadHandle;
use key::Key;
use protocol::network::FrameStream;
use protocol::{AudioData, GetInfo, Message, RoomOptions, Track, TrackArt};
use tokio::net::TcpStream;
use tokio::time::timeout;

mod audio;
mod key;

mod transmit;
use transmit::{AudioInfoReader, TransmitCommand, TransmitThread, TransmitThreadHandle};

use crate::audio::{AudioCommand, AudioThread};
use crate::gui::{UIEvent, UIThread, UIUpdateEvent};

mod gui;

type MessageRx = tokio::sync::mpsc::UnboundedReceiver<Message>;
type MessageTx = tokio::sync::mpsc::UnboundedSender<Message>;

struct Connection {
    stream: RefCell<FrameStream>,
    my_id: String,

    key: Key,

    playing: Option<Track>,
    queue: VecDeque<Track>,
    connected_users: HashMap<String, String>,
    buffering: bool,

    // we only need to use this once, when we first connect
    // if we see the start message -> we are in sync
    // if we see a frame without start message -> we need to catch up first
    is_synced: bool,

    transmit: TransmitThreadHandle,
    audio: AudioThreadHandle,

    // network messages
    message_rx: RefCell<MessageRx>,
    message_tx: MessageTx,

    // get status feedback from audio playing
    audio_status_rx: RefCell<AudioStatusRx>,
}
impl Connection {
    async fn create(addr: &str, my_id: &String) -> Result<Self, Box<dyn Error>> {
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
        println!("connecting as {:?}", my_id);

        stream
            .send(&Message::Authenticate {
                id: my_id.clone(),
                name: my_id.clone().repeat(5), // TODO temp
            })
            .await
            .unwrap();

        let (message_tx, message_rx) = tokio::sync::mpsc::unbounded_channel::<Message>();

        let (audio_tx, audio_rx) = std::sync::mpsc::channel::<AudioCommand>();
        let (audio_status_tx, audio_status_rx) =
            tokio::sync::mpsc::unbounded_channel::<AudioStatus>();

        let (transmit_tx, transmit_rx) = tokio::sync::mpsc::unbounded_channel::<TransmitCommand>();
        let transmit = TransmitThread::spawn(transmit_tx, transmit_rx, &message_tx, &audio_tx);

        let audio = AudioThread::spawn(audio_tx, audio_rx, &audio_status_tx);

        stream
            .send(&Message::GetInfo(GetInfo::Queue))
            .await
            .unwrap();

        Ok(Self {
            stream: RefCell::new(stream),
            my_id: my_id.to_owned(),

            key,

            playing: None,
            queue: VecDeque::new(),
            connected_users: HashMap::new(),
            buffering: false,

            is_synced: false,

            audio,
            transmit,

            message_rx: RefCell::new(message_rx),
            message_tx,
            audio_status_rx: RefCell::new(audio_status_rx),
        })
    }
}

// fn get_message_rx(t: &Option<Connection>) -> Option<std::cell::RefMut<'_, MessageRx>> {
async fn maybe_message_rx(t: &Option<Connection>) -> Option<Message> {
    match t {
        Some(c) => {
            let mut r = c.message_rx.borrow_mut();
            r.recv().await
        }
        None => None,
    }
}

async fn maybe_audio_status_rx(t: &Option<Connection>) -> Option<AudioStatus> {
    match t {
        Some(c) => {
            let mut r = c.audio_status_rx.borrow_mut();
            r.recv().await
        }
        None => None,
    }
}

async fn maybe_tcp_message(t: &Option<Connection>) -> Option<Result<Message, ()>> {
    match t {
        Some(c) => {
            let mut stream = c.stream.borrow_mut();
            let result = stream.get_inner().next().await;

            match result {
                Some(r) => match r {
                    Ok(bytes) => match bincode::deserialize::<Message>(&bytes) {
                        Ok(msg) => Some(Ok(msg)),
                        Err(e) => {
                            println!("failed to deserialize message: {:?}", e);
                            Some(Err(()))
                        }
                    },
                    Err(e) => {
                        println!("tcp stream error: {:?}", e);
                        Some(Err(()))
                    }
                },
                // stream disconnected
                // we need to differentiate from nothing happening
                None => Some(Err(())),
            }
        }

        // nothing happened yet
        None => None,
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let my_id = args[1].clone();

    let mut t = MainThread::setup(my_id).await;
    // t.connect().await; // temporary
    t.run().await;

    println!("exiting");

    Ok(())
}

struct MainThread {
    my_id: String,
    ui: UIThreadHandle,
    connection: Option<Connection>,

    loading_count: usize,
}

impl MainThread {
    async fn setup(my_id: String) -> Self {
        let ui_thread = UIThread::spawn(my_id.to_owned());

        MainThread {
            my_id: my_id,
            ui: ui_thread,
            connection: None,

            loading_count: 0,
        }
    }

    // main loop
    async fn run(&mut self) {
        self.update_ui_status();

        let mut ui_interval = tokio::time::interval(tokio::time::Duration::from_millis(50));

        loop {
            tokio::select! {
                // something to send
                Some(msg) = maybe_message_rx(&self.connection) => {
                    let c = self.connection.as_mut().unwrap();
                    c.stream.borrow_mut().send(&msg).await.unwrap();
                },

                // something from audio
                Some(msg) = maybe_audio_status_rx(&self.connection) => {
                    // guaranteed by maybe_tcp_rx?
                    let mut c = self.connection.as_mut().unwrap();
                    match msg {
                        AudioStatus::Elapsed(secs) => {
                            let total = match &c.playing {
                                Some(t) => {
                                    t.metadata.duration
                                },
                                None => 0
                            };
                            self.ui.update(UIUpdateEvent::SetTime(secs, total));
                        }
                        AudioStatus::Buffering(is_buffering) => {
                            c.buffering = is_buffering;
                            self.update_ui_status();
                        }
                        AudioStatus::Finished => {
                            self.update_ui_status();
                        }
                        AudioStatus::Visualizer(bars) => {
                            self.ui.update(UIUpdateEvent::Visualizer(bars))
                        }
                        AudioStatus::Buffer(val) => {
                            self.ui.update(UIUpdateEvent::Buffer(val))
                        }
                    }
                },

                // tcp message
                Some(message_or_disconnect) = maybe_tcp_message(&self.connection) => {
                    if let Ok(message) = message_or_disconnect {
                        let c = self.connection.as_mut().unwrap();

                        match message {
                            Message::Info(info) => {
                                match info {
                                    protocol::Info::Queue(queue) => {
                                        // TODO
                                        println!("got queue: {:?}", queue);
                                        self.ui.update(UIUpdateEvent::UpdateQueue(c.playing.clone(), queue.clone()));
                                        c.queue = queue;
                                    },
                                    protocol::Info::Playing(playing) => {
                                        // TODO
                                        println!("got playing: {:?}", playing);

                                        c.playing = playing.clone();

                                        if let Some(t) = playing {
                                            if t.owner == self.my_id {
                                                let path = c.key.decrypt_path(t.path).unwrap();
                                                c.transmit.send(TransmitCommand::Start(path)).unwrap();
                                            }
                                        }
                                    },
                                    protocol::Info::ConnectedUsers(list) => {
                                        c.connected_users = list;
                                        self.ui.update(UIUpdateEvent::UpdateUserList(c.connected_users.clone()));
                                    }
                                    protocol::Info::Room(opts) => {
                                        self.ui.update(UIUpdateEvent::UpdateRoomName(opts.map(|o| o.name)));
                                    }
                                    _ => {}
                                }
                            }
                            Message::AudioData(data) => {
                                match data {
                                    protocol::AudioData::Frame(f) => {
                                        if !c.is_synced {
                                            // we're late.... try and catch up will you
                                            c.is_synced = true;
                                            c.audio.send(AudioCommand::StartLate(f.frame as usize)).unwrap();
                                        }

                                        c.audio.send(AudioCommand::AudioData(AudioData::Frame(f))).unwrap();
                                    }
                                    protocol::AudioData::Start => {
                                        c.is_synced = true;
                                        c.audio.send(AudioCommand::AudioData(data)).unwrap();
                                    }
                                    _ => {
                                        // forward to audio thread
                                        c.audio.send(AudioCommand::AudioData(data)).unwrap();
                                    }
                                }
                            },

                            _ => {
                                println!("received message: {:?}", message);
                            }
                        }
                    } else {
                        if let Some(c) = self.connection.as_mut() {
                            // socket was disconnected (either forcefully or because we closed it)
                            // shutdown other threads
                            c.transmit.send(TransmitCommand::Shutdown).unwrap();
                            c.audio.send(AudioCommand::Shutdown).unwrap();

                            // unset connection now
                            self.connection = None;

                            self.update_ui_status();
                        } else {
                            // we dont have a connection right now so dont do anything
                        }
                    }
                }

                // some ui event
                Some(event) = self.ui.ui_rx.recv() => {
                    match event {
                        UIEvent::Connect(addr) => {
                            // awaiting here will block the loop maybe???
                            if self.connection.is_some() {
                                self.disconnect().await;
                            } else {
                                self.connect(addr).await;

                                // TEMP
                                let mut s = self.connection.as_mut().unwrap().stream.borrow_mut();
                                s.send(&Message::CreateRoom(RoomOptions {
                                    name: "test room".into(),
                                })).await.unwrap();

                                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                                s.send(&Message::JoinRoom(0)).await.unwrap();

                                s.send(&Message::GetInfo(GetInfo::Room)).await.unwrap();
                                s.send(&Message::GetInfo(GetInfo::ConnectedUsers)).await.unwrap();

                                drop(s);

                                self.update_ui_status();
                            }
                        }

                        UIEvent::DroppedFiles(data) => {
                            let paths: Vec<std::path::PathBuf> = data
                                .split("\n")
                                .map(|p| std::path::PathBuf::from(p.trim().replace("file://", "")))
                                .filter(|p| p.exists())
                                .collect();

                            let num_tracks = paths.len();
                            self.loading_count += num_tracks;
                            self.update_ui_status();

                            let Some(conn) = self.connection.as_mut() else {
                                self.loading_count -= num_tracks;
                                self.update_ui_status();
                                return
                            };

                            let tasks: Vec<tokio::task::JoinHandle<Option<Track>>> = paths
                                .into_iter()
                                .map(|p| {
                                    let file = p.into_os_string().into_string().unwrap();
                                    let encrypted_path = conn.key.encrypt_path(&file).unwrap();
                                    let my_id = self.my_id.clone();

                                    tokio::spawn(async move {
                                        if let Ok(mut reader) = AudioInfoReader::load(&file) {
                                            if let Ok((_, _, metadata)) = reader.read_info() {
                                                let track = protocol::Track {
                                                    path: encrypted_path,
                                                    owner: my_id,
                                                    metadata
                                                };

                                                Some(track)
                                            } else {
                                                None
                                            }
                                        } else {
                                            None
                                        }
                                    })
                                })
                                .collect();

                            let mut tracks = join_all(tasks).await.into_iter().filter_map(|t| match t {
                                Ok(Some(t)) => Some(t),
                                _ => None
                            }).collect::<Vec<Track>>();

                            tracks.sort_by_key(|t| {
                                let s = t.metadata.track_no.clone().unwrap_or_default();
                                let n = s.split("/").nth(0).unwrap().parse::<usize>().unwrap_or_default();
                                (t.metadata.album.clone().unwrap_or_default(), n)
                            });

                            for t in tracks {
                                conn.message_tx.send(Message::QueuePush(t)).unwrap();
                            }

                            drop(conn);

                            self.loading_count -= num_tracks;
                            self.update_ui_status();
                        }
                        UIEvent::Pause => {

                        }
                        UIEvent::Stop => {
                            // send network message to stop?
                            if let Some(conn) = &mut self.connection {
                                if let Some(p) = &conn.playing {
                                    if p.owner == self.my_id {
                                        conn.transmit.send(TransmitCommand::Stop).unwrap();
                                    }
                                }
                            }
                        }
                        UIEvent::VolumeSlider(pos) => {
                            if let Some(conn) = &mut self.connection {
                                conn.audio.send(AudioCommand::Volume(pos)).unwrap();
                            }
                        }
                        UIEvent::Test(text) => {
                            if let Some(conn) = &mut self.connection {
                                conn.message_tx.send(Message::Text(text)).unwrap();
                            }
                        },
                        UIEvent::GetInfo(i) => {
                            if let Some(conn) = &mut self.connection {
                                conn.message_tx.send(Message::GetInfo(i)).unwrap();
                            }
                        },
                        UIEvent::VolumeUp => {
                            self.ui.update(UIUpdateEvent::VolumeUp);
                        }
                        UIEvent::VolumeDown => {
                            self.ui.update(UIUpdateEvent::VolumeDown);
                        }
                        UIEvent::Quit => break,
                        _ => {}
                    }
                },

                _ = ui_interval.tick() => {
                    self.ui.update(UIUpdateEvent::Periodic);
                },
            }
        }
    }

    // set status based on state and priorities
    fn update_ui_status(&mut self) {
        let status = self.ui_status();
        self.ui.update(UIUpdateEvent::Status(status));
    }

    fn ui_status(&self) -> String {
        if let Some(connection) = &self.connection {
            if self.loading_count == 1 {
                return format!("Loading 1 file...");
            } else if self.loading_count > 1 {
                return format!("Loading {} files...", self.loading_count);
            }

            if connection.buffering {
                return format!("Buffering...");
            }

            if let Some(t) = &connection.playing {
                if t.owner == self.my_id {
                    return "Playing (transmitting)".to_string();
                } else {
                    return format!("Playing (receiving from {:?})", t.owner);
                }
            }

            format!(
                "Connected to {}",
                connection.stream.borrow().get_addr().unwrap()
            )
        } else {
            format!("<not connected>")
        }
    }

    async fn connect(&mut self, addr: String) {
        let connection = Connection::create(&addr, &self.my_id).await.unwrap();
        self.connection = Some(connection);
    }

    async fn disconnect(&mut self) {
        if let Some(c) = &mut self.connection {
            c.stream.borrow_mut().get_inner().close().await.unwrap();
        }
    }
}

#[derive(Debug, Clone)]
pub enum AudioStatus {
    Elapsed(usize),
    Buffering(bool),
    Finished,
    Visualizer([u8; 14]),
    Buffer(u8),
}
