use crate::{
    audio::AudioCommand,
    connection::{ConnectionActor, ConnectionActorHandle},
    gui::{ui_send, UIThread},
    key::Key,
    preferences::Preferences,
    state::{dispatch_update, Action, State, StateChanges, StateUpdate},
};
use protocol::{Message, PlaybackCommand, PlaybackState, Track};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::sync::RwLock;
use uuid::Uuid;

mod audio;
mod connection;
mod dnd_resolver;
mod gui;
mod key;
mod lrc;
mod preferences;
mod state;
mod transmit;

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
    // TODO: cli flag for auto connecting to a server for development
    let args: Vec<String> = std::env::args().collect();
    let is_temp = match args.get(1).map(|s| s.as_str()) {
        Some("-t") => true, // use temporary identity for testing
        _ => false,         // len is 1 but index is 1 :3
    };

    let mut t = MainThread::setup(is_temp).await;
    t.run().await;

    if !is_temp {
        let state = t.state.read().await;
        state.preferences.save(&state.preferences_path);
    } else {
        // don't bother saving.. delete even
        std::fs::remove_file(&t.state.read().await.preferences_path)?;
    }

    println!("exited gracefully");

    Ok(())
}

impl State {
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

struct MainThread {
    state: Arc<RwLock<State>>,

    connection: Option<ConnectionActorHandle>,

    action_rx: tokio::sync::mpsc::UnboundedReceiver<Action>,
}

impl MainThread {
    async fn setup(is_temp: bool) -> Self {
        let key = Key::load().expect("failed to load key");

        let preferences_path = if !is_temp {
            preferences::make_path("preferences.json")
        } else {
            // if prefs are saved in temp mode, will be written here and deleted during shutdown
            PathBuf::from("./preferences.tmp.json")
        };
        let preferences = if !is_temp {
            Preferences::load(&preferences_path)
        } else {
            Preferences::default()
        };

        // TODO: this should not be in 2 places. definitely never modify this ever please
        let my_id = preferences.id.to_string();

        let state = Arc::new(RwLock::new(State {
            my_id,
            key,
            preferences,
            preferences_path,
            loading_count: 0,
            connection: None,
            server_statuses: HashMap::new(),
            seek_position: None,
            visualizer: [0; 14],
            buffer: 0,
        }));

        // from ui to main thread logic
        let (action_tx, action_rx) = tokio::sync::mpsc::unbounded_channel::<Action>();

        // spawn ui thread
        std::thread::spawn({
            let state = state.clone();
            move || {
                let mut t = UIThread::new(state, action_tx);
                t.run();
            }
        });

        ui_send!(UIEvent::StateChanged(StateChanges::all()));

        MainThread {
            state,

            connection: None,

            action_rx,
        }
    }

