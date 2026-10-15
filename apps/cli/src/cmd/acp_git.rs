//! The relay behind `acp --repo`'s push. Git cannot verify a hub's self-signed certificate, which the
//! CLI checks by its pin, so git talks plain HTTP to a relay on 127.0.0.1 for the length of one push.
//! The relay forwards each request to the hub over the CLI's own client (its pin, its owner token)
//! and writes the answer back. Git never sees the token, and the relay answers only for the one
//! repository the push names.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use apz_http::client::Reply;

use super::acp::git;
use crate::ctx::Client;

/// The largest request body the relay takes: a push's pack. A bigger one is refused with 413.
pub const MAX_BODY: usize = 256 << 20;
/// The longest request or header line the relay reads.
const MAX_LINE: usize = 8 << 10;
/// The most header lines a request may carry.
const MAX_HEADERS: usize = 100;
/// How long a connection may stay silent before the relay drops it.
const IDLE: Duration = Duration::from_secs(60);
/// How often the listener checks the stop flag while it waits for a connection.
const POLL: Duration = Duration::from_millis(10);
/// The request headers the relay forwards: what git's smart HTTP needs. Authorization is never
/// taken from git; the relay's own client adds the owner token.
const FORWARDED: [&str; 4] = ["Content-Type", "Content-Encoding", "Accept", "Git-Protocol"];
const TEXT: &str = "text/plain; charset=utf-8";

/// One request as the relay forwards it to the hub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forward {
    pub method: String,
    /// The path and query, as git sent them.
    pub path: String,
    pub headers: Vec<(String, String)>,
    /// The body, with any chunked framing taken off.
    pub body: Vec<u8>,
}

/// Answers one forwarded request: the CLI's hub client in production, a closure in the tests.
pub type Upstream = Arc<dyn Fn(&Forward) -> io::Result<Reply> + Send + Sync>;

/// A relay for one plaza repository, listening on 127.0.0.1 until it is dropped.
pub struct Relay {
    port: u16,
    /// The path prefix the relay answers under. Only git is told it, so another local process that
    /// finds the port cannot borrow the owner token while the push runs.
    secret: String,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

impl Relay {
    /// Listens on an ephemeral loopback port. Each connection carries one request.
    pub fn start(repo: &str, upstream: Upstream) -> io::Result<Relay> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let secret = secret();
        let stop = Arc::new(AtomicBool::new(false));
        let accept = {
            let (stop, repo, key) = (stop.clone(), repo.to_string(), secret.clone());
            thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let (repo, key, upstream) =
                                (repo.clone(), key.clone(), upstream.clone());
                            thread::spawn(move || serve(stream, &repo, &key, &upstream));
                        }
                        Err(_) => thread::sleep(POLL),
                    }
                }
            })
        };
        Ok(Relay {
            port,
            secret,
            stop,
            accept: Some(accept),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The URL git pushes to for repository `name`: the loopback port and the secret prefix.
    pub fn url(&self, name: &str) -> String {
        format!(
            "http://127.0.0.1:{}/{}/v1/git/smart/{name}",
            self.port, self.secret
        )
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
    }
}

/// Pushes `refspec` from the repository at `top` to the plaza repository `name`, through a relay
/// that holds the client's pinned connection and owner token. Git gets only the loopback URL, and
/// the relay is up exactly as long as git runs.
pub fn push(client: &Client, top: &Path, name: &str, refspec: &str) -> Result<(), String> {
    let target = client.target.clone();
    let timeout = client.timeout;
    let upstream: Upstream = Arc::new(move |forward: &Forward| {
        let mut target = target.clone();
        target.headers = forward.headers.clone();
        let body = (forward.method == "POST").then_some(forward.body.as_slice());
        apz_http::client::request(&target, &forward.method, &forward.path, body, timeout)
    });
    let relay =
        Relay::start(name, upstream).map_err(|e| format!("cannot start the local relay: {e}"))?;
    let url = relay.url(name);
    let no_proxy = no_proxy();
    let env = [
        ("NO_PROXY", no_proxy.as_str()),
        ("no_proxy", no_proxy.as_str()),
    ];
    git(top, &["push", "--force", "--quiet", &url, refspec], &env).map(|_| ())
}

/// libcurl skips a proxy for the hosts in `no_proxy`. A proxy from the environment must never see
/// the loopback relay, so the list gets 127.0.0.1 appended.
fn no_proxy() -> String {
    let existing = std::env::var("no_proxy")
        .or_else(|_| std::env::var("NO_PROXY"))
        .unwrap_or_default();
    match existing.trim() {
        "" => "127.0.0.1".to_string(),
        list => format!("{list},127.0.0.1"),
    }
}

/// One connection: one request, one answer, then the connection closes (git reconnects as it needs).
fn serve(stream: TcpStream, repo: &str, secret: &str, upstream: &Upstream) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(IDLE));
    let _ = stream.set_write_timeout(Some(IDLE));
    let Ok(mut out) = stream.try_clone() else {
        return;
    };
    let mut input = BufReader::new(stream);
    let _ = handle(&mut input, &mut out, repo, secret, &**upstream);
}

