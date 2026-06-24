//! Encrypted credential vault.
//!
//! **v1 format** (current): keys are derived from the master password with
//! **Argon2id** (per-vault random salt + pinned params in `kdf.json`), and records
//! are sealed with **XChaCha20-Poly1305** (24-byte random nonce) and **AAD =
//! `site_id`** so a record can't be copied between sites. Each record is prefixed
//! with a **version byte** (`0x01`).
//!
//! **v0 format** (legacy): AES-256-GCM with a raw-`SHA-256(password)` key and a
//! 12-byte nonce, no version byte, no AAD. v0 records remain readable — retrieval
//! dispatches per record (try v1 when the version byte is present and the AEAD tag
//! verifies, else fall back to v0), so a mixed store never locks out and
//! `vault migrate` can upgrade lazily or on demand.
//!
//! **`kdf.json` is essential:** losing it makes existing v1 records permanently
//! undecryptable. Open therefore never regenerates it when v1 records exist (it
//! errors), and the file is written durably (temp + fsync + rename + parent fsync).

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use aes_gcm::aead::{Aead as _, KeyInit as _};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::Payload;
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Per-record version byte for the v1 (Argon2id + XChaCha20-Poly1305) format.
const V1: u8 = 0x01;
const V1_NONCE_LEN: usize = 24;
const V0_NONCE_LEN: usize = 12;

/// Reserved db key recording that this vault was initialised with a `kdf.json`. It
/// starts with a NUL byte so it can't collide with a real (UTF-8, non-NUL) site id,
/// and `list()` skips NUL-prefixed keys. Used to detect a *lost* `kdf.json` (rather
/// than heuristically inspecting records, which can't distinguish a v0 record whose
/// nonce happens to begin with the v1 marker byte).
const KDF_MARKER: &[u8] = b"\x00kdf_v1";

#[derive(Error, Debug)]
pub enum VaultError {
    #[error("sled error: {0}")]
    Db(#[from] sled::Error),
    #[error("io error: {0}")]
    Io(String),
    #[error("kdf error: {0}")]
    Kdf(String),
    #[error("encryption error")]
    Encryption,
    #[error("decryption error")]
    Decryption,
    #[error("invalid utf-8")]
    Utf8(#[from] std::string::FromUtf8Error),
}

/// Persisted KDF parameters (`kdf.json`). The salt is not secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct KdfFile {
    version: u32,
    alg: String,
    salt: Vec<u8>,
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
}

impl KdfFile {
    fn fresh() -> Self {
        let salt: [u8; 16] = rand::random();
        // OWASP Argon2id baseline.
        Self {
            version: 1,
            alg: "argon2id".into(),
            salt: salt.to_vec(),
            m_cost: 19_456,
            t_cost: 2,
            p_cost: 1,
        }
    }

    /// Validate a loaded file before trusting it to derive a key.
    fn validate(&self) -> Result<(), VaultError> {
        if self.version != 1 {
            return Err(VaultError::Kdf(format!(
                "unsupported kdf version {}",
                self.version
            )));
        }
        if self.alg != "argon2id" {
            return Err(VaultError::Kdf(format!(
                "unsupported kdf alg '{}'",
                self.alg
            )));
        }
        if self.salt.len() < 8 {
            return Err(VaultError::Kdf("kdf salt too short".into()));
        }
        // Minimums roughly at the OWASP floor; reject obviously-weak params.
        if self.m_cost < 8 * 1024 || self.t_cost < 1 || self.p_cost < 1 {
            return Err(VaultError::Kdf("kdf params below minimum".into()));
        }
        Ok(())
    }
}

pub struct Vault {
    db: sled::Db,
    /// v1 cipher (new writes + v1 reads).
    v1: XChaCha20Poly1305,
    /// v0 cipher (legacy reads).
    v0: Aes256Gcm,
}

impl Vault {
    pub fn open(path: &Path, master_password: &str) -> Result<Self, VaultError> {
        fs::create_dir_all(path).map_err(|e| VaultError::Io(e.to_string()))?;
        let db = sled::open(path.join("vault.db"))?;

        let kdf = load_kdf(path, &db)?;
        let v1_key = derive_argon2id(master_password, &kdf)?;
        let v1 = XChaCha20Poly1305::new_from_slice(&v1_key).map_err(|_| VaultError::Encryption)?;

        let v0_key = Sha256::digest(master_password.as_bytes());
        let v0 = Aes256Gcm::new_from_slice(&v0_key).map_err(|_| VaultError::Encryption)?;

        Ok(Self { db, v1, v0 })
    }

