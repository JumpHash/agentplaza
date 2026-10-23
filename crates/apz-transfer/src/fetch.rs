//! The receiving side: fetches the chunks of a manifest that this node lacks, straight from the node that holds them.
//!
//! The sender is reached by one of the [`Route`]s the hub gave: a direct dial, or the hub's broker, which relays the
//! bytes into the sender's tunnel (`broker.rs`). Either way the TLS session is the receiver's and the sender's, pinned
//! to the sender's certificate in the grant, so the hub cannot read what it carries.

use std::io;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use apz_blobs::{Blobs, Hash};
use apz_http::client::{Target, request_over};
use std::io::Write as _;

use crate::{GRANT_HEADER, Grant, Manifest, RECEIVER_HEADER};

const TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT: Duration = Duration::from_secs(3);

/// How the receiver reaches the sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// The sender's transfer address (`host:port`), dialed directly.
    Direct(String),
    /// The hub's broker (`host:port`), and the ticket the hub issued for the sender.
    Brokered { broker: String, ticket: String },
}

/// The path a fetch took.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    #[default]
    Direct,
    Brokered,
}

impl Via {
    pub fn as_str(self) -> &'static str {
        match self {
            Via::Direct => "direct",
            Via::Brokered => "brokered",
        }
    }
}

/// What a fetch did: the blobs it fetched, the blobs it already held, the bytes it fetched, and the path it took.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    pub fetched: usize,
    pub reused: usize,
    pub bytes: u64,
    pub via: Via,
}

/// Fetches the blobs of `manifest` that `blobs` lacks, for node `me`, presenting `grant` and pinning the sender's
/// certificate `pin` (the grant's). The first route the receiver can reach is used for every chunk; a blob already held
/// is not fetched.
pub fn fetch(
    blobs: &Blobs,
    me: &str,
    routes: &[Route],
    grant: &Grant,
    manifest: &Manifest,
) -> Result<Report, String> {
    let pin = pin_bytes(&grant.pin)?;
    let route = routes
        .iter()
        .find(|route| probe(route).is_ok())
        .ok_or("no route to the sender reaches it: the direct dial failed and the hub's broker is not offered")?;
    let via = match route {
        Route::Direct(_) => Via::Direct,
        Route::Brokered { .. } => Via::Brokered,
    };
    let get = |hash: &Hash| chunk(route, me, grant, &pin, hash);
    let mut report = Report {
        via,
        ..Report::default()
    };
    for file in &manifest.files {
        if file.chunks.is_empty() {
            if blobs.has(&file.root) {
                report.reused += 1;
                continue;
            }
            let bytes = get(&file.root)?;
            if bytes.len() as u64 != file.len {
                return Err(format!(
                    "{}: {} bytes came for {} bytes",
                    file.name,
                    bytes.len(),
                    file.len
                ));
            }
            blobs.put_checked(&file.root, &bytes)?;
            report.fetched += 1;
            report.bytes += file.len;
        } else {
            let got = blobs.fetch_missing(&file.root, &file.chunks, get)?;
            report.fetched += got.fetched;
            report.reused += got.reused;
            report.bytes += got.bytes;
        }
    }
    Ok(report)
}

/// The sender's pin from its hex text.
fn pin_bytes(text: &str) -> Result<[u8; 32], String> {
    let bytes = apz_blobs::unhex(text).map_err(|e| format!("the grant's pin: {e}"))?;
    <[u8; 32]>::try_from(bytes.as_slice())
        .map_err(|_| "the grant's pin is not 32 bytes".to_string())
}

/// Whether the route's first hop accepts a connection (the hub's broker, for a brokered route).
fn probe(route: &Route) -> io::Result<TcpStream> {
    dial(match route {
        Route::Direct(addr) => addr,
        Route::Brokered { broker, .. } => broker,
    })
}

fn dial(addr: &str) -> io::Result<TcpStream> {
    let target = addr
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::other(format!("{addr} does not resolve")))?;
    TcpStream::connect_timeout(&target, CONNECT)
}

/// One chunk from the sender over the route, in a pinned TLS session. A brokered route first says its ticket to the
/// hub's broker, which relays the rest of the connection.
fn chunk(
    route: &Route,
    me: &str,
    grant: &Grant,
    pin: &[u8; 32],
    hash: &Hash,
) -> Result<Vec<u8>, String> {
    let (addr, stream) = match route {
        Route::Direct(addr) => (addr.clone(), dial(addr)),
        Route::Brokered { broker, ticket } => (
            broker.clone(),
            dial(broker).and_then(|mut s| {
                s.write_all(format!("{}{ticket}\n", crate::broker::PREFIX).as_bytes())?;
                Ok(s)
            }),
        ),
    };
    let stream = stream.map_err(|err| format!("{addr}: {err}"))?;
    let target = Target {
        base: format!("https://{addr}"),
        pin: Some(*pin),
        bearer: None,
        headers: vec![
            (GRANT_HEADER.to_string(), grant.encode()),
            (RECEIVER_HEADER.to_string(), me.to_string()),
        ],
    };
    let path = format!("/v1/transfer/chunks/{}", hash.hex());
    let reply = request_over(
        stream,
        Some("localhost"),
        &target,
        "GET",
        &path,
        None,
        TIMEOUT,
    )
    .map_err(|err| format!("{addr}: {err}"))?;
    if reply.status != 200 {
        return Err(format!(
            "{addr} refused chunk {}: {} {}",
            hash.short(),
            reply.status,
            String::from_utf8_lossy(&reply.body)
        ));
    }
    Ok(reply.body)
}