/// Reads one request, forwards it when its path is under the `secret` prefix and inside `repo`
/// (the prefix is taken off before forwarding), and writes the answer to `out`.
fn handle(
    input: &mut impl BufRead,
    out: &mut impl Write,
    repo: &str,
    secret: &str,
    upstream: &dyn Fn(&Forward) -> io::Result<Reply>,
) -> io::Result<()> {
    let Ok((method, path, headers)) = read_head(input) else {
        return refuse(out, 400);
    };
    let Some(path) = unprefixed(&path, secret) else {
        return refuse(out, 404);
    };
    if !inside(&path, repo) {
        return refuse(out, 404);
    }
    if method != "GET" && method != "POST" {
        return refuse(out, 405);
    }
    if header(&headers, "Expect").is_some_and(|value| value.eq_ignore_ascii_case("100-continue")) {
        out.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
        out.flush()?;
    }
    let body = match read_body(input, &headers) {
        Ok(body) => body,
        Err(status) => return refuse(out, status),
    };
    let forward = Forward {
        method,
        path,
        headers: headers
            .into_iter()
            .filter(|(name, _)| FORWARDED.iter().any(|kept| kept.eq_ignore_ascii_case(name)))
            .collect(),
        body,
    };
    match upstream(&forward) {
        Ok(reply) => {
            let content_type = if reply.content_type.is_empty() {
                "application/octet-stream"
            } else {
                reply.content_type.as_str()
            };
            respond(out, reply.status, content_type, &reply.body)
        }
        Err(err) => respond(
            out,
            502,
            TEXT,
            format!("the hub did not answer: {err}\n").as_bytes(),
        ),
    }
}

/// `path` without the relay's secret prefix (`/<secret>/v1/...` → `/v1/...`), or None when it does
/// not carry it. An empty secret takes every path as it is (the tests' relay).
fn unprefixed(path: &str, secret: &str) -> Option<String> {
    if secret.is_empty() {
        return Some(path.to_string());
    }
    path.strip_prefix('/')
        .and_then(|rest| rest.strip_prefix(secret))
        .filter(|rest| rest.starts_with('/'))
        .map(str::to_string)
}

/// 128 unguessable bits as hex: two hashes from the standard library's randomly keyed hasher
/// (seeded from the operating system), of the time and a counter.
fn secret() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::BuildHasher;
    use std::sync::atomic::AtomicU64;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let a = RandomState::new().hash_one((at, n, 1u8));
    let b = RandomState::new().hash_one((at, n, 2u8));
    format!("{a:016x}{b:016x}")
}

/// Whether `path` is inside the repository `repo`: `/v1/git/smart/<repo>/...`. The name must end at
/// the slash, and no segment may climb (`.`, `..`), escape (`%`, `\`) or carry a control character.
fn inside(path: &str, repo: &str) -> bool {
    let Some(rest) = path
        .strip_prefix("/v1/git/smart/")
        .and_then(|rest| rest.strip_prefix(repo))
        .and_then(|rest| rest.strip_prefix('/'))
    else {
        return false;
    };
    !rest.is_empty()
        && !rest.contains(['%', '\\'])
        && !rest.chars().any(char::is_control)
        && rest
            .split('/')
            .all(|segment| segment != "." && segment != "..")
}

/// The request line and the headers, names as git sent them.
fn read_head(input: &mut impl BufRead) -> io::Result<(String, String, Vec<(String, String)>)> {
    let line = read_line(input)?;
    let mut parts = line.split(' ');
    let (Some(method), Some(path), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(invalid("the request line"));
    };
    if !version.starts_with("HTTP/1.") {
        return Err(invalid("the HTTP version"));
    }
    let mut headers = Vec::new();
    loop {
        let line = read_line(input)?;
        if line.is_empty() {
            return Ok((method.to_string(), path.to_string(), headers));
        }
        if headers.len() == MAX_HEADERS {
            return Err(invalid("too many headers"));
        }
        let (name, value) = line.split_once(':').ok_or_else(|| invalid("a header"))?;
        headers.push((name.trim().to_string(), value.trim().to_string()));
    }
}