    pub fn store(&self, site_id: &str, credential: &str) -> Result<(), VaultError> {
        let nonce_bytes: [u8; V1_NONCE_LEN] = rand::random();
        // Bind the ciphertext to the site id (AAD) so a record can't be copied to a
        // different site and decrypted there.
        let ciphertext = self
            .v1
            .encrypt(
                XNonce::from_slice(&nonce_bytes),
                Payload {
                    msg: credential.as_bytes(),
                    aad: site_id.as_bytes(),
                },
            )
            .map_err(|_| VaultError::Encryption)?;

        let mut stored = Vec::with_capacity(1 + V1_NONCE_LEN + ciphertext.len());
        stored.push(V1);
        stored.extend_from_slice(&nonce_bytes);
        stored.extend_from_slice(&ciphertext);

        self.db.insert(site_id.as_bytes(), stored)?;
        self.db.flush()?;
        Ok(())
    }

    pub fn retrieve(&self, site_id: &str) -> Result<Option<String>, VaultError> {
        let data = match self.db.get(site_id.as_bytes())? {
            Some(d) => d,
            None => return Ok(None),
        };

        // v1: version byte present and a valid AEAD seal under this site's AAD. (A v0
        // record that happens to begin with 0x01 fails the v1 tag and falls through.)
        if data.first() == Some(&V1) && data.len() > V1_NONCE_LEN {
            let (nonce, ct) = data[1..].split_at(V1_NONCE_LEN);
            if let Ok(pt) = self.v1.decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: ct,
                    aad: site_id.as_bytes(),
                },
            ) {
                return Ok(Some(String::from_utf8(pt)?));
            }
        }

        // v0 legacy: AES-256-GCM, 12-byte nonce, no version byte, no AAD.
        if data.len() < V0_NONCE_LEN {
            return Err(VaultError::Decryption);
        }
        let (nonce, ct) = data.split_at(V0_NONCE_LEN);
        let pt = self
            .v0
            .decrypt(Nonce::from_slice(nonce), ct)
            .map_err(|_| VaultError::Decryption)?;
        Ok(Some(String::from_utf8(pt)?))
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
            .filter_map(|k| match k {
                // skip reserved (NUL-prefixed) keys like the KDF marker
                Ok(key) if key.first() == Some(&0u8) => None,
                Ok(key) => Some(String::from_utf8(key.to_vec()).map_err(VaultError::from)),
                Err(e) => Some(Err(VaultError::from(e))),
            })
            .collect()
    }

    /// Re-encrypt every record into the v1 format. Idempotent and crash-safe: each
    /// record is rewritten and flushed individually, and a partially-migrated store
    /// stays fully readable (per-record version dispatch). Returns the number of
    /// records (re)written.
    pub fn migrate(&self) -> Result<usize, VaultError> {
        let sites = self.list()?;
        let mut n = 0;
        for site in sites {
            if let Some(cred) = self.retrieve(&site)? {
                self.store(&site, &cred)?;
                n += 1;
            }
        }
        Ok(n)
    }
}

fn derive_argon2id(password: &str, kdf: &KdfFile) -> Result<[u8; 32], VaultError> {
    let params = Params::new(kdf.m_cost, kdf.t_cost, kdf.p_cost, Some(32))
        .map_err(|e| VaultError::Kdf(e.to_string()))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    argon
        .hash_password_into(password.as_bytes(), &kdf.salt, &mut key)
        .map_err(|e| VaultError::Kdf(e.to_string()))?;
    Ok(key)
}

/// Load `kdf.json`, or create it **only when safe**. If the file is missing but the
/// vault was previously initialised with one (the `KDF_MARKER` is present), refuse:
/// regenerating a fresh salt would permanently lock out the existing v1 records.
/// A corrupted/invalid file is an error, never a silent regeneration.
fn load_kdf(dir: &Path, db: &sled::Db) -> Result<KdfFile, VaultError> {
    let path = dir.join("kdf.json");
    if path.exists() {
        let json = fs::read_to_string(&path).map_err(|e| VaultError::Io(e.to_string()))?;
        let kdf: KdfFile =
            serde_json::from_str(&json).map_err(|e| VaultError::Kdf(format!("kdf.json: {e}")))?;
        kdf.validate()?;
        return Ok(kdf);
    }
    if db.contains_key(KDF_MARKER)? {
        return Err(VaultError::Kdf(
            "kdf.json is missing but this vault was initialised with one (it may be lost); \
             refusing to regenerate it, which would permanently lock out existing credentials. \
             Restore kdf.json from backup."
                .into(),
        ));
    }
    // First initialisation (new vault, or a legacy v0-only store being upgraded):
    // create the KDF and record the marker so a future lost kdf.json is detected.
    let kdf = KdfFile::fresh();
    let json = serde_json::to_vec_pretty(&kdf).map_err(|e| VaultError::Kdf(e.to_string()))?;
    write_atomic(&path, &json)?;
    db.insert(KDF_MARKER, &[1u8])?;
    db.flush()?;
    Ok(kdf)
}

