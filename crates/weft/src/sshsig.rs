//! SSH signatures (the `SSHSIG` format `ssh-keygen -Y sign` makes), which git and GitHub verify for commits and
//! tags signed with an SSH key. Weft signs exported commits and tags with the hub's Ed25519 signing key.

use base64::Engine;
use sha2::{Digest, Sha512};

use crate::keys::KeyPair;

fn string(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

/// The key in SSH's wire form (`ssh-ed25519` and the 32 bytes).
fn public_blob(key: &KeyPair) -> Vec<u8> {
    let mut b = Vec::new();
    string(&mut b, b"ssh-ed25519");
    string(&mut b, &key.public().0);
    b
}

/// The public key as an `authorized_keys` line: what the owner's git platform account registers.
pub fn public_line(key: &KeyPair, comment: &str) -> String {
    format!(
        "ssh-ed25519 {} {comment}",
        base64::engine::general_purpose::STANDARD.encode(public_blob(key))
    )
}

/// An armored SSH signature of `message` in `namespace` (`git` for commits and tags).
pub fn sign(key: &KeyPair, namespace: &str, message: &[u8]) -> String {
    let digest = Sha512::digest(message);
    let mut signed = Vec::new();
    signed.extend_from_slice(b"SSHSIG");
    string(&mut signed, namespace.as_bytes());
    string(&mut signed, b"");
    string(&mut signed, b"sha512");
    string(&mut signed, &digest);
    let sig = key.sign(&signed);
    let mut sig_blob = Vec::new();
    string(&mut sig_blob, b"ssh-ed25519");
    string(&mut sig_blob, &sig);
    let mut blob = Vec::new();
    blob.extend_from_slice(b"SSHSIG");
    blob.extend_from_slice(&1u32.to_be_bytes());
    string(&mut blob, &public_blob(key));
    string(&mut blob, namespace.as_bytes());
    string(&mut blob, b"");
    string(&mut blob, b"sha512");
    string(&mut blob, &sig_blob);
    let b64 = base64::engine::general_purpose::STANDARD.encode(&blob);
    let mut out = String::from("-----BEGIN SSH SIGNATURE-----\n");
    for chunk in b64.as_bytes().chunks(70) {
        out.push_str(std::str::from_utf8(chunk).unwrap());
        out.push('\n');
    }
    out.push_str("-----END SSH SIGNATURE-----\n");
    out
}

/// Checks an armored signature against `message` and the key it carries; returns that key's line.
pub fn verify(armored: &str, namespace: &str, message: &[u8]) -> Result<String, String> {
    let b64: String = armored
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect();
    let blob = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| e.to_string())?;
    let mut at = 0;
    let mut take = |n: usize| -> Result<&[u8], String> {
        let s = blob.get(at..at + n).ok_or("signature cut short")?;
        at += n;
        Ok(s)
    };
    if take(6)? != b"SSHSIG" {
        return Err("not an SSH signature".into());
    }
    take(4)?;
    let mut read_string = || -> Result<Vec<u8>, String> {
        let n = u32::from_be_bytes(take(4)?.try_into().unwrap()) as usize;
        Ok(take(n)?.to_vec())
    };
    let public = read_string()?;
    let ns = read_string()?;
    let _reserved = read_string()?;
    let hash = read_string()?;
    let sig_blob = read_string()?;
    if ns != namespace.as_bytes() || hash != b"sha512" {
        return Err("wrong namespace or hash".into());
    }
    let key: [u8; 32] = public
        .get(public.len() - 32..)
        .ok_or("bad key")?
        .try_into()
        .unwrap();
    let sig = &sig_blob[sig_blob.len() - 64..];
    let mut signed = Vec::new();
    signed.extend_from_slice(b"SSHSIG");
    string(&mut signed, namespace.as_bytes());
    string(&mut signed, b"");
    string(&mut signed, b"sha512");
    string(&mut signed, &Sha512::digest(message));
    crate::keys::PublicKey(key).verify(&signed, sig)?;
    Ok(format!(
        "ssh-ed25519 {}",
        base64::engine::general_purpose::STANDARD.encode(&public)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_and_verifies() {
        let k = KeyPair::generate();
        let sig = sign(&k, "git", b"tree abc\n\nhello\n");
        assert!(sig.starts_with("-----BEGIN SSH SIGNATURE-----\n"));
        let line = verify(&sig, "git", b"tree abc\n\nhello\n").unwrap();
        assert!(public_line(&k, "weft").starts_with(&line));
        assert!(verify(&sig, "git", b"tree abc\n\nhellO\n").is_err());
    }
}
