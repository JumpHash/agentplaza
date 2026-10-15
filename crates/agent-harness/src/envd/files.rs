//! `GET /files` (download) and `POST /files` (upload), E2B's `spec/envd/envd.yaml`.
//!
//! An upload is `multipart/form-data` (one file per part, named by the part's `filename`; a part
//! without one is a plain form field and is skipped) or a raw `application/octet-stream` body
//! written to `path`. Each file is written to a temporary sibling and renamed over its target, so
//! a reader never sees a half-written file. What an upload writes, and the directories it creates,
//! belong to the user the request names (default: the sandbox's). Errors are `{code, message}`,
//! with the HTTP status as the code.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

use super::filesystem::{create_dirs, lookup_user, resolve};
use super::{Ctx, Reply, Request};

/// `ENOSPC`: the filesystem has no room left, which E2B answers with 507.
const ENOSPC: i32 = 28;

/// Tells the temporary files of concurrent uploads apart.
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// Serves `GET` or `POST /files`.
pub fn handle(request: &Request, ctx: &Ctx) -> Reply {
    let query = parse_query(&request.query);
    let who = match Who::new(query.get("username").map(String::as_str), ctx) {
        Ok(who) => who,
        Err(reply) => return reply,
    };
    let path = query.get("path").map_or("", String::as_str);
    match request.method.as_str() {
        "GET" => download(path, &who),
        "POST" => upload(request, path, &who),
        _ => error(405, format!("method {} is not allowed", request.method)),
    }
}

/// The user a request acts as: their home resolves relative paths, their ids own what is written.
struct Who {
    home: PathBuf,
    uid: u32,
    gid: u32,
}

impl Who {
    fn new(username: Option<&str>, ctx: &Ctx) -> Result<Self, Reply> {
        match username.filter(|name| !name.is_empty()) {
            None => Ok(Self {
                home: ctx.home.clone(),
                uid: ctx.uid,
                gid: ctx.gid,
            }),
            Some(name) => lookup_user(name)
                .map(|user| Self {
                    home: user.home,
                    uid: user.uid,
                    gid: user.gid,
                })
                .ok_or_else(|| error(401, format!("error looking up user '{name}': unknown user"))),
        }
    }
}

/// A file's bytes. A directory, or a path that does not exist, is an error.
fn download(raw: &str, who: &Who) -> Reply {
    if raw.is_empty() {
        return error(400, "path is required");
    }
    let path = resolve(raw, &who.home);
    match fs::metadata(&path) {
        Ok(meta) if meta.is_dir() => {
            error(400, format!("path '{}' is a directory", path.display()))
        }
        Ok(_) => match fs::read(&path) {
            Ok(body) => Reply::Full {
                status: 200,
                content_type: "application/octet-stream".to_string(),
                body,
            },
            Err(err) => io_error(&err, &path),
        },
        Err(err) => io_error(&err, &path),
    }
}

/// Writes the upload and answers the `EntryInfo` of each file written.
fn upload(request: &Request, raw: &str, who: &Who) -> Reply {
    let media = request.header("content-type").unwrap_or("");
    if !media.starts_with("multipart/form-data") {
        return upload_raw(request, raw, media, who);
    }
    let Some(boundary) = boundary_of(media) else {
        return error(400, "multipart upload has no boundary");
    };
    let files: Vec<(String, Vec<u8>)> = match parse_multipart(&request.body, &boundary) {
        Ok(parts) => parts
            .into_iter()
            .filter_map(|part| Some((part.file_name?, part.data)))
            .collect(),
        Err(message) => return error(400, message),
    };
    if files.is_empty() {
        return error(400, "the upload has no file");
    }
    // An empty path, a trailing slash, or an existing directory takes the files into it.
    let target = if raw.is_empty() {
        who.home.clone()
    } else {
        resolve(raw, &who.home)
    };
    let into_dir = raw.is_empty() || raw.ends_with('/') || target.is_dir();
    if !into_dir && files.len() > 1 {
        return error(
            400,
            format!(
                "path '{}' is not a directory, so one file goes there",
                target.display()
            ),
        );
    }
    let mut written = Vec::with_capacity(files.len());
    for (name, data) in files {
        let path = if into_dir {
            match base_name(&name) {
                Some(base) => target.join(base),
                None => return error(400, format!("invalid file name '{name}'")),
            }
        } else {
            target.clone()
        };
        if let Err(reply) = write_file(&path, &data, who) {
            return reply;
        }
        written.push(entry(&path));
    }
    Reply::json(200, &Value::Array(written))
}

