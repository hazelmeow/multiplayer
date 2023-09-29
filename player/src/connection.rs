use std::{
    collections::{HashMap, VecDeque},
    error::Error,
    sync::Arc,
    time::Duration,
};

use futures::{SinkExt, StreamExt};
use tokio::{
    net::TcpStream,
    sync::{mpsc, oneshot, RwLock},
    time::timeout,
};

use protocol::network::{message_stream, MessageStream};
use protocol::{
    Message, Notification, PlaybackCommand, PlaybackState, Request, Response, RoomListing,
    RoomOptions, TrackRequest,
};

use crate::{
    audio::{AudioCommand, AudioStatusRx, AudioThread, AudioThreadHandle},
    gui::{connection_window::ServerStatus, ui_update, UIUpdateEvent},
    preferences::Server,
    transmit::{TransmitCommand, TransmitThread},
    AudioStatus, ConnectionState, RoomState, State,
};

#[derive(Debug)]
pub enum NetworkCommand {
    /// Send a request over the network and receive a response
    Request(Request, oneshot::Sender<Response>),

    /// Send a message over the network
    Plain(Message),

    /// Receive a message as if it was from the server
    FakeReceive(Message),
}

// !Clone even though it should be probably since it's a handle??
// ideally we only want it in one place so we can drop it nicely
pub struct ConnectionActorHandle {
    // need references to these in other places
    pub audio: AudioThreadHandle,

    tx: mpsc::UnboundedSender<NetworkCommand>,
}

impl ConnectionActorHandle {
    pub async fn exited(&self) {
        self.tx.closed().await
    }

    pub fn fake_receive(&mut self, msg: Message) -> Result<(), Box<dyn Error>> {
        self.tx.send(NetworkCommand::FakeReceive(msg))?;
        Ok(())
    }

    pub fn send(&mut self, msg: Message) -> Result<(), Box<dyn Error>> {
        self.tx.send(NetworkCommand::Plain(msg))?;
        Ok(())
    }

    // keep this one private for sure though
    async fn send_request(&self, data: Request) -> Result<Response, Box<dyn Error>> {
        let (send, recv) = oneshot::channel::<Response>();

        // TODO: timeout
        self.tx.send(NetworkCommand::Request(data, send))?;

        let duration = std::time::Duration::from_millis(1000);
        if let Ok(result) = timeout(duration, recv).await {
            Ok(result?)
        } else {
            Err("request timed out".into())
        }
    }

    pub async fn handshake(&self) -> Result<String, Box<dyn Error>> {
        let r = self.send_request(Request::Handshake("meow".into())).await?;
        match r {
            Response::Handshake(s) => Ok(s),
            _ => Err("got the wrong response".into()),
        }
    }
    pub async fn authenticate(&self, id: String, name: String) -> Result<bool, Box<dyn Error>> {
        let r = self
            .send_request(Request::Authenticate { id, name })
            .await?;
        match r {
            Response::Success(s) => Ok(s),
            _ => Err("got the wrong response".into()),
        }
    }
    pub async fn join_room(&self, room_id: u32) -> Result<bool, Box<dyn Error>> {
        let r = self.send_request(Request::JoinRoom(room_id)).await?;
        match r {
            Response::Success(s) => Ok(s),
            _ => Err("got the wrong response".into()),
        }
    }
    pub async fn leave_room(&self) -> Result<bool, Box<dyn Error>> {
        let r = self.send_request(Request::LeaveRoom).await?;
        match r {
            Response::Success(s) => Ok(s),
            _ => Err("got the wrong response".into()),
        }
    }
    pub async fn create_room(&self, options: RoomOptions) -> Result<u32, Box<dyn Error>> {
        let r = self.send_request(Request::CreateRoom(options)).await?;
        match r {
            Response::CreateRoomResponse(id) => Ok(id),
            _ => Err("got the wrong response".into()),
        }
    }
    pub async fn queue_push(&self, track: TrackRequest) -> Result<bool, Box<dyn Error>> {
        let r = self.send_request(Request::QueuePush(track)).await?;
        match r {
            Response::Success(s) => Ok(s),
            _ => Err("got the wrong response".into()),
        }
    }
}

pub struct ConnectionActor {
    stream: MessageStream<TcpStream>,
    next_id: u32,
    pending: HashMap<u32, oneshot::Sender<Response>>,

