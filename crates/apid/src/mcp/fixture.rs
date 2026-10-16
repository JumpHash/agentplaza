#![allow(dead_code)]
//! A small Model Context Protocol server used as a test fixture. It speaks
//! newline-delimited JSON-RPC 2.0 over stdio and a minimal HTTP/1.1 subset
//! (POST /mcp for JSON, POST /mcp/sse for an event stream). std only.

use std::io::{BufRead, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

const SESSION: &str = "fixture-session-1";
const PAGE: usize = 3;
const MAX_HEAD: usize = 64 * 1024;
const DEFAULT_VERSION: &str = "2025-06-18";

/// A 1x1 RGBA PNG (signature and CRCs verified).
const PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(fields) => fields
                .iter()
                .find(|(k, _)| k.as_str() == key)
                .map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }
}

/// Parses one JSON document. Trailing non-whitespace is an error.
pub fn parse(text: &str) -> Result<Json, String> {
    let mut p = Parser {
        s: text.as_bytes(),
        i: 0,
    };
    let v = p.value()?;
    p.ws();
    if p.i != p.s.len() {
        return Err(p.err("trailing data"));
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn err(&self, what: &str) -> String {
        format!("{what} at byte {}", self.i)
    }

    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\r' | b'\n') {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn literal(&mut self, lit: &str, v: Json) -> Result<Json, String> {
        if self.s[self.i..].starts_with(lit.as_bytes()) {
            self.i += lit.len();
            Ok(v)
        } else {
            Err(self.err("bad literal"))
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.ws();
        match self.peek() {
            None => Err(self.err("unexpected end")),
            Some(b'n') => self.literal("null", Json::Null),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.peek() == Some(b']') {
                    self.i += 1;
                    return Ok(Json::Arr(items));
                }
                loop {
                    items.push(self.value()?);
                    self.ws();
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Json::Arr(items));
                        }
                        _ => return Err(self.err("expected , or ]")),
                    }
                }
            }
            Some(b'{') => {
                self.i += 1;
                let mut fields = Vec::new();
                self.ws();
                if self.peek() == Some(b'}') {
                    self.i += 1;
                    return Ok(Json::Obj(fields));
                }
                loop {
                    self.ws();
                    if self.peek() != Some(b'"') {
                        return Err(self.err("expected key"));
                    }
                    let key = self.string()?;
                    self.ws();
                    if self.peek() != Some(b':') {
                        return Err(self.err("expected :"));
                    }
                    self.i += 1;
                    let v = self.value()?;
                    fields.push((key, v));
                    self.ws();
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Json::Obj(fields));
                        }
                        _ => return Err(self.err("expected , or }")),
                    }
                }
            }
            Some(_) => Err(self.err("unexpected character")),
        }
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.i;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.' | b'e' | b'E') {
                self.i += 1;
            } else {
                break;
            }
        }
        let text = std::str::from_utf8(&self.s[start..self.i]).map_err(|e| e.to_string())?;
        text.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| format!("bad number {text}"))
    }

    fn hex4(&mut self) -> Result<u32, String> {
        if self.i + 4 > self.s.len() {
            return Err(self.err("short \\u escape"));
        }
        let h = std::str::from_utf8(&self.s[self.i..self.i + 4]).map_err(|e| e.to_string())?;
        let v = u32::from_str_radix(h, 16).map_err(|_| format!("bad \\u escape {h}"))?;
        self.i += 4;
        Ok(v)
    }

    /// Parses a string starting at the opening quote.
    fn string(&mut self) -> Result<String, String> {
        self.i += 1;
        let mut out = String::new();
        loop {
            let start = self.i;
            while self.i < self.s.len() && self.s[self.i] != b'"' && self.s[self.i] != b'\\' {
                self.i += 1;
            }
            out.push_str(std::str::from_utf8(&self.s[start..self.i]).map_err(|e| e.to_string())?);
            match self.peek() {
                None => return Err(self.err("unterminated string")),
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(_) => {} // a backslash: handled below
            }
            self.i += 1;
            let Some(c) = self.peek() else {
                return Err(self.err("unterminated escape"));
            };
            self.i += 1;
            match c {
                b'"' => out.push('"'),
                b'\\' => out.push('\\'),
                b'/' => out.push('/'),
                b'b' => out.push('\u{8}'),
                b'f' => out.push('\u{c}'),
                b'n' => out.push('\n'),
                b'r' => out.push('\r'),
                b't' => out.push('\t'),
                b'u' => {
                    let mut cp = self.hex4()?;
                    if (0xD800..0xDC00).contains(&cp) {
                        if !self.s[self.i..].starts_with(b"\\u") {
                            return Err(self.err("lone surrogate"));
                        }
                        self.i += 2;
                        let lo = self.hex4()?;
                        if !(0xDC00..0xE000).contains(&lo) {
                            return Err(self.err("bad low surrogate"));
                        }
                        cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                    }
                    let ch = char::from_u32(cp).ok_or_else(|| self.err("bad code point"))?;
                    out.push(ch);
                }
                _ => return Err(self.err("bad escape")),
            }
        }
    }
}

