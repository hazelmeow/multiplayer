use futures::StreamExt;
use protocol::network::FrameStream;
use protocol::{AudioFrame, AuthenticateRequest, GetInfo, Message, PlaybackState, PlayingState};
use tokio::time::timeout;
use tokio::{net::TcpStream, sync::mpsc};

mod audio;
use audio::Track;

// use cpal::traits::{DeviceTrait, HostTrait};

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let tcp_stream = TcpStream::connect("127.0.0.1:8080").await?;
    let mut stream = FrameStream::new(tcp_stream);

    /*
    let message = Message::PlaybackState(PlaybackState {
        state: PlayingState::Playing,
    });

    let encoded = bincode::serialize(&message).expect("failed to serialize message");
    let bytes = bytes::Bytes::from(encoded);
    frames.send(bytes).await?; */

    // handshake
    stream
        .send(&Message::Handshake("meow".to_string()))
        .await
        .unwrap();

    let hs_timeout = std::time::Duration::from_millis(1000);
    if let Ok(hsr) = timeout(hs_timeout, stream.get_inner().next()).await {
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

    // handshake okay, send authentication
    let args: Vec<String> = std::env::args().collect();
    let my_id = &args[1];
    println!("connecting as {:?}", my_id);

    stream
        .send(&Message::Authenticate(AuthenticateRequest {
            id: my_id.clone(),
            name: my_id.clone().repeat(5), // TODO temp
        }))
        .await
        .unwrap();

    let (message_tx, mut message_rx) = mpsc::unbounded_channel::<Message>();

    let (audio_tx, mut audio_rx) = std::sync::mpsc::channel::<AudioFrame>();

    // audio player thread
    std::thread::spawn(move || {
        // temp: each track needs its own player somehow
        let mut p = audio::Player::new();

        let mut temp = 0;

        loop {
            if let Ok(frame) = audio_rx.recv() {
                println!("player thread got {:?}", frame);
                p.receive(frame.data);
                temp += 1;
            }

            if temp == 200 {
                p.play();
            }
        }
    });

    if my_id == "1" {
        println!("id is '1', sending track");

        let track = protocol::Track {
            path: "blah".to_string(),
            owner: my_id.clone(),
            queue_position: 0,
        };

        stream.send(&Message::QueuePush(track)).await.unwrap();
        stream
            .send(&Message::GetInfo(GetInfo::QueueList))
            .await
            .unwrap();

        let audio_tx_2 = audio_tx.clone();
        tokio::spawn(async move {
            // these would be the two sides of it
            // todo.. how do we coordinate that?
            let mut t: Track = Track::load("../test.mp3").unwrap();

            dbg!(&t.samples.len());
            for _ in 0..200 {
                let f = t.encode_frame();

                // send to our own audio thread
                audio_tx_2.send(f.clone()).unwrap();

                // send to server
                message_tx.send(Message::AudioFrame(f)).unwrap();
            }

            loop {
                // todo: keep sending frames

                //tx.send(Message::AudioFrame(f)).unwrap();
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        });
    } else {
        println!("id is not '1', not sending anything")
    }

    loop {
        tokio::select! {
            // something to send
            Some(msg) = message_rx.recv() => {
                stream.send(&msg).await.unwrap();
            }

            // tcp message
            result = stream.get_inner().next() => match result {
                Some(Ok(bytes)) => {
                    let msg: Message =
                        bincode::deserialize(&bytes).expect("failed to deserialize message");
                    println!("received message: {:?}", msg);

                    match msg {
                        Message::AudioFrame(f) => {
                            audio_tx.send(f).unwrap();
                        }

                        _ => {}
                    }
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
