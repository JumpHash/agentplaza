//! Node-to-node transfers on the node (plan 17.5). A move's export and a base's files are published from this node's
//! blob store; an import or a fetch takes the chunks it lacks straight from the node that holds them, presenting the
//! grant the hub signed. The hub names this node and gives its grant key with each call; it never sends the bytes.
//!
//! Routes (hub → node, see `lib.rs`):
//!   POST /v1/exports/{export}/manifest  {hub, files}                       → {id, manifest}
//!   POST /v1/imports/from-transfer      {hub, from, grant, manifest, meta, files} → {import, fetched, reused, bytes}
//!   POST /v1/transfers/manifests        {hub, files: [{name, path}]}       → {id, manifest}
//!   POST /v1/transfers/fetch            {hub, from, grant, manifest}       → {fetched, reused, bytes}

use std::io::{self, Read};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use apz_blobs::Blobs;
use apz_transfer::{Grant, Manifest, Node};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::migrate::{self, MAX_READ, MAX_WRITE, MigrateError, Migrations};
use crate::registry::Registry;

/// The transfer port a node listens on by default (`APZ_TRANSFER_PORT` overrides it).
pub const DEFAULT_PORT: u16 = 7447;

/// The node's transfer state: its blob store and the server that offers its blobs.
pub struct Transfers {
    node: Option<Arc<Node>>,
    server: Mutex<Option<apz_http::Handle>>,
}

/// What every transfer call says: the hub's id for this node and the hub's grant key.
#[derive(Deserialize)]
struct Hub {
    node: String,
    hub_key: String,
}

impl Transfers {
    /// The transfer state over the blob store at `dir`. A store that cannot be opened leaves transfers refused.
    pub fn open(dir: &Path) -> Transfers {
        match Blobs::open(dir) {
            Ok(blobs) => Transfers {
                node: Some(Node::new("", Arc::new(blobs))),
                server: Mutex::new(None),
            },
            Err(err) => {
                eprintln!("noded: no transfer store at {}: {err}", dir.display());
                Transfers {
                    node: None,
                    server: Mutex::new(None),
                }
            }
        }
    }

    /// Offers this node's blobs on `addr`.
    pub fn listen(&self, addr: SocketAddr) -> io::Result<()> {
        let node = self
            .node
            .as_ref()
            .ok_or_else(|| io::Error::other("no transfer store"))?;
        let handle = node.spawn(addr)?;
        *self.server.lock().unwrap_or_else(PoisonError::into_inner) = Some(handle);
        Ok(())
    }

    /// The port the transfer server listens on, when it does.
    pub fn port(&self) -> Option<u16> {
        self.server
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|handle| handle.port())
    }

    /// This node's transfer state (its blob store and the grant key of the last call).
    pub fn node(&self) -> Result<&Arc<Node>, MigrateError> {
        self.node
            .as_ref()
            .ok_or_else(|| MigrateError::Invalid("this node has no transfer store".into()))
    }

    /// Takes the hub's name for this node and its grant key, for this call.
    fn adopt(&self, hub: &Hub) -> Result<Arc<Node>, MigrateError> {
        let node = self.node()?;
        let key = apz_transfer::key_from_text(&hub.hub_key).map_err(MigrateError::Invalid)?;
        node.set_id(&hub.node);
        node.trust(key);
        Ok(Arc::clone(node))
    }
}

fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, MigrateError> {
    serde_json::from_slice(body)
        .map_err(|err| MigrateError::Invalid(format!("invalid transfer request: {err}")))
}

/// `POST /v1/exports/{export}/manifest`: stores the export's files as blobs and offers them to the receiver.
pub fn publish_export(
    transfers: &Transfers,
    migrations: &Migrations,
    export: &str,
    body: &[u8],
) -> Result<Value, MigrateError> {
    #[derive(Deserialize)]
    struct Publish {
        hub: Hub,
        files: Vec<migrate::FileEntry>,
    }
    let request: Publish = parse(body)?;
    let node = transfers.adopt(&request.hub)?;
    let mut files = Vec::with_capacity(request.files.len());
    for entry in &request.files {
        let reader = ExportReader {
            migrations,
            export,
            name: &entry.name,
            offset: 0,
            buf: Vec::new(),
            pos: 0,
            done: false,
        };
        let file = Manifest::store(node.blobs(), &entry.name, reader)
            .map_err(|err| MigrateError::Invalid(format!("{}: {err}", entry.name)))?;
        if file.len != entry.len {
            return Err(MigrateError::Mismatch(format!(
                "{} is {} bytes, not {}",
                entry.name, file.len, entry.len
            )));
        }
        files.push(file);
    }
    publish_manifest(&node, Manifest { files })
}

