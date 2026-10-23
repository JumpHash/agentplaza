//! The receiving side: fetches the chunks of a manifest that this node lacks, straight from the node that holds them.

use std::time::Duration;

use apz_blobs::{Blobs, Hash};
use apz_http::client::{Target, request};

use crate::{GRANT_HEADER, Grant, Manifest, RECEIVER_HEADER};

const TIMEOUT: Duration = Duration::from_secs(120);

/// What a fetch did: the blobs it fetched, the blobs it already held, and the bytes it fetched.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    pub fetched: usize,
    pub reused: usize,
    pub bytes: u64,
}

/// Fetches the blobs of `manifest` that `blobs` lacks from the node at `from` (`host:port`), for node `me`, presenting
/// `grant`, and records the manifest's files in `blobs`. A blob already held is not fetched.
pub fn fetch(
    blobs: &Blobs,
    me: &str,
    from: &str,
    grant: &Grant,
    manifest: &Manifest,
) -> Result<Report, String> {
    let target = Target {
        base: format!("http://{from}"),
        pin: None,
        bearer: None,
        headers: vec![
            (GRANT_HEADER.to_string(), grant.encode()),
            (RECEIVER_HEADER.to_string(), me.to_string()),
        ],
    };
    let mut report = Report::default();
    for file in &manifest.files {
        if file.chunks.is_empty() {
            if blobs.has(&file.root) {
                report.reused += 1;
                continue;
            }
            let bytes = get(&target, &file.root)?;
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
            let got = blobs.fetch_missing(&file.root, &file.chunks, |hash| get(&target, hash))?;
            report.fetched += got.fetched;
            report.reused += got.reused;
            report.bytes += got.bytes;
        }
    }
    Ok(report)
}

/// One chunk from the sender; a refusal is its status and its message.
fn get(target: &Target, hash: &Hash) -> Result<Vec<u8>, String> {
    let path = format!("/v1/transfer/chunks/{}", hash.hex());
    let reply = request(target, "GET", &path, None, TIMEOUT)
        .map_err(|err| format!("{}: {err}", target.base))?;
    if reply.status != 200 {
        return Err(format!(
            "{} refused chunk {}: {} {}",
            target.base,
            hash.short(),
            reply.status,
            String::from_utf8_lossy(&reply.body)
        ));
    }
    Ok(reply.body)
}
