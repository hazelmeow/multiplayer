use crate::{
    state::{dispatch_update, StateUpdate},
    transmit::AudioInfoReader,
    State,
};
use futures::future::join_all;
use protocol::TrackRequest;
use std::{fs::read_dir, path::PathBuf, sync::Arc};
use tokio::sync::RwLock;

pub async fn resolve_dnd(state: Arc<RwLock<State>>, data: String) -> (Vec<TrackRequest>, usize) {
    let mut frontier: Vec<PathBuf> = data
        .split('\n')
        .map(|p| PathBuf::from(p.trim().replace("file://", "")))
        .filter(|p| p.exists())
        .collect();

    let mut track_paths: Vec<PathBuf> = vec![];

    while !frontier.is_empty() {
        let path = frontier.pop().unwrap();

        let Ok(metadata) = path.metadata() else {
            continue;
        };

        if metadata.is_symlink() {
            continue;
        } else if metadata.is_dir() {
            let Ok(dir_entries) = read_dir(path) else {
                continue;
            };
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

    dispatch_update!(StateUpdate::IncreaseLoadingCount(num_tracks));

    let tasks: Vec<tokio::task::JoinHandle<Option<TrackRequest>>> = {
        let s = state.read().await;

        track_paths
            .into_iter()
            .map(|p| {
                let file_string = p.as_os_str().to_string_lossy();
                let encrypted_path = s.key.encrypt_path(&file_string.to_string()).unwrap();

                tokio::spawn(async move {
                    match AudioInfoReader::load(&p) {
                        Ok(mut reader) => match reader.read_info() {
                            Ok((_, _, metadata)) => {
                                let track = protocol::TrackRequest {
                                    path: encrypted_path,
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
        .collect::<Vec<TrackRequest>>();

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
