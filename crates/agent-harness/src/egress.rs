//! The guest's outbound access: a forward-proxy shim over vsock, and the HTTP
//! client the file tools use through it.
//!
//! The guest has no NIC. Its one wire is the vsock stream to the host (CID 2),
//! and the orchestrator's egress end on [`EGRESS_PORT`] carries raw bytes out to
//! the network. [`spawn_shim`] puts a TCP listener on [`PROXY_ADDR`] and bridges
//! each accepted connection to a fresh vsock connection, byte for byte, in both
//! directions. The shim parses no HTTP: a plain request and an https `CONNECT`
//! tunnel cross it alike, so the proxy is a pipe and nothing more.
//!
//! The `bash` tool's environment points at the same proxy (see [`crate::bash`]),
//! and `fetch_url` and `download` (see [`crate::files`]) reach the network
//! through it with `ureq`.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use apz_proto::wire::{MAX_TOOL_OUTPUT, cap_output};
use serde_json::Value;

use crate::bash::{PROXY_ADDR, PROXY_URL};
use crate::files::Workspace;
use crate::tools::ToolError;
use crate::vsock::{HOST_CID, VsockStream};

/// The vsock port of the orchestrator's egress end, on the host CID.
const EGRESS_PORT: u32 = 5001;

/// The most `fetch_url` reads of a body: 512 KiB.
const FETCH_CAP: u64 = 512 * 1024;

/// The largest body `download` saves: 50 MiB.
pub const DOWNLOAD_CAP: u64 = 50 * 1024 * 1024;

/// The most one request may take, from connect to the last body byte.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// How long the accept loop waits after a failed accept before trying again.
const ACCEPT_RETRY: Duration = Duration::from_millis(50);

/// The copy buffer for one direction of a bridged connection.
const PUMP_BUF: usize = 16 * 1024;

/// A connection the shim can read, write and end, with every handle to it.
///
/// [`TcpStream`] and [`VsockStream`] implement it; the test bridges TCP to TCP
/// through the same code path the vsock one takes.
pub trait Link: Read + Write + Send + Sized + 'static {
    /// Another handle to the same connection, so one thread can read while
    /// another writes.
    fn duplicate(&self) -> io::Result<Self>;

    /// Ends the connection in both directions, for every handle to it.
    fn shut_down(&self);
}

impl Link for TcpStream {
    fn duplicate(&self) -> io::Result<Self> {
        self.try_clone()
    }

    fn shut_down(&self) {
        let _ = self.shutdown(std::net::Shutdown::Both);
    }
}

impl Link for VsockStream {
    fn duplicate(&self) -> io::Result<Self> {
        self.try_clone()
    }

    fn shut_down(&self) {
        self.shutdown();
    }
}

/// Starts the shim: a thread that accepts guest clients on [`PROXY_ADDR`] and
/// bridges each to the host's egress port over vsock.
///
/// Nothing here is fatal. A bind failure is printed and the harness runs on
/// without egress, which is the state it was in before the shim existed.
pub fn spawn_shim() {
    match TcpListener::bind(PROXY_ADDR) {
        Ok(listener) => {
            println!("agent-harness: egress: proxy on {PROXY_ADDR} -> host CID {HOST_CID} port {EGRESS_PORT}");
            thread::spawn(move || serve(listener, || VsockStream::connect(HOST_CID, EGRESS_PORT)));
        }
        Err(err) => println!("agent-harness: egress: cannot listen on {PROXY_ADDR}: {err}"),
    }
}

/// Accepts clients on `listener` forever, bridging each to a fresh upstream
/// from `connect`. A failed accept or upstream connect is printed, and the loop
/// goes on.
pub fn serve<U, F>(listener: TcpListener, connect: F)
where
    U: Link,
    F: Fn() -> io::Result<U> + Send + Sync + 'static,
{
    let connect = Arc::new(connect);
    loop {
        match listener.accept() {
            Ok((client, _peer)) => {
                let connect = Arc::clone(&connect);
                thread::spawn(move || match connect() {
                    Ok(upstream) => {
                        if let Err(err) = bridge(client, upstream) {
                            println!("agent-harness: egress: cannot split a connection: {err}");
                        }
                    }
                    Err(err) => println!("agent-harness: egress: cannot reach the host: {err}"),
                });
            }
            Err(err) => {
                println!("agent-harness: egress: accept failed: {err}");
                thread::sleep(ACCEPT_RETRY);
            }
        }
    }
}

/// Bridges one client to one upstream until either side closes.
///
/// One direction runs on a thread of its own and the other on this one. When a
/// direction ends, both ends are shut down, so the other direction's blocked
/// read returns too and nothing is left half open.
fn bridge<C: Link, U: Link>(client: C, upstream: U) -> io::Result<()> {
    let client_reader = client.duplicate()?;
    let upstream_reader = upstream.duplicate()?;
    let upward = thread::spawn(move || pump(client_reader, upstream));
    pump(upstream_reader, client);
    let _ = upward.join();
    Ok(())
}

