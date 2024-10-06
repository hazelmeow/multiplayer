use crate::{
    gui::connection_window::ServerStatus, key::Key, preferences::Preferences,
    transmit::TransmitThreadHandle,
};
use bitflags::bitflags;
use protocol::{PlaybackState, RoomListing, RoomOptions, Track};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
};
use uuid::Uuid;

/// Dispatches an [`Action`] to the main thread.
///
/// TODO: This currently uses the FLTK global sender to send a UIEvent::Action which
/// is forwarded to the main thread by the ui thread. In the future, we should replace this
/// with our own global sender that sends directly to the main thread instead of waking
/// the FLTK event loop to forward the actions.
macro_rules! dispatch {
    ($expression:expr) => {{
        use crate::gui::UIEvent;
        let s = fltk::app::Sender::<UIEvent>::get();
        s.send(UIEvent::Action($expression));
    }};
}
/// Dispatches a [`StateUpdate`] action.
macro_rules! dispatch_update {
    ($expression:expr) => {{
        use crate::state::dispatch;
        use crate::state::Action;
        dispatch!(Action::Update($expression));
    }};
}

pub(crate) use dispatch;
pub(crate) use dispatch_update;

/// Actions are handled on the main thread and usually have side effects.
#[derive(Debug, Clone)]
pub enum Action {
    Connect {
        server_id: Uuid,
    },
    CreateRoom(RoomOptions),
    JoinRoom(u32),

    RefreshServerStatuses,

    SavePreferences,

    Play,
    Stop,
    Pause,
    Next,
    Prev,

    DragSeekBar(f32), // percent
    Seek(f32),        // percent

    HandleDroppedFiles(String),

    /// Quit the app
    Quit,

    Update(StateUpdate),
}

/// Global state.
#[derive(Debug)]
pub struct State {
    pub my_id: String, // TODO: make this a Uuid
    pub key: Key,

    pub preferences: Preferences,
    pub preferences_path: PathBuf,

    pub connection: Option<ConnectionState>,

    pub server_statuses: HashMap<Uuid, ServerStatus>,

    pub loading_count: usize,
    pub seek_position: Option<f32>,

    pub visualizer: [u8; 14],
    pub buffer: u8,
}

#[derive(Debug)]
pub struct ConnectionState {
    pub server_id: Uuid,
    pub room: Option<RoomState>,
}

#[derive(Debug)]
pub struct RoomState {
    pub id: u32,
    pub name: String,

    pub buffering: bool,
    pub elapsed: f32, // seconds

    pub queue: VecDeque<Track>,
    pub current_track: u32,
    pub playback_state: PlaybackState,

    pub connected_users: HashMap<String, String>,

    pub transmit_thread: Option<TransmitThreadHandle>,
}

impl RoomState {
    pub fn current_track(&self) -> Option<&Track> {
        self.queue.iter().find(|t| t.id == self.current_track)
    }
}

// TODO: can these autoincrement somehow...
bitflags! {
    /// A bitmask describing a set of changes to the [`State`].
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct StateChanges: u32 {
        const Volume = 1 << 0;
        const Visualizer = 1 << 1;
        const Buffer = 1 << 2;
        const LoadingCount = 1 << 3;
        const ServerStatuses = 1 << 4;
        const Connection = 1 << 5;
        const RoomQueue = 1 << 6;
        const RoomBuffering = 1 << 7;
        const RoomElapsed = 1 << 8;
        const RoomConnectedUsers = 1 << 9;
        const RoomCurrentTrack = 1 << 10;
        const RoomPlaybackState = 1 << 11;
        const SeekPosition = 1 << 12;
    }
}

const ROOM_ALL: StateChanges = StateChanges::empty()
    .union(StateChanges::RoomQueue)
    .union(StateChanges::RoomBuffering)
    .union(StateChanges::RoomElapsed)
    .union(StateChanges::RoomConnectedUsers)
    .union(StateChanges::RoomCurrentTrack)
    .union(StateChanges::RoomPlaybackState);

/// An operation to be performed on the [`State`].
#[derive(Debug, Clone)]
pub enum StateUpdate {
    SetVolume(f32),
    VolumeUp,
    VolumeDown,

    ClearServerStatuses,
    SetServerStatus(Uuid, ServerStatus),
    SetConnectedServerStatus(ServerStatus),

    NewConnection {
        server_id: Uuid,
    },
    ClearConnection,

    NewRoom(RoomListing),
    ClearRoom,
    SetRoomBuffering(bool),
    SetRoomElapsed(f32),
    SetRoomConnectedUsers(HashMap<String, String>),
    SetRoomQueue(VecDeque<Track>),
    SetRoomCurrentTrack(u32),
    SetRoomPlaybackState(PlaybackState),

    SetSeekPosition(f32),
    ClearSeekPosition,

    SetPreferences {
        name: String,
        lyrics_show_warning_arrows: bool,
        display_album_artist: bool,
    },

    SetVisualizer([u8; 14]),
    SetBufferLevel(u8),
    IncreaseLoadingCount(usize),
    DecreaseLoadingCount(usize),
}

