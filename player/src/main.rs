use futures::{SinkExt, StreamExt};
use protocol::{Message, PlaybackState, PlayingState};
use tokio::net::TcpStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tokio::time::timeout;

use std::io::{Read, Write};

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
            },
            Some(Err(_)) |
            None => {
                eprintln!("handshake failed");
                return Ok(());
            }
        }
    } else {
        eprintln!("timed out waiting for handshake");
        return Ok(());
    };

    // handshake okay

    loop {
        let result = frames.next().await;

        match result {
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

    println!("disconnected");

    Ok(())

    // let host = cpal::default_host();
    // let device = host
    //     .default_output_device()
    //     .expect("no output device available");
    // let mut configs = device
    //     .supported_output_configs()
    //     .expect("error while querying configs");
    // let config = configs
    //     .next()
    //     .expect("no supported configs")
    //     .with_max_sample_rate();

    // let stream = device.build_output_stream(
    //     &config.into(),
    //     move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
    //         //
    //     },
    //     move |err| {
    //         //
    //     },
    //     None,
    // );

    // // The sound plays in a separate audio thread,
    // // so we need to keep the main thread alive while it's playing.
    // // Press ctrl + C to stop the process once you're done.
    // loop {}
}


fn serialize_message(m: &Message) -> Result<bytes::Bytes, bincode::Error> {
    let s = bincode::serialize(m)?;
    Ok(bytes::Bytes::from(s))
}