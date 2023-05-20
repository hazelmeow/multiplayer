use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub mod network;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum PlayingState {
    Playing,
    Paused,
    Stopped,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PlaybackState {
    pub state: PlayingState,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Track {
    pub owner: String,
    pub path: String,
    pub queue_position: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AudioFrame {
    pub frame: u32,
    pub data: Vec<u8>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AuthenticateRequest {
    pub id: String,
    pub name: String,
}

// server to client
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Message {
    Handshake(String),
    Authenticate(AuthenticateRequest),
    PlaybackState(PlaybackState),
    AudioFrame(AudioFrame),

    GetInfo(GetInfo),
    Info(Info),
    QueuePush(Track),

    ConnectedUsers(HashMap<String, String>),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Info {
    QueueList(Vec<Track>),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum GetInfo {
    QueueList,
}
