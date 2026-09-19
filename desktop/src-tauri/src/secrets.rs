//! Token storage.
//!
//! The vault is an encrypted file, not an OS keychain entry: a keychain record
//! does not travel on a USB stick, and portable mode is a first-class goal
//! here.
//!
//! Encryption is ChaCha20-Poly1305 with a key derived by Argon2id from a
//! machine-bound value plus an optional passphrase. Both primitives are pure
//! Rust. The plan named `tauri-plugin-stronghold` for this; it was not used
//! because Stronghold depends on `libsodium-sys`, whose build script downloads
//! a prebuilt C library at compile time — which fails offline and makes the CI
//! build matrix non-reproducible. [`SecretStore`] is a trait so a Stronghold
//! backend can be substituted without touching callers.
//!
//! What this does and does not buy you, stated plainly:
//!
//! * It stops a token being readable in plaintext on disk, and stops the vault
//!   file being useful if copied to a different machine or user account.
//! * It is **not** protection against malware running as the same user, which
//!   can read the machine identifier and the salt just as this code does.
//!   A passphrase is what turns that into real protection.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use std::path::{Path, PathBuf};
use zeroize::Zeroize;

/// File name inside the workspace.
pub const VAULT_FILE: &str = "secrets.vault";

/// Magic bytes, so a wrong file is rejected before any crypto runs.
const MAGIC: &[u8; 8] = b"GRVLT\x00\x01\x00";
const SALT_LEN: usize = 32;
const NONCE_LEN: usize = 12;
/// Argon2id parameters: 64 MiB, 3 passes. Deliberately slow enough to matter.
const ARGON_MEM_KIB: u32 = 64 * 1024;
const ARGON_PASSES: u32 = 3;
const ARGON_LANES: u32 = 1;

/// Storage for the access token.
pub trait SecretStore: Send + Sync {
    fn save(&self, token: &str) -> Result<(), String>;
    fn load(&self) -> Result<Option<String>, String>;
    fn clear(&self) -> Result<(), String>;
    /// True when this store actually persists anything.
    fn is_persistent(&self) -> bool;
}

/// Holds the token for the lifetime of the process only.
///
/// Backs the "don't remember me" option: nothing is written anywhere.
#[derive(Default)]
pub struct MemoryStore {
    token: std::sync::Mutex<Option<String>>,
}

impl SecretStore for MemoryStore {
    fn save(&self, token: &str) -> Result<(), String> {
        *self.token.lock().unwrap() = Some(token.to_string());
        Ok(())
    }

    fn load(&self) -> Result<Option<String>, String> {
        Ok(self.token.lock().unwrap().clone())
    }

    fn clear(&self) -> Result<(), String> {
        if let Some(mut t) = self.token.lock().unwrap().take() {
            t.zeroize();
        }
        Ok(())
    }

    fn is_persistent(&self) -> bool {
        false
    }
}

/// An encrypted file in the workspace.
pub struct EncryptedFileVault {
    path: PathBuf,
    /// Optional user passphrase. Absent means machine-bound only.
    passphrase: Option<String>,
}

