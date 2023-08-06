use std::sync::Arc;
use std::{fs::read_dir, path::PathBuf};

use futures::future::join_all;
use protocol::Track;
use tokio::sync::RwLock;

use crate::{
    gui::{ui_update, UIUpdateEvent},
    transmit::AudioInfoReader,
    State,
};

pub async fn resolve_dnd(state: Arc<RwLock<State>>, data: String) -> (Vec<Track>, usize) {
    let mut frontier: Vec<PathBuf> = data
        .split('\n')
        .map(|p| PathBuf::from(p.trim().replace("file://", "")))
        .filter(|p| p.exists())
        .collect();

    let mut track_paths: Vec<PathBuf> = vec![];

    while !frontier.is_empty() {
        let path = frontier.pop().unwrap();

        let Ok(metadata) = path.metadata() else { continue };

        if metadata.is_symlink() {
            continue;
        } else if metadata.is_dir() {
            let Ok(dir_entries) = read_dir(path) else { continue };
            for entry in dir_entries.flatten() {
                frontier.push(entry.path());
            }
        } else {
            let extension = path.extension().unwrap_or_default().to_str().unwrap();
            if matches!(
                extension,
                "mp3" | "m4a" | "flac" | "wav" | "ogg" | "wma" | "opus"
            ) {
                track_paths.push(path);
            }
        }
    }

    let num_tracks = track_paths.len();

    {
        let mut s = state.write().await;
        s.loading_count += num_tracks;
        ui_update!(UIUpdateEvent::Status);
    }

    let tasks: Vec<tokio::task::JoinHandle<Option<Track>>> = {
        let s = state.read().await;

        track_paths
            .into_iter()
            .map(|p| {
                let file_string = p.as_os_str().to_string_lossy();
                let encrypted_path = s.key.encrypt_path(&file_string.to_string()).unwrap();
                let my_id = s.my_id.clone();

                tokio::spawn(async move {
                    match AudioInfoReader::load(&p) {
                        Ok(mut reader) => match reader.read_info() {
                            Ok((_, _, metadata)) => {
                                let track = protocol::Track {
                                    path: encrypted_path,
                                    owner: my_id,
                                    metadata,
                                };
                                Some(track)
                            }
                            Err(e) => {
                                println!("errored reading metadata for {:?}: {e}", p);
                                None
                            }
                        },
                        Err(e) => {
                            println!("errored loading {:?}: {e}", p);
                            None
                        }
                    }
                })
            })
            .collect()
    };

    // wait for all tasks to finish and filter for successful reads
    let mut tracks = join_all(tasks)
        .await
        .into_iter()
        .filter_map(|t| match t {
            Ok(Some(t)) => Some(t),
            _ => None,
        })
        .collect::<Vec<Track>>();

    // sort tracks by album and track number
    tracks.sort_by_key(|t| {
        let track_no_string = t.metadata.track_no.clone().unwrap_or_default();
        let track_no = track_no_string
            .split('/')
            .next()
            .unwrap()
            .parse::<usize>()
            .unwrap_or_default();
        (t.metadata.album.clone().unwrap_or_default(), track_no)
    });

    (tracks, num_tracks)
}