    // main loop
    async fn run(&mut self) {
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

                Some(action) = self.action_rx.recv() => {
                    match action {
                        Action::Quit => break,
                        _ => self.handle_action(action).await,
                    }
                }

                _ = ui_interval.tick() => {
                    ui_send!(UIEvent::Periodic);
                },
            }
        }
    }

    async fn handle_action(&mut self, action: Action) {
        match action {
            Action::Connect { server_id } => {
                if self.connection.is_some() {
                    let current_id = self
                        .state
                        .read()
                        .await
                        .connection
                        .as_ref()
                        .map(|c| c.server_id);
                    if current_id != Some(server_id) {
                        self.disconnect().await;

                        self.connect(server_id).await;
                    }
                } else {
                    self.connect(server_id).await;
                }
            }

            Action::CreateRoom(options) => {
                if let Some(c) = self.connection.as_ref() {
                    let room_id = c.create_room(options).await;
                    // TODO check if this works
                    println!("create_room got response: {:?}", room_id);
                }
            }

            Action::JoinRoom(room_id) => {
                let Some(ref c) = self.connection else {
                    return;
                };

                // TODO: do something with this result
                let _ = c.join_room(room_id).await;
            }

            Action::RefreshServerStatuses => {
                dispatch_update!(StateUpdate::ClearServerStatuses);

                let state = self.state.read().await;
                for server in state.preferences.servers.clone() {
                    tokio::spawn(async move {
                        let status = connection::query_server(&server.addr).await;
                        dispatch_update!(StateUpdate::SetServerStatus(server.id, status))
                    });
                }
            }

            Action::HandleDroppedFiles(data) => {
                let Some(conn) = self.connection.as_mut() else {
                    return;
                };

                let (tracks, num_tracks) =
                    dnd_resolver::resolve_dnd(self.state.clone(), data).await;

                // TODO: do all at once instead of waiting for each
                for t in tracks {
                    // TODO: do something with the success status?
                    let _ = conn.queue_push(t).await;
                }

                dispatch_update!(StateUpdate::DecreaseLoadingCount(num_tracks));
            }

            Action::SavePreferences => {
                let s = self.state.read().await;
                s.preferences.save(&s.preferences_path);
            }

            Action::Play | Action::Stop | Action::Pause | Action::Next | Action::Prev => {
                let Some(conn) = self.connection.as_mut() else {
                    return;
                };

                let cmd = match action {
                    Action::Play => PlaybackCommand::Play,
                    Action::Stop => PlaybackCommand::Stop,
                    Action::Pause => PlaybackCommand::Pause,
                    Action::Next => PlaybackCommand::Next,
                    Action::Prev => PlaybackCommand::Prev,
                    _ => unreachable!(),
                };

                conn.send(Message::PlaybackCommand(cmd)).unwrap();
            }

            Action::DragSeekBar(percent) => {
                let state = self.state.read().await;
                if let Some(track) = state.current_track() {
                    let position = track.metadata.duration * percent;
                    dispatch_update!(StateUpdate::SetSeekPosition(position));
                }
            }
            Action::Seek(percent) => {
                let state = self.state.read().await;
                if let Some(track) = state.current_track() {
                    dispatch_update!(StateUpdate::ClearSeekPosition);

                    let position = track.metadata.duration * percent;

                    let Some(conn) = self.connection.as_mut() else {
                        return;
                    };
                    conn.send(Message::PlaybackCommand(PlaybackCommand::SeekTo(position)))
                        .unwrap();
                }
            }

            Action::Update(update) => {
                self.handle_update(update).await;
            }

            // handled in main loop
            Action::Quit => unreachable!(),
        }
    }

    async fn handle_update(&mut self, update: StateUpdate) {
        let changes = {
            // lock the state for writing
            let mut state = self.state.write().await;

            // update the state and return changes
            state.update(update)
        };

        if changes.contains(StateChanges::Volume) {
            // TODO: spawn task instead of await?
            self.try_update_volume().await;
        }

        // send to ui
        ui_send!(UIEvent::StateChanged(changes));
    }

    fn volume_scale(val: f32) -> f32 {
        val.powf(3.)
    }

    async fn try_update_volume(&mut self) {
        if let Some(conn) = &mut self.connection {
            let scaled = Self::volume_scale(self.state.read().await.preferences.volume);
            conn.audio.send(AudioCommand::Volume(scaled)).unwrap();
        }
    }

    async fn connect(&mut self, server_id: Uuid) {
        let result = ConnectionActor::spawn(self.state.clone(), server_id).await;
        match result {
            Ok(c) => {
                self.connection = Some(c);
                self.try_update_volume().await;
            }
            Err(e) => {
                println!("error while connecting: {:?}", e);
            }
        }
    }

    async fn disconnect(&mut self) {
        // unset the connection handle which drops the thread and starts cleanup
        // ""RAII""
        self.connection = None;

        let state = self.state.read().await;

        // query old server's list after we disconnect so it updates properly?
        if let Some(c) = &state.connection {
            if let Some(server) = state
                .preferences
                .servers
                .iter()
                .find(|s| s.id == c.server_id)
                .cloned()
            {
                tokio::spawn(async move {
                    let status = connection::query_server(&server.addr).await;
                    dispatch_update!(StateUpdate::SetServerStatus(server.id, status))
                });
            }
        }

        dispatch_update!(StateUpdate::ClearConnection);
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
