use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;

use futures::StreamExt;
use tokio::io;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};

use protocol::network::FrameStream;
use protocol::{Message, Track};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let bind_addr = "127.0.0.1:8080".to_string();
    let listener = TcpListener::bind(&bind_addr).await?;
    println!("listening on {}", bind_addr);

    // create the shared state
    // a handle is cloned and passed into the task that handles the client connection
    let state = Arc::new(Mutex::new(Shared::new()));

    loop {
        let (stream, addr) = listener.accept().await?;

        let state = Arc::clone(&state);

        tokio::spawn(async move {
            println!("accepted connection from {:?}", addr);

            if let Err(e) = handle_connection(state, stream, addr).await {
                println!("an error occurred: {:?}", e);
            }
        });
    }
}

type Tx = mpsc::UnboundedSender<Message>;
type Rx = mpsc::UnboundedReceiver<Message>;

struct Shared {
    peers: HashMap<String, Tx>, // map of mpsc channel senders
    peer_names: HashMap<String, String>,

    playing: Option<Track>,
    queue: VecDeque<Track>,
}

impl Shared {
    fn new() -> Self {
        Shared {
            peers: HashMap::new(),
            peer_names: HashMap::new(),

            playing: None,
            queue: VecDeque::new(),
        }
    }

    async fn broadcast(&mut self, message: &Message) {
        for (_, tx) in self.peers.iter_mut() {
            if let Err(e) = tx.send(message.clone()) {
                println!("error broadcasting message: {:?}", e);
            }
        }
    }

    async fn broadcast_others(&mut self, sender: &Peer, message: &Message) {
        for (peer_addr, tx) in self.peers.iter_mut() {
            if *peer_addr != sender.id {
                if let Err(e) = tx.send(message.clone()) {
                    println!("error broadcasting message: {:?}", e);
                }
            }
        }
    }

    fn playing_info(&self) -> Message {
        Message::Info(protocol::Info::Playing(self.playing.clone()))
    }

    fn queue_info(&self) -> Message {
        Message::Info(protocol::Info::Queue(self.queue.clone()))
    }

    fn connected_users_info(&self) -> Message {
        let names = self.peer_names.clone();
        Message::Info(protocol::Info::ConnectedUsers(names))
    }
}

struct Peer {
    id: String,
    name: String,
    addr: SocketAddr,
    stream: FrameStream,
    rx: Rx, // mpsc channel used to receive messages from peers
}

impl Peer {
    async fn new(
        state: Arc<Mutex<Shared>>,
        stream: FrameStream,
        id: String,
        name: String,
    ) -> io::Result<Peer> {
        // create channel
        let (tx, rx) = mpsc::unbounded_channel();

        // get our SocketAddr and insert the channel sender into the shared state
        let addr = stream.get_tcp_stream().peer_addr()?;

        let mut state = state.lock().await;
        state.peers.insert(id.clone(), tx);
        state.peer_names.insert(id.clone(), name.clone());

        Ok(Peer {
            id,
            name,
            addr,
            stream,
            rx,
        })
    }
}

async fn handle_connection(
    state: Arc<Mutex<Shared>>,
    tcp_stream: TcpStream,
    addr: SocketAddr,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut stream = FrameStream::new(tcp_stream);

    // handshake
    if let Some(handshake_msg) = stream.next_frame().await? {
        match handshake_msg {
            Message::Handshake(hs) => {
                println!("* {} - handshake: {:x?} ('{}')", addr, hs.as_bytes(), hs);
                // pretend it's a version number or something useful
                if hs == "meow" {
                    // now reply
                    stream.send(&Message::Handshake("nyaa".to_string())).await?;
                } else {
                    println!("* {} - invalid handshake", addr);
                    return Ok(());
                }
            }
            _ => {
                // wrong message type
                return Ok(());
            }
        }
    } else {
        // socket was disconnected
        return Ok(());
    }

    // authenticate and continue to other handler
    if let Some(auth_msg) = stream.next_frame().await? {
        match auth_msg {
            Message::Authenticate(request) => {
                println!("* {} - authenticate: {}", addr, request.id);

                // todo: check id format? check if in use? send success message?

                handle_authenticated_connection(state, stream, request.id, request.name).await
            }
            _ => {
                // wrong message type
                Ok(())
            }
        }
    } else {
        // socket was disconnected
        Ok(())
    }
}

