// TODO: heartbeat checks

use std::collections::VecDeque;
use std::error::Error;

use tokio::sync::{mpsc, oneshot};

use protocol::{
    AudioData, Message, Notification, PlaybackCommand, PlaybackState, Request, Response,
    RoomListing, RoomOptions, Track,
};

use crate::connection::{Client, Clients, Connection, Connections};
use crate::{Id, MainCommand};

#[derive(Debug)]
enum RoomCommand {
    Broadcast(Message),
    ListingRequest(oneshot::Sender<RoomListing>),
    TransferClient(Client),
}

#[derive(Debug)]
pub struct RoomHandle {
    tx: mpsc::UnboundedSender<RoomCommand>,
}

impl RoomHandle {
    pub fn broadcast(&self, message: &Message) {
        self.tx
            .send(RoomCommand::Broadcast(message.clone()))
            .expect("RoomHandle.broadcast failed");
    }

    pub async fn get_listing(&self) -> RoomListing {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(RoomCommand::ListingRequest(tx))
            .expect("RoomHandle.tx.send failed");
        rx.await.expect("RoomHandle.get_listing failed")
    }

    pub async fn transfer_client(&self, client: Client) {
        self.tx
            .send(RoomCommand::TransferClient(client))
            .expect("RoomHandle.tx.send failed");
    }
}

#[derive(Debug)]
pub struct RoomActor {
    id: u32,
    rx: mpsc::UnboundedReceiver<RoomCommand>,
    main_tx: mpsc::UnboundedSender<MainCommand>,

    name: String,

    next_track: u32,

    queue: VecDeque<Track>,
    current_track: u32,
    playback_state: PlaybackState,

    clients: Clients,
}

impl RoomActor {
    pub fn spawn(
        id: u32,
        options: RoomOptions,
        main_tx: mpsc::UnboundedSender<MainCommand>,
    ) -> RoomHandle {
        let (tx, rx) = mpsc::unbounded_channel();

        tokio::spawn(async move {
            let mut room = Self::new(id, options, rx, main_tx);
            room.run().await;
        });

        RoomHandle { tx }
    }

    fn new(
        id: u32,
        options: RoomOptions,
        rx: mpsc::UnboundedReceiver<RoomCommand>,
        main_tx: mpsc::UnboundedSender<MainCommand>,
    ) -> Self {
        RoomActor {
            id,
            rx,
            main_tx,

            name: options.name,

            next_track: 0,

            queue: VecDeque::new(),
            current_track: 0,
            playback_state: PlaybackState::Stopped,

            clients: Clients::new(),
        }
    }

    async fn run(&mut self) {
        loop {
            tokio::select! {
                (id, maybe_message) = &mut self.clients.next_message() => {
                    let id = id.clone();

                    match maybe_message {
                        Some(Ok(message)) => {
                            if let Err(e) = self.handle_message(&id, message).await {
                                self.clients.get(&id).unwrap().log_error_boxed(e);
                            }
                        },
                        Some(Err(e)) => {
                            self.clients.get(&id).unwrap().log_error(Box::new(e));
                        }
                        None => {
                            // client disconnected
                            self.remove_client(&id).await;
                            self.main_tx.send(MainCommand::ClientDisconnected).unwrap();
                        }
                    }
                }

                Some(command) = self.rx.recv() => {
                    match command {
                        RoomCommand::Broadcast(message) => self.broadcast(&message).await,
                        RoomCommand::ListingRequest(tx) => {
                            tx.send(self.room_listing()).unwrap();
                        },
                        RoomCommand::TransferClient(mut client) => {
                            // notify new client of current playing track and queue
                            client.notify(Notification::Room(Some(self.room_listing()))).await;
                            client.notify(self.queue_notification(true, true, true)).await;

                            self.clients.insert(client);

                            self.notify(self.connected_users_notification()).await;
                        },
                    }
                }
            }
        }
    }

    async fn remove_client(&mut self, id: &Id) -> Option<Client> {
        let c = self.clients.remove(id);
        self.notify(self.connected_users_notification()).await;
        c
    }

