//! The hub's HTTP to a preview's app (PLAN 18.1): one request over a port stream (`nodes::Store::open_port`), and the
//! answer read back whole. The app answers with `Connection: close` (or a framed body), so the answer ends with the
//! stream. A streamed answer (server-sent events) is read to its end, not relayed as it comes.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Duration;

use apz_http::mux::Stream;
use apz_http::{Body, Request, Response};

use crate::fleet::LOCAL;
use crate::nodes;

/// vsockd's inbound listener on the hub's own machine (apz-link's default): the port the local node's streams go to.
const LOCAL_INBOUND: u16 = 19095;

/// How long a preview's app may take to answer (a request that runs a build is not a preview's request).
const FETCH_TIMEOUT: Duration = Duration::from_secs(60);

/// The request headers the app gets from the viewer: what it needs to answer. No cookie, no authorization: the hub's
/// own credentials never reach a preview.
const FORWARDED: &[&str] = &[
    "accept",
    "accept-language",
    "content-type",
    "user-agent",
    "if-none-match",
    "if-modified-since",
];

/// The response headers the viewer gets back from the app.
const RELAYED: &[&str] = &[
    "content-type",
    "cache-control",
    "etag",
    "last-modified",
    "content-language",
    "location",
];

/// One request to the preview's app at `path` (with `query`), over the node's port `port` of guest `cid`, and its answer.
/// A node that is not connected, a port the app does not serve and an answer that does not parse are all errors to the
/// viewer (502 or 503), named.
pub fn fetch(
    nodes: &nodes::Store,
    node: &str,
    cid: u64,
    port: u32,
    req: &Request,
    path: &str,
    query: &str,
) -> Response {
    fetch_via(nodes, node, LOCAL_INBOUND, cid, port, req, (path, query))
}

/// [`fetch`] with the hub's own machine's inbound listener on `local_inbound` (a test serves its own). `target` is the
/// path and the query.
fn fetch_via(
    nodes: &nodes::Store,
    node: &str,
    local_inbound: u16,
    cid: u64,
    port: u32,
    req: &Request,
    target: (&str, &str),
) -> Response {
    let Ok(port) = u16::try_from(port) else {
        return Response::error(500, "the preview's port is not a port");
    };
    let bytes = request_bytes(req, target.0, target.1);
    let answer = if node == LOCAL {
        apz_link::inbound::connect(local_inbound, cid, port)
            .map_err(|why| format!("the preview's machine does not answer: {why}"))
            .and_then(|conn| exchange_tcp(conn, &bytes))
    } else {
        nodes
            .open_port(node, cid, port)
            .map_err(|why| format!("the preview's node is not reachable: {why}"))
            .and_then(|stream| exchange_stream(&stream, &bytes))
    };
    match answer {
        Ok(bytes) => parse(&bytes).unwrap_or_else(|why| Response::error(502, &why)),
        Err(why) => Response::error(503, &why),
    }
}

/// One request over a node's stream (the tunnel's port stream): written, finished, and its answer read to the end.
fn exchange_stream(stream: &Stream, request: &[u8]) -> Result<Vec<u8>, String> {
    stream
        .send(request)
        .and_then(|()| stream.finish())
        .map_err(|_| "the preview's app did not take the request".to_string())?;
    stream
        .read_all(FETCH_TIMEOUT)
        .map_err(|err| format!("the preview's app did not answer: {err}"))
}

/// One request over a local connection (vsockd's inbound listener): written, its write side shut, and the answer read.
fn exchange_tcp(mut conn: TcpStream, request: &[u8]) -> Result<Vec<u8>, String> {
    conn.set_read_timeout(Some(FETCH_TIMEOUT))
        .and_then(|()| conn.write_all(request))
        .and_then(|()| conn.shutdown(Shutdown::Write))
        .map_err(|err| format!("the preview's app did not take the request: {err}"))?;
    let mut answer = Vec::new();
    conn.read_to_end(&mut answer)
        .map_err(|err| format!("the preview's app did not answer: {err}"))?;
    Ok(answer)
}

/// The request as HTTP/1.1 bytes: the method, the target, the forwarded headers, `Connection: close`, and the body.
pub fn request_bytes(req: &Request, path: &str, query: &str) -> Vec<u8> {
    let target = if query.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{query}")
    };
    let mut head = format!(
        "{} {target} HTTP/1.1\r\nHost: preview\r\nConnection: close\r\nContent-Length: {}\r\n",
        req.method,
        req.body.len()
    );
    for name in FORWARDED {
        if let Some(value) = req.header(name) {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
    }
    head.push_str("\r\n");
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(&req.body);
    bytes
}