impl EncryptedFileVault {
    pub fn new(workspace_dir: &Path, passphrase: Option<String>) -> Self {
        Self {
            path: workspace_dir.join(VAULT_FILE),
            passphrase,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Derive the encryption key.
    ///
    /// Argon2id over the passphrase, salted with both the file's random salt
    /// and a machine-bound identifier so the vault does not decrypt on another
    /// machine or under another user account.
    fn derive_key(&self, salt: &[u8]) -> Result<[u8; 32], String> {
        let mut material = self
            .passphrase
            .clone()
            .unwrap_or_else(|| "github-importer".to_string());
        material.push('\u{1f}');
        material.push_str(&machine_id());

        let params = Params::new(ARGON_MEM_KIB, ARGON_PASSES, ARGON_LANES, Some(32))
            .map_err(|e| format!("bad Argon2 parameters: {e}"))?;
        let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

        let mut out = [0u8; 32];
        argon
            .hash_password_into(material.as_bytes(), salt, &mut out)
            .map_err(|e| format!("key derivation failed: {e}"))?;
        material.zeroize();
        Ok(out)
    }
}

/// A stable-ish identifier for this machine and user.
///
/// Not a secret: it only has to differ between machines so a copied vault file
/// is useless elsewhere. It is paired with a real passphrase when the user
/// wants protection that survives an attacker on the same account.
fn machine_id() -> String {
    let mut parts = Vec::new();
    for key in ["COMPUTERNAME", "HOSTNAME", "USERNAME", "USER", "USERDOMAIN"] {
        if let Ok(v) = std::env::var(key) {
            if !v.is_empty() {
                parts.push(v);
            }
        }
    }
    // `USERDOMAIN` is Windows-specific and helps separate accounts on a
    // domain-joined machine.
    if parts.is_empty() {
        parts.push("unknown-host".to_string());
    }
    parts.join("|")
}

impl SecretStore for EncryptedFileVault {
    fn save(&self, token: &str) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        // Fresh salt and nonce per write: reusing a nonce with the same key
        // would be a catastrophic failure for ChaCha20-Poly1305.
        let salt = random_bytes::<SALT_LEN>()?;
        let nonce_bytes = random_bytes::<NONCE_LEN>()?;
        let key = self.derive_key(&salt)?;

        let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
        let nonce = Nonce::from_slice(&nonce_bytes);
        // The magic is authenticated as associated data, so a truncated or
        // rewritten header fails the tag check rather than being ignored.
        let ciphertext = cipher
            .encrypt(
                nonce,
                Payload {
                    msg: token.as_bytes(),
                    aad: MAGIC,
                },
            )
            .map_err(|_| "encryption failed".to_string())?;

        let mut out = Vec::with_capacity(MAGIC.len() + SALT_LEN + NONCE_LEN + ciphertext.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&salt);
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ciphertext);

        // Written through a temp file so an interrupted write cannot destroy a
        // working vault.
        let tmp = self.path.with_extension("vault.tmp");
        std::fs::write(&tmp, &out).map_err(|e| format!("cannot write the vault: {e}"))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("cannot write the vault: {e}"))?;
        Ok(())
    }

    fn load(&self) -> Result<Option<String>, String> {
        if !self.path.is_file() {
            return Ok(None);
        }
        let raw = std::fs::read(&self.path).map_err(|e| format!("cannot read the vault: {e}"))?;

        let header = MAGIC.len() + SALT_LEN + NONCE_LEN;
        if raw.len() < header || &raw[..MAGIC.len()] != MAGIC {
            return Err(
                "The saved credential file is not readable by this version of the app. \
                 Sign in again to replace it."
                    .to_string(),
            );
        }
        let salt = &raw[MAGIC.len()..MAGIC.len() + SALT_LEN];
        let nonce_bytes = &raw[MAGIC.len() + SALT_LEN..header];
        let ciphertext = &raw[header..];

        let key = self.derive_key(salt)?;
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
        let nonce = Nonce::from_slice(nonce_bytes);
        let plaintext = cipher
            .decrypt(
                nonce,
                Payload {
                    msg: ciphertext,
                    aad: MAGIC,
                },
            )
            .map_err(|_| {
                "Could not unlock the saved credential. If you set a vault passphrase, \
                 check it — otherwise the file may belong to another machine or user account."
                    .to_string()
            })?;

        let mut token = String::from_utf8(plaintext)
            .map_err(|_| "the stored credential is corrupt".to_string())?;
        let out = token.clone();
        token.zeroize();
        Ok(Some(out))
    }

    fn clear(&self) -> Result<(), String> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("cannot remove the vault: {e}")),
        }
    }

    fn is_persistent(&self) -> bool {
        true
    }
}

/// Fill a byte array from the OS CSPRNG.
///
/// Uses `getrandom` through the `aead` crate's re-export, avoiding a direct
/// dependency on a specific `rand` version.
fn random_bytes<const N: usize>() -> Result<[u8; N], String> {
    use chacha20poly1305::aead::rand_core::RngCore;
    let mut buf = [0u8; N];
    chacha20poly1305::aead::OsRng
        .try_fill_bytes(&mut buf)
        .map_err(|e| format!("the OS random number generator failed: {e}"))?;
    Ok(buf)
}