    async fn handle_message(
        &mut self,
        client_id: &String,
        message: Message,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        match &message {
            Message::Request {
                request_id,
                data: request,
            } => {
                let result = self.handle_request(client_id, request).await;

                match result {
                    // send response
                    Ok(response) => {
                        let client = self.clients.get_mut(client_id).unwrap();

                        client
                            .send(&Message::Response {
                                request_id: *request_id,
                                data: response,
                            })
                            .await;

                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            }

            Message::PlaybackCommand(command) => {
                // tell everyone so they can clear their buffers
                self.broadcast(&Message::PlaybackCommand(*command)).await;

                match command {
                    PlaybackCommand::SeekTo(_) => {
                        // this is handled by the transmitter

                        Ok(())
                    }

                    PlaybackCommand::Play => {
                        if self.playback_state != PlaybackState::Playing {
                            self.playback_state = PlaybackState::Playing;
                            self.notify(self.queue_notification(false, false, true))
                                .await;
                        } else {
                            // aleady playing
                            // pretend we stopped playing and started again
                            self.playback_state = PlaybackState::Stopped;
                            self.notify(self.queue_notification(false, false, true))
                                .await;
                            self.playback_state = PlaybackState::Playing;
                            self.notify(self.queue_notification(false, false, true))
                                .await;
                        }

                        Ok(())
                    }
                    PlaybackCommand::Pause => {
                        if self.playback_state == PlaybackState::Playing {
                            self.playback_state = PlaybackState::Paused;
                            self.notify(self.queue_notification(false, false, true))
                                .await;
                        } else if self.playback_state == PlaybackState::Paused {
                            self.playback_state = PlaybackState::Playing;
                            self.notify(self.queue_notification(false, false, true))
                                .await;
                        }

                        Ok(())
                    }
                    PlaybackCommand::Stop => {
                        if self.playback_state != PlaybackState::Stopped {
                            self.playback_state = PlaybackState::Stopped;
                            self.notify(self.queue_notification(false, false, true))
                                .await;
                        }

                        Ok(())
                    }
                    PlaybackCommand::Prev | PlaybackCommand::Next => {
                        let current_idx = self
                            .queue
                            .iter()
                            .enumerate()
                            .find(|(_, track)| track.id == self.current_track)
                            .map(|(idx, _)| idx);

                        if let Some(idx) = current_idx {
                            let new_idx = match command {
                                PlaybackCommand::Prev => {
                                    // overflowing sub so the idx is actually different
                                    // otherwise with saturating_sub we wouldn't stop playback
                                    // because the track would still be found
                                    idx.overflowing_sub(1).0
                                }
                                PlaybackCommand::Next => idx + 1,
                                _ => unreachable!(),
                            };

                            if let Some(new_track) = self.queue.get(new_idx) {
                                self.current_track = new_track.id;
                            } else {
                                self.playback_state = PlaybackState::Stopped;
                            }

                            self.notify(self.queue_notification(false, true, true))
                                .await;
                        }

                        Ok(())
                    }
                }
            }

            Message::AudioData(data) => {
                let client = self.clients.get_mut(client_id).unwrap();

                match data {
                    protocol::AudioData::Frame(_) => {} // dont log
                    _ => {
                        client.log(format!("audio data {:?}", data));
                    }
                }

                // TODO check that the correct person is sending this

                match data {
                    AudioData::Finish => {
                        let current_pos = self
                            .queue
                            .iter()
                            .enumerate()
                            .find(|(_, track)| track.id == self.current_track)
                            .map(|(idx, _)| idx);

                        let Some(current_pos) = current_pos else {
							eprintln!("current track not found in queue");
							return Ok(())
						};

                        let maybe_track = self.queue.get(current_pos + 1);
                        if let Some(t) = maybe_track {
                            self.current_track = t.id;
                        } else {
                            // the track finished and we have nothing else to play
                            // send playing state
                            self.playback_state = PlaybackState::Stopped;
                        }

                        // rebroadcast Finish message
                        self.broadcast_others(client_id, &message).await;

                        self.notify(self.queue_notification(false, true, true))
                            .await;
                    }
                    _ => {
                        // resend other AudioData messages to all other peers
                        self.broadcast_others(client_id, &message).await;
                    }
                }

                Ok(())
            }

            Message::Text(text) => {
                {
                    let client = self.clients.get(client_id).unwrap();
                    client.log(format!("'{}'", text));
                }

                // resend this message to all other peers
                self.broadcast_others(client_id, &message).await;

                Ok(())
            }

            Message::Heartbeat => {
                let client = self.clients.get_mut(client_id).unwrap();
                client.set_heartbeat();
                Ok(())
            }

            // should not be reached
            Message::QueryRoomList => Ok(()),
            Message::Notification(_) => Ok(()),
            Message::Response { .. } => Ok(()),
        }
    }

    async fn handle_request(
        &mut self,
        client_id: &Id,
        request: &Request,
    ) -> Result<Response, Box<dyn Error + Send + Sync>> {
        match request {
            Request::QueuePush(track_request) => {
                if self.queue.is_empty() {
                    self.playback_state = PlaybackState::Playing;
                }

                let track = Track {
                    id: self.next_track_id(),
                    owner: client_id.clone(),

                    path: track_request.path.clone(),
                    metadata: track_request.metadata.clone(),
                };

                self.queue.push_back(track.to_owned());

                // notify of what could've changed
                self.notify(self.queue_notification(true, false, true))
                    .await;

                Ok(Response::Success(true))
            }

            Request::CreateRoom(options) => {
                let (tx, rx) = oneshot::channel();
                self.main_tx
                    .send(MainCommand::CreateRoom {
                        options: options.clone(),
                        tx,
                    })
                    .unwrap();
                Ok(rx.await.unwrap())
            }

            Request::JoinRoom(room_id) => {
                if let Some(client) = self.remove_client(client_id).await {
                    let (tx, rx) = oneshot::channel();

                    self.main_tx
                        .send(MainCommand::TransferClientToRoom {
                            client,
                            room_id: *room_id,
                            fail: tx,
                        })
                        .unwrap();

                    if let Some(client) = rx.await.unwrap() {
                        // we failed, put them back
                        self.clients.insert(client);
                        Ok(Response::Success(false))
                    } else {
                        // they left
                        self.notify(self.connected_users_notification()).await;
                        Ok(Response::Success(true))
                    }
                } else {
                    Ok(Response::Success(false))
                }
            }

            Request::LeaveRoom => {
                if let Some(client) = self.remove_client(client_id).await {
                    self.main_tx
                        .send(MainCommand::TransferClient(client))
                        .unwrap();
                    self.notify(self.connected_users_notification()).await;
                    Ok(Response::Success(true))
                } else {
                    Ok(Response::Success(false))
                }
            }

            // shouldn't happen
            Request::Handshake(_) => Err("unexpected request".into()),
            Request::Authenticate { .. } => Err("unexpected request".into()),
        }
    }

    fn next_track_id(&mut self) -> u32 {
        let track_id = self.next_track;
        self.next_track += 1;
        track_id
    }

    async fn broadcast(&mut self, message: &Message) {
        self.clients.broadcast(message).await;
    }

    async fn notify(&mut self, notification: Notification) {
        self.broadcast(&Message::Notification(notification)).await
    }

    async fn broadcast_others(&mut self, exclude_id: &Id, message: &Message) {
        self.clients.broadcast_others(exclude_id, message).await;
    }

    fn queue_notification(&self, queue: bool, pos: bool, state: bool) -> Notification {
        Notification::Queue {
            maybe_queue: queue.then(|| self.queue.clone()),
            maybe_current_track: pos.then_some(self.current_track),
            maybe_playback_state: state.then_some(self.playback_state),
        }
    }

    fn connected_users_notification(&self) -> Notification {
        Notification::ConnectedUsers(self.clients.names())
    }

    fn room_listing(&self) -> RoomListing {
        RoomListing {
            id: self.id,
            name: self.name.clone(),
            user_names: self.clients.names().keys().cloned().collect(),
        }
    }
}