/// Serializes a value as compact JSON.
pub fn dump(v: &Json) -> String {
    let mut out = String::new();
    write_json(v, &mut out);
    out
}

fn write_json(v: &Json, out: &mut String) {
    match v {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Num(n) => {
            if !n.is_finite() {
                out.push_str("null");
            } else if n.fract() == 0.0 && n.abs() < 9.0e15 {
                out.push_str(&format!("{}", *n as i64));
            } else {
                out.push_str(&format!("{n}"));
            }
        }
        Json::Str(s) => write_str(s, out),
        Json::Arr(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json(item, out);
            }
            out.push(']');
        }
        Json::Obj(fields) => {
            out.push('{');
            for (i, (k, val)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_str(k, out);
                out.push(':');
                write_json(val, out);
            }
            out.push('}');
        }
    }
}

fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn obj(fields: Vec<(&str, Json)>) -> Json {
    Json::Obj(
        fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}

fn s(text: &str) -> Json {
    Json::Str(text.to_string())
}

fn num(n: i64) -> Json {
    Json::Num(n as f64)
}

fn ty(name: &str) -> Json {
    obj(vec![("type", s(name))])
}

fn schema(props: Vec<(&str, Json)>, required: &[&str]) -> Json {
    let mut fields = vec![("type", s("object")), ("properties", obj(props))];
    if !required.is_empty() {
        fields.push((
            "required",
            Json::Arr(required.iter().map(|r| s(r)).collect()),
        ));
    }
    obj(fields)
}

fn read_only() -> Json {
    obj(vec![("readOnlyHint", Json::Bool(true))])
}

fn text_result(text: impl Into<String>, is_error: bool) -> Json {
    obj(vec![
        (
            "content",
            Json::Arr(vec![obj(vec![
                ("type", s("text")),
                ("text", Json::Str(text.into())),
            ])]),
        ),
        ("isError", Json::Bool(is_error)),
    ])
}

fn reply(id: &Json, result: Json) -> Json {
    obj(vec![
        ("jsonrpc", s("2.0")),
        ("id", id.clone()),
        ("result", result),
    ])
}

fn error(id: &Json, code: i64, message: &str) -> Json {
    obj(vec![
        ("jsonrpc", s("2.0")),
        ("id", id.clone()),
        (
            "error",
            obj(vec![("code", num(code)), ("message", s(message))]),
        ),
    ])
}

/// Request headers (names stored lowercase). `None` means stdio.
pub struct Ctx<'a> {
    pub headers: Option<&'a [(String, String)]>,
}

impl Ctx<'_> {
    pub fn header(&self, name: &str) -> Option<&str> {
        find_header(self.headers?, name)
    }
}

fn find_header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    let name = name.to_ascii_lowercase();
    headers
        .iter()
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.as_str())
}

