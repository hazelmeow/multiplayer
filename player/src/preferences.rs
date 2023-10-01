use std::{
    fs::{self, OpenOptions},
    io::{BufReader, Write},
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
};

use platform_dirs::AppDirs;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Server {
    pub name: String,
    pub addr: String,
}

#[inline]
fn _true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreferencesData {
    #[serde(default = "default_volume")]
    pub volume: f32,
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(default = "generate_id")]
    id: String,

    #[serde(default = "_true")]
    pub lyrics_show_warning_arrows: bool,

    #[serde(default)]
    pub display_album_artist: bool,

    #[serde(default = "default_servers")]
    pub servers: Vec<Server>,
}

const fn default_volume() -> f32 {
    0.5
}
fn default_name() -> String {
    "anon".into()
}
fn generate_id() -> String {
    Uuid::new_v4().to_string()
}
fn default_servers() -> Vec<Server> {
    vec![Server {
        name: "local".into(),
        addr: "127.0.0.1:5119".into(),
    }]
}

impl Default for PreferencesData {
    fn default() -> Self {
        Self {
            volume: default_volume(),
            name: default_name(),
            id: generate_id(),
            lyrics_show_warning_arrows: true,
            display_album_artist: false,
            servers: default_servers(),
        }
    }
}

#[derive(Debug)]
pub struct Preferences {
    inner: PreferencesData,

    // lol
    path: PathBuf,
}

impl Deref for Preferences {
    type Target = PreferencesData;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl DerefMut for Preferences {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl Default for Preferences {
    fn default() -> Self {
        Preferences {
            inner: PreferencesData::default(),
            path: PathBuf::from("./preferences.tmp.json"),
        }
    }
}

impl Preferences {
    pub fn id(&self) -> String {
        self.inner.id.clone()
    }
    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }
}

pub fn make_path<P: AsRef<Path>>(filename: P) -> PathBuf {
    let app_dirs = AppDirs::new(Some("multiplayer"), true).unwrap();
    let path = app_dirs.config_dir.join(filename);

    fs::create_dir_all(&app_dirs.config_dir).unwrap();

    path
}

impl Preferences {
    pub async fn load() -> Self {
        let path = make_path("preferences.json");

        if path.exists() {
            let file = OpenOptions::new()
                .read(true)
                .open(&path)
                .expect("failed to open preferences file");
            let reader = BufReader::new(&file);

            match serde_json::from_reader::<_, PreferencesData>(reader) {
                Ok(p) => Preferences { inner: p, path },
                Err(e) => {
                    // TODO: copy the old file to a backup

                    // TODO: tell you somehow
                    println!("failed to read preferences: {:?}", e);
                    Self::initialize(path).await
                }
            }
        } else {
            Self::initialize(path).await
        }
    }

    async fn initialize(path: PathBuf) -> Self {
        let mut p = Preferences {
            inner: PreferencesData::default(),
            path,
        };
        p.save();
        p
    }

    pub fn save(&mut self) {
        let json =
            serde_json::to_string_pretty(&self.inner).expect("failed to serialize preferences");
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&self.path)
            .expect("failed to open preferences file")
            .write_all(json.as_bytes())
            .expect("failed to write preferences file");
    }
}
