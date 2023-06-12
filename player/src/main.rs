use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use connection::{ConnectionActor, ConnectionActorHandle};
use futures::future::join_all;
use gui::UIThreadHandle;
use key::Key;
use tokio::sync::RwLock;

mod connection;
mod key;

mod transmit;
use transmit::{AudioInfoReader, TransmitCommand};

mod audio;
use crate::audio::AudioCommand;

mod gui;
use crate::gui::{ConnectionDlgEvent, UIEvent, UIThread, UIUpdateEvent};

use protocol::{RoomOptions, Track};

async fn maybe_connection_exited(maybe_actor: &Option<ConnectionActorHandle>) -> Option<()> {
    match maybe_actor {
        Some(actor) => Some(actor.exited().await),
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

pub struct State {
    my_id: String,
    key: Key,

    connection: Option<ConnectionState>,

    loading_count: usize,
}

pub struct ConnectionState {
    addr: String,
    room: Option<RoomState>,

    // TODO (in a long long time): transmit and audio should belong to the room not the connection........maybe
    buffering: bool,
}

pub struct RoomState {
    id: u32,
    name: String,

    playing: Option<Track>,
    queue: VecDeque<Track>,
    connected_users: HashMap<String, String>,
}

impl State {
    fn is_connected(&self) -> bool {
        if let Some(c) = &self.connection {
            return true;
        }
        false
    }
    fn is_in_room(&self) -> bool {
        if let Some(c) = &self.connection {
            if let Some(r) = &c.room {
                return true;
            }
        }
        false
    }
    fn is_transmitting(&self) -> bool {
        if let Some(c) = &self.connection {
            if let Some(r) = &c.room {
                if let Some(p) = &r.playing {
                    if p.owner == self.my_id {
                        return true;
                    }
                }
            }
        }
        false
    }
    fn is_receiving(&self) -> bool {
        if let Some(c) = &self.connection {
            if let Some(r) = &c.room {
                if let Some(p) = &r.playing {
                    if p.owner != self.my_id {
                        return true;
                    }
                }
            }
        }
        false
    }
}

struct MainThread {
    state: Arc<RwLock<State>>,

    connection: Option<ConnectionActorHandle>,

    ui: UIThreadHandle,
    ui_rx: tokio::sync::mpsc::UnboundedReceiver<UIEvent>,

    volume: f32,
}

impl MainThread {
    async fn setup(my_id: String) -> Self {
        let key = Key::load().expect("failed to load key");

        let state = Arc::new(RwLock::new(State {
            my_id,
            key,
            loading_count: 0,
            connection: None,
        }));

        let (ui, ui_rx) = UIThread::spawn(state.clone());

        MainThread {
            state,

            connection: None,

            ui,
            ui_rx,

            volume: 0.5,
        }
    }

    // main loop
    async fn run(&mut self) {
        self.ui.update(UIUpdateEvent::Status);

        let mut ui_interval = tokio::time::interval(tokio::time::Duration::from_millis(50));

        loop {
            tokio::select! {
                // watch for connection exits
                Some(()) = maybe_connection_exited(&self.connection) => {
                    self.disconnect();
                }

                // some ui event
                Some(event) = self.ui_rx.recv() => {
                    match event {
                        UIEvent::Connect(addr) => {
                            if self.connection.is_none() {
                                self.connect(addr).await;
                                if let Some(c) = &mut self.connection {
                                    c.refresh_room_list().unwrap();
                                }
                            }
                        }

                        UIEvent::JoinRoom(room_id) => {
                            let Some(ref c) = self.connection else { continue };

                            // TODO: do something with this result
                            let _ = c.join_room(room_id).await;
                        }

                        UIEvent::ConnectionDlg(ConnectionDlgEvent::BtnRefresh) => {
                            let servers_tmp = vec!["127.0.0.1:8080", "example.com:443"];
                            let mut servers: Vec<crate::gui::connection_window::Server> = vec![];
                            // TODO: query everything in ~parallel~
                            for addr in servers_tmp {
                                let name = addr.split(":").next().unwrap();
                                let rooms_future = connection::query_room_list(addr);
                                let mut s = crate::gui::connection_window::Server {
                                    name: name.into(),
                                    addr: addr.into(),
                                    rooms: None,
                                };
                                if let Ok(rooms) = rooms_future.await {
                                    s.rooms = Some(rooms);
                                }
                                servers.push(s);
                            }
                            println!("sending event: {:?}", servers);
                            self.ui.update(UIUpdateEvent::UpdateConnectionTree(servers));
                        },

                        UIEvent::ConnectionDlg(ConnectionDlgEvent::BtnNewRoom) => {
                            if let Some(ref c) = self.connection {
                                let room_id = c.create_room(RoomOptions {
                                    name: "meow room".into(),
                                }).await;
                                // TODO check if this works
                                println!("create_room got response: {:?}", room_id);

                                self.ui.update(UIUpdateEvent::Status);
                            }
                        },

                        UIEvent::DroppedFiles(data) => {
                            let Some(conn) = self.connection.as_mut() else { continue };

                            let paths: Vec<std::path::PathBuf> = data
                                .split("\n")
                                .map(|p| std::path::PathBuf::from(p.trim().replace("file://", "")))
                                .filter(|p| p.exists())
                                .collect();

                            let num_tracks = paths.len();

                            let tasks: Vec<tokio::task::JoinHandle<Option<Track>>> = {
                                let mut s = self.state.write().await;

                                s.loading_count += num_tracks;
                                self.ui.update(UIUpdateEvent::Status);

                                paths
                                    .into_iter()
                                    .map(|p| {
                                        let file = p.into_os_string().into_string().unwrap();
                                        let encrypted_path = s.key.encrypt_path(&file).unwrap();
                                        let my_id = s.my_id.clone();

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
                                    .collect()
                            };

                            let mut tracks = join_all(tasks).await.into_iter().filter_map(|t| match t {
                                Ok(Some(t)) => Some(t),
                                _ => None
                            }).collect::<Vec<Track>>();

                            tracks.sort_by_key(|t| {
                                let s = t.metadata.track_no.clone().unwrap_or_default();
                                let n = s.split("/").nth(0).unwrap().parse::<usize>().unwrap_or_default();
                                (t.metadata.album.clone().unwrap_or_default(), n)
                            });

                            // TODO: do all at once instead of waiting for each
                            for t in tracks {
                                // TODO: do something with the success status?
                                let _ = conn.queue_push(t).await;
                            }

                            drop(conn);

                            {
                                let mut s = self.state.write().await;
                                s.loading_count -= num_tracks;
                                self.ui.update(UIUpdateEvent::Status);
                            }
                        }

                        UIEvent::BtnNext => {
                            let Some(conn) = self.connection.as_mut() else { continue };
                            let s = self.state.read().await;
                            if s.is_transmitting() {
                                conn.transmit.send(TransmitCommand::Stop).unwrap();
                            }
                       }
                        UIEvent::Pause => {

                        }
                        UIEvent::Stop => {
                            // TODO do this obviously
                        }
                        UIEvent::VolumeSlider(pos) => {
                            self.volume = pos;
                            self.try_update_volume();
                        }
                        UIEvent::VolumeUp => {
                            self.volume = (self.volume + 0.02).min(1.);
                            self.ui.update(UIUpdateEvent::Volume(self.volume));
                            self.try_update_volume();
                        }
                        UIEvent::VolumeDown => {
                            self.volume = (self.volume - 0.02).max(0.);
                            self.ui.update(UIUpdateEvent::Volume(self.volume));
                            self.try_update_volume();
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

    fn volume_scale(val: f32) -> f32 {
        val.powf(3.)
    }

    fn try_update_volume(&mut self) {
        if let Some(conn) = &mut self.connection {
            let scaled = Self::volume_scale(self.volume);
            conn.audio.send(AudioCommand::Volume(scaled)).unwrap();
        }
    }

    async fn connect(&mut self, addr: String) {
        let result = ConnectionActor::spawn(self.state.clone(), self.ui.clone(), addr).await;
        match result {
            Ok(c) => {
                self.connection = Some(c);
                self.try_update_volume();
            }
            Err(e) => {
                println!("error while connecting: {:?}", e);
            }
        }
    }

    fn disconnect(&mut self) {
        // unset the connection handle which drops the thread and starts cleanup
        // ""RAII""
        self.connection = None;

        self.state.blocking_write().connection = None;

        self.ui.update(UIUpdateEvent::Status);
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
