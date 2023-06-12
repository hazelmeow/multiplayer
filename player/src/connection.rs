use std::{
    collections::{HashMap, VecDeque},
    error::Error,
    sync::Arc,
};

use futures::StreamExt;
use tokio::{
    net::TcpStream,
    sync::{mpsc, oneshot, RwLock},
    task::block_in_place,
    time::timeout,
};

use protocol::{
    network::FrameStream, AudioData, Message, Notification, Request, Response, RoomListing,
    RoomOptions, Track,
};

use crate::{
    audio::{AudioCommand, AudioStatusRx, AudioThread, AudioThreadHandle},
    gui::{UIThreadHandle, UIUpdateEvent},
    transmit::{TransmitCommand, TransmitThread, TransmitThreadHandle},
    AudioStatus, ConnectionState, RoomState, State,
};

// ew
#[derive(Debug)]
pub enum Command {
    Request(Request, oneshot::Sender<Response>),
    Plain(Message),
}

// !Clone even though it should be probably since it's a handle??
// ideally we only want it in one place so we can drop it nicely
pub struct ConnectionActorHandle {
    // need references to these in other places
    pub transmit: TransmitThreadHandle,
    pub audio: AudioThreadHandle,

    tx: mpsc::UnboundedSender<Command>,
    shutdown_tx: oneshot::Sender<()>, // TODO do we actually shut down properly????
}

impl ConnectionActorHandle {
    pub async fn exited(&self) {
        self.tx.closed().await
    }

    // maybe we don't make this pub and make methods?? or maybe that's unnecessary
    fn send(&mut self, msg: Message) -> Result<(), Box<dyn Error>> {
        let _ = self.tx.send(Command::Plain(msg))?;
        Ok(())
    }

    pub fn refresh_room_list(&mut self) -> Result<(), Box<dyn Error>> {
        self.send(Message::RefreshRoomList)
    }

    // keep this one private for sure though
    async fn send_request(&self, data: Request) -> Result<Response, Box<dyn Error>> {
        let (send, recv) = oneshot::channel::<Response>();

        // TODO: timeout
        let _ = self.tx.send(Command::Request(data, send))?;

        let duration = std::time::Duration::from_millis(1000);
        if let Ok(result) = timeout(duration, recv).await {
            Ok(result?)
        } else {
            return Err("request timed out".into());
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
    pub async fn queue_push(&self, track: Track) -> Result<bool, Box<dyn Error>> {
        let r = self.send_request(Request::QueuePush(track)).await?;
        match r {
            Response::Success(s) => Ok(s),
            _ => Err("got the wrong response".into()),
        }
    }
}

pub struct ConnectionActor {
    stream: FrameStream,
    next_id: u32,
    pending: HashMap<u32, oneshot::Sender<Response>>,

    state: Arc<RwLock<State>>,

    // we only need to use this once, when we first connect
    // if we see the start message -> we are in sync
    // if we see a frame without start message -> we need to catch up first
    is_synced: bool,

    ui: UIThreadHandle,

    transmit: TransmitThreadHandle,
    audio: AudioThreadHandle,
    audio_data_rx: mpsc::UnboundedReceiver<AudioData>,
    audio_status_rx: AudioStatusRx,

    rx: mpsc::UnboundedReceiver<Command>,
    shutdown: oneshot::Receiver<()>,
}

impl ConnectionActor {
    pub async fn spawn(
        state: Arc<RwLock<State>>,
        ui: UIThreadHandle,
        addr: String,
    ) -> Result<ConnectionActorHandle, Box<dyn Error>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let (audio_tx, audio_rx) = mpsc::unbounded_channel::<AudioCommand>();
        let (audio_status_tx, audio_status_rx) = mpsc::unbounded_channel::<AudioStatus>();

        let (audio_data_tx, audio_data_rx) = mpsc::unbounded_channel::<AudioData>();

        let (transmit_tx, transmit_rx) = mpsc::unbounded_channel::<TransmitCommand>();
        let transmit = TransmitThread::spawn(transmit_tx, transmit_rx, audio_data_tx, &audio_tx);

        let audio = AudioThread::spawn(audio_tx, audio_rx, &audio_status_tx);

        let state2 = state.clone();
        let transmit2 = transmit.clone();
        let audio2 = audio.clone();

        tokio::spawn(async move {
            let mut t = ConnectionActor::new(
                state2,
                addr,
                ui,
                transmit2,
                audio2,
                audio_data_rx,
                audio_status_rx,
                rx,
                shutdown_rx,
            )
            .await
            // TODO
            .expect("failed to authenticate or something");
            t.run().await;
        });

        let handle = ConnectionActorHandle {
            transmit,
            audio,
            tx,
            shutdown_tx,
        };

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
                .authenticate(s.my_id.clone(), s.my_id.clone().repeat(5))
                .await?;
            if !result {
                return Err("failed to authenticate somehow".into());
            }
        }

        Ok(handle)
    }