/// One line without its CRLF. A line that runs past `MAX_LINE` without its break is an error.
fn read_line(input: &mut impl BufRead) -> io::Result<String> {
    let mut line = Vec::new();
    Read::take(&mut *input, MAX_LINE as u64 + 1).read_until(b'\n', &mut line)?;
    if !line.ends_with(b"\n") {
        return Err(invalid("a line with no end"));
    }
    line.pop();
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    String::from_utf8(line).map_err(|_| invalid("a line that is not UTF-8"))
}

/// The request body: the bytes of its Content-Length, or the chunks of a chunked body. The error is
/// the status to answer with.
fn read_body(input: &mut impl BufRead, headers: &[(String, String)]) -> Result<Vec<u8>, u16> {
    if header(headers, "Transfer-Encoding")
        .is_some_and(|value| value.eq_ignore_ascii_case("chunked"))
    {
        return read_chunked(input);
    }
    let Some(length) = header(headers, "Content-Length") else {
        return Ok(Vec::new());
    };
    let length: usize = length.parse().map_err(bad)?;
    if length > MAX_BODY {
        return Err(413);
    }
    let mut body = vec![0; length];
    input.read_exact(&mut body).map_err(bad)?;
    Ok(body)
}

/// De-chunks a chunked body. A body that grows past `MAX_BODY` is refused before its data is read.
fn read_chunked(input: &mut impl BufRead) -> Result<Vec<u8>, u16> {
    let mut body = Vec::new();
    loop {
        let line = read_line(input).map_err(bad)?;
        let size = line.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size, 16).map_err(bad)?;
        if size == 0 {
            // The trailers, then the empty line that ends the body.
            while !read_line(input).map_err(bad)?.is_empty() {}
            return Ok(body);
        }
        if size > MAX_BODY - body.len() {
            return Err(413);
        }
        let start = body.len();
        body.resize(start + size, 0);
        input.read_exact(&mut body[start..]).map_err(bad)?;
        if !read_line(input).map_err(bad)?.is_empty() {
            return Err(400);
        }
    }
}

fn bad<E>(_: E) -> u16 {
    400
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("{what} cannot be read"))
}

/// Writes one answer and says the connection closes.
fn respond(out: &mut impl Write, status: u16, content_type: &str, body: &[u8]) -> io::Result<()> {
    write!(
        out,
        "HTTP/1.1 {status} {}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reason(status),
        body.len()
    )?;
    out.write_all(body)?;
    out.flush()
}