/// `POST /v1/transfers/manifests`: stores local files as blobs and offers them (a base's files on the node that built it).
pub fn publish_files(transfers: &Transfers, body: &[u8]) -> Result<Value, MigrateError> {
    #[derive(Deserialize)]
    struct Local {
        name: String,
        path: String,
    }
    #[derive(Deserialize)]
    struct Publish {
        hub: Hub,
        files: Vec<Local>,
    }
    let request: Publish = parse(body)?;
    let node = transfers.adopt(&request.hub)?;
    let mut files = Vec::with_capacity(request.files.len());
    for local in &request.files {
        let handle = std::fs::File::open(&local.path)
            .map_err(|err| MigrateError::Invalid(format!("{}: {err}", local.path)))?;
        files.push(
            Manifest::store(node.blobs(), &local.name, handle)
                .map_err(|err| MigrateError::Invalid(format!("{}: {err}", local.name)))?,
        );
    }
    publish_manifest(&node, Manifest { files })
}

fn publish_manifest(node: &Node, manifest: Manifest) -> Result<Value, MigrateError> {
    let id = node.publish(&manifest).map_err(MigrateError::Invalid)?;
    Ok(json!({"id": id, "manifest": manifest}))
}

/// `POST /v1/imports/from-transfer`: creates an import of the VM's files as `POST /v1/imports` does, fetches the
/// chunks this node lacks from the sender, and writes the files into the import. The import is finished as before.
pub fn import_from_transfer(
    transfers: &Transfers,
    migrations: &Migrations,
    registry: &Registry,
    body: &[u8],
) -> Result<Value, MigrateError> {
    #[derive(Deserialize)]
    struct Pull {
        hub: Hub,
        from: String,
        grant: String,
        manifest: Manifest,
    }
    let pull: Pull = parse(body)?;
    let node = transfers.adopt(&pull.hub)?;
    let grant = Grant::decode(&pull.grant).map_err(MigrateError::Invalid)?;
    let import = migrate::create_import(migrations, registry, body)?;
    let report = match apz_transfer::fetch(
        node.blobs(),
        &pull.hub.node,
        &pull.from,
        &grant,
        &pull.manifest,
    ) {
        Ok(report) => report,
        Err(err) => {
            let _ = migrate::abort_import(migrations, &import);
            return Err(MigrateError::Invalid(format!(
                "the fetch from {}: {err}",
                pull.from
            )));
        }
    };
    if let Err(err) = write_files(migrations, &import, &node, &pull.manifest) {
        let _ = migrate::abort_import(migrations, &import);
        return Err(err);
    }
    Ok(json!({
        "import": import,
        "fetched": report.fetched,
        "reused": report.reused,
        "bytes": report.bytes,
    }))
}

/// Writes each file of `manifest` from the blob store into the import, in pieces the import takes.
fn write_files(
    migrations: &Migrations,
    import: &str,
    node: &Node,
    manifest: &Manifest,
) -> Result<(), MigrateError> {
    for file in &manifest.files {
        let mut reader = node
            .blobs()
            .reader(&file.root)
            .map_err(MigrateError::Invalid)?;
        let mut offset = 0u64;
        let mut piece = vec![0u8; MAX_WRITE];
        loop {
            let n = read_full(&mut reader, &mut piece)
                .map_err(|err| MigrateError::Invalid(format!("{}: {err}", file.name)))?;
            if n == 0 {
                break;
            }
            let body = serde_json::to_vec(&json!({
                "offset": offset,
                "data": base64::engine::general_purpose::STANDARD.encode(&piece[..n]),
            }))
            .unwrap_or_default();
            migrate::write_import(migrations, import, &file.name, &body)?;
            offset += n as u64;
        }
        if offset != file.len {
            return Err(MigrateError::Mismatch(format!(
                "{} is {offset} bytes, not {}",
                file.name, file.len
            )));
        }
    }
    Ok(())
}

