//! Keyslots: each wraps the root key under its own key-encryption key (KEK).
//! - `keyfile`: the KEK is 32 random bytes in the key file, `apz-vault-keyfile/v1 <base64>`.
//! - `passphrase`: the KEK is Argon2id of the passphrase, with the cost stored in the slot.
//! - `recovery`: the KEK is HKDF of a 32-byte code shown once, as 52 base32 characters.

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use zeroize::Zeroizing;

use super::crypto::{self, Key, Opaque};
use super::disk;
use super::model::{self, Kdf, Slot, Wrap};
use super::VaultError;

pub const KEYFILE_TAG: &str = "apz-vault-keyfile/v1";
pub const MIN_PASSPHRASE_CHARS: usize = 12;
const MAX_PASSPHRASE_CHARS: usize = 1024;
const SLOT_LABEL: &str = "apz-vault/v1/slot";

/// The key file's KEK. Refuses a missing, non-regular, group- or world-readable file.
pub fn keyfile_read(path: &Path) -> Result<Key, VaultError> {
    let file = File::open(path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            VaultError::KeyFile(format!("the key file {} does not exist", path.display()))
        } else {
            VaultError::KeyFile(format!("{}: {err}", path.display()))
        }
    })?;
    let meta = file.metadata().map_err(|err| VaultError::KeyFile(format!("{}: {err}", path.display())))?;
    if !meta.is_file() {
        return Err(VaultError::KeyFile(format!("{} is not a regular file", path.display())));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(VaultError::KeyFile(format!("{} is readable by other users (mode {mode:o}); it must be 0600", path.display())));
        }
    }
    let text = Zeroizing::new(fs::read_to_string(path).map_err(|err| VaultError::KeyFile(format!("{}: {err}", path.display())))?);
    let mut parts = text.split_whitespace();
    let (Some(tag), Some(encoded), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(VaultError::KeyFile(format!("{} is not a vault key file", path.display())));
    };
    if tag != KEYFILE_TAG {
        return Err(VaultError::KeyFile(format!("{} is not a vault key file", path.display())));
    }
    let bytes = Zeroizing::new(STANDARD.decode(encoded).map_err(|_| VaultError::KeyFile(format!("{} holds no key", path.display())))?);
    crypto::key_from(&bytes).ok_or_else(|| VaultError::KeyFile(format!("{} holds no 32-byte key", path.display())))
}

fn keyfile_line(kek: &[u8; 32]) -> String {
    format!("{KEYFILE_TAG} {}\n", STANDARD.encode(kek))
}

/// Creates a new key file with a fresh KEK: `O_EXCL`, mode 0600, synced. Fails when it exists.
pub fn keyfile_create(path: &Path) -> Result<Key, VaultError> {
    // A missing config directory is made private; an existing one keeps its mode (others use it).
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty() && !dir.exists()) {
        apz_os::private::create_dir(dir).map_err(|err| VaultError::KeyFile(format!("{}: {err}", dir.display())))?;
    }
    let kek = crypto::random_key().map_err(VaultError::Storage)?;
    let line = Zeroizing::new(keyfile_line(&kek));
    let mut file = match disk::create_private(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(VaultError::Conflict(format!("{} already exists; move it aside first", path.display())));
        }
        Err(err) => return Err(VaultError::KeyFile(format!("{}: {err}", path.display()))),
    };
    let written = file.write_all(line.as_bytes()).and_then(|()| file.sync_all());
    drop(file);
    if let Err(err) = written {
        let _ = fs::remove_file(path);
        return Err(VaultError::KeyFile(format!("{}: {err}", path.display())));
    }
    if let Some(dir) = path.parent() {
        let _ = disk::sync_dir(dir);
    }
    Ok(kek)
}

/// Replaces the key file's contents with `kek`, atomically.
pub fn keyfile_replace(path: &Path, kek: &[u8; 32]) -> Result<(), VaultError> {
    let line = Zeroizing::new(keyfile_line(kek));
    disk::write_atomic(path, line.as_bytes()).map_err(|err| VaultError::KeyFile(format!("{}: {err}", path.display())))
}

/// Overwrites and removes a key file whose slot was deleted: its key must not survive.
pub fn keyfile_shred(path: &Path) {
    if let Ok(meta) = fs::metadata(path)
        && let Ok(mut file) = fs::OpenOptions::new().write(true).open(path)
    {
        let zeros = vec![0u8; meta.len() as usize];
        let _ = file.write_all(&zeros).and_then(|()| file.sync_all());
    }
    let _ = fs::remove_file(path);
}

