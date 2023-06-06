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
    pub duration: usize,
    pub track_no: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub art: Option<TrackArt>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Info {
    Playing(Option<Track>),
    Queue(VecDeque<Track>),
    ConnectedUsers(HashMap<String, String>),
    Room(Option<RoomOptions>),
    RoomList(Vec<RoomListing>),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum GetInfo {
    Playing,
    Queue,
    ConnectedUsers,
    Room,
    RoomList,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RoomListing {
    pub id: usize,
    pub name: String,
    // pub options: RoomOptions, //?
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RoomOptions {
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Message {
    QueryRoomList,

    Handshake(String),
    Authenticate { id: String, name: String },

    JoinRoom(usize),
    CreateRoom(RoomOptions),

    AudioData(AudioData),

    Text(String),

    QueuePush(Track),

    GetInfo(GetInfo),
    Info(Info),
}
