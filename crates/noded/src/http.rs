//! The smallest HTTP/1.1 the node daemon needs: a request-line and header
//! parser, one request per connection, JSON responses.
//!
//! The daemon binds `127.0.0.1` only and serves a handful of JSON endpoints
//! plus the console's static files, so this is deliberately not a general HTTP
//! implementation: no keep-alive, no pipelining, no chunked bodies. What it
//! does have to do is refuse everything it does not understand with a 400 and
//! never panic on a malformed request, because the socket is the daemon's
//! whole attack surface.

use std::io::{BufRead, Read, Write};

/// The most a request line and its headers may occupy, together.
pub const MAX_HEAD: usize = 16 * 1024;

/// The most a request body may occupy.
pub const MAX_BODY: usize = 1024 * 1024;

/// Why a request could not be read. Every one of these is a 400.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError(&'static str);

impl ParseError {
    /// The reason, as it is reported to the client.
    pub fn message(self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for ParseError {}

/// One parsed request: the method, the request target (path plus any query)
/// and the body.
#[derive(Debug)]
pub struct Request {
    method: String,
    target: String,
    body: Vec<u8>,
}

impl Request {
    /// The request method, as it arrived (`GET`, `POST`, ...).
    pub fn method(&self) -> &str {
        &self.method
    }

    /// The request target without its query string or fragment: what routing
    /// matches on.
    pub fn path(&self) -> &str {
        let end = self.target.find(['?', '#']).unwrap_or(self.target.len());
        &self.target[..end]
    }

    /// The request body, empty when the request had no `Content-Length`.
    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

/// Reads one request: the request line, the headers (bounded together by
/// [`MAX_HEAD`]) and the body (bounded by [`MAX_BODY`]).
///
/// Anything malformed - a line that is not ASCII, a request line that is not
/// `METHOD TARGET HTTP/1.x`, a header without a `:`, a body that does not
/// match its `Content-Length`, a chunked body, a body or header block that is
/// too large, a connection that ends mid-request - is a [`ParseError`], never
/// a panic.
pub fn read_request(reader: &mut impl BufRead) -> Result<Request, ParseError> {
    let mut line = Vec::new();
    read_line(reader, MAX_HEAD, &mut line)?;
    let request_line = std::str::from_utf8(&line)
        .map_err(|_| ParseError("the request line is not ASCII"))?;
    let mut parts = request_line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(ParseError("the request line must be `METHOD TARGET HTTP/1.x`"));
    };
    if method.is_empty() || !method.bytes().all(is_token_byte) {
        return Err(ParseError("the method is not a token"));
    }
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(ParseError("only HTTP/1.0 and HTTP/1.1 are served"));
    }
    // Origin-form only: no absolute-form (`http://host/path`) and no bytes a
    // response header could not carry.
    if !target.starts_with('/') || !target.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) {
        return Err(ParseError("the request target must be an origin-form path"));
    }
    // The line buffer is reused for the headers, so the request line's parts
    // are taken before it is.
    let method = method.to_string();
    let target = target.to_string();

    let mut head_bytes = line.len() + 2;
    let mut content_length = None;
    loop {
        read_line(reader, MAX_HEAD, &mut line)?;
        head_bytes += line.len() + 2;
        if head_bytes > MAX_HEAD {
            return Err(ParseError("the request headers are too large"));
        }
        if line.is_empty() {
            break;
        }
        let header =
            std::str::from_utf8(&line).map_err(|_| ParseError("a request header is not ASCII"))?;
        let Some((name, value)) = header.split_once(':') else {
            return Err(ParseError("a request header has no `:`"));
        };
        let value = value.trim_matches([' ', '\t']);
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(ParseError("the request has more than one Content-Length"));
            }
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(ParseError("Content-Length is not a number"));
            }
            let length = value
                .parse::<usize>()
                .map_err(|_| ParseError("Content-Length is too large"))?;
            if length > MAX_BODY {
                return Err(ParseError("the request body is too large"));
            }
            content_length = Some(length);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            // Refusing chunked bodies outright keeps a request from having two
            // readings of its own length.
            return Err(ParseError("chunked request bodies are not supported"));
        }
    }

    let mut body = Vec::new();
    if let Some(length) = content_length {
        (&mut *reader)
            .take(length as u64)
            .read_to_end(&mut body)
            .map_err(|_| ParseError("the request body could not be read"))?;
        if body.len() != length {
            return Err(ParseError("the request body is shorter than Content-Length"));
        }
    }
    Ok(Request {
        method,
        target,
        body,
    })
}

/// Reads one `\n`-terminated line, without its `\r\n` or `\n`, into `line`.
///
/// A line longer than `limit`, or a connection that ends before the newline,
/// is an error: the request cannot be read as sent.
fn read_line(reader: &mut impl BufRead, limit: usize, line: &mut Vec<u8>) -> Result<(), ParseError> {
    line.clear();
    let read = (&mut *reader)
        .take(limit as u64 + 1)
        .read_until(b'\n', line)
        .map_err(|_| ParseError("the request could not be read"))?;
    if read == 0 || line.last() != Some(&b'\n') {
        return Err(ParseError("the request ended before it was complete"));
    }
    if line.len() > limit {
        return Err(ParseError("a request line or header is too long"));
    }
    line.pop();
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Ok(())
}

/// Whether `byte` may appear in a method token (RFC 9110 `tchar`).
fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