fn tool(name: &str, description: &str, input_schema: Json, annotations: Json) -> Json {
    obj(vec![
        ("name", s(name)),
        ("description", s(description)),
        ("inputSchema", input_schema),
        ("annotations", annotations),
    ])
}

/// An object schema with one required property.
fn one_arg(key: &str, t: &str) -> Json {
    schema(vec![(key, ty(t))], &[key])
}

fn tool_defs() -> Vec<Json> {
    let no_args = || schema(vec![], &[]);
    let destructive = || obj(vec![("destructiveHint", Json::Bool(true))]);
    vec![
        tool(
            "echo",
            "Echoes its text argument.",
            one_arg("text", "string"),
            read_only(),
        ),
        tool("fail", "Always fails.", no_args(), obj(vec![])),
        tool(
            "slow",
            "Sleeps ms milliseconds, then returns.",
            one_arg("ms", "integer"),
            read_only(),
        ),
        tool(
            "big",
            "Returns bytes characters of text.",
            one_arg("bytes", "integer"),
            read_only(),
        ),
        tool(
            "leak_env",
            "Returns the value of the environment variable name.",
            one_arg("name", "string"),
            read_only(),
        ),
        tool(
            "leak_header",
            "Returns the value of the HTTP request header name (HTTP transport only; over stdio it returns an error result).",
            one_arg("name", "string"),
            read_only(),
        ),
        tool("image", "Returns a tiny PNG image.", no_args(), read_only()),
        tool(
            "crash",
            "Exits the process with status 3 (stdio only; over HTTP it returns an error result).",
            no_args(),
            destructive(),
        ),
        tool(
            "delete_all",
            "Pretends to delete everything. Irreversible.",
            no_args(),
            obj(vec![
                ("destructiveHint", Json::Bool(true)),
                ("readOnlyHint", Json::Bool(false)),
            ]),
        ),
    ]
}

fn tools_list(params: &Json) -> Json {
    let all = tool_defs();
    let start = params
        .get("cursor")
        .and_then(Json::as_str)
        .and_then(|c| c.parse::<usize>().ok())
        .unwrap_or(0)
        .min(all.len());
    let end = (start + PAGE).min(all.len());
    let mut fields = vec![("tools".to_string(), Json::Arr(all[start..end].to_vec()))];
    if end < all.len() {
        fields.push(("nextCursor".to_string(), s(&end.to_string())));
    }
    Json::Obj(fields)
}

fn initialize(params: &Json) -> Json {
    let requested = params
        .get("protocolVersion")
        .and_then(Json::as_str)
        .unwrap_or("");
    let version = if requested == "2025-06-18" || requested == "2025-03-26" {
        requested
    } else {
        DEFAULT_VERSION
    };
    obj(vec![
        ("protocolVersion", s(version)),
        (
            "capabilities",
            obj(vec![
                ("tools", obj(vec![])),
                ("resources", obj(vec![])),
                ("prompts", obj(vec![])),
            ]),
        ),
        (
            "serverInfo",
            obj(vec![("name", s("fixture")), ("version", s("1.0.0"))]),
        ),
    ])
}

fn str_arg(args: &Json, key: &str) -> Result<String, Json> {
    match args.get(key).and_then(Json::as_str) {
        Some(v) => Ok(v.to_string()),
        None => Err(text_result(format!("missing string argument {key}"), true)),
    }
}

fn int_arg(args: &Json, key: &str) -> Result<i64, Json> {
    match args.get(key).and_then(Json::as_f64) {
        Some(n) => Ok(n as i64),
        None => Err(text_result(format!("missing integer argument {key}"), true)),
    }
}

fn echo(args: &Json) -> Json {
    match str_arg(args, "text") {
        Ok(text) => text_result(text, false),
        Err(e) => e,
    }
}

