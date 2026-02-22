use std::path::Path;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum VaultError {
    #[error("sled error: {0}")]
    Db(#[from] sled::Error),
    #[error("encryption error")]
    Encryption,
    #[error("decryption error")]
    Decryption,
    #[error("invalid utf-8")]
    Utf8(#[from] std::string::FromUtf8Error),
}

pub struct Vault {
    db: sled::Db,
    cipher: Aes256Gcm,
}

impl Vault {
    pub fn open(path: &Path, master_password: &str) -> Result<Self, VaultError> {
        let db = sled::open(path.join("vault.db"))?;
        let key = Sha256::digest(master_password.as_bytes());
        let cipher =
            Aes256Gcm::new_from_slice(&key).map_err(|_| VaultError::Encryption)?;
        Ok(Self { db, cipher })
    }

    pub fn store(&self, site_id: &str, credential: &str) -> Result<(), VaultError> {
        let nonce_bytes: [u8; 12] = rand::random();
        let nonce = Nonce::from_slice(&nonce_bytes);
        let encrypted = self
            .cipher
            .encrypt(nonce, credential.as_bytes())
            .map_err(|_| VaultError::Encryption)?;

        let mut stored = nonce_bytes.to_vec();
        stored.extend(encrypted);
        self.db.insert(site_id.as_bytes(), stored)?;
        self.db.flush()?;
        Ok(())
    }

    pub fn retrieve(&self, site_id: &str) -> Result<Option<String>, VaultError> {
        match self.db.get(site_id.as_bytes())? {
            Some(data) => {
                if data.len() < 12 {
                    return Err(VaultError::Decryption);
                }
                let (nonce_bytes, encrypted) = data.split_at(12);
                let nonce = Nonce::from_slice(nonce_bytes);
                let decrypted = self
                    .cipher
                    .decrypt(nonce, encrypted)
                    .map_err(|_| VaultError::Decryption)?;
                Ok(Some(String::from_utf8(decrypted)?))
            }
            None => Ok(None),
        }
    }

    pub fn delete(&self, site_id: &str) -> Result<(), VaultError> {
        self.db.remove(site_id.as_bytes())?;
        self.db.flush()?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<String>, VaultError> {
        self.db
            .iter()
            .keys()
            .map(|k| {
                k.map_err(VaultError::from)
                    .and_then(|v| String::from_utf8(v.to_vec()).map_err(VaultError::from))
            })
            .collect()
    }
}