/// A raw `application/octet-stream` body, written to `path` itself.
fn upload_raw(request: &Request, raw: &str, media: &str, who: &Who) -> Reply {
    if !(media.is_empty() || media.starts_with("application/octet-stream")) {
        return error(400, format!("unsupported content type '{media}'"));
    }
    if raw.is_empty() || raw.ends_with('/') {
        return error(400, "a raw upload needs the file's path");
    }
    let path = resolve(raw, &who.home);
    match write_file(&path, &request.body, who) {
        Ok(()) => Reply::json(200, &Value::Array(vec![entry(&path)])),
        Err(reply) => reply,
    }
}

/// Writes `data` to `path`: missing parent directories are created, then the bytes go to a
/// temporary sibling owned by the user, which is renamed over `path`. A failure leaves nothing
/// behind.
fn write_file(path: &Path, data: &[u8], who: &Who) -> Result<(), Reply> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    create_dirs(parent, who.uid, who.gid).map_err(|err| {
        error(
            if err.code == "invalid_argument" {
                400
            } else {
                500
            },
            err.message,
        )
    })?;
    let name = path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let temp = parent.join(format!(
        ".{name}.upload-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    if let Err(err) = write_temp(&temp, path, data, who) {
        let _ = fs::remove_file(&temp);
        return Err(io_error(&err, path));
    }
    Ok(())
}

fn write_temp(temp: &Path, path: &Path, data: &[u8], who: &Who) -> io::Result<()> {
    let mut file = File::create(temp)?;
    file.write_all(data)?;
    file.sync_all()?;
    drop(file);
    std::os::unix::fs::lchown(temp, Some(who.uid), Some(who.gid))?;
    fs::rename(temp, path)
}

/// The `EntryInfo` of a written file, as envd.yaml answers an upload.
fn entry(path: &Path) -> Value {
    json!({
        "name": path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
        "type": "file",
        "path": path.to_string_lossy(),
    })
}

/// The file name a part names: its last path component, never `.` or `..` or empty.
fn base_name(name: &str) -> Option<&str> {
    let base = name.rsplit(['/', '\\']).next()?;
    (!base.is_empty() && base != "." && base != "..").then_some(base)
}

/// An error answer: envd.yaml's `{code, message}`, with the HTTP status as the code.
fn error(status: u16, message: impl Into<String>) -> Reply {
    Reply::json(status, &json!({"code": status, "message": message.into()}))
}

/// Maps a filesystem error to the status envd.yaml gives it: 404 missing, 507 full, else 500.
fn io_error(err: &io::Error, path: &Path) -> Reply {
    match err.kind() {
        io::ErrorKind::NotFound => error(404, format!("path '{}' does not exist", path.display())),
        _ if err.raw_os_error() == Some(ENOSPC) => error(507, "not enough disk space available"),
        _ => error(
            500,
            format!("error opening file '{}': {err}", path.display()),
        ),
    }
}

/// The query's parameters, percent-decoded (`+` is a space, as in a form). A repeated name keeps
/// its first value.
fn parse_query(raw: &str) -> BTreeMap<String, String> {
    let mut params = BTreeMap::new();
    for pair in raw.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        params
            .entry(percent_decode(key))
            .or_insert_with(|| percent_decode(value));
    }
    params
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The `boundary` parameter of a `multipart/form-data` content type, quoted or not.
fn boundary_of(media: &str) -> Option<String> {
    media
        .split(';')
        .skip(1)
        .find_map(|param| {
            let (key, value) = param.trim().split_once('=')?;
            if key.trim().eq_ignore_ascii_case("boundary") {
                Some(value.trim().trim_matches('"').to_string())
            } else {
                None
            }
        })
        .filter(|boundary| !boundary.is_empty())
}

/// One part of a multipart body: its `filename` (absent for a plain form field) and its bytes.
#[derive(Debug, PartialEq)]
struct Part {
    file_name: Option<String>,
    data: Vec<u8>,
}

