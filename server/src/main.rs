use std::collections::HashMap;
use std::error::Error;
use std::time::Duration;

use connection::{
    Client, Clients, Connection, Connections, UnauthedConnection, UnauthedConnections,
};
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use room::{RoomActor, RoomHandle};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

use protocol::{Message, Notification, Request, Response, RoomListing, RoomOptions};

mod connection;
mod room;

// TODO: maybe move to protocol eventually and validate
type Id = String;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let bind_addr = "0.0.0.0:5119".to_string();

    let mut main_actor = MainActor::new(&bind_addr).await?;
    main_actor.run().await;

    Ok(())
}

#[derive(Debug)]
pub enum MainCommand {
    /// RoomActor sends this to transfer a client back to us if they leave the room
    TransferClient(Client),

    /// Transfer a client inside a room to another room
    TransferClientToRoom {
        client: Client,
        room_id: u32,

        fail: oneshot::Sender<Option<Client>>,
    },

    /// RoomActor sends this when a client disconnects to notify users of the room list
    ClientDisconnected,

    // forwarded
    CreateRoom {
        options: RoomOptions,
        tx: oneshot::Sender<Response>,
    },
}

#[derive(Debug)]
struct MainActor {
    tx: mpsc::UnboundedSender<MainCommand>,
    rx: mpsc::UnboundedReceiver<MainCommand>,

    tcp_listener: TcpListener,

    unauthed_connections: UnauthedConnections,
    roomless_clients: Clients,

    next_room: u32,
    rooms: HashMap<u32, RoomHandle>,
}

impl MainActor {
    async fn new(bind_addr: &String) -> Result<Self, std::io::Error> {
        let tcp_listener = TcpListener::bind(bind_addr).await?;
        println!("listening on {}", bind_addr);

        let (tx, rx) = mpsc::unbounded_channel();

        Ok(Self {
            tcp_listener,

            unauthed_connections: UnauthedConnections::new(),
            roomless_clients: Clients::new(),

            next_room: 0,
            rooms: HashMap::new(),

            tx,
            rx,
        })
    }

