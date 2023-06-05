use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;

use futures::StreamExt;
use tokio::io;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};

use protocol::network::FrameStream;
use protocol::{Message, RoomOptions, Track};

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

type Id = String;

struct Room {
    name: String,

    playing: Option<Track>,
    queue: VecDeque<Track>,

    peers: HashMap<Id, PeerHandle>,
}

impl Room {
    fn new(options: RoomOptions) -> Self {
        Room {
            name: options.name,

            playing: None,
            queue: VecDeque::new(),

            peers: HashMap::new(),
        }
    }

    async fn broadcast(&mut self, message: &Message) {
        for (id, peer) in self.peers.iter_mut() {
            if let Err(e) = peer.tx.send(message.clone()) {
                println!("error broadcasting message: {:?}", e);
            }
        }
    }

    async fn broadcast_others(&mut self, sender: &Peer, message: &Message) {
        for (id, peer) in self.peers.iter_mut() {
            if *id != sender.id {
                if let Err(e) = peer.tx.send(message.clone()) {
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

    fn connected_users_info(&self, mut names: HashMap<Id, String>) -> Message {
        names.retain(|id, _| self.peers.contains_key(id));
        Message::Info(protocol::Info::ConnectedUsers(names))
    }
}

struct Shared {
    next_room: usize,
    rooms: HashMap<usize, Room>,

    waiting_peers: HashMap<Id, PeerHandle>,

    names: HashMap<Id, String>,
}

impl Shared {
    fn new() -> Self {
        Shared {
            next_room: 0,
            rooms: HashMap::new(),

            waiting_peers: HashMap::new(),

            names: HashMap::new(),
        }
    }
}

struct PeerHandle {
    tx: Tx,
}

struct Peer {
    id: String,
    room_id: Option<usize>,
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
        let (tx, rx) = mpsc::unbounded_channel();

        let addr = stream.get_tcp_stream().peer_addr()?;

        let mut state = state.lock().await;

        let handle = PeerHandle { tx };
        state.waiting_peers.insert(id.clone(), handle);

        state.names.insert(id.clone(), name.clone());

        Ok(Peer {
            id,
            room_id: None,
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
            Message::Authenticate { id, name } => {
                println!("* {} - authenticate: {}", addr, id);

                // todo: check id format? check if in use? send success message?

                handle_authenticated_connection(state, stream, id, name).await
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

async fn leave_room(state: Arc<Mutex<Shared>>, peer: &mut Peer) {
    let mut state = state.lock().await;
    let names = state.names.clone();

    let Some(room_id) = peer.room_id else { return };
    let Some(room) = state.rooms.get_mut(&room_id) else { return };

    let handle = room.peers.remove(&peer.id).unwrap();
    peer.room_id = None;

    // TODO: FIX THIS
    // if room.peers.is_empty() {
    //     state.rooms.remove(&room_id);
    // }

    let connected_users = room.connected_users_info(names);
    room.broadcast(&connected_users).await;

    state.waiting_peers.insert(peer.id.clone(), handle);
}

async fn join_room(state: Arc<Mutex<Shared>>, peer: &mut Peer, room_id: usize) {
    let mut state = state.lock().await;
    let names = state.names.clone();

    let Some(handle) = state.waiting_peers.remove(&peer.id) else { return };

    let Some(room) = state.rooms.get_mut(&room_id) else { return };
    room.peers.insert(peer.id.clone(), handle);

    peer.room_id = Some(room_id);

    // notify everyone of the new ConnectedUsers list
    let connected_users = room.connected_users_info(names);
    room.broadcast(&connected_users).await;

    // notify new peer of current playing track and queue
    let playing = room.playing_info();
    peer.stream.send(&playing).await.unwrap();
    let queue = room.queue_info();
    peer.stream.send(&queue).await.unwrap();
}

async fn handle_authenticated_connection(
    state: Arc<Mutex<Shared>>,
    stream: FrameStream,
    id: String,
    name: String,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    // create a new Peer
    let mut peer = Peer::new(state.clone(), stream, id, name).await?;

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
        Message::CreateRoom(options) => {
            let mut state = state.lock().await;

            let room_id = state.next_room;
            state.next_room += 1;

            let room = Room::new(options.clone());
            state.rooms.insert(room_id, room);

            Ok(())
        }

        Message::JoinRoom(room_id) => {
            join_room(state, peer, *room_id).await;
            Ok(())
        }

        Message::AudioData(data) => {
            match data {
                protocol::AudioData::Frame(_) => {} // dont log
                _ => {
                    println!("* {} - audio data {:?}", peer.addr, data);
                }
            }

            let mut state = state.lock().await;
            let Some(room) = peer.room_id.map(|i| state.rooms.get_mut(&i).unwrap()) else { return Ok(()) };

            match data {
                // intercept Finish in case we want to actually just start the next song instead
                protocol::AudioData::Finish => {
                    let next_track = room.queue.pop_front();
                    if let Some(track) = next_track {
                        room.playing = Some(track);

                        let p = room.playing_info();
                        room.broadcast(&p).await;
                        let q = room.queue_info();
                        room.broadcast(&q).await;
                    } else {
                        // pass to rest
                        room.playing = None;
                        let p = room.playing_info();
                        room.broadcast(&p).await;

                        room.broadcast(m).await;
                    }
                }
                _ => {
                    // resend other messages to all other peers
                    room.broadcast_others(&peer, m).await;
                }
            }

            Ok(())
        }

        Message::GetInfo(info) => {
            let mut state = state.lock().await;

            match info {
                protocol::GetInfo::Playing => {
                    let Some(room) = peer.room_id.map(|i| state.rooms.get_mut(&i).unwrap()) else { return Ok(()) };

                    let m = room.playing_info();
                    peer.stream.send(&m).await?;

                    Ok(())
                }
                protocol::GetInfo::Queue => {
                    let Some(room) = peer.room_id.map(|i| state.rooms.get_mut(&i).unwrap()) else { return Ok(()) };

                    let m = room.queue_info();
                    peer.stream.send(&m).await?;

                    Ok(())
                }
                protocol::GetInfo::ConnectedUsers => {
                    let names = state.names.clone();
                    let Some(room) = peer.room_id.map(|i| state.rooms.get_mut(&i).unwrap()) else { return Ok(()) };

                    let m = room.connected_users_info(names);
                    peer.stream.send(&m).await?;

                    Ok(())
                }
            }
        }

        Message::QueuePush(track) => {
            let mut state = state.lock().await;
            let Some(room) = peer.room_id.map(|i| state.rooms.get_mut(&i).unwrap()) else { return Ok(()) };

            // just play it now
            if room.playing.is_none() {
                room.playing = Some(track.to_owned());

                let playing_msg = room.playing_info();
                room.broadcast(&playing_msg).await;
            } else {
                room.queue.push_back(track.to_owned());
            }

            let queue_msg = room.queue_info();
            room.broadcast(&queue_msg).await;

            Ok(())
        }

        Message::Text(text) => {
            println!("* {} - '{}'", peer.addr, text);

            let mut state = state.lock().await;
            let Some(room) = peer.room_id.map(|i| state.rooms.get_mut(&i).unwrap()) else { return Ok(()) };

            // resend this message to all other peers
            room.broadcast_others(&peer, m).await;

            Ok(())
        }

        // should not be reached
        Message::Handshake(_) => Ok(()),
        Message::Authenticate { id, name } => Ok(()),
        Message::Info(_) => Ok(()),
    }
}

async fn handle_disconnect(state: Arc<Mutex<Shared>>, peer: Peer) {
    let mut state = state.lock().await;
    state.names.remove(&peer.id);
    let names = state.names.clone();

    if let Some(room_id) = peer.room_id {
        let room = state.rooms.get_mut(&room_id).unwrap();

        room.peers.remove(&peer.id);

        let msg = room.connected_users_info(names);
        room.broadcast(&msg).await;
    }

    // TODO: notify waiting peers of new room list
}
