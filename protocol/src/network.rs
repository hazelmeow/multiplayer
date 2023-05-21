use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use std::{error::Error, net::SocketAddr};

use crate::Message;

pub struct FrameStream {
    // LengthDelimitedCodec is used to convert a TcpStream of bytes (may be fragmented) into frames
    // which will be serialized/deserialized with bincode
    inner: Framed<TcpStream, LengthDelimitedCodec>,
}

impl FrameStream {
    pub fn new(stream: TcpStream) -> Self {
        FrameStream {
            inner: Framed::new(stream, LengthDelimitedCodec::new()),
        }
    }

    pub fn get_tcp_stream(&self) -> &TcpStream {
        self.inner.get_ref()
    }
    pub fn get_inner(&mut self) -> &mut Framed<TcpStream, LengthDelimitedCodec> {
        &mut self.inner
    }
    pub fn get_addr(&self) -> Result<SocketAddr, std::io::Error> {
        self.inner.get_ref().peer_addr()
    }

    pub async fn send(&mut self, msg: &Message) -> Result<(), Box<dyn Error + Send + Sync>> {
        let serialized = bincode::serialize(msg)?;
        let bytes = bytes::Bytes::from(serialized);
        self.inner.send(bytes).await?;
        Ok(())
    }

    // waits for the next frame and deserializes it
    // returns Ok(Some(Message)) if successful or returns Ok(None) if the socket disconnected
    pub async fn next_frame(&mut self) -> Result<Option<Message>, Box<dyn Error + Send + Sync>> {
        let addr = self.inner.get_ref().peer_addr()?;

        match self.inner.next().await {
            Some(Ok(bytes)) => {
                let msg = bincode::deserialize::<Message>(&bytes)?;
                Ok(Some(msg))
            }
            Some(Err(e)) => {
                println!("* {} - err: {}", addr, e);
                Err(Box::new(e))
            }
            None => {
                println!("* {} - socket disconnected", addr);
                Ok(None)
            }
        }
    }

}