impl State {
    /// Apply a `StateUpdate` and return a `StateChanges` describing what changed.
    pub fn update(&mut self, update: StateUpdate) -> StateChanges {
        match update {
            StateUpdate::SetVolume(volume) => {
                self.preferences.volume = volume.clamp(0.0, 1.0);
                StateChanges::Volume
            }
            StateUpdate::VolumeUp => {
                self.preferences.volume = (self.preferences.volume + 0.02).clamp(0.0, 1.0);
                StateChanges::Volume
            }
            StateUpdate::VolumeDown => {
                self.preferences.volume = (self.preferences.volume - 0.02).clamp(0.0, 1.0);
                StateChanges::Volume
            }
            StateUpdate::ClearServerStatuses => {
                self.server_statuses.clear();
                StateChanges::ServerStatuses
            }
            StateUpdate::SetServerStatus(id, status) => {
                self.server_statuses.insert(id, status);
                StateChanges::ServerStatuses
            }
            StateUpdate::SetConnectedServerStatus(status) => {
                if let Some(c) = &self.connection {
                    self.server_statuses.insert(c.server_id, status);
                    StateChanges::ServerStatuses
                } else {
                    StateChanges::empty()
                }
            }
            StateUpdate::NewConnection { server_id } => {
                self.connection = Some(ConnectionState {
                    server_id,
                    room: None,
                });
                StateChanges::Connection
            }
            StateUpdate::ClearConnection => {
                self.connection = None;
                StateChanges::Connection
            }
            StateUpdate::NewRoom(room) => {
                let Some(connection) = self.connection.as_mut() else {
                    println!("StateUpdate::NewRoom handled with no connection");
                    return StateChanges::empty();
                };

                connection.room = Some(RoomState {
                    id: room.id,
                    name: room.name,
                    queue: VecDeque::new(),
                    current_track: 0,
                    playback_state: PlaybackState::Stopped,
                    connected_users: HashMap::new(),
                    buffering: false,
                    elapsed: 0.0,
                    transmit_thread: None,
                });

                ROOM_ALL
            }
            StateUpdate::ClearRoom => {
                let Some(connection) = self.connection.as_mut() else {
                    println!("StateUpdate::ClearRoom handled with no connection");
                    return StateChanges::empty();
                };

                connection.room = None;

                ROOM_ALL
            }
            StateUpdate::SetRoomBuffering(buffering) => {
                let Some(room) = self.connection.as_mut().and_then(|c| c.room.as_mut()) else {
                    println!("StateUpdate::SetRoomBuffering handled with no room");
                    return StateChanges::empty();
                };

                room.buffering = buffering;

                StateChanges::RoomBuffering
            }
            StateUpdate::SetRoomElapsed(elapsed) => {
                let Some(room) = self.connection.as_mut().and_then(|c| c.room.as_mut()) else {
                    println!("StateUpdate::SetRoomElapsed handled with no room");
                    return StateChanges::empty();
                };

                room.elapsed = elapsed;

                StateChanges::RoomElapsed
            }
            StateUpdate::SetRoomConnectedUsers(connected_users) => {
                let Some(room) = self.connection.as_mut().and_then(|c| c.room.as_mut()) else {
                    println!("StateUpdate::SetRoomConnectedUsers handled with no room");
                    return StateChanges::empty();
                };

                room.connected_users = connected_users;

                StateChanges::RoomConnectedUsers
            }
            StateUpdate::SetRoomQueue(queue) => {
                let Some(room) = self.connection.as_mut().and_then(|c| c.room.as_mut()) else {
                    println!("StateUpdate::SetRoomQueue handled with no room");
                    return StateChanges::empty();
                };

                room.queue = queue;

                StateChanges::RoomQueue
            }
            StateUpdate::SetRoomCurrentTrack(current_track) => {
                let Some(room) = self.connection.as_mut().and_then(|c| c.room.as_mut()) else {
                    println!("StateUpdate::SetRoomCurrentTrack handled with no room");
                    return StateChanges::empty();
                };

                room.current_track = current_track;

                StateChanges::RoomCurrentTrack
            }
            StateUpdate::SetRoomPlaybackState(playback_state) => {
                let Some(room) = self.connection.as_mut().and_then(|c| c.room.as_mut()) else {
                    println!("StateUpdate::SetRoomPlaybackState handled with no room");
                    return StateChanges::empty();
                };

                room.playback_state = playback_state;

                StateChanges::RoomPlaybackState
            }

            StateUpdate::SetSeekPosition(seek_position) => {
                self.seek_position = Some(seek_position);
                StateChanges::SeekPosition
            }
            StateUpdate::ClearSeekPosition => {
                self.seek_position = None;
                StateChanges::SeekPosition
            }
            StateUpdate::SetPreferences {
                name,
                lyrics_show_warning_arrows,
                display_album_artist,
            } => {
                self.preferences.name = name;
                self.preferences.lyrics_show_warning_arrows = lyrics_show_warning_arrows;
                self.preferences.display_album_artist = display_album_artist;
                StateChanges::empty()
            }
            StateUpdate::SetVisualizer(visualizer) => {
                self.visualizer = visualizer;
                StateChanges::Visualizer
            }
            StateUpdate::SetBufferLevel(buffer) => {
                self.buffer = buffer;
                StateChanges::Buffer
            }
            StateUpdate::IncreaseLoadingCount(x) => {
                self.loading_count += x;
                StateChanges::LoadingCount
            }
            StateUpdate::DecreaseLoadingCount(x) => {
                self.loading_count -= x;
                StateChanges::LoadingCount
            }
        }
    }
}
