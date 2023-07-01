use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use tokio::io;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};

use protocol::network::FrameStream;
use protocol::{
    AudioData, Message, Notification, Request, Response, RoomListing, RoomOptions, Track,
};

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
        for (_, peer) in self.peers.iter_mut() {
            if let Err(e) = peer.tx.send(message.clone()) {
                println!("error broadcasting message: {:?}", e);
            }
        }
    }

    async fn notify(&mut self, notification: Notification) {
        self.broadcast(&Message::Notification(notification)).await
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

    fn playing_notification(&self) -> Notification {
        Notification::Playing(self.playing.clone())
    }

    fn queue_notification(&self) -> Notification {
        Notification::Queue(self.queue.clone())
    }

    fn connected_users_notification(&self, mut names: HashMap<Id, String>) -> Notification {
        names.retain(|id, _| self.peers.contains_key(id));
        Notification::ConnectedUsers(names)
    }

    fn room_users(&self, mut names: HashMap<Id, String>) -> Vec<String> {
        names.retain(|id, _| self.peers.contains_key(id));
        names.keys().into_iter().map(|x| x.to_owned()).collect()
    }
}

struct Shared {
    next_room: u32,
    rooms: HashMap<u32, Room>,

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

    async fn notify(&mut self, notification: Notification) {
        let m = Message::Notification(notification);
        for p in self.waiting_peers.values_mut() {
            let _ = p.tx.send(m.clone());
        }

        for r in self.rooms.values_mut() {
            r.broadcast(&m).await;
        }
    }

    fn room_list_notification(&self) -> Notification {
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

        Notification::RoomList(list)
    }
}

struct PeerHandle {
    tx: Tx,
}

struct Peer {
    id: String,
    room_id: Option<u32>,
    last_heartbeat: Instant,
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
            last_heartbeat: Instant::now(),
            addr,
            stream,
            rx,
        })
    }

    async fn notify(
        &mut self,
        notification: Notification,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.stream.send(&Message::Notification(notification)).await
    }
}

