//! The hub's object store (apz-blobs, in `<state-dir>/blobs`): the bytes the Library, the artifacts, the skills and the
//! stations keep, the responses that serve them, and the sweep that frees what nothing references (PLAN 30.5).
//!
//! A consumer keeps a blob's hash in its row and nothing else. Removing the row only drops the reference: the sweep
//! (`POST /v1/storage/sweep`, and once a day) frees the bytes of every blob that no consumer names.

use std::collections::HashSet;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use apz_blobs::{Blobs, Hash, SweepOptions, SweepReport};
use apz_http::{Body, Params, Request, Response, RouteDoc, Router};
use serde_json::json;

use crate::state::App;

/// How often the hub sweeps the store on its own.
const SWEEP_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// The blob `hash` (its text) as a response: the whole blob, streamed through its reader with its length, or the
/// range the request's `Range` header asks for, read with `read_range` so that only the chunks it covers are read.
/// `content_type` and `headers` are what the route sends for the file (`deliverables::served`). A hash the store does
/// not hold is a 404.
pub fn blob_response(
    blobs: &Arc<Blobs>,
    req: &Request,
    hash: &str,
    content_type: &str,
    headers: Vec<(&'static str, String)>,
) -> Response {
    let Some((hash, size)) = hash
        .parse::<Hash>()
        .ok()
        .and_then(|hash| Some((hash, blobs.size(&hash)?)))
    else {
        return Response::error(404, "no such blob");
    };
    let range = match requested_range(req, size) {
        Ok(range) => range,
        Err(()) => {
            return Response::error(416, "the range is past the end of the file")
                .with_header("Content-Range", &format!("bytes */{size}"));
        }
    };
    let Some((first, last)) = range else {
        // The whole blob, streamed: its reader holds one chunk at a time.
        let mut response = Response::new(200, content_type, Vec::new())
            .with_header("Content-Length", &size.to_string())
            .with_header("Accept-Ranges", "bytes");
        for (name, value) in headers {
            response = response.with_header(name, &value);
        }
        let blobs = Arc::clone(blobs);
        response.body = Body::Stream(Box::new(move |out: &mut dyn io::Write| -> io::Result<()> {
            let mut reader = blobs.reader(&hash).map_err(io::Error::other)?;
            io::copy(&mut reader, out).map(|_| ())
        }));
        return response;
    };
    match blobs.read_range(&hash, first, last - first + 1) {
        Ok(bytes) => {
            let mut response = Response::new(206, content_type, bytes)
                .with_header("Content-Range", &format!("bytes {first}-{last}/{size}"))
                .with_header("Accept-Ranges", "bytes");
            for (name, value) in headers {
                response = response.with_header(name, &value);
            }
            response
        }
        Err(e) => Response::error(500, &e),
    }
}

/// The one byte range a `Range: bytes=` header asks for, as `(first, last)` inclusive. `Ok(None)` when there is no such
/// header or it is one this server does not serve (another unit, several ranges, or a range that is malformed: it is
/// ignored, as HTTP asks). `Err` when the range lies past the end of a blob `size` bytes long.
fn requested_range(req: &Request, size: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(spec) = req
        .header("range")
        .and_then(|value| value.trim().strip_prefix("bytes="))
    else {
        return Ok(None);
    };
    let Some((first, last)) = spec.split_once('-').filter(|_| !spec.contains(',')) else {
        return Ok(None);
    };
    let (first, last) = (first.trim(), last.trim());
    let range = match (first.is_empty(), last.is_empty()) {
        // The last `n` bytes.
        (true, false) => match last.parse::<u64>() {
            Ok(n) if n > 0 && size > 0 => (size.saturating_sub(n), size - 1),
            Ok(_) => return Err(()),
            Err(_) => return Ok(None),
        },
        (false, _) => {
            let Ok(start) = first.parse::<u64>() else {
                return Ok(None);
            };
            let end = match last {
                "" => size.saturating_sub(1),
                _ => match last.parse::<u64>() {
                    Ok(end) => end.min(size.saturating_sub(1)),
                    Err(_) => return Ok(None),
                },
            };
            if start >= size {
                return Err(());
            }
            if end < start {
                return Ok(None);
            }
            (start, end)
        }
        (true, true) => return Ok(None),
    };
    Ok(Some(range))
}

/// Every hash the hub's rows reference: the Library's files, the artifacts' blobs, the skills' files and the stations'
/// frames and sync content. The sweep keeps each of them.
pub fn roots(app: &App) -> Result<HashSet<Hash>, String> {
    let mut roots: HashSet<Hash> = HashSet::new();
    roots.extend(app.deliverables.hashes());
    roots.extend(app.artifacts.hashes()?);
    roots.extend(app.skills.hashes()?);
    roots.extend(crate::stations::blob_hashes(app)?);
    Ok(roots)
}

/// Frees the blobs no row references: [`sweep_with`] with every hub row as the roots and the default grace window (a
/// blob written within the last hour is kept whatever the rows say, so a write that races the sweep is safe).
pub fn sweep(app: &App) -> Result<SweepReport, String> {
    let roots = roots(app)?;
    sweep_with(
        &app.blobs,
        &|hash: &Hash| roots.contains(hash),
        SweepOptions::default(),
    )
}

/// The mark and sweep of the store with `referenced` as the roots and `options` as the grace window and compaction.
pub fn sweep_with(
    blobs: &Blobs,
    referenced: &dyn Fn(&Hash) -> bool,
    options: SweepOptions,
) -> Result<SweepReport, String> {
    blobs.sweep(referenced, options)
}

/// `GET /v1/storage` and `POST /v1/storage/sweep` (the console's storage view and the owner's sweep).
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/storage", stats_route)
        .doc(RouteDoc::new(
            "The hub's object store: its blobs and their bytes on disk",
        ))
        .post("/v1/storage/sweep", sweep_route)
        .doc(
            RouteDoc::new("Free the blobs nothing references; answers the bytes freed")
                .errors(&[500]),
        )
}