    async fn run(&mut self) {
        let mut heartbeat_interval = tokio::time::interval(tokio::time::Duration::from_secs(60));

        loop {
            tokio::select! {
                // Accept TCP connections
                Ok((tcp_stream, socket_addr)) = self.tcp_listener.accept() => {
                    let mut conn = UnauthedConnection::from_tcp(tcp_stream, socket_addr);

                    conn.log("accepted connection".to_string());

                    if let Err(e) = handshake(&mut conn).await {
                        conn.log_error_boxed(e);
                        continue;
                    }

                    self.unauthed_connections.insert(conn);
                }

                // Handle messages from handshake but not authenticated connections
                // Can QueryRoomList or Request::Authenticate or Heartbeat maybe

                (conn_id, maybe_message) = &mut self.unauthed_connections.next_message() => {
                    match maybe_message {
                        Some(Ok(Message::QueryRoomList)) => {
                            let n = self.room_list_notification().await;
                            let conn = self.unauthed_connections.get_mut(&conn_id).unwrap();

                            conn.send(&Message::Notification(n)).await;
                        }
                        Some(Ok(Message::Request{ request_id, data: Request::Authenticate {id, name}})) => {
                            let mut conn = self.unauthed_connections.remove(&conn_id).unwrap();
                            conn.log(format!("authenticating as {id}"));

                            // todo: check id format? check if in use? send success message?
                            let _ = conn.send(&Message::Response {
                                request_id,
                                data: Response::Success(true),
                            })
                            .await;

                            let client = conn.authenticate(id, name);
                            self.roomless_clients.insert(client);
                        }
                        Some(Ok(Message::Heartbeat)) => {
                            let c = self.unauthed_connections.get_mut(&conn_id).unwrap();
                            c.set_heartbeat();
                        }
                        Some(Ok(m)) => {
                            let conn = self.unauthed_connections.get(&conn_id).unwrap();
                            conn.log(format!("sent message while not authenticated: {m:?}"));
                        }
                        Some(Err(e)) => {
                            let conn = self.unauthed_connections.get(&conn_id).unwrap();
                            conn.log_error(e);
                        }
                        None => {
                            // connection disconnected
                            let conn = self.unauthed_connections.get(&conn_id).unwrap();
                            conn.log("disconnected".to_string());
                            self.unauthed_connections.remove(&conn_id);
                        }
                    }
                }

                // Handle messages from authenticated clients
                (conn_id, maybe_message) = &mut self.roomless_clients.next_message() => {
                    let conn_id = conn_id.clone();

                    match maybe_message {
                        Some(Ok(message)) => {
                            if let Err(e) = self.handle_message(&conn_id, message).await {
                                let conn = self.roomless_clients.get(&conn_id).unwrap();
                                conn.log_error_boxed(e);
                            }
                        }
                        Some(Err(e)) => {
                            let conn = self.roomless_clients.get(&conn_id).unwrap();
                            conn.log_error(e);
                        }
                        None => {
                            // connection disconnected
                            let conn = self.roomless_clients.get(&conn_id).unwrap();
                            conn.log("disconnected".to_string());
                            self.roomless_clients.remove(&conn_id);
                        }
                    }
                }

                Some(command) = self.rx.recv() => {
                    match command {
                        MainCommand::TransferClient(client) => {
                            self.roomless_clients.insert(client);
                            self.notify_room_list().await;
                        },
                        MainCommand::TransferClientToRoom {client, room_id, fail} => {
                            if let Some(room_handle) = self.rooms.get(&room_id) {
                                room_handle.transfer_client(client).await;
                                self.notify_room_list().await;

                                fail.send(None).unwrap();
                            } else {
                                // room wasn't found, we couldn't transfer them
                                fail.send(Some(client)).unwrap();
                            }

                        }
                        MainCommand::ClientDisconnected => self.notify_room_list().await,
                        MainCommand::CreateRoom { options, tx} => {
                            let response = self.create_room(options);
                            tx.send(response.await).unwrap();
                        },
                    }
                }

                _ = heartbeat_interval.tick() => {
                    let notify = {
                        let should_remove: Vec<Id> = self.roomless_clients
                            .get_inner()
                            .iter()
                            .filter(|(_, c)| {
                                c.since_heartbeat() > Duration::from_secs(120)
                            })
                            .map(|(id, c)| {
                                c.log("heartbeat timed out".into());
                                id.clone()
                            })
                            .collect();

                        for id in should_remove.iter() {
                            self.roomless_clients.remove(id);
                        }
                        !should_remove.is_empty()
                    };
                    let notify2 = {
                        let should_remove: Vec<usize> = self.unauthed_connections
                            .get_inner()
                            .iter()
                            .filter(|(_, c)| {
                                c.since_heartbeat() > Duration::from_secs(120)
                            })
                            .map(|(id, c)| {
                                c.log("heartbeat timed out".into());
                                *id
                            })
                            .collect();

                        for id in should_remove.iter() {
                            self.unauthed_connections.remove(id);
                        }
                        !should_remove.is_empty()
                    };

                    if notify || notify2 {
                        self.notify_room_list().await;
                    }
                }
            }
        }
    }