/// Wraps the root key under `kek` for slot `slot_id`.
pub fn wrap_root(kek: &[u8; 32], vault_id: &str, slot_id: &str, root: &[u8; 32]) -> Result<Wrap, VaultError> {
    let aad = crypto::aad(SLOT_LABEL, &[vault_id, slot_id]);
    let (nonce, ct) = crypto::seal(kek, &aad, root).map_err(VaultError::Storage)?;
    Ok(Wrap { nonce: model::enc(&nonce), ct: model::enc(&ct) })
}

/// Unwraps a slot's root key with `kek`. Fails when the KEK is wrong or the slot was changed.
pub fn unwrap_root(kek: &[u8; 32], vault_id: &str, slot: &Slot) -> Result<Key, Opaque> {
    let aad = crypto::aad(SLOT_LABEL, &[vault_id, &slot.id]);
    let nonce = model::dec(&slot.wrap.nonce).map_err(|_| Opaque)?;
    let ct = model::dec(&slot.wrap.ct).map_err(|_| Opaque)?;
    let root = crypto::open(kek, &nonce, &aad, &ct)?;
    crypto::key_from(&root).ok_or(Opaque)
}

/// A new passphrase slot's KDF: a fresh salt and the current cost.
pub fn new_kdf() -> Result<Kdf, VaultError> {
    let salt = crypto::random_vec(16).map_err(VaultError::Storage)?;
    Ok(Kdf {
        alg: "argon2id".into(),
        salt: model::enc(&salt),
        memory_kib: crypto::KDF_MEMORY_KIB,
        passes: crypto::KDF_PASSES,
        lanes: crypto::KDF_LANES,
    })
}

/// Refuses a stored KDF whose cost is outside the range this build accepts.
pub fn check_kdf(kdf: &Kdf) -> Result<(), VaultError> {
    let ok = kdf.alg == "argon2id"
        && (8 * 1024..=1024 * 1024).contains(&kdf.memory_kib)
        && (1..=10).contains(&kdf.passes)
        && (1..=8).contains(&kdf.lanes);
    if ok { Ok(()) } else { Err(VaultError::Corrupt("a passphrase slot has an unsupported cost".into())) }
}

/// The KEK of a passphrase under a slot's KDF.
pub fn kek_from_passphrase(passphrase: &str, kdf: &Kdf) -> Result<Key, VaultError> {
    check_kdf(kdf)?;
    let salt = model::dec(&kdf.salt).map_err(VaultError::Corrupt)?;
    crypto::argon2id(passphrase.as_bytes(), &salt, kdf.memory_kib, kdf.passes, kdf.lanes).map_err(VaultError::Storage)
}

/// The KEK of a recovery code.
pub fn kek_from_recovery(code: &[u8; 32], vault_id: &str) -> Key {
    crypto::subkey(code, vault_id, crypto::RECOVERY_INFO)
}

/// A passphrase is at least 12 characters.
pub fn check_passphrase(passphrase: &str) -> Result<(), VaultError> {
    let chars = passphrase.chars().count();
    if chars < MIN_PASSPHRASE_CHARS {
        return Err(VaultError::Invalid(format!("the passphrase must be at least {MIN_PASSPHRASE_CHARS} characters")));
    }
    if chars > MAX_PASSPHRASE_CHARS {
        return Err(VaultError::Invalid("the passphrase is too long".into()));
    }
    Ok(())
}

/// A new recovery code: 32 random bytes, 52 base32 characters in groups of four.
pub fn new_recovery_code() -> Result<(Key, Zeroizing<String>), VaultError> {
    let code = crypto::random_key().map_err(VaultError::Storage)?;
    let text = crypto::base32(code.as_ref());
    let groups: Vec<&str> = text.as_bytes().chunks(4).map(|c| std::str::from_utf8(c).unwrap_or("")).collect();
    Ok((code, Zeroizing::new(groups.join("-"))))
}

/// The bytes of a recovery code as typed: dashes and spaces ignored, case ignored.
pub fn parse_recovery_code(text: &str) -> Option<Key> {
    let cleaned: String = text.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
    if cleaned.len() != 52 {
        return None;
    }
    let bytes = Zeroizing::new(crypto::unbase32(&cleaned)?);
    crypto::key_from(&bytes[..32])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recovery_code_round_trips_through_its_text() {
        let (code, text) = new_recovery_code().expect("made");
        assert_eq!(text.replace('-', "").len(), 52);
        assert_eq!(text.split('-').count(), 13);
        let parsed = parse_recovery_code(&text.to_lowercase()).expect("parsed");
        assert_eq!(parsed.as_ref(), code.as_ref());
        assert!(parse_recovery_code("ABCD").is_none());
    }

    #[test]
    fn passphrases_shorter_than_twelve_characters_are_refused() {
        assert!(check_passphrase("short pass").is_err());
        assert!(check_passphrase("twelve chars!").is_ok());
    }
}