/// Splits a `multipart/form-data` body (RFC 2046). Parts are introduced by `--boundary` lines
/// (CRLF-ended), each followed by header lines and a blank line; a part's bytes end just before the
/// CRLF of the next delimiter. Anything before the first delimiter is a preamble and is ignored.
fn parse_multipart(body: &[u8], boundary: &str) -> Result<Vec<Part>, &'static str> {
    let delimiter = format!("--{boundary}").into_bytes();
    let mut next_delimiter = b"\r\n--".to_vec();
    next_delimiter.extend_from_slice(boundary.as_bytes());
    let mut cursor =
        find(body, &delimiter, 0).ok_or("the multipart body has no boundary")? + delimiter.len();
    let mut parts = Vec::new();
    // `--` right after a delimiter is the closing one.
    while !body[cursor..].starts_with(b"--") {
        let line_end =
            find(body, b"\r\n", cursor).ok_or("a multipart delimiter is not followed by CRLF")?;
        cursor = line_end + 2;
        // A part with no headers starts with its blank line.
        let (headers, content) = if body[cursor..].starts_with(b"\r\n") {
            (String::new(), cursor + 2)
        } else {
            let headers_end =
                find(body, b"\r\n\r\n", cursor).ok_or("a multipart part has no end of headers")?;
            (
                String::from_utf8_lossy(&body[cursor..headers_end]).into_owned(),
                headers_end + 4,
            )
        };
        let content_end =
            find(body, &next_delimiter, content).ok_or("a multipart part is not terminated")?;
        parts.push(Part {
            file_name: filename_of(&headers),
            data: body[content..content_end].to_vec(),
        });
        cursor = content_end + next_delimiter.len();
    }
    Ok(parts)
}

/// The `filename` of a part's `Content-Disposition` header, unquoted.
fn filename_of(headers: &str) -> Option<String> {
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if !name.trim().eq_ignore_ascii_case("content-disposition") {
            return None;
        }
        value.split(';').find_map(|param| {
            let (key, value) = param.trim().split_once('=')?;
            (key.trim() == "filename").then(|| value.trim().trim_matches('"').replace("\\\"", "\""))
        })
    })
}

/// The first position at or after `from` where `needle` occurs in `haystack`.
fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|position| position + from)
}

#[cfg(test)]
mod tests {
    use super::super::filesystem::test_support::Scratch;
    use super::*;

    fn request(method: &str, query: &str, content_type: Option<&str>, body: &[u8]) -> Request {
        let mut headers = Vec::new();
        if let Some(content_type) = content_type {
            headers.push(("content-type".to_string(), content_type.to_string()));
        }
        Request {
            method: method.to_string(),
            path: "/files".to_string(),
            query: query.to_string(),
            headers,
            body: body.to_vec(),
        }
    }

    fn answer(reply: Reply) -> (u16, Vec<u8>) {
        match reply {
            Reply::Full { status, body, .. } => (status, body),
        }
    }

    fn json_answer(reply: Reply) -> (u16, Value) {
        let (status, body) = answer(reply);
        (status, serde_json::from_slice(&body).unwrap())
    }

    const MULTIPART: &str = "multipart/form-data; boundary=\"XyZ\"";