    async fn handle_message(&mut self, id: &Id, message: Message) -> Result<(), Box<dyn Error>> {
        match message {
            Message::Request {
                request_id,
                data: request,
            } => {
                let result = self.handle_request(id, request, request_id).await;

                match result {
                    // send response
                    Ok(response) => {
                        // it's possible that handle_request removed this client from roomless_clients
                        // (i.e. joining a room) so uhh if you did that then send the response urself
                        if let Some(client) = self.roomless_clients.get_mut(id) {
                            client
                                .send(&Message::Response {
                                    request_id,
                                    data: response,
                                })
                                .await;
                        }

                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            }

            Message::QueryRoomList => {
                let n = self.room_list_notification().await;
                let c = self.roomless_clients.get_mut(id).unwrap();
                c.send(&Message::Notification(n)).await;
                Ok(())
            }

            Message::Heartbeat => {
                let c = self.roomless_clients.get_mut(id).unwrap();
                c.set_heartbeat();
                Ok(())
            }

            // handled by room
            Message::AudioData(_) => Err(format!("unexpected message: {:?}", message).into()),
            Message::Text(_) => Err(format!("unexpected message: {:?}", message).into()),
            Message::Notification(_) => Err(format!("unexpected message: {:?}", message).into()),
            Message::PlaybackCommand(_) => Err(format!("unexpected message: {:?}", message).into()),
            Message::Response { .. } => Err(format!("unexpected message: {:?}", message).into()),
        }
    }

    async fn handle_request(
        &mut self,
        client_id: &Id,
        request: Request,
        request_id: u32,
    ) -> Result<Response, Box<dyn Error>> {
        match request {
            Request::CreateRoom(options) => Ok(self.create_room(options).await),

            Request::JoinRoom(room_id) => {
                let Some(room_handle) = self.rooms.get(&room_id) else { return Ok(Response::Success(false)); };
                let Some(mut client) = self.roomless_clients.remove(client_id) else { return Ok(Response::Success(false)); };

                // TODO technically we shouldn't send this until after the transfer succeeded
                // but we have to send client to the room task for that so it's a little annoying
                let resp = Response::Success(true);
                client
                    .send(&Message::Response {
                        request_id,
                        data: resp,
                    })
                    .await;

                room_handle.transfer_client(client).await;
                self.notify_room_list().await;

                // this doesnt actually do anything cause we probably removed this client from roomless_clients
                // but we have to return something soo
                Ok(Response::Success(true))
            }

            // shouldn't happen
            Request::Handshake(_) => Err("unexpected request".into()),
            Request::Authenticate { .. } => Err("unexpected request".into()),

            Request::LeaveRoom { .. } => Err("unexpected request".into()),
            Request::QueuePush { .. } => Err("unexpected request".into()),
        }
    }

    async fn create_room(&mut self, options: RoomOptions) -> Response {
        let room_id = self.next_room;
        self.next_room += 1;

        let room_handle = RoomActor::spawn(room_id, options, self.tx.clone());
        self.rooms.insert(room_id, room_handle);

        self.notify_room_list().await;

        Response::CreateRoomResponse(room_id)
    }

    async fn notify_all(&mut self, notification: Notification) {
        let m = Message::Notification(notification);

        self.unauthed_connections.broadcast(&m).await;
        self.roomless_clients.broadcast(&m).await;

        for r in self.rooms.values() {
            r.broadcast(&m);
        }
    }

    async fn notify_room_list(&mut self) {
        self.notify_all(self.room_list_notification().await).await
    }

    async fn room_list_notification(&self) -> Notification {
        let futs: FuturesUnordered<_> = self.rooms.values().map(|r| r.get_listing()).collect();
        let rooms: Vec<RoomListing> = futs.collect().await;
        Notification::RoomList(rooms)
    }
}

static HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(500);

async fn handshake(conn: &mut UnauthedConnection) -> Result<(), Box<dyn Error>> {
    let maybe_message = tokio::time::timeout(HANDSHAKE_TIMEOUT, conn.get_mut().next()).await?;

    match maybe_message {
        Some(Ok(Message::Request {
            request_id,
            data: Request::Handshake(hs),
        })) => {
            conn.log("handshake".to_string());
            // pretend it's a version number or something useful
            if hs == "meow" {
                // now reply
                conn.send(&Message::Response {
                    request_id,
                    data: Response::Handshake("nyaa".to_string()),
                })
                .await;

                Ok(())
            } else {
                Err("invalid handshake".into())
            }
        }
        Some(_) => Err("invalid handshake".into()),
        None => Err("disconnected before handshake completed".into()),
    }
}