/// An answer that is only its status, as text.
fn refuse(out: &mut impl Write, status: u16) -> io::Result<()> {
    respond(
        out,
        status,
        TEXT,
        format!("{}\n", reason(status)).as_bytes(),
    )
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        422 => "Unprocessable Entity",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io::{Cursor, Read, Write};
    use std::net::TcpStream;

    use super::*;

    /// A hub answer with git's upload-pack advertisement type.
    fn answer(body: &[u8]) -> Reply {
        Reply {
            status: 200,
            content_type: "application/x-git-upload-pack-advertisement".into(),
            body: body.to_vec(),
            headers: Vec::new(),
        }
    }

    /// Runs one request through the relay's handler for the repository `acme`; returns the answer.
    fn run(request: &[u8], hub: &dyn Fn(&Forward) -> io::Result<Reply>) -> String {
        let mut out = Vec::new();
        handle(
            &mut Cursor::new(request.to_vec()),
            &mut out,
            "acme",
            "",
            hub,
        )
        .expect("the answer is written");
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn a_get_is_forwarded_with_git_s_headers_and_its_answer_comes_back() {
        let seen = RefCell::new(Vec::new());
        let hub = |forward: &Forward| -> io::Result<Reply> {
            seen.borrow_mut().push(forward.clone());
            Ok(answer(b"001e# service=git-upload-pack\n"))
        };
        let out = run(
            b"GET /v1/git/smart/acme/info/refs?service=git-upload-pack HTTP/1.1\r\nHost: 127.0.0.1:9\r\nAuthorization: Bearer stolen\r\nGit-Protocol: version=2\r\nUser-Agent: git/2\r\n\r\n",
            &hub,
        );
        assert!(
            out.starts_with(
                "HTTP/1.1 200 OK\r\nContent-Type: application/x-git-upload-pack-advertisement\r\n"
            ),
            "{out}"
        );
        assert!(out.ends_with("001e# service=git-upload-pack\n"), "{out}");
        let seen = seen.into_inner();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, "GET");
        assert_eq!(
            seen[0].path,
            "/v1/git/smart/acme/info/refs?service=git-upload-pack"
        );
        assert_eq!(
            seen[0].headers,
            vec![("Git-Protocol".to_string(), "version=2".to_string())],
            "git's own Authorization and User-Agent are not forwarded"
        );
        assert!(seen[0].body.is_empty());
    }

    #[test]
    fn a_chunked_post_is_de_chunked_and_forwarded_with_git_s_content_type() {
        let seen = RefCell::new(Vec::new());
        let hub = |forward: &Forward| -> io::Result<Reply> {
            seen.borrow_mut().push(forward.clone());
            Ok(answer(b"ok"))
        };
        let out = run(
            b"POST /v1/git/smart/acme/git-receive-pack HTTP/1.1\r\nHost: 127.0.0.1\r\nTransfer-Encoding: chunked\r\nContent-Type: application/x-git-receive-pack-request\r\n\r\n5\r\nhello\r\n6;ext=1\r\n world\r\n0\r\n\r\n",
            &hub,
        );
        assert!(out.starts_with("HTTP/1.1 200 OK"), "{out}");
        let seen = seen.into_inner();
        assert_eq!(seen[0].method, "POST");
        assert_eq!(seen[0].body, b"hello world");
        assert!(seen[0].headers.contains(&(
            "Content-Type".to_string(),
            "application/x-git-receive-pack-request".to_string()
        )));
        assert!(
            !seen[0]
                .headers
                .iter()
                .any(|(name, _)| name == "Transfer-Encoding"),
            "the body is de-chunked, so its framing is not forwarded"
        );
    }

    #[test]
    fn a_path_outside_the_repository_is_refused_and_never_forwarded() {
        let hub = |_: &Forward| -> io::Result<Reply> { panic!("a refused path reached the hub") };
        for path in [
            "/v1/git/smart/other/info/refs",
            "/v1/git/smart/acme-2/info/refs",
            "/v1/git/smart/acme/../other/info/refs",
            "/v1/git/smart/acme/%2e%2e/x",
            "/v1/git/smart/acme",
            "/v1/git/smart/acme/",
            "/v1/hub/tokens",
        ] {
            let request = format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n");
            assert!(
                run(request.as_bytes(), &hub).starts_with("HTTP/1.1 404 Not Found\r\n"),
                "{path}"
            );
        }
    }

    #[test]
    fn a_body_over_the_limit_is_refused_before_its_data_is_read() {
        let hub =
            |_: &Forward| -> io::Result<Reply> { panic!("an oversized body reached the hub") };
        let declared = format!(
            "POST /v1/git/smart/acme/git-receive-pack HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY + 1
        );
        assert!(run(declared.as_bytes(), &hub).starts_with("HTTP/1.1 413 Payload Too Large\r\n"));
        let chunked = b"POST /v1/git/smart/acme/git-receive-pack HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n10000001\r\n";
        assert!(run(chunked, &hub).starts_with("HTTP/1.1 413 Payload Too Large\r\n"));
    }

    #[test]
    fn the_secret_prefix_is_required_and_taken_off() {
        assert_eq!(
            unprefixed("/abc/v1/git/smart/acme/info/refs", "abc").as_deref(),
            Some("/v1/git/smart/acme/info/refs")
        );
        assert_eq!(unprefixed("/v1/git/smart/acme/info/refs", "abc"), None);
        assert_eq!(unprefixed("/abcd/v1/x", "abc"), None);
        assert_ne!(secret(), secret());
        assert_eq!(secret().len(), 32);
    }

    #[test]
    fn a_relay_answers_over_tcp_and_is_gone_once_dropped() {
        let hub: Upstream = Arc::new(|_: &Forward| -> io::Result<Reply> { Ok(answer(b"ok")) });
        let relay = Relay::start("acme", hub).expect("listens on loopback");
        let port = relay.port();
        let ask = |path: &str| {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connects");
            stream
                .write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
                .expect("writes");
            let mut text = String::new();
            stream
                .read_to_string(&mut text)
                .expect("the relay closes after one answer");
            text
        };
        // Only the URL git is given carries the secret prefix; without it the relay answers 404.
        let url = relay.url("acme");
        let path = url
            .strip_prefix(&format!("http://127.0.0.1:{port}"))
            .expect("a loopback URL");
        let text = ask(&format!("{path}/info/refs"));
        assert!(
            text.starts_with("HTTP/1.1 200 OK") && text.ends_with("ok"),
            "{text}"
        );
        assert!(ask("/v1/git/smart/acme/info/refs").starts_with("HTTP/1.1 404"));
        drop(relay);
        assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    }
}
