//! The node's identity on disk, under its state directory (mode 0700):
//! - `node.json` (0600): [`Identity`] — hub URL, certificate pin, node id, CID range, hub name;
//! - `node.key` (0600): the Ed25519 seed (32 raw bytes). It never leaves this machine.
//! - `internal-token` (0600): the secret the local vsockd presents to the forwarder.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use apz_http::client::Target;
use apz_trust::node::NodeKey;
use serde::{Deserialize, Serialize};

/// What the hub gave this node when it joined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// `https://host:port` (no trailing slash).
    pub hub_url: String,
    /// SHA-256 of the hub's TLS certificate, base64url (as in the join token).
    pub pin: String,
    pub node_id: String,
    pub cid_base: u64,
    pub cid_count: u64,
    #[serde(default)]
    pub hub_name: String,
}

impl Identity {
    /// The pin as bytes; Err when it is not 32 bytes of base64url.
    pub fn pin_bytes(&self) -> Result<[u8; 32], String> {
        let raw = apz_trust::unb64(&self.pin).map_err(|e| format!("pin: {e}"))?;
        raw.try_into().map_err(|v: Vec<u8>| format!("pin: {} bytes, want 32", v.len()))
    }

    /// The hub as an apz-http target (pinned, no bearer, no extra headers).
    pub fn target(&self) -> Result<Target, String> {
        Ok(Target { base: self.hub_url.clone(), pin: Some(self.pin_bytes()?), bearer: None, headers: Vec::new() })
    }

    /// [`Identity::target`] with this node's signature headers for `method path` (path with its
    /// query) and `body`.
    pub fn signed(&self, key: &NodeKey, method: &str, path: &str, body: &[u8]) -> Result<Target, String> {
        let mut target = self.target()?;
        let ts = now_secs()?;
        target.headers.extend(key.sign(&self.node_id, method, path, ts, body).into_iter().map(|(name, value)| (name.to_string(), value)));
        Ok(target)
    }

    /// The inclusive guest-CID range as `noded --run-cids` takes it (and people read it): `A-B`.
    pub fn cid_range(&self) -> String {
        format!("{}-{}", self.cid_base, self.cid_base + self.cid_count - 1)
    }

    /// The same CIDs as `vsockd --cids` takes them: `A,A+1,…,B`.
    pub fn cid_list(&self) -> String {
        (self.cid_base..self.cid_base + self.cid_count).map(|cid| cid.to_string()).collect::<Vec<_>>().join(",")
    }
}

/// Writes the identity and key (creating `dir` 0700, the files 0600, atomically).
pub fn save(dir: &Path, identity: &Identity, key: &NodeKey) -> Result<(), String> {
    ensure_dir(dir)?;
    let json = serde_json::to_vec_pretty(identity).map_err(|e| format!("node.json: {e}"))?;
    write_private(&dir.join("node.json"), &json)?;
    write_private(&dir.join("node.key"), &key.to_bytes())
}

/// Reads them back; Err naming the file when one is missing or malformed.
pub fn load(dir: &Path) -> Result<(Identity, NodeKey), String> {
    let json_path = dir.join("node.json");
    let json = fs::read(&json_path).map_err(|e| format!("{}: {e}", json_path.display()))?;
    let identity: Identity = serde_json::from_slice(&json).map_err(|e| format!("{}: {e}", json_path.display()))?;
    let key_path = dir.join("node.key");
    let seed = fs::read(&key_path).map_err(|e| format!("{}: {e}", key_path.display()))?;
    let key = NodeKey::from_bytes(&seed).map_err(|e| format!("{}: {e}", key_path.display()))?;
    Ok((identity, key))
}

/// The local forwarder's secret: read `internal-token`, or create it (32 random bytes,
/// base64url, 0600) when absent. Returns the token text.
pub fn internal_token(dir: &Path) -> Result<String, String> {
    let path = dir.join("internal-token");
    match fs::read_to_string(&path) {
        Ok(text) => {
            let token = text.trim().to_string();
            if token.is_empty() {
                Err(format!("{}: empty", path.display()))
            } else {
                Ok(token)
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            ensure_dir(dir)?;
            let token = apz_trust::b64(&apz_trust::random::<32>());
            write_private(&path, token.as_bytes())?;
            Ok(token)
        }
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Creates `dir` (and parents) with mode 0700; an existing directory is accepted.
fn ensure_dir(dir: &Path) -> Result<(), String> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|e| format!("{}: {e}", dir.display()))
}

/// Writes `bytes` to `path` with mode 0600: a fresh `<name>.tmp` beside it, then a rename.
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
    let _ = fs::remove_file(&tmp);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
    file.write_all(bytes).and_then(|_| file.sync_all()).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

fn now_secs() -> Result<u64, String> {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apz-link-identity-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn sample() -> Identity {
        Identity {
            hub_url: "https://hub.example:8443".into(),
            pin: apz_trust::b64(&[7u8; 32]),
            node_id: "node-1".into(),
            cid_base: 3,
            cid_count: 2,
            hub_name: "hub".into(),
        }
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn save_load_round_trip_with_modes() {
        let root = scratch("roundtrip");
        let dir = root.join("state");
        let identity = sample();
        let key = NodeKey::generate();
        save(&dir, &identity, &key).unwrap();
        let (loaded, loaded_key) = load(&dir).unwrap();
        assert_eq!(loaded, identity);
        assert_eq!(loaded_key.to_bytes(), key.to_bytes());
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join("node.json")), 0o600);
        assert_eq!(mode(&dir.join("node.key")), 0o600);
        assert!(!dir.join("node.json.tmp").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn internal_token_is_created_once_and_stable() {
        let dir = scratch("token");
        let first = internal_token(&dir).unwrap();
        let second = internal_token(&dir).unwrap();
        assert_eq!(first, second);
        assert_eq!(apz_trust::unb64(&first).unwrap().len(), 32);
        assert_eq!(mode(&dir.join("internal-token")), 0o600);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn signed_carries_headers_and_pin_must_be_32_bytes() {
        let identity = sample();
        let key = NodeKey::generate();
        let target = identity.signed(&key, "POST", "/v1/ping?x=1", b"{}").unwrap();
        assert_eq!(target.base, identity.hub_url);
        assert_eq!(target.pin, Some([7u8; 32]));
        assert!(target.bearer.is_none());
        assert_eq!(target.headers.len(), 4);
        assert!(target.headers.iter().all(|(name, _)| name.to_ascii_lowercase().starts_with("x-apz-")));
        let short = Identity { pin: apz_trust::b64(&[7u8; 16]), ..sample() };
        assert!(short.pin_bytes().is_err());
        assert!(short.target().is_err());
    }
}