    state: Arc<RwLock<State>>,

    audio: AudioThreadHandle,
    audio_status_rx: AudioStatusRx,

    tx: mpsc::UnboundedSender<NetworkCommand>,
    rx: mpsc::UnboundedReceiver<NetworkCommand>,
}

impl ConnectionActor {
    pub async fn spawn(
        state: Arc<RwLock<State>>,
        server: Server,
    ) -> Result<ConnectionActorHandle, Box<dyn Error>> {
        let (tx, rx) = mpsc::unbounded_channel();

        let (audio_tx, audio_rx) = mpsc::unbounded_channel::<AudioCommand>();
        let (audio_status_tx, audio_status_rx) = mpsc::unbounded_channel::<AudioStatus>();

        let audio = AudioThread::spawn(audio_tx, audio_rx, &audio_status_tx);

        let state2 = state.clone();
        let audio2 = audio.clone();
        let tx2 = tx.clone();

        tokio::spawn(async move {
            let mut t = ConnectionActor::new(state2, server, audio2, audio_status_rx, tx2, rx)
                .await
                // TODO
                .expect("failed to authenticate or something");
            t.run().await;
        });

        let handle = ConnectionActorHandle { audio, tx };

        // do required connection stuff first

        // handshake
        let hsr = handle.handshake().await?;
        if hsr != "nyaa" {
            return Err("invalid handshake".into());
        }

        {
            let s = state.read().await;
            println!("connecting as {:?}", s.my_id);

            // handshake okay, send authentication
            let result = handle
                .authenticate(s.my_id.clone(), s.preferences.name.clone())
                .await?;
            if !result {
                return Err("failed to authenticate somehow".into());
            }
        }

        Ok(handle)
    }

    pub async fn new(
        state: Arc<RwLock<State>>,
        server: Server,
        audio: AudioThreadHandle,
        audio_status_rx: AudioStatusRx,
        tx: mpsc::UnboundedSender<NetworkCommand>,
        rx: mpsc::UnboundedReceiver<NetworkCommand>,
    ) -> Result<Self, tokio::io::Error> {
        let tcp_stream = TcpStream::connect(&server.addr).await?;
        let stream = message_stream(tcp_stream);

        state.write().await.connection = Some(ConnectionState { server, room: None });

        Ok(ConnectionActor {
            state,

            stream,
            next_id: 0,
            pending: HashMap::new(),

            audio,
            audio_status_rx,

            tx,
            rx,
        })
    }

    async fn run(&mut self) {
        let mut heartbeat_interval = tokio::time::interval(tokio::time::Duration::from_secs(60));

        loop {
            tokio::select! {
                // received a command with a message to send to the server
                result = self.rx.recv() => {
                    match result {
                        Some(command) => match command {
                            NetworkCommand::Request(data, sender) => {
                                let id = self.next_id;
                                self.next_id += 1;

                                self.pending.insert(id, sender);

                                self.stream.send(&Message::Request { request_id: id, data }).await.unwrap();
                            }

                            NetworkCommand::Plain(message) => {
                                self.stream.send(&message).await.unwrap();
                            }

                            NetworkCommand::FakeReceive(message) => {
                                self.handle_message(message).await;
                            }
                        }
                        // channel closed or all senders have been dropped
                        None => {
                            println!("connection rx closed, shutting down connection");
                            break;
                        }
                    }
                }

                // received a tcp message
                result = self.stream.next() => {
                    match result {
                        Some(r) => match r {
                            Ok(msg) => self.handle_message(msg).await,
                            Err(e) => {
                                println!("tcp stream error: {:?}", e);
                            }
                        },

                        // stream disconnected
                        None => {
                            println!("stream disconnected");
                            break;
                        },
                    }
                }

                // received a status from audio
                Some(msg) = self.audio_status_rx.recv() => {
                    match msg {
                        AudioStatus::Elapsed(elapsed) => {
                            let s = self.state.read().await;
                            let total = s.current_track().expect("weird state").metadata.duration;
                            ui_update!(UIUpdateEvent::SetTime(elapsed as f32 / 1000.0, total));
                        }
                        AudioStatus::Buffering(is_buffering) => {
                            let mut s = self.state.write().await;
                            s.connection.as_mut().unwrap().room.as_mut().unwrap().buffering = is_buffering;

                            ui_update!(UIUpdateEvent::Status);
                        }
                        AudioStatus::Finished => {
                            ui_update!(UIUpdateEvent::SetTime(0.0, 0.0));

                            ui_update!(UIUpdateEvent::Status);
                        }
                        AudioStatus::Visualizer(bars) => {
                            ui_update!(UIUpdateEvent::Visualizer(bars))
                        }
                        AudioStatus::Buffer(val) => {
                            ui_update!(UIUpdateEvent::Buffer(val))
                        }
                    }
                },

                // time to send a heartbeat
                _ = heartbeat_interval.tick() => {
                    self.stream.send(&Message::Heartbeat).await.unwrap();
                },
            }
        }

        // actor is exiting (stream closed or application disconnected it)
        // shutdown other actors
        // we could also drop their handles everywhere which drops the sender
        //     and then the receiver returns None and the actor should exit itself?
        //     and it could clean up there?
        self.audio.send(AudioCommand::Shutdown).unwrap();

        // TODO: lets try just dropping the handle?
        // we could get it from the state and send it a shutdown tho
        // self.transmit.send(TransmitCommand::Shutdown).unwrap();
    }

