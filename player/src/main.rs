use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use connection::{ConnectionActor, ConnectionActorHandle};
use key::Key;
use preferences::Preferences;
use tokio::sync::RwLock;

mod connection;
mod key;
mod preferences;
use crate::preferences::Server;

mod transmit;
use crate::transmit::TransmitCommand;

mod audio;
use crate::audio::AudioCommand;

mod gui;
use crate::gui::connection_window::ServerStatus;
use crate::gui::{ui_update, ConnectionDlgEvent, UIEvent, UIThread, UIUpdateEvent};

mod dnd_resolver;

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

        let (ui_rx) = UIThread::spawn(state.clone());

        ui_update!(UIUpdateEvent::Volume(volume));

        MainThread {
            state,

            connection: None,

            ui_rx,

            volume,
        }
    }

    // main loop
    async fn run(&mut self) {
        ui_update!(UIUpdateEvent::Status);

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

                            ui_update!(UIUpdateEvent::UpdateConnectionTree(servers.iter().map(|s| ServerStatus::from(s.clone())).collect()));

                            for server in servers {
                                tokio::spawn(async move {
                                    let s = connection::query_server(server).await;
                                    ui_update!(UIUpdateEvent::UpdateConnectionTreePartial(s));
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

                                ui_update!(UIUpdateEvent::Status);
                            }
                        },

                        UIEvent::DroppedFiles(data) => {
                            let Some(conn) = self.connection.as_mut() else { continue };

                            let (tracks, num_tracks) = dnd_resolver::resolve_dnd(self.state.clone(), data).await;

                            // TODO: do all at once instead of waiting for each
                            for t in tracks {
                                // TODO: do something with the success status?
                                let _ = conn.queue_push(t).await;
                            }

                            {
                                let mut s = self.state.write().await;
                                s.loading_count -= num_tracks;
                                ui_update!(UIUpdateEvent::Status);
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
                            ui_update!(UIUpdateEvent::Volume(self.volume));
                            self.try_update_volume();
                        }
                        UIEvent::VolumeDown => {
                            self.volume = (self.volume - 0.02).max(0.);
                            ui_update!(UIUpdateEvent::Volume(self.volume));
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
                    ui_update!(UIUpdateEvent::Periodic);
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
        let result = ConnectionActor::spawn(self.state.clone(), server).await;
        match result {
            Ok(c) => {
                self.connection = Some(c);
                self.try_update_volume();
            }
            Err(e) => {
                println!("error while connecting: {:?}", e);
            }
        }

        ui_update!(UIUpdateEvent::ConnectionChanged);
        ui_update!(UIUpdateEvent::Status);
    }

    async fn disconnect(&mut self) {
        // unset the connection handle which drops the thread and starts cleanup
        // ""RAII""
        self.connection = None;

        let mut state = self.state.write().await;

        // query old server's list after we disconnect so it updates properly?
        if let Some(c) = &state.connection {
            let server = c.server.clone();
            tokio::spawn(async move {
                let s = connection::query_server(server).await;
                ui_update!(UIUpdateEvent::UpdateConnectionTreePartial(s));
            });
        }

        state.connection = None;

        drop(state);

        ui_update!(UIUpdateEvent::ConnectionChanged);
        ui_update!(UIUpdateEvent::UserListChanged);
        ui_update!(UIUpdateEvent::QueueChanged);
        ui_update!(UIUpdateEvent::Reset);
    }
}

#[derive(Debug, Clone)]
pub enum AudioStatus {
    Elapsed(f32),
    Buffering(bool),
    Finished,
    Visualizer([u8; 14]),
    Buffer(u8),
}