/// Atomic, durable write: temp + fsync(file) + rename + fsync(parent dir), so a crash
/// never loses `kdf.json` after `vault.db` has flushed v1 records.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), VaultError> {
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = File::create(&tmp).map_err(|e| VaultError::Io(e.to_string()))?;
        f.write_all(bytes)
            .map_err(|e| VaultError::Io(e.to_string()))?;
        f.sync_all().map_err(|e| VaultError::Io(e.to_string()))?;
    }
    fs::rename(&tmp, path).map_err(|e| VaultError::Io(e.to_string()))?;
    if let Some(parent) = path.parent() {
        // Best-effort directory fsync to make the rename durable (no-op on platforms
        // where opening a directory isn't supported).
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn td() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn write_v0_record(dir: &Path, password: &str, site: &[u8], value: &[u8], nonce0: u8) {
        let key = Sha256::digest(password.as_bytes());
        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let nonce = [nonce0; V0_NONCE_LEN];
        let ct = cipher.encrypt(Nonce::from_slice(&nonce), value).unwrap();
        let mut rec = nonce.to_vec();
        rec.extend_from_slice(&ct);
        let db = sled::open(dir.join("vault.db")).unwrap();
        db.insert(site, rec).unwrap();
        db.flush().unwrap();
    }

    #[test]
    fn v1_round_trip_and_kdf_file() {
        let dir = td();
        let v = Vault::open(dir.path(), "correct horse battery staple").unwrap();
        v.store("github", "ghp_token").unwrap();
        assert_eq!(v.retrieve("github").unwrap().as_deref(), Some("ghp_token"));
        assert!(dir.path().join("kdf.json").exists());
        let raw = v.db.get(b"github").unwrap().unwrap();
        assert_eq!(raw[0], V1);
        assert!(raw.len() > 1 + V1_NONCE_LEN);
    }

    #[test]
    fn wrong_password_fails() {
        let dir = td();
        Vault::open(dir.path(), "pw-one")
            .unwrap()
            .store("s", "secret")
            .unwrap();
        let v2 = Vault::open(dir.path(), "pw-two").unwrap();
        assert!(v2.retrieve("s").is_err());
    }

    #[test]
    fn reads_legacy_v0_record() {
        let dir = td();
        write_v0_record(dir.path(), "legacy-pw", b"oldsite", b"legacy-secret", 7);
        let v = Vault::open(dir.path(), "legacy-pw").unwrap();
        assert_eq!(
            v.retrieve("oldsite").unwrap().as_deref(),
            Some("legacy-secret")
        );
    }

    #[test]
    fn reads_v0_record_whose_first_byte_is_v1_marker() {
        // A v0 record whose 12-byte nonce starts with 0x01 must still decrypt: v1 is
        // attempted, fails the AEAD tag, and falls back to v0.
        let dir = td();
        write_v0_record(dir.path(), "pw", b"site", b"v0value", V1);
        let v = Vault::open(dir.path(), "pw").unwrap();
        assert_eq!(v.retrieve("site").unwrap().as_deref(), Some("v0value"));
    }

    #[test]
    fn migrate_upgrades_and_reopen_reads() {
        let dir = td();
        write_v0_record(dir.path(), "mig-pw", b"site", b"v0val", 3);
        {
            let v = Vault::open(dir.path(), "mig-pw").unwrap();
            assert_eq!(v.migrate().unwrap(), 1);
            assert_eq!(v.db.get(b"site").unwrap().unwrap()[0], V1);
        }
        // reopen with the same password reads the migrated v1 record
        let v2 = Vault::open(dir.path(), "mig-pw").unwrap();
        assert_eq!(v2.retrieve("site").unwrap().as_deref(), Some("v0val"));
    }

    #[test]
    fn missing_kdf_with_v1_records_refuses_to_open() {
        let dir = td();
        {
            let v = Vault::open(dir.path(), "pw").unwrap();
            v.store("s", "secret").unwrap();
        }
        // Simulate a lost kdf.json while v1 records remain.
        fs::remove_file(dir.path().join("kdf.json")).unwrap();
        assert!(
            matches!(Vault::open(dir.path(), "pw"), Err(VaultError::Kdf(_))),
            "expected a kdf error when kdf.json is missing but v1 records exist"
        );
    }

    #[test]
    fn corrupted_kdf_fails_not_regenerates() {
        let dir = td();
        Vault::open(dir.path(), "pw").unwrap(); // creates kdf.json
        fs::write(dir.path().join("kdf.json"), b"{ not json").unwrap();
        assert!(Vault::open(dir.path(), "pw").is_err());
    }

    #[test]
    fn aad_prevents_cross_site_record_copy() {
        let dir = td();
        let v = Vault::open(dir.path(), "pw").unwrap();
        v.store("siteA", "A-secret").unwrap();
        // Copy siteA's raw record to siteB.
        let raw = v.db.get(b"siteA").unwrap().unwrap();
        v.db.insert(b"siteB", raw.to_vec()).unwrap();
        v.db.flush().unwrap();
        // retrieve(siteB) must NOT return A's plaintext: v1 AAD (siteB) mismatches, and
        // the record isn't valid v0 either → decryption error.
        assert!(v.retrieve("siteB").is_err());
    }
}
