//! Keys and signatures: Ed25519 for agents, the hub and the owner's devices.
//!
//! A key is named by its id: the first 16 bytes of the BLAKE3 hash of its public key, as hex. Private keys
//! are 32-byte seeds; Weft never writes one to its own state folder (the hub's keys live in apid's vault and
//! reach `weft serve` in memory; the owner's key lives on the owner's device).

use std::fmt;
use std::path::Path;

use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::hash::{hex, unhex};

/// A public key, as 64 hex digits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PublicKey(pub [u8; 32]);

impl PublicKey {
    /// The key's id: 32 hex digits.
    pub fn id(&self) -> String {
        hex(&blake3::hash(&self.0).as_bytes()[..16])
    }

    pub fn hex(&self) -> String {
        hex(&self.0)
    }

    pub fn verify(&self, message: &[u8], sig: &[u8]) -> Result<(), String> {
        let key = VerifyingKey::from_bytes(&self.0).map_err(|e| format!("bad public key: {e}"))?;
        let sig: [u8; 64] = sig
            .try_into()
            .map_err(|_| "a signature is 64 bytes".to_string())?;
        key.verify(message, &ed25519_dalek::Signature::from_bytes(&sig))
            .map_err(|_| "the signature does not match".to_string())
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", &self.id()[..8])
    }
}

impl TryFrom<String> for PublicKey {
    type Error = String;
    fn try_from(s: String) -> Result<PublicKey, String> {
        let bytes = unhex(&s)?;
        Ok(PublicKey(
            bytes
                .try_into()
                .map_err(|_| "a public key is 32 bytes".to_string())?,
        ))
    }
}

impl From<PublicKey> for String {
    fn from(k: PublicKey) -> String {
        k.hex()
    }
}

/// A signing key.
pub struct KeyPair {
    key: SigningKey,
}

impl KeyPair {
    pub fn generate() -> KeyPair {
        KeyPair {
            key: SigningKey::generate(&mut rand_core::OsRng),
        }
    }

    pub fn from_seed(seed: &[u8]) -> Result<KeyPair, String> {
        let seed: [u8; 32] = seed
            .try_into()
            .map_err(|_| "a key seed is 32 bytes".to_string())?;
        Ok(KeyPair {
            key: SigningKey::from_bytes(&seed),
        })
    }

    /// A seed written as 64 hex digits (what key files and the hub's handover hold).
    pub fn from_hex(s: &str) -> Result<KeyPair, String> {
        KeyPair::from_seed(&unhex(s.trim())?)
    }

    pub fn seed_hex(&self) -> String {
        hex(&self.key.to_bytes())
    }

    pub fn public(&self) -> PublicKey {
        PublicKey(self.key.verifying_key().to_bytes())
    }

    pub fn id(&self) -> String {
        self.public().id()
    }

    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.key.sign(message).to_bytes()
    }

    /// Reads a key file (hex seed). Refuses a file others can read, on Unix.
    pub fn load(path: &Path) -> Result<KeyPair, String> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(path)
                && meta.permissions().mode() & 0o077 != 0
            {
                return Err(format!(
                    "{} is readable by others; chmod 600 it",
                    path.display()
                ));
            }
        }
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        KeyPair::from_hex(&text)
    }

    /// Writes a new key file, 0600.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        use std::io::Write;
        let mut file = opts
            .open(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        file.write_all(format!("{}\n", self.seed_hex()).as_bytes())
            .map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signature_verifies_with_its_key_and_no_other() {
        let a = KeyPair::generate();
        let b = KeyPair::generate();
        let sig = a.sign(b"landed w-1041");
        assert!(a.public().verify(b"landed w-1041", &sig).is_ok());
        assert!(b.public().verify(b"landed w-1041", &sig).is_err());
        assert!(a.public().verify(b"landed w-1042", &sig).is_err());
    }

    #[test]
    fn a_seed_round_trips() {
        let a = KeyPair::generate();
        let b = KeyPair::from_hex(&a.seed_hex()).unwrap();
        assert_eq!(a.public(), b.public());
        assert_eq!(a.id().len(), 32);
    }
}
