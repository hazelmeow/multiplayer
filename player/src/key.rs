use std::fmt::Debug;
use std::path::PathBuf;
use std::{error::Error, fs};

use aes_gcm_siv::{
    aead::{generic_array::GenericArray, Aead, KeyInit, OsRng},
    Aes256GcmSiv, Nonce,
};

use crate::preferences;

#[derive(Clone)]
pub struct Key {
    cipher: Aes256GcmSiv,
}

impl Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Key").finish_non_exhaustive()
    }
}

impl Key {
    pub fn load() -> Result<Self, Box<dyn Error>> {
        let key_path = preferences::make_path("key.key");

        let cipher = Self::load_key(key_path)?;

        Ok(Key { cipher })
    }

    fn load_key(key_path: PathBuf) -> Result<Aes256GcmSiv, Box<dyn Error>> {
        if key_path.exists() {
            let key = fs::read_to_string(key_path)?;
            let key: Vec<u8> = hex::decode(key.as_bytes())?;
            let key = GenericArray::from_slice(key.as_slice());
            Ok(Aes256GcmSiv::new(key))
        } else {
            let key = Aes256GcmSiv::generate_key(&mut OsRng);
            fs::write(key_path, hex::encode(key))?;
            Ok(Aes256GcmSiv::new(&key))
        }
    }

    pub fn encrypt_path(&self, path: &String) -> Result<Vec<u8>, aes_gcm_siv::Error> {
        // we could do something like
        //     let mut nonce = [0u8; 12];
        //     OsRng.fill_bytes(&mut nonce);
        // but then we have to pass the nonce with each message
        // repeating the nonce only reveals message equality which we already do
        //  when we send metadata so it doesn't matter much
        let nonce = Nonce::from_slice(b"12bytestring");

        self.cipher.encrypt(nonce, path.as_bytes())
    }

    pub fn decrypt_path<T: AsRef<[u8]>>(&self, data: T) -> Result<String, aes_gcm_siv::Error> {
        let nonce = Nonce::from_slice(b"12bytestring");

        let decrypted = self
            .cipher
            .decrypt(Nonce::from_slice(nonce), data.as_ref())?;
        Ok(String::from_utf8(decrypted).unwrap())
    }
}
