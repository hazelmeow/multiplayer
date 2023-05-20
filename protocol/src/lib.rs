use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;

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
    pub owner: usize,
    pub path: String,
    pub queue_position: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AudioFrame {
    pub frame: u32,
    pub data: Vec<u8>
}

// server to client
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Message {
    Handshake(String),
    PlaybackState(PlaybackState),
    AudioFrame(AudioFrame),

    GetInfo(GetInfo),
    Info(Info),
    QueuePush(Track)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Info {
    QueueList(Vec<Track>),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum GetInfo {
    QueueList,
}

// client to server
#[derive(Serialize, Deserialize)]
pub enum Command {
    Handshake(String),
    Test(String),
    AudioFrame(AudioFrame),
}

