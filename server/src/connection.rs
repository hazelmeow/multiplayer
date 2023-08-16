use std::collections::HashMap;
use std::error::Error;
use std::hash::Hash;
use std::net::SocketAddr;
use std::task::Poll;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::future::{join_all, poll_fn};
use futures::task::AtomicWaker;
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;

use protocol::network::{message_stream, MessageStream, MessageStreamError};
use protocol::{Message, Notification};

use crate::Id;

#[async_trait]
pub trait Connections {
    type Key: Hash + PartialEq + Eq + Clone + Send + Sync;
    type Conn: Connection + Send + Sync;

    fn new() -> Self
    where
        Self: Sized;

    fn get_inner(&self) -> &HashMap<Self::Key, Self::Conn>;
    fn get_inner_mut(&mut self) -> &mut HashMap<Self::Key, Self::Conn>;
    fn get_waker(&self) -> &AtomicWaker;

    fn remove(&mut self, k: &Self::Key) -> Option<Self::Conn>;

    fn get(&self, k: &Self::Key) -> Option<&Self::Conn> {
        self.get_inner().get(k)
    }
    fn get_mut(&mut self, k: &Self::Key) -> Option<&mut Self::Conn> {
        self.get_inner_mut().get_mut(k)
    }

    async fn broadcast(&mut self, message: &Message) {
        let futs = self
            .get_inner_mut()
            .values_mut()
            // do some error handling (this comment is so the formatter does it how i want)
            .map(|c| c.send(message));
        join_all(futs).await;
    }
    async fn broadcast_others(&mut self, exclude_id: &Self::Key, message: &Message) {
        let futs = self
            .get_inner_mut()
            .iter_mut()
            .filter(|(id, _)| *id != exclude_id)
            .map(|(_, c)| c.send(message));
        join_all(futs).await;
    }
    async fn send_to(&mut self, key: Self::Key, message: &Message) {
        if let Some(c) = self.get_inner_mut().get_mut(&key) {
            c.send(message).await;
        }
    }

    /// Polls all clients' streams for the next message/error/disconnect.
    /// Returns the first thing received and the id of the client who created it.
    async fn next_message(&mut self) -> (Self::Key, Option<Result<Message, MessageStreamError>>) {
        poll_fn(|cx| {
            // wake up this task when we add/remove clients
            self.get_waker().register(cx.waker());

            // if any streams are ready, return their value
            for (id, client) in self.get_inner_mut().iter_mut() {
                match client.get_mut().poll_next_unpin(cx) {
                    // TODO: fix this clone maybe (make Clients::Key Arc<String>???)
                    Poll::Ready(message) => {
                        return Poll::Ready((id.clone(), message));
                    }
                    Poll::Pending => continue,
                }
            }

            // no streams had anything, return Pending
            // stream.poll_next_unpin will register the context for wakeup for us
            Poll::Pending
        })
        .await
    }
}

#[derive(Debug)]
pub struct UnauthedConnections {
    next_id: usize,

    inner: HashMap<usize, UnauthedConnection>,
    waker: AtomicWaker,
}

impl Connections for UnauthedConnections {
    type Key = usize;
    type Conn = UnauthedConnection;

    fn new() -> Self {
        Self {
            next_id: 0,
            inner: HashMap::new(),
            waker: AtomicWaker::new(),
        }
    }
    fn get_inner(&self) -> &HashMap<Self::Key, Self::Conn> {
        &self.inner
    }
    fn get_inner_mut(&mut self) -> &mut HashMap<Self::Key, Self::Conn> {
        &mut self.inner
    }
    fn get_waker(&self) -> &AtomicWaker {
        &self.waker
    }
    fn remove(&mut self, key: &Self::Key) -> Option<Self::Conn> {
        self.inner.remove(key)
    }
}

impl UnauthedConnections {
    pub fn insert(&mut self, conn: UnauthedConnection) {
        let id = self.next_id;
        self.next_id += 1;
        self.get_inner_mut().insert(id, conn);
    }
}

#[derive(Debug)]
pub struct Clients {
    inner: HashMap<Id, Client>,
    waker: AtomicWaker,
}