fn slow(args: &Json) -> Json {
    match int_arg(args, "ms") {
        Ok(ms) => {
            std::thread::sleep(Duration::from_millis(ms.max(0) as u64));
            text_result("slept", false)
        }
        Err(e) => e,
    }
}

fn big(args: &Json) -> Json {
    match int_arg(args, "bytes") {
        Ok(n) => text_result("x".repeat(n.max(0) as usize), false),
        Err(e) => e,
    }
}

fn leak_env(args: &Json) -> Json {
    match str_arg(args, "name") {
        Ok(name) => text_result(std::env::var(&name).unwrap_or_default(), false),
        Err(e) => e,
    }
}

fn leak_header(args: &Json, ctx: &Ctx<'_>) -> Json {
    if ctx.headers.is_none() {
        return text_result("no headers over stdio", true);
    }
    match str_arg(args, "name") {
        Ok(name) => text_result(ctx.header(&name).unwrap_or("").to_string(), false),
        Err(e) => e,
    }
}

fn image() -> Json {
    obj(vec![
        (
            "content",
            Json::Arr(vec![obj(vec![
                ("type", s("image")),
                ("mimeType", s("image/png")),
                ("data", s(PNG_B64)),
            ])]),
        ),
        ("isError", Json::Bool(false)),
    ])
}

fn crash(ctx: &Ctx<'_>) -> Json {
    if ctx.headers.is_some() {
        return text_result("no crash over http", true);
    }
    let _ = std::io::stdout().flush();
    std::process::exit(3)
}

/// Runs a tool. `None` means the tool name is unknown.
pub fn call_tool(name: &str, args: &Json, ctx: &Ctx<'_>) -> Option<Json> {
    Some(match name {
        "echo" => echo(args),
        "fail" => text_result("fixture failure", true),
        "slow" => slow(args),
        "big" => big(args),
        "leak_env" => leak_env(args),
        "leak_header" => leak_header(args, ctx),
        "image" => image(),
        "crash" => crash(ctx),
        "delete_all" => text_result("deleted", false),
        _ => return None,
    })
}

/// Handles one parsed JSON-RPC message. Returns `None` for notifications
/// (messages without an "id"), which get no reply.
pub fn handle(msg: &Json, ctx: &Ctx<'_>) -> Option<Json> {
    if !matches!(msg, Json::Obj(_)) {
        return Some(error(&Json::Null, -32600, "invalid request"));
    }
    let id = msg.get("id")?;
    let empty = Json::Obj(Vec::new());
    let params = msg.get("params").unwrap_or(&empty);
    let method = msg.get("method").and_then(Json::as_str).unwrap_or("");
    let result = match method {
        "initialize" => initialize(params),
        "ping" => Json::Obj(Vec::new()),
        "tools/list" => tools_list(params),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Json::as_str) else {
                return Some(error(id, -32602, "missing tool name"));
            };
            let args = params.get("arguments").unwrap_or(&empty);
            match call_tool(name, args, ctx) {
                Some(result) => result,
                None => return Some(error(id, -32602, &format!("unknown tool: {name}"))),
            }
        }
        "resources/list" => obj(vec![(
            "resources",
            Json::Arr(vec![obj(vec![
                ("uri", s("fixture://readme")),
                ("name", s("readme")),
                ("mimeType", s("text/plain")),
            ])]),
        )]),
        "prompts/list" => obj(vec![(
            "prompts",
            Json::Arr(vec![obj(vec![
                ("name", s("greet")),
                ("description", s("says hello")),
            ])]),
        )]),
        _ => return Some(error(id, -32601, "method not found")),
    };
    Some(reply(id, result))
}

/// Exits with status 1 or hangs forever when FIXTURE_MODE asks for it.
pub fn mode_exit_at_start() {
    match std::env::var("FIXTURE_MODE").as_deref() {
        Ok("exit_at_start") => std::process::exit(1),
        Ok("hang_at_start") => loop {
            std::thread::sleep(Duration::from_secs(3600));
        },
        _ => {}
    }
}