/// A response: a status, a content type and a body.
#[derive(Debug)]
pub struct Response {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

impl Response {
    /// A JSON response with the given status.
    pub fn json(status: u16, body: String) -> Self {
        Self {
            status,
            content_type: "application/json; charset=utf-8",
            body: body.into_bytes(),
        }
    }

    /// A JSON error object, `{"error": "..."}`.
    pub fn error(status: u16, message: &str) -> Self {
        Self::json(status, serde_json::json!({ "error": message }).to_string())
    }

    /// A 200 with `content_type` and `body`: a static file.
    pub fn file(content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            status: 200,
            content_type,
            body,
        }
    }

    /// The response's status code.
    pub fn status(&self) -> u16 {
        self.status
    }

    /// Writes the response head and body. The connection is closed afterwards
    /// by the caller, which is what the `Connection: close` says.
    pub fn write_to(&self, writer: &mut impl Write) -> std::io::Result<()> {
        let head = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
            self.status,
            reason(self.status),
            self.content_type,
            self.body.len(),
        );
        writer.write_all(head.as_bytes())?;
        writer.write_all(&self.body)?;
        writer.flush()
    }
}

/// The reason phrase for a status code.
fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        500 => "Internal Server Error",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn read(raw: &str) -> Result<Request, ParseError> {
        read_request(&mut Cursor::new(raw.as_bytes()))
    }

    #[test]
    fn a_get_request_is_read() {
        let request = read("GET /api/v0/runs HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .expect("a well-formed GET");
        assert_eq!(request.method(), "GET");
        assert_eq!(request.path(), "/api/v0/runs");
        assert!(request.body().is_empty());
    }

    #[test]
    fn a_post_body_is_read_and_the_query_is_dropped() {
        let request = read(
            "POST /api/v0/runs?debug=1 HTTP/1.1\r\nHost: localhost\r\n\
             Content-Type: application/json\r\nContent-Length: 13\r\n\r\n{\"n\":1,\"x\":2}",
        )
        .expect("a well-formed POST");
        assert_eq!(request.method(), "POST");
        assert_eq!(request.path(), "/api/v0/runs");
        assert_eq!(request.body(), b"{\"n\":1,\"x\":2}");
    }

    #[test]
    fn header_names_are_case_insensitive() {
        let request = read("POST /x HTTP/1.0\r\ncontent-length: 2\r\n\r\nhi")
            .expect("a well-formed POST");
        assert_eq!(request.body(), b"hi");
    }

    #[test]
    fn malformed_requests_are_refused() {
        let bad = [
            "",
            "\r\n",
            "GET\r\n\r\n",
            "GET /api/v0/runs\r\n\r\n",
            "GET /api/v0/runs HTTP/2\r\n\r\n",
            "GET  /api/v0/runs HTTP/1.1\r\n\r\n",
            "GET api/v0/runs HTTP/1.1\r\n\r\n",
            "G ET /api/v0/runs HTTP/1.1\r\n\r\n",
            "GET /a\tb HTTP/1.1\r\n\r\n",
            "GET /api/v0/runs HTTP/1.1\r\nno-colon\r\n\r\n",
            "POST /api/v0/runs HTTP/1.1\r\nContent-Length: two\r\n\r\n",
            "POST /api/v0/runs HTTP/1.1\r\nContent-Length: +5\r\n\r\n12345",
            "POST /api/v0/runs HTTP/1.1\r\nContent-Length: 5\r\nContent-Length: 5\r\n\r\n12345",
            "POST /api/v0/runs HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
            "POST /api/v0/runs HTTP/1.1\r\nContent-Length: 9\r\n\r\nshort",
            "GET /api/v0/runs HTTP/1.1\r\nHost: localhost\r\n",
        ];
        for raw in bad {
            assert!(read(raw).is_err(), "should have been refused: {raw:?}");
        }
    }

    #[test]
    fn an_oversized_head_or_body_is_refused() {
        let long_header = format!(
            "GET / HTTP/1.1\r\nHost: localhost\r\nX: {}\r\n\r\n",
            "a".repeat(MAX_HEAD)
        );
        assert!(read(&long_header).is_err());

        let huge_body = format!(
            "POST /api/v0/runs HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY + 1
        );
        assert!(read(&huge_body).is_err());
    }

    #[test]
    fn non_ascii_bytes_are_refused() {
        let mut raw = b"GET /api/v0/runs HTTP/1.1\r\nX: ".to_vec();
        raw.push(0xff);
        raw.extend_from_slice(b"\r\n\r\n");
        assert!(read_request(&mut Cursor::new(&raw)).is_err());
    }

    #[test]
    fn an_error_response_is_a_json_object() {
        let response = Response::error(400, "say \"what\"");
        assert_eq!(response.status(), 400);
        assert_eq!(
            std::str::from_utf8(&response.body).expect("JSON is UTF-8"),
            "{\"error\":\"say \\\"what\\\"\"}"
        );
    }

    #[test]
    fn a_response_carries_its_length_and_closes() {
        let response = Response::json(201, "{\"id\":\"r1\"}".to_string());
        let mut written = Vec::new();
        response.write_to(&mut written).expect("writing to a Vec");
        let written = String::from_utf8(written).expect("the head is ASCII");
        assert!(written.starts_with("HTTP/1.1 201 Created\r\n"), "{written}");
        assert!(written.contains("Content-Length: 11\r\n"), "{written}");
        assert!(written.contains("Connection: close\r\n"), "{written}");
        assert!(written.ends_with("\r\n\r\n{\"id\":\"r1\"}"), "{written}");
    }
}