    async fn handle_message(&mut self, message: Message) {
        match message {
            Message::Notification(notification) => self.handle_notification(notification).await,

            Message::Response { request_id, data } => {
                let callback = self.pending.remove(&request_id);
                match callback {
                    Some(sender) => sender
                        .send(data)
                        .expect("response callback channel failed?"),
                    None => println!("got a response with an unknown request_id?"),
                }
            }

            Message::PlaybackCommand(command) => match command {
                PlaybackCommand::Stop | PlaybackCommand::Next | PlaybackCommand::Prev => {
                    let state = self.state.write().await;
                    let room = state.connection.as_ref().unwrap().room.as_ref().unwrap();

                    if let Some(transmit) = &room.transmit_thread {
                        let _ = transmit.send(TransmitCommand::Stop);
                    }

                    let _ = self.audio.send(AudioCommand::Clear);
                }
                PlaybackCommand::Play => {}
                PlaybackCommand::Pause => {}

                PlaybackCommand::SeekTo(secs) => {
                    let state = self.state.write().await;
                    let room = state.connection.as_ref().unwrap().room.as_ref().unwrap();

                    if let Some(transmit) = &room.transmit_thread {
                        let _ = transmit.send(TransmitCommand::SeekTo(secs));
                    }

                    let _ = self.audio.send(AudioCommand::Clear);
                    let _ = self.audio.send(AudioCommand::WaitForBuffer);
                }
            },

            Message::AudioData(data) => {
                self.audio.send(AudioCommand::AudioData(data)).unwrap();
            }

            // should not be receiving these
            Message::QueryRoomList => println!("unexpected message: {:?}", message),
            Message::Text(_) => println!("unexpected message: {:?}", message),
            Message::Request { .. } => println!("unexpected message: {:?}", message),
            Message::Heartbeat => println!("unexpected message: {:?}", message),
        }
    }