/// Reads until `buf` is full or the reader ends; the bytes read.
fn read_full(reader: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

/// `POST /v1/transfers/fetch`: fetches a manifest's chunks this node lacks into its store (a base's files).
pub fn fetch_manifest(transfers: &Transfers, body: &[u8]) -> Result<Value, MigrateError> {
    #[derive(Deserialize)]
    struct Fetch {
        hub: Hub,
        from: String,
        grant: String,
        manifest: Manifest,
    }
    let request: Fetch = parse(body)?;
    let node = transfers.adopt(&request.hub)?;
    let grant = Grant::decode(&request.grant).map_err(MigrateError::Invalid)?;
    let report = apz_transfer::fetch(
        node.blobs(),
        &request.hub.node,
        &request.from,
        &grant,
        &request.manifest,
    )
    .map_err(|err| MigrateError::Invalid(format!("the fetch from {}: {err}", request.from)))?;
    Ok(json!({
        "fetched": report.fetched,
        "reused": report.reused,
        "bytes": report.bytes,
    }))
}

/// An export's file read as a stream: the export's chunks in order, each read from the export as `GET` reads it.
struct ExportReader<'a> {
    migrations: &'a Migrations,
    export: &'a str,
    name: &'a str,
    offset: u64,
    buf: Vec<u8>,
    pos: usize,
    done: bool,
}

impl Read for ExportReader<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        while self.pos == self.buf.len() {
            if self.done {
                return Ok(0);
            }
            let query = format!("offset={}&len={MAX_READ}", self.offset);
            let chunk = migrate::read_export(self.migrations, self.export, self.name, &query)
                .map_err(|err| io::Error::other(err.to_string()))?;
            self.buf = match chunk.zeros {
                Some(zeros) => vec![0; usize::try_from(zeros).unwrap_or(usize::MAX)],
                None => base64::engine::general_purpose::STANDARD
                    .decode(&chunk.data)
                    .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?,
            };
            if self.buf.is_empty() && !chunk.eof {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the export ended before its file did",
                ));
            }
            self.offset += self.buf.len() as u64;
            self.pos = 0;
            self.done = chunk.eof;
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    fn noise(path: &Path, len: usize) {
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let bytes: Vec<u8> = (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect();
        std::fs::write(path, bytes).expect("a file");
    }

    /// Two node stores on loopback, each with its transfer server: a base's files published on "a" reach "b" with the
    /// grant the hub signed, and "b" fetches nothing it already holds.
    #[test]
    fn a_base_reaches_a_second_node_store_and_a_bad_grant_is_refused() {
        let dir = tempfile::tempdir().expect("a dir");
        let (a, b) = (
            Transfers::open(&dir.path().join("a")),
            Transfers::open(&dir.path().join("b")),
        );
        a.listen("127.0.0.1:0".parse().expect("an address"))
            .expect("a listener");
        let file = dir.path().join("base.bin");
        noise(&file, 6 << 20);
        let hub = SigningKey::from_bytes(&[3; 32]);
        let hub_key = apz_transfer::key_to_text(&hub.verifying_key());
        let published = publish_files(
            &a,
            json!({
                "hub": {"node": "a", "hub_key": hub_key},
                "files": [{"name": "base.bin", "path": file}],
            })
            .to_string()
            .as_bytes(),
        )
        .expect("published");
        let id = published["id"].as_str().expect("an id").to_string();
        let from = format!("127.0.0.1:{}", a.port().expect("a port"));
        let grant_for = |to: &str| {
            apz_transfer::Grant::issue(
                &hub,
                "base:p:build",
                &id,
                "a",
                to,
                apz_transfer::now_ms() + 60_000,
            )
            .encode()
        };
        let fetch = |to: &str, grant: String| {
            fetch_manifest(
                &b,
                json!({
                    "hub": {"node": to, "hub_key": hub_key},
                    "from": from,
                    "grant": grant,
                    "manifest": published["manifest"],
                })
                .to_string()
                .as_bytes(),
            )
        };
        // A grant for another node is refused, and nothing is sent.
        let refused = fetch("b", grant_for("c")).expect_err("refused");
        assert!(
            refused.to_string().contains("another node to receive"),
            "{refused}"
        );
        assert_eq!(a.node().expect("a node").served_bytes(), 0);

        let got = fetch("b", grant_for("b")).expect("fetched");
        assert_eq!(got["bytes"], 6 << 20);
        assert_eq!(a.node().expect("a node").served_bytes(), 6 << 20);
        // The same base again moves nothing: every chunk is held.
        let again = fetch("b", grant_for("b")).expect("fetched");
        assert_eq!(again["bytes"], 0);
    }
}