    #[test]
    fn multipart_parts_are_split_on_crlf_delimiters() {
        let body = b"--XyZ\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\nhello\r\n--XyZ\r\nContent-Disposition: form-data; name=\"file\"; filename=\"b.bin\"\r\n\r\n\x00\x01\r\n2\r\n--XyZ--\r\n";
        let parts = parse_multipart(body, "XyZ").unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].file_name.as_deref(), Some("a.txt"));
        assert_eq!(parts[0].data, b"hello");
        assert_eq!(parts[1].file_name.as_deref(), Some("b.bin"));
        assert_eq!(parts[1].data, b"\x00\x01\r\n2");
    }

    #[test]
    fn multipart_skips_preamble_and_keeps_plain_fields_without_a_file_name() {
        let body = b"preamble\r\n--b\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\nnot a file\r\n--b\r\nContent-Disposition: form-data; name=\"file\"; filename=\"c.txt\"\r\n\r\nc\r\n--b--\r\n";
        let parts = parse_multipart(body, "b").unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].file_name, None);
        assert_eq!(parts[1].file_name.as_deref(), Some("c.txt"));
        assert_eq!(parts[1].data, b"c");
    }

    #[test]
    fn multipart_accepts_an_empty_part_and_rejects_a_truncated_body() {
        let body = b"--b\r\nContent-Disposition: form-data; filename=\"e\"\r\n\r\n\r\n--b--\r\n";
        let parts = parse_multipart(body, "b").unwrap();
        assert_eq!(parts[0].data, b"");
        assert!(parse_multipart(b"--b\r\nContent-Disposition: x\r\n\r\nno end", "b").is_err());
        assert!(parse_multipart(b"no boundary here", "b").is_err());
    }

    #[test]
    fn boundary_is_read_quoted_or_not() {
        assert_eq!(boundary_of(MULTIPART).as_deref(), Some("XyZ"));
        assert_eq!(
            boundary_of("multipart/form-data;boundary=abc123").as_deref(),
            Some("abc123")
        );
        assert_eq!(boundary_of("multipart/form-data"), None);
    }

    #[test]
    fn query_values_are_percent_decoded() {
        let query = parse_query("path=a%20b+c.txt&username=user&path=ignored");
        assert_eq!(query.get("path").map(String::as_str), Some("a b c.txt"));
        assert_eq!(query.get("username").map(String::as_str), Some("user"));
    }

    #[test]
    fn download_returns_the_bytes_and_errors_on_missing_or_directory() {
        let scratch = Scratch::new("download");
        let ctx = scratch.ctx();
        fs::write(scratch.0.join("a b.txt"), "payload").unwrap();
        fs::create_dir(scratch.0.join("dir")).unwrap();

        let (status, body) = answer(handle(&request("GET", "path=a%20b.txt", None, b""), &ctx));
        assert_eq!((status, body.as_slice()), (200, b"payload".as_slice()));

        let (status, body) = json_answer(handle(&request("GET", "path=missing", None, b""), &ctx));
        assert_eq!(status, 404);
        assert_eq!(body["code"], 404);
        assert!(body["message"].as_str().unwrap().contains("does not exist"));

        let (status, _) = json_answer(handle(&request("GET", "path=dir", None, b""), &ctx));
        assert_eq!(status, 400);
    }

    #[test]
    fn multipart_upload_into_a_directory_writes_each_file() {
        let scratch = Scratch::new("upload-dir");
        let ctx = scratch.ctx();
        fs::create_dir(scratch.0.join("uploads")).unwrap();
        let body = b"--XyZ\r\nContent-Disposition: form-data; name=\"file\"; filename=\"one.txt\"\r\n\r\n1\r\n--XyZ\r\nContent-Disposition: form-data; name=\"file\"; filename=\"nested/two.txt\"\r\n\r\n22\r\n--XyZ--\r\n";
        let (status, answer) = json_answer(handle(
            &request("POST", "path=uploads/", Some(MULTIPART), body),
            &ctx,
        ));
        assert_eq!(status, 200);
        let entries = answer.as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1]["name"], "two.txt");
        assert_eq!(entries[1]["type"], "file");
        assert_eq!(fs::read(scratch.0.join("uploads/one.txt")).unwrap(), b"1");
        assert_eq!(fs::read(scratch.0.join("uploads/two.txt")).unwrap(), b"22");
        // Only the two files: no temporary sibling is left behind.
        assert_eq!(fs::read_dir(scratch.0.join("uploads")).unwrap().count(), 2);
    }

    #[test]
    fn multipart_upload_to_a_file_path_writes_one_file_and_refuses_several() {
        let scratch = Scratch::new("upload-file");
        let ctx = scratch.ctx();
        let one = b"--XyZ\r\nContent-Disposition: form-data; name=\"file\"; filename=\"ignored.txt\"\r\n\r\nbody\r\n--XyZ--\r\n";
        let (status, _) = json_answer(handle(
            &request("POST", "path=sub/deep/named.txt", Some(MULTIPART), one),
            &ctx,
        ));
        assert_eq!(status, 200);
        assert_eq!(
            fs::read(scratch.0.join("sub/deep/named.txt")).unwrap(),
            b"body"
        );
        assert!(!scratch.0.join("sub/deep/ignored.txt").exists());

        let two = b"--XyZ\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a\"\r\n\r\na\r\n--XyZ\r\nContent-Disposition: form-data; name=\"file\"; filename=\"b\"\r\n\r\nb\r\n--XyZ--\r\n";
        let (status, _) = json_answer(handle(
            &request("POST", "path=sub/deep/named.txt", Some(MULTIPART), two),
            &ctx,
        ));
        assert_eq!(status, 400);
    }

    #[test]
    fn raw_upload_writes_the_body_and_creates_parents() {
        let scratch = Scratch::new("upload-raw");
        let ctx = scratch.ctx();
        let (status, answer) = json_answer(handle(
            &request(
                "POST",
                "path=deep/er/raw.bin",
                Some("application/octet-stream"),
                &[0, 1, 2, 255],
            ),
            &ctx,
        ));
        assert_eq!(status, 200);
        assert_eq!(answer[0]["name"], "raw.bin");
        assert_eq!(
            fs::read(scratch.0.join("deep/er/raw.bin")).unwrap(),
            [0, 1, 2, 255]
        );
        let (status, _) = json_answer(handle(
            &request("POST", "path=deep/", Some("application/octet-stream"), b"x"),
            &ctx,
        ));
        assert_eq!(status, 400);
    }

    #[test]
    fn an_unknown_user_is_unauthorized() {
        let scratch = Scratch::new("user");
        let ctx = scratch.ctx();
        let (status, body) = json_answer(handle(
            &request("GET", "path=x&username=no-such-user-apz", None, b""),
            &ctx,
        ));
        assert_eq!(status, 401);
        assert_eq!(body["code"], 401);
    }
}
