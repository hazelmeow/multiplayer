use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;

use futures::StreamExt;
use tokio::io;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};

use protocol::network::FrameStream;
use protocol::{AudioData, Message, RoomListing, RoomOptions, Track};

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
    fn room_users(&self, mut names: HashMap<Id, String>) -> Vec<String> {
        names.retain(|id, _| self.peers.contains_key(id));
        names.keys().into_iter().map(|x| x.to_owned()).collect()
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

    fn room_list_info(&self) -> Message {
        let list: Vec<RoomListing> = self
            .rooms
            .iter()
            .map(|(room_id, room)| RoomListing {
                id: *room_id,
                name: room.name.clone(),
                user_names: room
                    .peers
                    .keys()
                    .map(|id| self.names.get(id).unwrap().to_owned())
                    .collect(),
            })
            .collect();

        Message::Info(protocol::Info::RoomList(list))
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
            Message::QueryRoomList => {
                let state = state.lock().await;
                stream.send(&state.room_list_info()).await?;

                return Ok(());
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

    peer.stream.send(&Message::Info(protocol::Info::Room(None))).await.unwrap();

    state.waiting_peers.insert(peer.id.clone(), handle);
}

async fn join_room(state_mutex: Arc<Mutex<Shared>>, peer: &mut Peer, room_id: usize) {
    let mut state = state_mutex.lock().await;
    let names = state.names.clone();
    let names2 = state.names.clone(); // blehhh

    let Some(handle) = state.waiting_peers.remove(&peer.id) else { return };

    let Some(room) = state.rooms.get_mut(&room_id) else { return };
    room.peers.insert(peer.id.clone(), handle);

    peer.room_id = Some(room_id);

    // notify everyone of the new ConnectedUsers list
    let connected_users = room.connected_users_info(names);
    room.broadcast(&connected_users).await;

    // notify new peer of current playing track and queue
    let room_info = protocol::Info::Room(Some(RoomListing { id: room_id, name: room.name.clone(), user_names: room.room_users(names2) }));
    peer.stream.send(&Message::Info(room_info)).await.unwrap();
    let playing = room.playing_info();
    peer.stream.send(&playing).await.unwrap();
    let queue = room.queue_info();
    peer.stream.send(&queue).await.unwrap();

    drop(state);
    let state = state_mutex.lock().await;
    let room_list_info = state.room_list_info();
    peer.stream.send(&room_list_info).await.unwrap();
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
    handle_disconnect(state, &mut peer).await;

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

        Message::LeaveRoom => {
            leave_room(state, peer).await;
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
                AudioData::Finish => {
                    let next_track = room.queue.pop_front();
                    if let Some(track) = next_track {
                        // we have a new track to play, don't send the finish method anywhere
                        // this is REALLY silly maybe?
                        room.playing = Some(track);

                        // send playing and queue state
                        let p = room.playing_info();
                        room.broadcast(&p).await;
                        let q = room.queue_info();
                        room.broadcast(&q).await;
                    } else {
                        // the track finished and we have nothing else to play
                        // send playing state
                        room.playing = None;
                        let p = room.playing_info();
                        room.broadcast(&p).await;

                        // broadcast a finish method to everyone
                        room.broadcast(&Message::AudioData(AudioData::Finish)).await;
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
                protocol::GetInfo::Room => {
                    
                    Ok(())
                }
                protocol::GetInfo::RoomList => {
                    peer.stream.send(&state.room_list_info()).await?;
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
        Message::QueryRoomList => Ok(()),
        Message::Handshake(_) => Ok(()),
        Message::Authenticate { id, name } => Ok(()),
        Message::Info(_) => Ok(()),
    }
}

async fn handle_disconnect(state: Arc<Mutex<Shared>>, peer: &mut Peer) {
    // remove them from the room cleanly and notify others
    leave_room(state.clone(), peer).await;

    // remove name from the map
    let mut state = state.lock().await;
    state.names.remove(&peer.id);

    // remove handle
    state.waiting_peers.remove(&peer.id);
}
