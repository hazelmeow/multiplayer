use futures::{SinkExt, StreamExt};
use protocol::{GetInfo, Message, PlaybackState, PlayingState};
use tokio::time::timeout;
use tokio::{net::TcpStream, sync::mpsc};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

mod audio;
use audio::Track;

// use cpal::traits::{DeviceTrait, HostTrait};

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let stream = TcpStream::connect("127.0.0.1:8080").await?;

    let mut frames = Framed::new(stream, LengthDelimitedCodec::new());

    /*
    let message = Message::PlaybackState(PlaybackState {
        state: PlayingState::Playing,
    });

    let encoded = bincode::serialize(&message).expect("failed to serialize message");
    let bytes = bytes::Bytes::from(encoded);
    frames.send(bytes).await?; */

    // handshake
    let hs = serialize_message(&Message::Handshake("meow".to_string())).unwrap();
    frames.send(hs).await?;

    let hs_timeout = std::time::Duration::from_millis(1000);
    if let Ok(hsr) = timeout(hs_timeout, frames.next()).await {
        match hsr {
            Some(Ok(r)) => {
                let response: Message = bincode::deserialize(&r).unwrap();
                if let Message::Handshake(r) = response {
                    if r != "nyaa" {
                        return Ok(()); // not okay
                    }
                }
            }
            Some(Err(_)) | None => {
                eprintln!("handshake failed");
                return Ok(());
            }
        }
    } else {
        eprintln!("timed out waiting for handshake");
        return Ok(());
    };

    // handshake okay



    let track = protocol::Track {
        path: "blah".to_string(),
        owner: 0,
        queue_position: 0,
    };

    frames
        .send(serialize_message(&Message::QueuePush(track)).unwrap())
        .await?;
    frames
        .send(serialize_message(&Message::GetInfo(GetInfo::QueueList)).unwrap())
        .await?;

    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    tokio::spawn(async move {
        // these would be the two sides of it
        // todo.. how do we coordinate that?
        let mut t = Track::load("./test.mp3").unwrap();
        let mut p = audio::Player::new();

        dbg!(&t.samples.len());
        for _ in 0..200 {
            let f = t.encode_frame();
            tx.send(Message::AudioFrame(f.clone())).unwrap();
            p.receive(f.data);
        }
        p.debug_export();

        loop {
            //tx.send(Message::AudioFrame(f)).unwrap();
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    });


    loop {
        tokio::select! {
            // something to send
            Some(msg) = rx.recv() => {
                frames.send(serialize_message(&msg).unwrap()).await?;
            }

            // tcp message
            result = frames.next() => match result {
                Some(Ok(bytes)) => {
                    let msg: Message =
                        bincode::deserialize(&bytes).expect("failed to deserialize message");
                    println!("received message: {:?}", msg);
                }

                Some(Err(e)) => {
                    println!("error occurred while processing message: {:?}", e)
                }

                // socket disconnected
                None => break,
            }
        }
    }

    println!("disconnected");

    Ok(())
}

// this thingy should maybeeeee be inside protocol? or something...
// it's not right to put it there but we really need it in both player and server
fn serialize_message(m: &Message) -> Result<bytes::Bytes, bincode::Error> {
    let s = bincode::serialize(m)?;
    Ok(bytes::Bytes::from(s))
}

