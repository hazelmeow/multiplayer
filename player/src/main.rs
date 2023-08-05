use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use connection::{ConnectionActor, ConnectionActorHandle};
use futures::future::join_all;
use gui::UIThreadHandle;
use key::Key;
use preferences::Preferences;
use tokio::sync::RwLock;

mod connection;
mod key;
mod preferences;
use crate::preferences::Server;

mod transmit;
use transmit::{AudioInfoReader, TransmitCommand};

mod audio;
use crate::audio::AudioCommand;

mod gui;
use crate::gui::connection_window::ServerStatus;
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
    let is_temp = match args.get(1).map(|s| s.as_str()) {
        Some("-t") => true, // use temporary identity for testing
        _ => false,         // len is 1 but index is 1 :3
    };

    let mut t = MainThread::setup(is_temp).await;
    // t.connect().await; // temporary
    t.run().await;

    if !is_temp {
        let p = &mut t.state.write().await.preferences;
        p.volume = t.volume;
        p.save().await;
    } else {
        // don't bother saving.. delete even
        std::fs::remove_file(t.state.read().await.preferences.path())?;
    }

    println!("exited gracefully");

    Ok(())
}

pub struct State {
    my_id: String,
    key: Key,

    preferences: Preferences,

    connection: Option<ConnectionState>,

    loading_count: usize,
}

pub struct ConnectionState {
    server: Server,
    room: Option<RoomState>,
}

pub struct RoomState {
    id: u32,
    name: String,

    // we only need to use this once, when we first connect
    // if we see the start message -> we are in sync
    // if we see a frame without start message -> we need to catch up first
    is_synced: bool,

    buffering: bool,

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
    async fn setup(is_temp: bool) -> Self {
        let key = Key::load().expect("failed to load key");

        let prefs = if !is_temp {
            Preferences::load().await
        } else {
            Preferences::default()
        };
        let volume = prefs.volume;
        let my_id = prefs.id();

        let state = Arc::new(RwLock::new(State {
            my_id,
            key,
            preferences: prefs,
            loading_count: 0,
            connection: None,
        }));

        let (mut ui, ui_rx) = UIThread::spawn(state.clone());

        ui.update(UIUpdateEvent::Volume(volume));

        MainThread {
            state,

            connection: None,

            ui,
            ui_rx,

            volume,
        }
    }

    // main loop
    async fn run(&mut self) {
        self.ui.update(UIUpdateEvent::Status);

        let mut ui_interval = tokio::time::interval(tokio::time::Duration::from_millis(50));

        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    println!("handling ctrl+c");
                    break;
                }

                // watch for connection exits
                Some(()) = maybe_connection_exited(&self.connection) => {
                    self.disconnect().await;
                }

                // some ui event
                Some(event) = self.ui_rx.recv() => {
                    match event {
                        UIEvent::Connect(server) => {
                            if self.connection.is_some() {
                                let current_addr = self.state.read().await.connection.as_ref().unwrap().server.addr.clone();
                                if current_addr != server.addr {
                                    self.disconnect().await;

                                    self.connect(server).await;
                                }
                            } else {
                                self.connect(server).await;
                            }
                        }

                        UIEvent::JoinRoom(room_id) => {
                            let Some(ref c) = self.connection else { continue };

                            // TODO: do something with this result
                            let _ = c.join_room(room_id).await;
                        }

                        UIEvent::ConnectionDlg(ConnectionDlgEvent::BtnRefresh) => {
                            let state = self.state.read().await;
                            let servers = state.preferences.servers.clone();

                            self.ui.update(UIUpdateEvent::UpdateConnectionTree(servers.iter().map(|s| ServerStatus::from(s.clone())).collect()));

                            for server in servers {
                                let mut ui2 = self.ui.clone();
                                tokio::spawn(async move {
                                    let s = connection::query_server(server).await;
                                    ui2.update(UIUpdateEvent::UpdateConnectionTreePartial(s));
                                });
                            }
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
                                .split('\n')
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
                                        let file_string = p.as_os_str().to_string_lossy();
                                        let encrypted_path = s.key.encrypt_path(&file_string.to_string()).unwrap();
                                        let my_id = s.my_id.clone();

                                        tokio::spawn(async move {
                                            match AudioInfoReader::load(&p) {
                                                Ok(mut reader) => {
                                                    match reader.read_info() {
                                                        Ok((_, _, metadata)) => {
                                                            let track = protocol::Track {
                                                                path: encrypted_path,
                                                                owner: my_id,
                                                                metadata
                                                            };
                                                            Some(track)
                                                        }
                                                        Err(e) => {
                                                            println!("errored reading metadata for {:?}: {e}", p);
                                                            None
                                                        }
                                                    }
                                                }
                                                Err(e) => {
                                                    println!("errored loading {:?}: {e}", p);
                                                    None
                                                }
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
                                let n = s.split('/').next().unwrap().parse::<usize>().unwrap_or_default();
                                (t.metadata.album.clone().unwrap_or_default(), n)
                            });

                            // TODO: do all at once instead of waiting for each
                            for t in tracks {
                                // TODO: do something with the success status?
                                let _ = conn.queue_push(t).await;
                            }

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
                        UIEvent::BtnPause => {
                            // TODO UNDO ME
                            self.disconnect().await;
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
                        UIEvent::SavePreferences{name} => {
                            let mut s = self.state.write().await;
                            s.preferences.name = name;

                            s.preferences.volume = self.volume;
                            s.preferences.save().await;
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

    async fn connect(&mut self, server: Server) {
        let result = ConnectionActor::spawn(self.state.clone(), self.ui.clone(), server).await;
        match result {
            Ok(c) => {
                self.connection = Some(c);
                self.try_update_volume();
            }
            Err(e) => {
                println!("error while connecting: {:?}", e);
            }
        }

        self.ui.update(UIUpdateEvent::ConnectionChanged);
        self.ui.update(UIUpdateEvent::Status);
    }

    async fn disconnect(&mut self) {
        // unset the connection handle which drops the thread and starts cleanup
        // ""RAII""
        self.connection = None;

        let mut state = self.state.write().await;

        // query old server's list after we disconnect so it updates properly?
        if let Some(c) = &state.connection {
            let mut ui2 = self.ui.clone();
            let server = c.server.clone();
            tokio::spawn(async move {
                let s = connection::query_server(server).await;
                ui2.update(UIUpdateEvent::UpdateConnectionTreePartial(s));
            });
        }

        state.connection = None;

        drop(state);

        self.ui.update(UIUpdateEvent::ConnectionChanged);
        self.ui.update(UIUpdateEvent::UserListChanged);
        self.ui.update(UIUpdateEvent::QueueChanged);
        self.ui.update(UIUpdateEvent::Reset);
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