/// Serves newline-delimited JSON-RPC on stdin/stdout until stdin closes.
pub fn serve_stdio() {
    let stdin = std::io::stdin();
    let ctx = Ctx { headers: None };
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match parse(&line) {
            Ok(msg) => handle(&msg, &ctx),
            Err(e) => Some(error(&Json::Null, -32700, &format!("parse error: {e}"))),
        };
        if let Some(reply) = reply {
            let mut out = dump(&reply);
            out.push('\n');
            let stdout = std::io::stdout();
            let mut lock = stdout.lock();
            let _ = lock.write_all(out.as_bytes());
            let _ = lock.flush();
        }
    }
}

struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Resp {
    fn status(status: u16) -> Resp {
        Resp {
            status,
            headers: Vec::new(),
            body: String::new(),
        }
    }

    fn json(status: u16, v: &Json) -> Resp {
        let mut r = Resp::status(status);
        r.headers
            .push(("Content-Type".to_string(), "application/json".to_string()));
        r.body = dump(v);
        r
    }

    fn event(v: &Json) -> Resp {
        let mut r = Resp::status(200);
        r.headers
            .push(("Content-Type".to_string(), "text/event-stream".to_string()));
        r.body = format!("event: message\ndata: {}\n\n", dump(v));
        r
    }
}

fn route(method: &str, path: &str, headers: &[(String, String)], body: &[u8]) -> Resp {
    let sse = match path {
        "/mcp" => false,
        "/mcp/sse" => true,
        _ => return Resp::status(404),
    };
    if method != "POST" {
        return Resp::status(405);
    }
    let text = String::from_utf8_lossy(body);
    let msg = match parse(&text) {
        Ok(m) => m,
        Err(e) => {
            return Resp::json(
                400,
                &error(&Json::Null, -32700, &format!("parse error: {e}")),
            );
        }
    };
    let ctx = Ctx {
        headers: Some(headers),
    };
    let is_init = msg.get("method").and_then(Json::as_str) == Some("initialize");
    if msg.get("id").is_some() && !is_init && ctx.header("mcp-session-id") != Some(SESSION) {
        return Resp::status(404);
    }
    let Some(reply) = handle(&msg, &ctx) else {
        return Resp::status(202);
    };
    let mut resp = if sse {
        Resp::event(&reply)
    } else {
        Resp::json(200, &reply)
    };
    if is_init {
        resp.headers
            .push(("Mcp-Session-Id".to_string(), SESSION.to_string()));
    }
    resp
}

fn write_response(stream: &mut TcpStream, r: &Resp) -> std::io::Result<()> {
    let reason = match r.status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        431 => "Request Header Fields Too Large",
        _ => "Error",
    };
    let mut head = format!("HTTP/1.1 {} {}\r\n", r.status, reason);
    for (k, v) in &r.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        r.body.len()
    ));
    stream.write_all(head.as_bytes())?;
    stream.write_all(r.body.as_bytes())?;
    stream.flush()
}

fn handle_conn(mut stream: TcpStream) -> std::io::Result<()> {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > MAX_HEAD {
            return write_response(&mut stream, &Resp::status(431));
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
    };

    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut parts = lines.next().unwrap_or("").split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();

    let body_start = head_end + 4;
    let content_length: usize = find_header(&headers, "content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if buf.len() < body_start + content_length {
        if find_header(&headers, "expect").is_some_and(|v| v.eq_ignore_ascii_case("100-continue")) {
            stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
        }
        while buf.len() < body_start + content_length {
            let n = stream.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
    }
    let body_end = (body_start + content_length).min(buf.len());
    let resp = route(&method, &path, &headers, &buf[body_start..body_end]);
    write_response(&mut stream, &resp)
}

/// Accepts connections forever, one thread per connection.
pub fn serve_http(listener: TcpListener) {
    for stream in listener.incoming().flatten() {
        let _ = std::thread::spawn(move || {
            let _ = handle_conn(stream);
        });
    }
}