    async fn handle_notification(&mut self, notification: Notification) {
        let mut state = self.state.write().await;

        // TODO: this is a hack lol
        let my_id = state.my_id.clone();
        let key = state.key.clone();

        let mut conn = state.connection.as_mut().unwrap();

        match notification {
            Notification::Queue {
                maybe_queue,
                maybe_current_track,
                maybe_playback_state,
            } => {
                let mut room = conn
                    .room
                    .as_mut()
                    .expect("got notification but we arent in a room o_O"); // lol

                if let Some(queue) = maybe_queue {
                    room.queue = queue;
                }
                if let Some(current_track) = maybe_current_track {
                    room.current_track = current_track;
                }
                if let Some(playback_state) = maybe_playback_state {
                    room.playback_state = playback_state;
                }

                let should_transmit = {
                    if room.playback_state == PlaybackState::Stopped {
                        None
                    } else if let Some(current_track) = room.current_track() {
                        if current_track.owner == my_id {
                            Some((current_track.id, current_track.path.clone()))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                };

                if let Some(transmit) = &room.transmit_thread {
                    // we're transmitting something, check if we should keep/replace it
                    if let Some((track_id, track_path)) = should_transmit {
                        if transmit.track_id != track_id {
                            // we were transmitting the wrong thing, replace the transmitter
                            let path = key.decrypt_path(track_path).unwrap();
                            room.transmit_thread = Some(TransmitThread::spawn(
                                track_id,
                                path.into(),
                                self.tx.clone(),
                                self.audio.clone(),
                            ));
                        }
                    } else {
                        // we're not supposed to be transmitting
                        // drop the thread
                        room.transmit_thread = None;
                    }
                } else {
                    // no transmit thread active, maybe make one
                    if let Some((track_id, track_path)) = should_transmit {
                        let path = key.decrypt_path(track_path).unwrap();
                        room.transmit_thread = Some(TransmitThread::spawn(
                            track_id,
                            path.into(),
                            self.tx.clone(),
                            self.audio.clone(),
                        ));
                    }
                }

                if let Some(transmit) = &room.transmit_thread {
                    let _ = transmit.send(TransmitCommand::PauseState(
                        room.playback_state == PlaybackState::Paused,
                    ));
                }

                self.audio
                    .send(AudioCommand::PlaybackState(room.playback_state))
                    .unwrap();

                ui_update!(UIUpdateEvent::QueueChanged);
                ui_update!(UIUpdateEvent::Status);
            }
            Notification::ConnectedUsers(list) => {
                let mut room = conn
                    .room
                    .as_mut()
                    .expect("got notification but we arent in a room o_O");
                room.connected_users = list;

                ui_update!(UIUpdateEvent::UserListChanged);
            }
            Notification::Room(maybe_room) => {
                match maybe_room {
                    Some(room) => {
                        conn.room = Some(RoomState {
                            id: room.id,
                            name: room.name,
                            queue: VecDeque::new(),
                            current_track: 0,
                            playback_state: PlaybackState::Stopped,
                            connected_users: HashMap::new(),
                            buffering: false,
                            transmit_thread: None,
                        })
                    }
                    None => {
                        conn.room = None;
                    }
                }

                ui_update!(UIUpdateEvent::RoomChanged);
                ui_update!(UIUpdateEvent::QueueChanged);
                ui_update!(UIUpdateEvent::UserListChanged);
                ui_update!(UIUpdateEvent::Status);
            }
            Notification::RoomList(list) => {
                // TODO: we should probably do the server status state differently
                let s: ServerStatus = crate::gui::connection_window::ServerStatus {
                    inner: conn.server.clone(),
                    rooms: Some(list),
                    tried: true,
                };
                ui_update!(UIUpdateEvent::UpdateConnectionTreePartial(s));
            }
        }
    }
}

pub async fn query_server(server: Server) -> ServerStatus {
    if let Ok(rooms) = query_room_list(&server.addr).await {
        ServerStatus {
            inner: server,
            rooms: Some(rooms),
            tried: true,
        }
    } else {
        ServerStatus {
            inner: server,
            rooms: None,
            tried: true,
        }
    }
}

const QUERY_TCP_TIMEOUT: Duration = Duration::from_secs(1);
const QUERY_PROTOCOL_TIMEOUT: Duration = Duration::from_secs(1);

pub async fn query_room_list(addr: &str) -> Result<Vec<RoomListing>, Box<dyn Error + Send + Sync>> {
    let tcp_stream = timeout(QUERY_TCP_TIMEOUT, TcpStream::connect(addr)).await??;
    let mut stream = message_stream(tcp_stream);

    // send a handshake
    stream
        .send(&Message::Request {
            request_id: 88888888,
            data: Request::Handshake("meow".into()),
        })
        .await?;

    // wait for a handshake response to send a room list query,
    // then wait for the room list or time out if the server does not comply
    while let Ok(Some(Ok(message))) = timeout(QUERY_PROTOCOL_TIMEOUT, stream.next()).await {
        match message {
            Message::Response {
                request_id,
                data: Response::Handshake(hsr),
            } => {
                if request_id != 88888888 || hsr != "nyaa" {
                    return Err("invalid handshake".into());
                }

                stream.send(&Message::QueryRoomList).await.unwrap();
            }
            Message::Notification(Notification::RoomList(room_list)) => {
                // we got what we were looking for, break
                return Ok(room_list);
            }
            _ => {}
        }
    }

    println!("timed out querying {}", addr);

    Err("timed out".into())
}
