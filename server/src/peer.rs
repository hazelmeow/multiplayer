use std::sync::Arc;
use std::time::Instant;

use futures::{SinkExt, StreamExt};
use tokio::io;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};
use tokio::time::Duration;

use protocol::network::{MessageStream, MessageStreamError};
use protocol::{Message, Notification};

use crate::{handle_message, Shared};

#[derive(Debug)]
pub struct PeerHandle {
    pub tx: mpsc::UnboundedSender<Message>,
}

pub struct Peer {
    pub id: String,
    pub room_id: Option<u32>,
    pub last_heartbeat: Instant,
    stream: MessageStream<TcpStream>,

    // receives messages from other tasks to be sent
    pub rx: mpsc::UnboundedReceiver<Message>,
}

impl Peer {
    pub async fn new(
        state: Arc<Mutex<Shared>>,
        stream: MessageStream<TcpStream>,
        id: String,
        name: String,
    ) -> io::Result<Peer> {
        let (tx, rx) = mpsc::unbounded_channel();

        let mut state = state.lock().await;

        let handle = PeerHandle { tx };
        state.waiting_peers.insert(id.clone(), handle);

        state.names.insert(id.clone(), name.clone());

        Ok(Peer {
            id,
            room_id: None,
            last_heartbeat: Instant::now(),
            stream,
            rx,
        })
    }

    pub fn log(&self, s: String) {
        println!("* peer {} - {}", self.id, s);
    }

    pub async fn send(&mut self, message: &Message) -> Result<(), MessageStreamError> {
        self.stream.send(message).await
    }
    pub async fn notify(&mut self, notification: Notification) -> Result<(), MessageStreamError> {
        self.send(&Message::Notification(notification)).await
    }

    pub async fn run(&mut self, state: Arc<Mutex<Shared>>) {
        let mut heartbeat_check_interval = tokio::time::interval(Duration::from_secs(10));

        // process incoming messages until disconnected
        loop {
            tokio::select! {
                // forward messages from other tasks to network
                Some(msg) = self.rx.recv() => {
                    self.send(&msg).await.expect("MessageStream.send failed");
                }

                // react to frames from tcp stream
                result = self.stream.next() => match result {
                    // a full frame was received
                    Some(Ok(msg)) => {
                        // TODO: move and split up
                        handle_message(state.clone(), self, &msg).await;
                    }

                    Some(Err(e)) => {
                        println!("error occurred while processing message: {:?}", e)
                    }

                    // socket disconnected
                    None => break
                },

                // if we haven't received a heartbeat in the last 2 minutes, disconnect them
                _ = heartbeat_check_interval.tick() => {
                    let elapsed = self.last_heartbeat.elapsed();
                    if elapsed > Duration::from_secs(120) {
                        break
                    }
                }
            }
        }
    }
}
