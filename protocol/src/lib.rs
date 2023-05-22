use std::collections::{HashMap, VecDeque};

use serde::{Deserialize, Serialize};

pub mod network;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AudioFrame {
    pub frame: u32,
    pub data: Vec<u8>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum AudioData {
    Start,
    StartLate(usize),
    Frame(AudioFrame),
    Stop,
    Resume,
    Finish,
    Clear,
    Shutdown,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Track {
    pub owner: String,
    pub path: String, // TODO: encrypt this??
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AuthenticateRequest {
    pub id: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Info {
    Playing(Option<Track>),
    Queue(VecDeque<Track>),
    ConnectedUsers(HashMap<String, String>),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum GetInfo {
    Playing,
    Queue,
    ConnectedUsers,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Message {
    Handshake(String),
    Authenticate(AuthenticateRequest),

    AudioData(AudioData),

    Text(String),

    QueuePush(Track),

    GetInfo(GetInfo),
    Info(Info),
}