impl Connections for Clients {
    type Key = Id;
    type Conn = Client;

    fn new() -> Self {
        Self {
            inner: HashMap::new(),
            waker: AtomicWaker::new(),
        }
    }
    fn get_inner(&self) -> &HashMap<Self::Key, Self::Conn> {
        &self.inner
    }
    fn get_inner_mut(&mut self) -> &mut HashMap<Self::Key, Self::Conn> {
        &mut self.inner
    }
    fn get_waker(&self) -> &AtomicWaker {
        &self.waker
    }
    fn remove(&mut self, key: &Id) -> Option<Client> {
        self.waker.wake();
        self.inner.remove(key)
    }
}

impl Clients {
    pub fn insert(&mut self, client: Client) {
        self.inner.insert(client.id.clone(), client);

        // wake our poll method
        self.waker.wake();
    }

    pub fn names(&self) -> HashMap<Id, String> {
        self.inner
            .iter()
            .map(|(key, client)| (key.clone(), client.name.clone()))
            .collect()
    }
}

#[async_trait]
pub trait Connection {
    fn get_ref(&self) -> &MessageStream<TcpStream>;
    fn get_mut(&mut self) -> &mut MessageStream<TcpStream>;

    fn log(&self, s: String);
    fn log_error(&self, e: impl Error);
    fn log_error_boxed(&self, e: Box<dyn Error>);

    async fn send(&mut self, message: &Message) {
        let inner = self.get_mut();
        if let Err(e) = inner.send(message).await {
            self.log_error(e);
        }
    }
    async fn notify(&mut self, notification: Notification) {
        self.send(&Message::Notification(notification)).await
    }
}

#[derive(Debug)]
pub struct UnauthedConnection {
    stream: MessageStream<TcpStream>,
    addr: SocketAddr,
    last_heartbeat: Instant,
}

impl Connection for UnauthedConnection {
    fn get_ref(&self) -> &MessageStream<TcpStream> {
        &self.stream
    }
    fn get_mut(&mut self) -> &mut MessageStream<TcpStream> {
        &mut self.stream
    }

    fn log(&self, s: String) {
        println!("* {} - {}", self.addr, s);
    }
    fn log_error(&self, e: impl Error) {
        eprintln!("* {} - err: {}", self.addr, e);
    }
    fn log_error_boxed(&self, e: Box<dyn Error>) {
        eprintln!("* {} - err: {}", self.addr, e);
    }
}

impl UnauthedConnection {
    pub fn from_tcp(stream: TcpStream, addr: SocketAddr) -> Self {
        Self {
            stream: message_stream(stream),
            addr,
            last_heartbeat: Instant::now(),
        }
    }

    pub fn authenticate(self, id: Id, name: String) -> Client {
        Client {
            stream: self.stream,
            id,
            name,
            last_heartbeat: Instant::now(),
        }
    }

    pub fn set_heartbeat(&mut self) {
        self.last_heartbeat = Instant::now();
    }
    pub fn since_heartbeat(&self) -> Duration {
        self.last_heartbeat.elapsed()
    }
}

#[derive(Debug)]
pub struct Client {
    stream: MessageStream<TcpStream>,
    pub id: String,
    name: String,
    last_heartbeat: Instant,
}

impl Connection for Client {
    fn get_ref(&self) -> &MessageStream<TcpStream> {
        &self.stream
    }
    fn get_mut(&mut self) -> &mut MessageStream<TcpStream> {
        &mut self.stream
    }

    fn log(&self, s: String) {
        println!("* client {} - {}", self.id, s);
    }
    fn log_error(&self, e: impl Error) {
        eprintln!("* client {} - err: {}", self.id, e);
    }
    fn log_error_boxed(&self, e: Box<dyn Error>) {
        eprintln!("* client {} - err: {}", self.id, e);
    }
}

impl Client {
    pub fn set_heartbeat(&mut self) {
        self.last_heartbeat = Instant::now();
    }
    pub fn since_heartbeat(&self) -> Duration {
        self.last_heartbeat.elapsed()
    }
}
