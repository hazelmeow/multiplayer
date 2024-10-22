use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use tokio::sync::RwLock;

mod audio;
mod connection;
mod dnd_resolver;
mod gui;
mod key;
mod lrc;
mod preferences;
mod transmit;

use crate::audio::AudioCommand;
use crate::connection::{ConnectionActor, ConnectionActorHandle};
use crate::gui::connection_window::ServerStatus;
use crate::gui::{ui_update, ConnectionDlgEvent, UIEvent, UIThread, UIUpdateEvent};
use crate::key::Key;
use crate::preferences::{Preferences, Server};
use crate::transmit::TransmitThreadHandle;

use protocol::{Message, PlaybackCommand, PlaybackState, RoomOptions, Track};

async fn maybe_connection_exited(maybe_actor: &Option<ConnectionActorHandle>) -> Option<()> {
    match maybe_actor {
        Some(actor) => {
            actor.exited().await;
            Some(())
        }
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
        let mut state = t.state.write().await;
        state.save_volume();
    } else {
        // don't bother saving.. delete even
        std::fs::remove_file(t.state.read().await.preferences.path())?;
    }

    println!("exited gracefully");

    Ok(())
}

#[derive(Debug)]
pub struct State {
    my_id: String,
    key: Key,

    preferences: Preferences,

    connection: Option<ConnectionState>,

    loading_count: usize,
    seek_position: Option<f32>,
    volume: f32,
}

impl State {
    fn save_volume(&mut self) {
        self.preferences.volume = self.volume;
        self.preferences.save();
    }

    fn current_track(&self) -> Option<&Track> {
        let Some(connection) = &self.connection else {
            return None;
        };
        let Some(room) = &connection.room else {
            return None;
        };
        room.current_track()
    }

    fn is_transmitter(&self) -> bool {
        self.current_track()
            .map_or(false, |t| t.owner == self.my_id)
    }

    /// Checks if playback is stopped.
    /// Returns true if not in a server or room.
    fn is_stopped(&self) -> bool {
        let Some(connection) = &self.connection else {
            return true;
        };
        let Some(room) = &connection.room else {
            return true;
        };
        room.playback_state == PlaybackState::Stopped
    }

    /// Checks if playback is paused.
    /// Returns false if not in a server or room.
    fn is_paused(&self) -> bool {
        let Some(connection) = &self.connection else {
            return false;
        };
        let Some(room) = &connection.room else {
            return false;
        };
        room.playback_state == PlaybackState::Paused
    }
}

#[derive(Debug)]
pub struct ConnectionState {
    server: Server,
    room: Option<RoomState>,
}

#[derive(Debug)]
pub struct RoomState {
    id: u32,
    name: String,

    buffering: bool,

    queue: VecDeque<Track>,
    current_track: u32,
    playback_state: PlaybackState,

    connected_users: HashMap<String, String>,

    transmit_thread: Option<TransmitThreadHandle>,
}

impl RoomState {
    fn current_track(&self) -> Option<&Track> {
        self.queue.iter().find(|t| t.id == self.current_track)
    }
}

struct MainThread {
    state: Arc<RwLock<State>>,

    connection: Option<ConnectionActorHandle>,

    ui_rx: tokio::sync::mpsc::UnboundedReceiver<UIEvent>,
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
            seek_position: None,
            volume,
        }));

        let ui_rx = UIThread::spawn(state.clone());

        ui_update!(UIUpdateEvent::Volume(volume));

        MainThread {
            state,

            connection: None,

            ui_rx,
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

                        UIEvent::BtnPlay => {
                            let Some(conn) = self.connection.as_mut() else { continue };
                            conn.send(Message::PlaybackCommand(PlaybackCommand::Play)).unwrap();
                        }
                        UIEvent::BtnStop => {
                            let Some(conn) = self.connection.as_mut() else { continue };
                            conn.send(Message::PlaybackCommand(PlaybackCommand::Stop)).unwrap();
                        }
                        UIEvent::BtnPause => {
                            let Some(conn) = self.connection.as_mut() else { continue };
                            conn.send(Message::PlaybackCommand(PlaybackCommand::Pause)).unwrap();
                        }
                        UIEvent::BtnNext => {
                            let Some(conn) = self.connection.as_mut() else { continue };
                            conn.send(Message::PlaybackCommand(PlaybackCommand::Next)).unwrap();
                        }
                        UIEvent::BtnPrev => {
                            let Some(conn) = self.connection.as_mut() else { continue };
                            conn.send(Message::PlaybackCommand(PlaybackCommand::Prev)).unwrap();
                        }

                        UIEvent::SeekBarMoved(progress) => {
                            let mut state = self.state.write().await;
                            if let Some(track) = state.current_track() {
                                let secs = track.metadata.duration * progress;
                                state.seek_position = Some(secs);
                                ui_update!(UIUpdateEvent::Status);
                            }
                        }
                        UIEvent::SeekBarFinished(progress) => {
                            let Some(conn) = self.connection.as_mut() else { continue };

                            let mut state = self.state.write().await;
                            if let Some(track) = state.current_track() {
                                let secs = track.metadata.duration * progress;
                                state.seek_position = None;
                                ui_update!(UIUpdateEvent::Status);
                                conn.send(Message::PlaybackCommand(PlaybackCommand::SeekTo(secs))).unwrap();
                            }
                        }

                        UIEvent::VolumeSlider(pos) => {
                            self.state.write().await.volume = pos;
                            self.try_update_volume().await;
                        }
                        UIEvent::VolumeUp => {
                            let mut state = self.state.write().await;
                            state.volume = (state.volume + 0.02).min(1.);
                            ui_update!(UIUpdateEvent::Volume(state.volume));
                            drop(state);
                            self.try_update_volume().await;
                        }
                        UIEvent::VolumeDown => {
                            let mut state = self.state.write().await;
                            state.volume = (state.volume - 0.02).max(0.);
                            ui_update!(UIUpdateEvent::Volume(state.volume));
                            drop(state);
                            self.try_update_volume().await;
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

    async fn try_update_volume(&mut self) {
        if let Some(conn) = &mut self.connection {
            let scaled = Self::volume_scale(self.state.read().await.volume);
            conn.audio.send(AudioCommand::Volume(scaled)).unwrap();
        }
    }

    async fn connect(&mut self, server: Server) {
        let result = ConnectionActor::spawn(self.state.clone(), server).await;
        match result {
            Ok(c) => {
                self.connection = Some(c);
                self.try_update_volume().await;
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
    Elapsed(u32),
    Buffering(bool),
    Finished,
    Visualizer([u8; 14]),
    Buffer(u8),
}