    pub async fn new(
        state: Arc<RwLock<State>>,
        addr: String,
        ui: UIThreadHandle,
        transmit: TransmitThreadHandle,
        audio: AudioThreadHandle,
        audio_data_rx: mpsc::UnboundedReceiver<AudioData>,
        audio_status_rx: AudioStatusRx,
        rx: mpsc::UnboundedReceiver<Command>,
        shutdown_rx: oneshot::Receiver<()>,
    ) -> Result<Self, tokio::io::Error> {
        let tcp_stream = TcpStream::connect(addr.clone()).await?;
        let stream = FrameStream::new(tcp_stream);

        state.write().await.connection = Some(ConnectionState {
            addr,
            room: None,
            buffering: false,
        });

        Ok(ConnectionActor {
            state,

            stream,
            next_id: 0,
            pending: HashMap::new(),

            is_synced: false,

            ui,

            transmit,
            audio,
            audio_data_rx,
            audio_status_rx,

            rx,
            shutdown: shutdown_rx,
        })
    }

    async fn run(&mut self) {
        loop {
            tokio::select! {
                // received a command with a message to send to the server
                Some(command) = self.rx.recv() => {
                    match command {
                        Command::Request(data, sender) => {
                            let id = self.next_id;
                            self.next_id += 1;

                            self.pending.insert(id, sender);

                            self.stream.send(&Message::Request { request_id: id, data }).await.unwrap();
                        }

                        Command::Plain(message) => {
                            self.stream.send(&message).await.unwrap();
                        }
                    }
                }

                // received a tcp message
                result = self.stream.get_inner().next() => {
                    match result {
                        Some(r) => match r {
                            Ok(bytes) => match bincode::deserialize::<Message>(&bytes) {
                                Ok(msg) => self.handle_message(msg).await,
                                Err(e) => {
                                    println!("failed to deserialize message: {:?}", e);
                                }
                            },
                            Err(e) => {
                                println!("tcp stream error: {:?}", e);
                            }
                        },

                        // stream disconnected
                        None => {
                            dbg!("stream disconnected");
                            break;
                        },
                    }
                }

                // received audiodata from audio
                Some(data) = self.audio_data_rx.recv() => {
                    self.stream.send(&Message::AudioData(data)).await.unwrap();
                }

                // received a status from audio
                Some(msg) = self.audio_status_rx.recv() => {
                    match msg {
                        AudioStatus::Elapsed(secs) => {
                            let s = self.state.read().await;
                            let conn = s.connection.as_ref().unwrap();
                            let room = conn.room.as_ref().expect("lol");

                            let total = match &room.playing {
                                Some(t) => {
                                    t.metadata.duration
                                },
                                None => 0
                            };
                            self.ui.update(UIUpdateEvent::SetTime(secs, total as usize));
                        }
                        AudioStatus::Buffering(is_buffering) => {
                            let mut s = self.state.write().await;
                            s.connection.as_mut().unwrap().buffering = is_buffering;

                            self.ui.update(UIUpdateEvent::Status);
                        }
                        AudioStatus::Finished => {
                            self.ui.update(UIUpdateEvent::Status);
                        }
                        AudioStatus::Visualizer(bars) => {
                            self.ui.update(UIUpdateEvent::Visualizer(bars))
                        }
                        AudioStatus::Buffer(val) => {
                            self.ui.update(UIUpdateEvent::Buffer(val))
                        }
                    }
                },

                _ = &mut self.shutdown => break,
            }
        }

        // actor is exiting (stream closed or application disconnected it)
        // shutdown other actors
        // we could also drop their handles everywhere which drops the sender
        //     and then the receiver returns None and the actor should exit itself?
        //     and it could clean up there?
        self.transmit.send(TransmitCommand::Shutdown).unwrap();
        self.audio.send(AudioCommand::Shutdown).unwrap();
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

            Message::AudioData(data) => {
                match data {
                    protocol::AudioData::Frame(f) => {
                        if !self.is_synced {
                            // we're late.... try and catch up will you
                            self.is_synced = true;
                            self.audio
                                .send(AudioCommand::StartLate(f.frame as usize))
                                .unwrap();
                        }

                        self.audio
                            .send(AudioCommand::AudioData(AudioData::Frame(f)))
                            .unwrap();
                    }
                    protocol::AudioData::Start => {
                        self.is_synced = true;
                        self.audio.send(AudioCommand::AudioData(data)).unwrap();
                    }
                    _ => {
                        // forward to audio thread
                        self.audio.send(AudioCommand::AudioData(data)).unwrap();
                    }
                }
            }

            // should not be receiving these
            Message::QueryRoomList => println!("unexpected message: {:?}", message),
            Message::RefreshRoomList => println!("unexpected message: {:?}", message),
            Message::Text(_) => println!("unexpected message: {:?}", message),
            Message::Request { .. } => println!("unexpected message: {:?}", message),
        }
    }

    async fn handle_notification(&mut self, notification: Notification) {
        let mut state = self.state.write().await;
        let mut conn = state.connection.as_mut().unwrap();

        match notification {
            Notification::Queue(queue) => {
                println!("got queue: {:?}", queue);

                let mut room = conn.room.as_mut().expect("lol");
                room.queue = queue;

                self.ui.update(UIUpdateEvent::QueueChanged);
                self.ui.update(UIUpdateEvent::Status);
            }
            Notification::Playing(playing) => {
                println!("got playing: {:?}", playing);

                let mut room = conn.room.as_mut().expect("lol");
                room.playing = playing.clone();

                if let Some(t) = playing {
                    if t.owner == state.my_id {
                        let path = state.key.decrypt_path(t.path).unwrap();
                        self.transmit.send(TransmitCommand::Start(path)).unwrap();
                    }
                }

                self.ui.update(UIUpdateEvent::QueueChanged);
                self.ui.update(UIUpdateEvent::Status);
            }
            Notification::ConnectedUsers(list) => {
                let mut room = conn.room.as_mut().expect("lol");
                room.connected_users = list;

                self.ui.update(UIUpdateEvent::UserListChanged);
            }
            Notification::Room(maybe_room) => {
                match maybe_room {
                    Some(room) => {
                        conn.room = Some(RoomState {
                            id: room.id,
                            name: room.name,
                            playing: None,
                            queue: VecDeque::new(),
                            connected_users: HashMap::new(),
                        })
                    }
                    None => {
                        conn.room = None;
                    }
                }

                self.ui.update(UIUpdateEvent::RoomChanged);
                self.ui.update(UIUpdateEvent::QueueChanged);
                self.ui.update(UIUpdateEvent::UserListChanged);
                self.ui.update(UIUpdateEvent::Status);
            }
            Notification::RoomList(list) => {
                let name = conn.addr.split(":").next().unwrap();
                let s = crate::gui::connection_window::Server {
                    name: name.into(),
                    addr: conn.addr.clone(),
                    rooms: Some(list),
                };
                self.ui
                    .update(UIUpdateEvent::UpdateConnectionTreePartial(s));
            }
        }
    }
}

pub async fn query_room_list(addr: &str) -> Result<Vec<RoomListing>, Box<dyn Error + Send + Sync>> {
    let duration = tokio::time::Duration::from_secs(1);

    let tcp_stream = timeout(duration, TcpStream::connect(addr)).await??;
    let mut stream = FrameStream::new(tcp_stream);

    stream
        .send(&Message::Request {
            request_id: 88888888,
            data: Request::Handshake("meow".into()),
        })
        .await?;

    while let Ok(Ok(Some(message))) = timeout(duration, stream.next_frame()).await {
        match message {
            Message::Response { request_id, data } => match data {
                Response::Handshake(hsr) => {
                    if request_id != 88888888 || hsr != "nyaa" {
                        return Err("invalid handshake".into());
                    }

                    stream.send(&Message::QueryRoomList).await.unwrap();
                }
                _ => {}
            },
            Message::Notification(n) => match n {
                Notification::RoomList(room_list) => return Ok(room_list),
                _ => {}
            },
            _ => {}
        }
    }

    println!("timed out querying {}", addr);

    Err("timed out".into())
}
