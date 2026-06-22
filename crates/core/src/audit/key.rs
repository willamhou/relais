//! Gateway signing key + opaque credential-reference store (C3).
//!
//! The key lives under `dir/keys/relais.{key,pub}` via signet's `fs_ops` (so the
//! on-disk format and pubkey string match what verifiers expect). Loading or
//! generating the key does **not** trust it for verification — trust is an explicit,
//! out-of-band step (C6 `trusted_keys.json`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use signet_core::identity::fs_ops::{generate_and_save, load_signing_key};
use signet_core::SignetError;

use super::AuditError;

/// The fixed key name relais uses within the signet dir.
pub const KEY_NAME: &str = "relais";

impl From<SignetError> for AuditError {
    fn from(e: SignetError) -> Self {
        AuditError::Signet(e.to_string())
    }
}

/// The gateway's Ed25519 signing key and its public identity.
pub struct AuditKey {
    signing: SigningKey,
    /// Signet-formatted public key, e.g. `ed25519:<base64>` — matches `signer.pubkey`
    /// in receipts so the C6 trust anchor can compare directly.
    pub pubkey: String,
    pub owner: String,
}

impl AuditKey {
    /// Load `dir/keys/relais.key`, or generate it on first use.
    ///
    /// Generation does not imply trust (NF-4): verification still requires an
    /// out-of-band trust anchor.
    ///
    /// **v1 key-at-rest:** with `passphrase = None` (the default) signet stores the
    /// signing seed **unencrypted** on disk (protected only by file perms). Pass a
    /// passphrase to encrypt it (Argon2id + XChaCha20-Poly1305). Encrypting the
    /// audit log at rest is otherwise a deployment concern (design §4.3.5).
    pub fn load_or_init(
        dir: &Path,
        owner: &str,
        passphrase: Option<&str>,
    ) -> Result<Self, AuditError> {
        match load_signing_key(dir, KEY_NAME, passphrase) {
            Ok(signing) => {
                let pubkey = pubkey_string(&signing);
                Ok(Self {
                    signing,
                    pubkey,
                    owner: owner.to_string(),
                })
            }
            Err(SignetError::KeyNotFound(_)) => {
                generate_and_save(dir, KEY_NAME, Some(owner), passphrase, None)?;
                let signing = load_signing_key(dir, KEY_NAME, passphrase)?;
                let pubkey = pubkey_string(&signing);
                Ok(Self {
                    signing,
                    pubkey,
                    owner: owner.to_string(),
                })
            }
            Err(e) => Err(AuditError::from(e)),
        }
    }

    pub fn signing(&self) -> &SigningKey {
        &self.signing
    }
}

/// What an opaque `credential_ref` resolves to, locally. This map is **never**
/// serialized into a receipt/sidecar and is omitted from exports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredBinding {
    pub site: String,
}

/// Persists `kref_… -> CredBinding` at `dir/credential_refs.json`. Minting is
/// idempotent per binding, so a credential keeps one stable opaque ref.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CredRefStore {
    #[serde(skip)]
    path: PathBuf,
    map: HashMap<String, CredBinding>,
}

impl CredRefStore {
    /// Load the store, or start empty if the file does not exist.
    pub fn load(dir: &Path) -> Result<Self, AuditError> {
        let path = dir.join("credential_refs.json");
        let mut store = if path.exists() {
            let json = std::fs::read_to_string(&path).map_err(|e| AuditError::Io(e.to_string()))?;
            serde_json::from_str::<CredRefStore>(&json)
                .map_err(|e| AuditError::Io(format!("credential_refs.json: {e}")))?
        } else {
            CredRefStore::default()
        };
        store.path = path;
        Ok(store)
    }

    /// Return the existing opaque ref for `binding`, or mint, persist and return a
    /// new one. Idempotent per binding.
    pub fn mint(&mut self, binding: CredBinding) -> Result<String, AuditError> {
        if let Some((kref, _)) = self.map.iter().find(|(_, b)| **b == binding) {
            return Ok(kref.clone());
        }
        let kref = new_ref();
        self.map.insert(kref.clone(), binding);
        self.persist()?;
        Ok(kref)
    }

    fn persist(&self) -> Result<(), AuditError> {
        if self.path.as_os_str().is_empty() {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AuditError::Io(e.to_string()))?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| AuditError::Io(e.to_string()))?;
        std::fs::write(&self.path, json).map_err(|e| AuditError::Io(e.to_string()))?;
        // ref → vault-binding map: restrict to the owner (0600), since default umask
        // would commonly leave it world-readable.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| AuditError::Io(e.to_string()))?;
        }
        Ok(())
    }
}

/// Derive the receipt-form public key `ed25519:<STANDARD base64>` **directly from
/// the loaded signing key** (not from the `.pub` file, which signet does not
/// validate against the secret key). This is byte-identical to signet's
/// `format_pubkey`, so it matches `signer.pubkey` in receipts and the C6 trust
/// anchor.
fn pubkey_string(signing: &SigningKey) -> String {
    use base64::Engine;
    format!(
        "ed25519:{}",
        base64::engine::general_purpose::STANDARD.encode(signing.verifying_key().as_bytes())
    )
}

/// A random, non-reversible opaque handle: `kref_<16 hex>`.
fn new_ref() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("kref_{}", hex::encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_or_init_creates_then_reuses() {
        let dir = tempfile::tempdir().unwrap();
        let k1 = AuditKey::load_or_init(dir.path(), "acme", None).unwrap();
        assert!(dir.path().join("keys").join("relais.key").exists());
        assert!(dir.path().join("keys").join("relais.pub").exists());
        assert!(k1.pubkey.starts_with("ed25519:"));

        let k2 = AuditKey::load_or_init(dir.path(), "acme", None).unwrap();
        assert_eq!(k1.pubkey, k2.pubkey, "reload must reuse the same key");
        assert_eq!(k1.signing().to_bytes(), k2.signing().to_bytes());
    }

    #[test]
    fn mint_is_idempotent_per_binding_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = CredRefStore::load(dir.path()).unwrap();
        let a1 = store.mint(CredBinding { site: "scs".into() }).unwrap();
        let a2 = store.mint(CredBinding { site: "scs".into() }).unwrap();
        let b = store
            .mint(CredBinding {
                site: "github".into(),
            })
            .unwrap();
        assert_eq!(a1, a2, "same binding → same ref");
        assert_ne!(a1, b, "different binding → different ref");
        assert!(a1.starts_with("kref_"));

        // reload from disk sees the persisted refs
        let reloaded = CredRefStore::load(dir.path()).unwrap();
        assert_eq!(
            reloaded.map.get(&a1),
            Some(&CredBinding { site: "scs".into() })
        );
    }

    #[cfg(unix)]
    #[test]
    fn credential_refs_file_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let mut store = CredRefStore::load(dir.path()).unwrap();
        store.mint(CredBinding { site: "scs".into() }).unwrap();
        let mode = std::fs::metadata(dir.path().join("credential_refs.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "credential_refs.json must be owner-only");
    }

    #[test]
    fn cred_ref_map_not_in_default_serialization_path() {
        // The store serializes the map (for its own file) but the path field is
        // skipped; receipts/sidecars never embed a CredRefStore.
        let store = CredRefStore::default();
        let json = serde_json::to_string(&store).unwrap();
        assert!(!json.contains("path"));
    }
}
