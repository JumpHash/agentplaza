//! Micro-benchmarks for the HTTP hot path: reading a request head and body off a
//! connection, and serialising a JSON response onto one. Only the crate's public API is used.

use std::hint::black_box;
use std::io::Cursor;

use apz_http::{Limits, Request, Response};
use criterion::{Criterion, criterion_group, criterion_main};
use serde_json::json;

/// A browser's GET for one agent's status: ten headers, the usual browser set.
const BROWSER_GET: &[u8] = b"GET /v1/agents/agt_7f3a9c21/status?verbose=1 HTTP/1.1\r\n\
Host: 127.0.0.1:7700\r\n\
User-Agent: Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0\r\n\
Accept: text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8\r\n\
Accept-Encoding: gzip, deflate, br\r\n\
Accept-Language: en-GB,en;q=0.7,de;q=0.3\r\n\
Cookie: apz_session=bench; theme=dark\r\n\
Connection: keep-alive\r\n\
Sec-Fetch-Site: same-origin\r\n\
Referer: http://127.0.0.1:7700/\r\n\
Authorization: Bearer bench-token\r\n\
\r\n";

/// A POST whose JSON body is exactly 1 KiB, with its `Content-Length`.
fn post_with_json_body() -> Vec<u8> {
    // `{"text":"` and `"}` are 11 fixed bytes around the text.
    let body = format!("{{\"text\":\"{}\"}}", "x".repeat(1024 - 11));
    let mut raw = format!(
        "POST /v1/agents/agt_7f3a9c21/tell HTTP/1.1\r\n\
         Host: 127.0.0.1:7700\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         \r\n",
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(body.as_bytes());
    raw
}

fn parse_get_10_headers(c: &mut Criterion) {
    c.bench_function("parse_get_10_headers", |b| {
        b.iter(|| Request::read(&mut Cursor::new(black_box(BROWSER_GET)), Limits::default()))
    });
}

fn parse_post_1kib_json(c: &mut Criterion) {
    let raw = post_with_json_body();
    c.bench_function("parse_post_1kib_json", |b| {
        b.iter(|| {
            Request::read(
                &mut Cursor::new(black_box(raw.as_slice())),
                Limits::default(),
            )
        })
    });
}

fn serialise_json_response_200(c: &mut Criterion) {
    // About 200 bytes of JSON: an agent's status with a short summary.
    let payload = json!({
        "id": "agt_7f3a9c21",
        "status": "running",
        "room": "main",
        "summary": "x".repeat(150)
    });
    c.bench_function("serialise_json_response_200", |b| {
        b.iter(|| {
            let mut out: Vec<u8> = Vec::with_capacity(512);
            let written = Response::json(200, black_box(&payload)).write_to(&mut out);
            black_box(written.map(|_| out))
        })
    });
}

criterion_group!(
    benches,
    parse_get_10_headers,
    parse_post_1kib_json,
    serialise_json_response_200
);
criterion_main!(benches);
