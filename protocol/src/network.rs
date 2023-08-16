use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Decoder, Encoder, Framed, LengthDelimitedCodec};

use crate::Message;

/// A stream and sink of Messages wrapping a stream `S`
/// where `S` is `AsyncRead + AsyncWrite`.
///
/// Messages are serialized with bincode and sent in
/// length-delimited frames.
pub type MessageStream<S> = Framed<S, MessageCodec>;

/// Wraps a stream `S: AsyncRead + AsyncWrite` in a MessageCodec.
pub fn message_stream<S: AsyncRead + AsyncWrite>(stream: S) -> MessageStream<S> {
    Framed::new(stream, MessageCodec::new())
}

#[derive(Debug)]
pub struct MessageCodec {
    inner: LengthDelimitedCodec,
}

/// Wraps a LengthDelimitedCodec and converts between
/// bytes and Messages using bincode.
impl MessageCodec {
    fn new() -> Self {
        Self {
            inner: LengthDelimitedCodec::new(),
        }
    }
}

impl Decoder for MessageCodec {
    type Item = Message;
    type Error = MessageStreamError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.inner
            .decode(src)?
            .map(|b| bincode::deserialize::<Message>(&b))
            .transpose()
            .map_err(MessageStreamError::from)
    }
}

impl Encoder<&Message> for MessageCodec {
    type Error = MessageStreamError;

    fn encode(&mut self, item: &Message, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let bytes_vec = bincode::serialize(item)?;
        let bytes = Bytes::from(bytes_vec);
        self.inner.encode(bytes, dst)?;
        Ok(())
    }
}

#[derive(Debug, Error, Clone)]
pub enum MessageStreamError {
    #[error(transparent)]
    Bincode(#[from] Arc<bincode::Error>),

    #[error(transparent)]
    Io(#[from] Arc<std::io::Error>),
}

impl From<bincode::Error> for MessageStreamError {
    fn from(value: bincode::Error) -> Self {
        Self::Bincode(Arc::new(value))
    }
}

impl From<std::io::Error> for MessageStreamError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(Arc::new(value))
    }
}