/// Copies `from` to `to` until `from` ends or a write fails, then ends the
/// connection.
fn pump<R: Link, W: Link>(mut from: R, mut to: W) {
    let mut buf = vec![0u8; PUMP_BUF];
    loop {
        match from.read(&mut buf) {
            Ok(0) => break,
            Ok(read) => {
                if to.write_all(&buf[..read]).is_err() {
                    break;
                }
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    from.shut_down();
    to.shut_down();
}

/// `fetch_url`: GETs an http(s) URL through the proxy and returns the body as
/// text, prefixed with `Fetched <url>:`.
///
/// Arguments: `url` (required, http or https). The body is read up to
/// [`FETCH_CAP`]; HTML (by content type) has its tags stripped, crudely. The
/// reply is capped to the tool-output limit, and a cut is said so, never hidden.
pub fn fetch_url(_workspace: &Workspace, args: &Value) -> Result<String, ToolError> {
    let url = http_url_arg(args)?;
    let fetched = fetch(&url, FETCH_CAP)?;
    let raw = String::from_utf8_lossy(&fetched.body);
    let text = if fetched.html { strip_tags(&raw) } else { raw.into_owned() };
    let mut reply = format!("Fetched {url}:\n{text}");
    if fetched.truncated {
        reply.push_str(&format!("\n[truncated: the body is over the {FETCH_CAP}-byte read cap]"));
    }
    Ok(cap_output(&reply, MAX_TOOL_OUTPUT))
}

/// The `url` argument of a web tool: an http or https URL, as given.
pub fn http_url_arg(args: &Value) -> Result<String, ToolError> {
    let url = args
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("\"url\" must be a string"))?;
    if url.starts_with("http://") || url.starts_with("https://") {
        Ok(url.to_string())
    } else {
        Err(invalid("\"url\" must be an http or https URL"))
    }
}

/// One GET through the proxy: the body, cut at the cap, and whether it was cut.
#[derive(Debug)]
pub struct Fetched {
    /// The body, at most the cap's bytes.
    pub body: Vec<u8>,
    /// Whether the body was longer than the cap.
    pub truncated: bool,
    /// Whether the content type says HTML.
    pub html: bool,
}

/// GETs `url` through the proxy, reading at most `cap` bytes of the body.
///
/// A non-2xx status is an error, as `ureq` reports it: the reason names the URL
/// and the status.
pub fn fetch(url: &str, cap: u64) -> Result<Fetched, ToolError> {
    let agent = agent()?;
    let response = agent.get(url).call().map_err(|err| ToolError::Unavailable {
        reason: format!("GET {url} failed: {err}"),
    })?;
    let html = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("text/html"));
    let mut reader = response.into_body().into_reader().take(cap + 1);
    let mut body = Vec::new();
    reader.read_to_end(&mut body).map_err(|err| ToolError::Unavailable {
        reason: format!("the body of {url} did not finish: {err}"),
    })?;
    let truncated = body.len() as u64 > cap;
    body.truncate(usize::try_from(cap).unwrap_or(usize::MAX));
    Ok(Fetched { body, truncated, html })
}

/// The HTTP client, configured to send every request through the shim.
fn agent() -> Result<ureq::Agent, ToolError> {
    let proxy = ureq::Proxy::new(PROXY_URL).map_err(|err| ToolError::Internal {
        reason: format!("the proxy {PROXY_URL} is not a usable proxy: {err}"),
    })?;
    let config = ureq::Agent::config_builder()
        .proxy(Some(proxy))
        .timeout_global(Some(REQUEST_TIMEOUT))
        .build();
    Ok(ureq::Agent::new_with_config(config))
}

/// Crude HTML-to-text: every tag becomes a space, and blank lines are dropped.
fn strip_tags(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                text.push(' ');
            }
            '>' if in_tag => in_tag = false,
            _ if in_tag => {}
            _ => text.push(ch),
        }
    }
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn invalid(reason: &str) -> ToolError {
    ToolError::InvalidInput {
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shim carries bytes both ways between a local client and a stand-in
    /// upstream: here a TCP echo server, where production dials the host.
    #[test]
    fn the_shim_forwards_bytes_both_ways() {
        let echo = TcpListener::bind("127.0.0.1:0").expect("an echo listener");
        let echo_addr = echo.local_addr().expect("the echo address");
        thread::spawn(move || {
            for mut stream in echo.incoming().flatten() {
                thread::spawn(move || {
                    let mut buf = [0u8; 1024];
                    while let Ok(read) = stream.read(&mut buf) {
                        if read == 0 || stream.write_all(&buf[..read]).is_err() {
                            break;
                        }
                    }
                });
            }
        });

        let shim = TcpListener::bind("127.0.0.1:0").expect("a shim listener");
        let shim_addr = shim.local_addr().expect("the shim address");
        thread::spawn(move || serve(shim, move || TcpStream::connect(echo_addr)));

        let message = b"hello through the shim";
        let mut client = TcpStream::connect(shim_addr).expect("the shim accepts");
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("a read timeout");
        client.write_all(message).expect("the request crosses the shim");
        let mut echoed = vec![0u8; message.len()];
        client.read_exact(&mut echoed).expect("the echo comes back");
        assert_eq!(echoed, message);
    }
}
