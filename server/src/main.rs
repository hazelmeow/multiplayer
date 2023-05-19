use futures::{SinkExt, StreamExt};
use protocol::Message;
use tokio::io;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use std::collections::HashMap;
use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;

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
    // map of mpsc channel senders
    peers: HashMap<SocketAddr, Tx>,
}

impl Shared {
    fn new() -> Self {
        Shared {
            peers: HashMap::new(),
        }
    }

    async fn broadcast(&mut self, sender: SocketAddr, message: &Message) {
        for (peer_addr, tx) in self.peers.iter_mut() {
            if *peer_addr != sender {
                if let Err(e) = tx.send(message.clone()) {
                    println!("error broadcasting message: {:?}", e);
                }
            }
        }
    }
}

struct Peer {
    // LengthDelimitedCodec is used to convert a TcpStream of bytes (may be fragmented) into frames
    // which will be deserialized with bincode
    frames: Framed<TcpStream, LengthDelimitedCodec>,

    // mpsc channel used to receive messages from peers
    rx: Rx,
}

impl Peer {
    async fn new(
        state: Arc<Mutex<Shared>>,
        frames: Framed<TcpStream, LengthDelimitedCodec>,
    ) -> io::Result<Peer> {
        // create channel
        let (tx, rx) = mpsc::unbounded_channel();

        // get our SocketAddr and insert the channel sender into the shared state
        let addr = frames.get_ref().peer_addr()?;
        state.lock().await.peers.insert(addr, tx);

        Ok(Peer { frames, rx })
    }
}

async fn handle_connection(
    state: Arc<Mutex<Shared>>,
    stream: TcpStream,
    addr: SocketAddr,
) -> Result<(), Box<dyn Error>> {
    let frames = Framed::new(stream, LengthDelimitedCodec::new());

    let mut peer = Peer::new(state.clone(), frames).await?;

    tokio::select! {
        result = peer.frames.next() => match result {
            Some(Ok(bytes)) => {
                if let Ok(msg) = bincode::deserialize::<Message>(&bytes) {
                    if let Message::Handshake(hs) = msg {
                        println!("* {} - handshake: {:x?} ('{}')", addr, hs.as_bytes(), hs);
                        // pretend it's a version number or something useful
                        if hs == "meow" {
                            // now reply
                            let hsr = bincode::serialize(&Message::Handshake("nyaa".to_string())).unwrap();
                            let bytes = bytes::Bytes::from(hsr);
                            peer.frames.send(bytes).await?;
                        } else {
                            println!("* {} - invalid handshake", addr);
                        }
                        
                    }
                } else {
                    println!("* {} - invalid handshake", addr);
                    // TODO: do something
                }
            },
            Some(Err(e)) => {
                println!("* {} - err: {}", addr, e);
                // TODO: do something
            },
            None => {
                println!("* {} - socket disconnected", addr);
                {
                    let mut state = state.lock().await;
                    state.peers.remove(&addr);
            
                }
                return Ok(())
            }

        }
    }

    // register peer

    // TODO: broadcast connected message
    // {
    // let mut state = state.lock().await;
    // state.broadcast()
    // }

    // process incoming messages until disconnected
    loop {
        tokio::select! {
            // react to messages on mpsc channel
            Some(msg) = peer.rx.recv() => {
                // encode Message with bincode and send throguh TcpStream
                let encoded = bincode::serialize(&msg).expect("failed to serialize message");
                let bytes = bytes::Bytes::from(encoded);
                peer.frames.send(bytes).await?;
            }

            // react to frames from tcp stream
            result = peer.frames.next() => match result {
                // a full frame was received
                Some(Ok(bytes)) => {
                    let mut state = state.lock().await;

                    let msg: Message = bincode::deserialize(&bytes).expect("failed to deserialize message");
                    state.broadcast(addr, &msg).await;
                    println!("* {} - {:?}", addr, &msg);
                }

                Some(Err(e)) => {
                    println!("error occurred while processing message: {:?}", e)
                }

                // socket disconnected
                None => break
            },
        }
    }

    // client was disconnected
    {
        let mut state = state.lock().await;
        state.peers.remove(&addr);

        // TODO: leave message
    }

    Ok(())

}