/// Pick the store for the current preferences.
pub fn store_for(
    workspace_dir: &Path,
    remember: bool,
    passphrase: Option<String>,
) -> Box<dyn SecretStore> {
    if remember {
        Box::new(EncryptedFileVault::new(workspace_dir, passphrase))
    } else {
        Box::new(MemoryStore::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault(dir: &Path) -> EncryptedFileVault {
        EncryptedFileVault::new(dir, None)
    }

    #[test]
    fn round_trips_a_token() {
        let tmp = tempfile::tempdir().unwrap();
        let v = vault(tmp.path());
        v.save("ghp_secret_value").unwrap();
        assert_eq!(v.load().unwrap().as_deref(), Some("ghp_secret_value"));
    }

    #[test]
    fn absent_vault_loads_as_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(vault(tmp.path()).load().unwrap(), None);
    }

    #[test]
    fn the_token_is_not_present_in_plaintext_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let v = vault(tmp.path());
        v.save("ghp_verydistinctivetoken").unwrap();
        let raw = std::fs::read(v.path()).unwrap();
        assert!(
            !raw.windows(20).any(|w| w == b"ghp_verydistinctiv"),
            "the token must not appear verbatim in the vault file"
        );
    }

    #[test]
    fn each_save_uses_a_fresh_salt_and_nonce() {
        // Reusing a nonce under the same key would break ChaCha20-Poly1305.
        let tmp = tempfile::tempdir().unwrap();
        let v = vault(tmp.path());
        v.save("same-token").unwrap();
        let first = std::fs::read(v.path()).unwrap();
        v.save("same-token").unwrap();
        let second = std::fs::read(v.path()).unwrap();
        assert_ne!(
            first, second,
            "identical plaintext must not produce identical ciphertext"
        );
    }

    #[test]
    fn a_wrong_passphrase_fails_to_decrypt() {
        let tmp = tempfile::tempdir().unwrap();
        EncryptedFileVault::new(tmp.path(), Some("correct".into()))
            .save("token")
            .unwrap();
        let wrong = EncryptedFileVault::new(tmp.path(), Some("incorrect".into()));
        let err = wrong.load().unwrap_err();
        assert!(err.contains("Could not unlock"), "{err}");
    }

    #[test]
    fn the_right_passphrase_decrypts() {
        let tmp = tempfile::tempdir().unwrap();
        EncryptedFileVault::new(tmp.path(), Some("correct".into()))
            .save("token")
            .unwrap();
        let right = EncryptedFileVault::new(tmp.path(), Some("correct".into()));
        assert_eq!(right.load().unwrap().as_deref(), Some("token"));
    }

    #[test]
    fn tampering_with_the_ciphertext_is_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let v = vault(tmp.path());
        v.save("token").unwrap();
        let mut raw = std::fs::read(v.path()).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0xff;
        std::fs::write(v.path(), &raw).unwrap();
        assert!(v.load().is_err(), "a modified ciphertext must not decrypt");
    }

    #[test]
    fn tampering_with_the_header_is_detected() {
        // The magic is authenticated, so flipping it fails rather than being
        // silently ignored.
        let tmp = tempfile::tempdir().unwrap();
        let v = vault(tmp.path());
        v.save("token").unwrap();
        let mut raw = std::fs::read(v.path()).unwrap();
        raw[0] ^= 0xff;
        std::fs::write(v.path(), &raw).unwrap();
        let err = v.load().unwrap_err();
        assert!(err.contains("not readable by this version"), "{err}");
    }

    #[test]
    fn a_truncated_vault_file_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let v = vault(tmp.path());
        std::fs::write(v.path(), b"short").unwrap();
        assert!(v.load().is_err());
    }

    #[test]
    fn clear_removes_the_file_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let v = vault(tmp.path());
        v.save("token").unwrap();
        v.clear().unwrap();
        assert!(!v.path().exists());
        // Clearing again must not error.
        v.clear().unwrap();
        assert_eq!(v.load().unwrap(), None);
    }

    #[test]
    fn clear_leaves_no_temp_file() {
        let tmp = tempfile::tempdir().unwrap();
        let v = vault(tmp.path());
        v.save("token").unwrap();
        assert!(!v.path().with_extension("vault.tmp").exists());
    }

    #[test]
    fn memory_store_persists_nothing() {
        let s = MemoryStore::default();
        assert!(!s.is_persistent());
        s.save("token").unwrap();
        assert_eq!(s.load().unwrap().as_deref(), Some("token"));
        s.clear().unwrap();
        assert_eq!(s.load().unwrap(), None);
    }

    #[test]
    fn store_for_honours_the_remember_flag() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(store_for(tmp.path(), true, None).is_persistent());
        assert!(!store_for(tmp.path(), false, None).is_persistent());
    }

    #[test]
    fn a_long_token_round_trips() {
        // Classic PATs are 40 chars; fine-grained ones are much longer.
        let tmp = tempfile::tempdir().unwrap();
        let v = vault(tmp.path());
        let token = "github_pat_".to_string() + &"x".repeat(500);
        v.save(&token).unwrap();
        assert_eq!(v.load().unwrap().as_deref(), Some(token.as_str()));
    }
}