fn stats_route(app: &App, _: &Request, _: &Params<'_>) -> Response {
    let (blobs, bytes) = app.blobs.stats();
    Response::json(200, &json!({ "blobs": blobs, "bytes": bytes }))
}

fn sweep_route(app: &App, _: &Request, _: &Params<'_>) -> Response {
    match sweep(app) {
        Ok(report) => Response::json(200, &report_json(&report)),
        Err(e) => Response::error(500, &e),
    }
}

fn report_json(report: &SweepReport) -> serde_json::Value {
    json!({
        "live": report.live,
        "dead": report.dead,
        "freed_bytes": report.freed_bytes,
        "packs_compacted": report.packs_compacted,
        "packs_removed": report.packs_removed,
    })
}

/// Sweeps the store once a day, from the day after start.
pub fn spawn_sweeper(app: Arc<App>) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(SWEEP_EVERY);
            match sweep(&app) {
                Ok(report) => eprintln!(
                    "apid: storage sweep: {} blobs dropped, {} bytes freed",
                    report.dead, report.freed_bytes
                ),
                Err(e) => eprintln!("apid: storage sweep failed: {e}"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store in a scratch folder named for the test, as the hub opens it (`<state-dir>/blobs`).
    fn store(name: &str) -> Arc<Blobs> {
        let dir = std::env::temp_dir()
            .join(format!("apid-storage-{}-{name}", std::process::id()))
            .join("blobs");
        let _ = std::fs::remove_dir_all(dir.parent().expect("a parent"));
        Arc::new(Blobs::open(&dir).expect("the store opens"))
    }

    /// Bytes that do not repeat within a few chunks, so a chunked blob's chunks differ.
    fn big(n: u32) -> Vec<u8> {
        (0..n)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect()
    }

    fn request(range: Option<&str>) -> Request {
        let mut headers = vec![("host".to_string(), "127.0.0.1".to_string())];
        if let Some(range) = range {
            headers.push(("range".to_string(), range.to_string()));
        }
        Request {
            method: "GET".to_string(),
            path: "/".to_string(),
            headers,
            ..Request::default()
        }
    }

    /// The bytes after a response's head, as a client reads them off the wire.
    fn wire(response: Response) -> (String, Vec<u8>) {
        let mut out = Vec::new();
        response.write_to(&mut out).expect("written");
        let split = out
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("a head");
        let head = String::from_utf8_lossy(&out[..split]).to_string();
        (head, out[split + 4..].to_vec())
    }

    #[test]
    fn a_range_header_is_read_as_the_one_range_it_asks() {
        let size = 1000;
        let range = |spec: &str| requested_range(&request(Some(spec)), size);
        assert_eq!(range("bytes=0-9"), Ok(Some((0, 9))));
        assert_eq!(range("bytes=995-"), Ok(Some((995, 999))));
        assert_eq!(range("bytes=-3"), Ok(Some((997, 999))));
        assert_eq!(
            range("bytes=990-5000"),
            Ok(Some((990, 999))),
            "clipped to the end"
        );
        assert_eq!(range("bytes=1000-"), Err(()), "past the end");
        assert_eq!(range("bytes=-0"), Err(()));
        assert_eq!(
            range("bytes=9-3"),
            Ok(None),
            "a last byte before the first is ignored"
        );
        assert_eq!(
            range("bytes=0-1,5-6"),
            Ok(None),
            "several ranges are not served"
        );
        assert_eq!(range("items=0-1"), Ok(None), "another unit is ignored");
        assert_eq!(range("bytes=x-y"), Ok(None), "a malformed range is ignored");
        assert_eq!(requested_range(&request(None), size), Ok(None));
    }

    #[test]
    fn a_blob_is_served_whole_or_as_its_range_and_a_missing_one_is_not_found() {
        let blobs = store("serve");
        let data = big(12_000_000);
        let hash = blobs.put(&data).expect("put");
        let text = hash.to_string();
        let headers = vec![("x-content-type-options", "nosniff".to_string())];
        let (head, body) = wire(blob_response(
            &blobs,
            &request(None),
            &text,
            "application/octet-stream",
            headers.clone(),
        ));
        assert!(head.starts_with("HTTP/1.1 200 OK"), "{head}");
        assert!(head.contains("Content-Length: 12000000"), "{head}");
        assert!(head.contains("x-content-type-options: nosniff"), "{head}");
        assert!(head.contains("Accept-Ranges: bytes"), "{head}");
        assert!(body == data, "the whole blob, streamed");
        let before = blobs.entry_reads();
        let (head, part) = wire(blob_response(
            &blobs,
            &request(Some("bytes=10000000-10000999")),
            &text,
            "application/octet-stream",
            headers,
        ));
        assert!(head.starts_with("HTTP/1.1 206 Partial Content"), "{head}");
        assert!(
            head.contains("Content-Range: bytes 10000000-10000999/12000000"),
            "{head}"
        );
        assert_eq!(part, data[10_000_000..10_001_000]);
        let reads = blobs.entry_reads() - before;
        assert!(
            reads <= 4,
            "{reads} entries read for a 1000-byte range of a chunked blob"
        );
        let (head, _) = wire(blob_response(
            &blobs,
            &request(Some("bytes=12000000-")),
            &text,
            "application/octet-stream",
            Vec::new(),
        ));
        assert!(head.starts_with("HTTP/1.1 416"), "{head}");
        assert!(head.contains("Content-Range: bytes */12000000"), "{head}");
        let (head, _) = wire(blob_response(
            &blobs,
            &request(None),
            &Hash::of(b"nobody").to_string(),
            "text/plain",
            Vec::new(),
        ));
        assert!(head.starts_with("HTTP/1.1 404"), "{head}");
        let (head, _) = wire(blob_response(
            &blobs,
            &request(None),
            "not a hash",
            "text/plain",
            Vec::new(),
        ));
        assert!(head.starts_with("HTTP/1.1 404"), "{head}");
    }

    #[test]
    fn a_sweep_frees_what_no_row_names_and_keeps_what_is_written_during_it() {
        let blobs = store("sweep");
        let kept = blobs.put(b"named by a row").expect("put");
        let dropped = blobs.put(b"named by nothing").expect("put");
        std::thread::sleep(Duration::from_millis(20));
        let during = std::sync::Mutex::new(None);
        let report = sweep_with(
            &blobs,
            &|hash: &Hash| {
                // A write that lands while the sweep marks: it is newer than the sweep's cutoff, so it is kept.
                let mut written = during.lock().unwrap();
                if written.is_none() {
                    *written = Some(blobs.put(b"written during the sweep").unwrap());
                }
                *hash == kept
            },
            SweepOptions {
                grace: Duration::from_millis(10),
                compact_below: 0.5,
            },
        )
        .expect("swept");
        let during = during.into_inner().unwrap().expect("the write ran");
        assert!(blobs.has(&kept));
        assert!(!blobs.has(&dropped), "{report:?}");
        assert!(
            blobs.has(&during),
            "a blob written during the sweep survives it"
        );
        assert_eq!(report.dead, 1);
        let json = report_json(&report);
        assert_eq!(json["dead"], 1);
        assert_eq!(json["freed_bytes"], report.freed_bytes);
    }
}
