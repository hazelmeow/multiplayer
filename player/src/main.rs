use futures::{SinkExt, StreamExt};
use protocol::{Message, PlaybackState, PlayingState};
use tokio::net::TcpStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use std::io::{Read, Write};

// use cpal::traits::{DeviceTrait, HostTrait};

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let stream = TcpStream::connect("127.0.0.1:8080").await?;

    let mut frames = Framed::new(stream, LengthDelimitedCodec::new());

    let message = Message::PlaybackState(PlaybackState {
        state: PlayingState::Playing,
    });

    let encoded = bincode::serialize(&message).expect("failed to serialize message");
    let bytes = bytes::Bytes::from(encoded);
    frames.send(bytes).await?;

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
