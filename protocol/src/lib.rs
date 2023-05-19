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
pub struct AudioFrame {
    frame: u32,

    #[serde(with = "BigArray")]
    data: [u8; 960], // 48khz -> 960samples in a 20ms frame
}

// server to client
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Message {
    PlaybackState(PlaybackState),
    AudioFrame(AudioFrame),
}

// client to server
#[derive(Serialize, Deserialize)]
pub enum Command {
    Test(String),
    AudioFrame(AudioFrame),
}