/// An HTTP/1.1 answer as the viewer gets it: its status, the relayed headers, and its body (de-chunked when it is
/// chunked).
pub fn parse(bytes: &[u8]) -> Result<Response, String> {
    let split = find(bytes, b"\r\n\r\n").ok_or("the preview's answer has no head")?;
    let head = std::str::from_utf8(&bytes[..split])
        .map_err(|_| "the preview's answer head is not text")?;
    let body = &bytes[split + 4..];
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status: u16 = status_line
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or("the preview's answer has no status")?;
    let mut headers = Vec::new();
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().to_string();
        if name == "transfer-encoding" && value.eq_ignore_ascii_case("chunked") {
            chunked = true;
        }
        if RELAYED.contains(&name.as_str()) {
            headers.push((name, value));
        }
    }
    let body = if chunked {
        dechunk(body)?
    } else {
        body.to_vec()
    };
    Ok(Response {
        status,
        headers,
        body: Body::Bytes(body),
    })
}

/// The body of a chunked answer: each chunk's bytes, up to the zero-size chunk.
fn dechunk(mut rest: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let eol = find(rest, b"\r\n").ok_or("a chunk has no size line")?;
        let size_text =
            std::str::from_utf8(&rest[..eol]).map_err(|_| "a chunk size is not text")?;
        let size =
            usize::from_str_radix(size_text.split(';').next().unwrap_or_default().trim(), 16)
                .map_err(|_| "a chunk size is not hex")?;
        rest = &rest[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        if rest.len() < size + 2 {
            return Err("a chunk is cut short".into());
        }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[size + 2..];
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use std::net::{SocketAddr, TcpListener};
    use std::sync::Arc;
    use std::thread;

    use apz_link::inbound::{Duplex, GuestDial, PortCheck};

    use super::*;
    use crate::nodes::Store;

    /// vsockd's check and dial on the hub's machine, with the fork's app behind them.
    struct Local(SocketAddr);

    impl PortCheck for Local {
        fn served(&self, cid: u64, port: u16) -> bool {
            (cid, port) == (7, 8080)
        }
    }

    impl GuestDial for Local {
        fn dial(&self, _cid: u64, _port: u16) -> std::io::Result<Box<dyn Duplex>> {
            Ok(Box::new(TcpStream::connect(self.0)?))
        }
    }

    /// The fork's app: answers any request with its path, and closes.
    fn app() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
        let addr = listener.local_addr().expect("address");
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { return };
                thread::spawn(move || {
                    let mut head = Vec::new();
                    let mut buf = [0u8; 1024];
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match conn.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => head.extend_from_slice(&buf[..n]),
                        }
                    }
                    let line = String::from_utf8_lossy(&head)
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_string();
                    let path = line.split(' ').nth(1).unwrap_or_default().to_string();
                    let body = format!("you asked for {path}");
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = conn.write_all(reply.as_bytes());
                });
            }
        });
        addr
    }

    #[test]
    fn a_preview_on_the_hubs_own_machine_is_reached_through_vsockds_inbound_listener() {
        let check = Arc::new(Local(app()));
        let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
        let inbound = listener.local_addr().expect("address").port();
        let served = Arc::clone(&check);
        thread::spawn(move || {
            apz_link::inbound::serve(listener, served.clone(), served);
        });
        let req = Request {
            method: "GET".into(),
            path: "/".into(),
            query: String::new(),
            headers: Vec::new(),
            body: Vec::new(),
            peer: None,
        };
        let nodes = Store::holding(Vec::new());
        let answer = fetch_via(&nodes, LOCAL, inbound, 7, 8080, &req, ("/docs", "x=1"));
        assert_eq!(answer.status, 200);
        match answer.body {
            Body::Bytes(bytes) => assert_eq!(bytes, b"you asked for /docs?x=1"),
            _ => panic!("a fetched answer is bytes"),
        }
        let refused = fetch_via(&nodes, LOCAL, inbound, 7, 9999, &req, ("/", ""));
        assert_eq!(refused.status, 503, "a port no run serves is refused");
    }
}
