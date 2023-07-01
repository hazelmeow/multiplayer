use std::collections::{HashMap, VecDeque};

use serde::{Deserialize, Serialize};

pub mod network;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AudioFrame {
    pub frame: u32,
    pub data: Vec<u8>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum AudioData {
    Start,
    Frame(AudioFrame),
    Finish,
    Resume,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Track {
    pub owner: String,
    pub path: Vec<u8>,
    pub metadata: TrackMetadata,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum TrackArt {
    Jpeg(Vec<u8>),
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct TrackMetadata {
    pub duration: u32,
    pub track_no: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub art: Option<TrackArt>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RoomListing {
    pub id: u32,
    pub name: String,
    pub user_names: Vec<String>,
    // pub options: RoomOptions, //?
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RoomOptions {
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Message {
    QueryRoomList,

    RefreshRoomList,
    AudioData(AudioData),
    Text(String),

    Heartbeat,

    Notification(Notification),

    Request { request_id: u32, data: Request },
    Response { request_id: u32, data: Response },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Notification {
    Playing(Option<Track>),
    Queue(VecDeque<Track>),
    ConnectedUsers(HashMap<String, String>),
    Room(Option<RoomListing>),
    RoomList(Vec<RoomListing>),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Request {
    Handshake(String),
    Authenticate { id: String, name: String },

    JoinRoom(u32),
    LeaveRoom,
    CreateRoom(RoomOptions),

    QueuePush(Track),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Response {
    Success(bool),

    Handshake(String),
    CreateRoomResponse(u32),
}