async fn handle_authenticated_connection(
    state: Arc<Mutex<Shared>>,
    stream: FrameStream,
    id: String,
    name: String,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    // create a new Peer
    let mut peer = Peer::new(state.clone(), stream, id, name).await?;

    {
        let mut state = state.lock().await;

        // notify everyone of the new ConnectedUsers list
        let connected_users = state.connected_users_info();
        state.broadcast(&connected_users).await;

        // notify new peer of current playing track and queue
        let playing = state.playing_info();
        peer.stream.send(&playing).await.unwrap();
        let queue = state.queue_info();
        peer.stream.send(&queue).await.unwrap();
    }

    // process incoming messages until disconnected
    loop {
        tokio::select! {
            // react to messages on mpsc channel
            Some(msg) = peer.rx.recv() => {
                peer.stream.send(&msg).await?;
            }

            // react to frames from tcp stream
            result = peer.stream.get_inner().next() => match result {
                // a full frame was received
                Some(Ok(bytes)) => {
                    let msg: Message = bincode::deserialize(&bytes).expect("failed to deserialize message");
                    //println!("* {} - {:?}", addr, &msg);

                    handle_message(state.clone(), &mut peer, &msg).await?;
                }

                Some(Err(e)) => {
                    println!("error occurred while processing message: {:?}", e)
                }

                // socket disconnected
                None => break
            },
        }
    }

    // client disconnected gracefully
    handle_disconnect(state, peer).await;

    Ok(())
}

async fn handle_message(
    state: Arc<Mutex<Shared>>,
    peer: &mut Peer,
    m: &Message,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    match m {
        Message::AudioData(data) => {
            match data {
                protocol::AudioData::Frame(_) => {} // dont log
                _ => {
                    println!("* {} - audio data {:?}", peer.addr, data);
                }
            }

            let mut state = state.lock().await;

            match data {
                // intercept Finish in case we want to actually just start the next song instead
                protocol::AudioData::Finish => {
                    let next_track = state.queue.pop_front();
                    if let Some(track) = next_track {
                        state.playing = Some(track);

                        let p = state.playing_info();
                        state.broadcast(&p).await;
                        let q = state.queue_info();
                        state.broadcast(&q).await;
                    } else {
                        // pass to rest
                        state.playing = None;
                        let p = state.playing_info();
                        state.broadcast(&p).await;
                        
                        state.broadcast_others(&peer, m).await;
                    }
                }
                _ => {
                    // resend other messages to all other peers
                    state.broadcast_others(&peer, m).await;
                }
            }

            Ok(())
        }

        Message::GetInfo(info) => {
            let state = state.lock().await;
            match info {
                protocol::GetInfo::Playing => {
                    let m = state.playing_info();
                    peer.stream.send(&m).await?;

                    Ok(())
                }
                protocol::GetInfo::Queue => {
                    let m = state.queue_info();
                    peer.stream.send(&m).await?;

                    Ok(())
                }
                protocol::GetInfo::ConnectedUsers => {
                    let m = state.connected_users_info();
                    peer.stream.send(&m).await?;

                    Ok(())
                }
            }
        }

        Message::QueuePush(track) => {
            let mut state = state.lock().await;

            // just play it now
            if state.playing.is_none() {
                state.playing = Some(track.to_owned());

                let playing_msg = state.playing_info();
                state.broadcast(&playing_msg).await;
            } else {
                state.queue.push_back(track.to_owned());
            }

            let queue_msg = state.queue_info();
            state.broadcast(&queue_msg).await;

            Ok(())
        }

        Message::Text(text) => {
            println!("* {} - '{}'", peer.addr, text);
            // resend this message to all other peers
            state.lock().await.broadcast_others(&peer, m).await;
            Ok(())
        }

        // should not be reached
        Message::Handshake(_) => Ok(()),
        Message::Authenticate(_) => Ok(()),
        Message::Info(_) => Ok(()),
    }
}

async fn handle_disconnect(state: Arc<Mutex<Shared>>, peer: Peer) {
    let mut state = state.lock().await;
    state.peers.remove(&peer.id);
    state.peer_names.remove(&peer.id);

    let msg = state.connected_users_info();
    state.broadcast(&msg).await;
}
