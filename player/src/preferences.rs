use platform_dirs::AppDirs;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{BufReader, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Server {
    pub id: Uuid,
    pub name: String,
    pub addr: String,
}

/// Persisted state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub volume: f32,

    pub name: String,

    // TODO: can you break things by setting your id to someone else's?
    // maybe id should be based on your generated private key, w/ a challenge from the server during the handshake
    pub id: Uuid,

    pub lyrics_show_warning_arrows: bool,

    pub display_album_artist: bool,

    pub servers: Vec<Server>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            volume: 0.5,
            name: "anon".into(),
            id: Uuid::new_v4(),
            lyrics_show_warning_arrows: true,
            display_album_artist: false,
            servers: vec![Server {
                id: Uuid::new_v4(),
                name: "local".into(),
                addr: "127.0.0.1:5119".into(),
            }],
        }
    }
}

pub fn make_path<P: AsRef<Path>>(filename: P) -> PathBuf {
    let app_dirs = AppDirs::new(Some("multiplayer"), true).unwrap();
    let path = app_dirs.config_dir.join(filename);

    fs::create_dir_all(&app_dirs.config_dir).unwrap();

    path
}

impl Preferences {
    pub fn load(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();

        if path.exists() {
            let file = OpenOptions::new()
                .read(true)
                .open(path)
                .expect("failed to open preferences file");
            let reader = BufReader::new(&file);

            match serde_json::from_reader::<_, Preferences>(reader) {
                Ok(p) => p,
                Err(e) => {
                    // TODO: copy the old file to a backup

                    // TODO: tell you somehow
                    println!("failed to read preferences: {:?}", e);

                    let prefs = Preferences::default();
                    prefs.save(path);
                    prefs
                }
            }
        } else {
            let prefs = Preferences::default();
            prefs.save(path);
            prefs
        }
    }

    // TODO: better error handling, dont use expect
    pub fn save(&self, path: impl AsRef<Path>) {
        let json = serde_json::to_string_pretty(self).expect("failed to serialize preferences");
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .expect("failed to open preferences file")
            .write_all(json.as_bytes())
            .expect("failed to write preferences file");
    }
}