async fn handle_connection(
    state: Arc<Mutex<Shared>>,
    tcp_stream: TcpStream,
    addr: SocketAddr,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut stream = FrameStream::new(tcp_stream);

    // first message received must be a valid handshake
    if let Ok(Some(first_msg)) = stream.next_frame().await {
        match first_msg {
            Message::Request {
                request_id,
                data: request,
            } => match request {
                Request::Handshake(hs) => {
                    println!("* {} - handshake", addr);
                    // pretend it's a version number or something useful
                    if hs == "meow" {
                        // now reply
                        stream
                            .send(&Message::Response {
                                request_id,
                                data: Response::Handshake("nyaa".to_string()),
                            })
                            .await?;

                        return handle_handshaken_connection(state, stream).await;
                    } else {
                        println!("* {} - invalid handshake", addr);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    // invalid first message or the socket disconnected or errored
    Err("handshake failed".into())
}

async fn handle_handshaken_connection(
    state: Arc<Mutex<Shared>>,
    mut stream: FrameStream,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    // loop for un-upgraded connections
    while let Some(msg) = stream.next_frame().await? {
        match msg {
            Message::QueryRoomList => {
                let state = state.lock().await;
                stream
                    .send(&Message::Notification(state.room_list_notification()))
                    .await?;
            }

            Message::Request {
                request_id,
                data: request,
            } => match request {
                Request::Authenticate { id, name } => {
                    println!("* {} - authenticate: {}", stream.get_addr().unwrap(), id);

                    stream
                        .send(&Message::Response {
                            request_id,
                            data: Response::Success(true),
                        })
                        .await?;

                    // todo: check id format? check if in use? send success message?

                    return handle_authenticated_connection(state, stream, id, name).await;
                }

                _ => {
                    println!("not allowed request was {:?}", request);
                    return Err("not allowed".into());
                }
            },

            // sure
            Message::Heartbeat => {}

            _ => {
                println!("not allowed message was {:?}", msg);
                return Err("not allowed".into());
            }
        }
    }

    // stream disconnected
    Ok(())
}

// Ok(success true/false), Err(we should disconnect them?)
async fn leave_room(
    state: Arc<Mutex<Shared>>,
    peer: &mut Peer,
) -> Result<bool, Box<dyn Error + Send + Sync>> {
    let mut state = state.lock().await;
    let names = state.names.clone();

    let Some(room_id) = peer.room_id else { return Ok(false) };
    let Some(room) = state.rooms.get_mut(&room_id) else { return Ok(false) };

    let handle = room.peers.remove(&peer.id).unwrap();
    peer.room_id = None;

    // TODO: FIX THIS
    // if room.peers.is_empty() {
    //     state.rooms.remove(&room_id);
    // }

    room.notify(room.connected_users_notification(names)).await;
    peer.notify(Notification::Room(None)).await?;

    state.waiting_peers.insert(peer.id.clone(), handle);

    let room_list = state.room_list_notification();
    state.notify(room_list).await;

    Ok(true)
}

async fn join_room(
    state_mutex: Arc<Mutex<Shared>>,
    peer: &mut Peer,
    room_id: u32,
) -> Result<bool, Box<dyn Error + Send + Sync>> {
    let mut state = state_mutex.lock().await;
    let names = state.names.clone();
    let names2 = state.names.clone(); // blehhh

    let Some(handle) = state.waiting_peers.remove(&peer.id) else { return Ok(false) };

    let Some(room) = state.rooms.get_mut(&room_id) else { return Ok(false) };
    room.peers.insert(peer.id.clone(), handle);

    peer.room_id = Some(room_id);

    // notify everyone of the new ConnectedUsers list
    room.notify(room.connected_users_notification(names)).await;

    // notify new peer of current playing track and queue
    peer.notify(Notification::Room(Some(RoomListing {
        id: room_id,
        name: room.name.clone(),
        user_names: room.room_users(names2),
    })))
    .await?;
    peer.notify(room.playing_notification()).await?;
    peer.notify(room.queue_notification()).await?;

    let room_list = state.room_list_notification();
    state.notify(room_list).await;

    Ok(true)
}

async fn handle_authenticated_connection(
    state: Arc<Mutex<Shared>>,
    stream: FrameStream,
    id: String,
    name: String,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    // create a new Peer
    let mut peer = Peer::new(state.clone(), stream, id, name).await?;

    let mut heartbeat_check_interval = tokio::time::interval(tokio::time::Duration::from_secs(10));

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

            // if we haven't received a heartbeat in the last 2 minutes, disconnect them
            _ = heartbeat_check_interval.tick() => {
                let elapsed = peer.last_heartbeat.elapsed();
                if elapsed > Duration::from_secs(120) {
                    break
                }
            }
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
    match &m {
        Message::Request {
            request_id,
            data: request,
        } => {
            let result = handle_request(state, peer, request).await;

            match result {
                Ok(Some(response)) => {
                    peer.stream
                        .send(&Message::Response {
                            request_id: *request_id,
                            data: response,
                        })
                        .await?;

                    Ok(())
                }
                Ok(None) => Ok(()),
                Err(e) => return Err(e),
            }
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
                        let p = room.playing_notification();
                        room.broadcast(&Message::Notification(p)).await;
                        let q = room.queue_notification();
                        room.broadcast(&Message::Notification(q)).await;
                    } else {
                        // the track finished and we have nothing else to play
                        // send playing state
                        room.playing = None;
                        let p = room.playing_notification();
                        room.broadcast(&Message::Notification(p)).await;

                        // broadcast a finish method to everyone
                        room.broadcast(&Message::AudioData(AudioData::Finish)).await;
                    }
                }
                _ => {
                    // resend other messages to all other peers
                    room.broadcast_others(&peer, &m).await;
                }
            }

            Ok(())
        }

        Message::RefreshRoomList => {
            let state = state.lock().await;
            peer.notify(state.room_list_notification()).await?;
            Ok(())
        }

        Message::Text(text) => {
            println!("* {} - '{}'", peer.addr, text);

            let mut state = state.lock().await;
            let Some(room) = peer.room_id.map(|i| state.rooms.get_mut(&i).unwrap()) else { return Ok(()) };

            // resend this message to all other peers
            room.broadcast_others(&peer, &m).await;

            Ok(())
        }

        Message::Heartbeat => {
            peer.last_heartbeat = Instant::now();
            Ok(())
        }

        // should not be reached
        Message::QueryRoomList => Ok(()),
        Message::Notification(_) => Ok(()),
        Message::Response { .. } => Ok(()),
    }
}

// Err -> something bad happened and we should disconnect them
// Ok(Some) -> send a response
// Ok(None) -> no response to send (something happened that wasn't supposed to happen but it's fine)
async fn handle_request(
    state: Arc<Mutex<Shared>>,
    peer: &mut Peer,
    r: &Request,
) -> Result<Option<Response>, Box<dyn Error + Send + Sync>> {
    match r {
        Request::CreateRoom(options) => {
            let mut state = state.lock().await;

            let room_id = state.next_room;
            state.next_room += 1;

            let room = Room::new(options.clone());
            state.rooms.insert(room_id, room);

            let room_list = state.room_list_notification();
            state.notify(room_list).await;

            Ok(Some(Response::CreateRoomResponse(room_id)))
        }

        Request::JoinRoom(room_id) => {
            // leave room if we are already in one
            if peer.room_id.is_some() {
                match leave_room(state.clone(), peer).await {
                    Ok(success) => {
                        // if we couldn't leave our room, we can't join a new room
                        if !success {
                            return Ok(Some(Response::Success(false)));
                        }
                    }

                    // unrecoverable
                    Err(e) => return Err(e),
                }
            }

            match join_room(state, peer, *room_id).await {
                Ok(success) => Ok(Some(Response::Success(success))),

                // unrecoverable
                Err(e) => return Err(e),
            }
        }

        Request::LeaveRoom => match leave_room(state, peer).await {
            Ok(success) => Ok(Some(Response::Success(success))),
            Err(e) => Err(e),
        },

        Request::QueuePush(track) => {
            let mut state = state.lock().await;
            let Some(room) = peer.room_id.map(|i| state.rooms.get_mut(&i).unwrap()) else { return Ok(Some(Response::Success(false))) };

            // just play it now
            if room.playing.is_none() {
                room.playing = Some(track.to_owned());

                room.notify(room.playing_notification()).await;
            } else {
                room.queue.push_back(track.to_owned());
            }

            room.notify(room.queue_notification()).await;

            Ok(Some(Response::Success(true)))
        }

        // shouldn't happen
        Request::Handshake(_) => Ok(None),
        Request::Authenticate { .. } => Ok(None),
    }
}

async fn handle_disconnect(state: Arc<Mutex<Shared>>, peer: &mut Peer) {
    // remove them from the room cleanly and notify others
    // (we don't care what happened at this point, we tried our best^^)
    let _ = leave_room(state.clone(), peer).await;

    // remove name from the map
    let mut state = state.lock().await;
    state.names.remove(&peer.id);

    // remove handle
    state.waiting_peers.remove(&peer.id);
}
