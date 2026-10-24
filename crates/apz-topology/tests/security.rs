//! The built-in DAST scan against seeded HTTP servers (PLAN 22.8). A server with the flaws (SQL injection against an
//! SQLite database, reflected markup, a file read, a shell, an open redirect) is found class by class; the fixed server
//! of each route is not flagged. Each class has its seeded positive and its clean negative here.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;

use apz_topology::security::dast::{self, Fetch, Http, Reply};
use apz_topology::security::{self, Context, Hit};
use apz_topology::spec;
use rusqlite::Connection;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Vulnerable,
    Fixed,
}

/// Starts a server in `mode` on a free loopback port and returns its base URL.
fn start(mode: Mode) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let db = Arc::new(Mutex::new(seeded_database()));
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let db = Arc::clone(&db);
            thread::spawn(move || answer(stream, mode, &db));
        }
    });
    format!("http://{addr}")
}

fn seeded_database() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch(
        "CREATE TABLE users (name TEXT); INSERT INTO users VALUES ('Alice'), ('Bob');",
    )
    .unwrap();
    db
}

fn answer(stream: TcpStream, mode: Mode, db: &Mutex<Connection>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    // Headers are read and ignored, up to the blank line.
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
            break;
        }
    }
    let target = line.split_whitespace().nth(1).unwrap_or("/").to_string();
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let params: BTreeMap<String, String> = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (decode(k), decode(v)))
        .collect();
    let arg = |name: &str| params.get(name).cloned().unwrap_or_default();
    let (status, location, body) = match path {
        "/users" => users(mode, db, &arg("name")),
        "/echo" => echo(mode, &arg("text")),
        "/file" => file(mode, &arg("name")),
        "/run" => run(mode, &arg("arg")),
        "/go" => go(mode, &arg("to")),
        _ => (404, None, "not found".to_string()),
    };
    let mut stream = stream;
    let location = location
        .map(|l| format!("Location: {l}\r\n"))
        .unwrap_or_default();
    let _ = write!(
        stream,
        "HTTP/1.1 {status} X\r\nContent-Type: text/html\r\n{location}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn users(mode: Mode, db: &Mutex<Connection>, name: &str) -> (u16, Option<String>, String) {
    let db = db.lock().unwrap();
    match mode {
        // The flaw: the value is spliced into the SQL text.
        Mode::Vulnerable => {
            let sql = format!("SELECT name FROM users WHERE name = '{name}'");
            match db.prepare(&sql) {
                Err(error) => (500, None, error.to_string()),
                Ok(mut stmt) => {
                    let names: Vec<String> = stmt
                        .query_map([], |row| row.get::<_, String>(0))
                        .unwrap()
                        .flatten()
                        .collect();
                    (200, None, names.join("\n") + "\n")
                }
            }
        }
        // The fix: the value is a bound parameter.
        Mode::Fixed => {
            let mut stmt = db
                .prepare("SELECT name FROM users WHERE name = ?1")
                .unwrap();
            let names: Vec<String> = stmt
                .query_map([name], |row| row.get::<_, String>(0))
                .unwrap()
                .flatten()
                .collect();
            (200, None, names.join("\n") + "\n")
        }
    }
}

fn echo(mode: Mode, text: &str) -> (u16, Option<String>, String) {
    let shown = match mode {
        Mode::Vulnerable => text.to_string(),
        Mode::Fixed => escape(text),
    };
    (200, None, format!("<p>{shown}</p>"))
}

fn file(mode: Mode, name: &str) -> (u16, Option<String>, String) {
    let docroot = std::env::temp_dir().join("apz-dast-docroot");
    std::fs::create_dir_all(&docroot).unwrap();
    let path = match mode {
        Mode::Vulnerable => docroot.join(name),
        // The fix: only a file name inside the document root.
        Mode::Fixed => docroot.join(Path::new(name).file_name().unwrap_or_default()),
    };
    match std::fs::read_to_string(path) {
        Ok(text) => (200, None, text),
        Err(_) => (404, None, "not found".into()),
    }
}

fn run(mode: Mode, arg: &str) -> (u16, Option<String>, String) {
    let out = match mode {
        // The flaw: the value is handed to a shell.
        Mode::Vulnerable => Command::new("sh")
            .current_dir(shell_dir())
            .arg("-c")
            .arg(format!("echo {arg}"))
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default(),
        // The fix: no shell, a fixed answer that repeats nothing of the input.
        Mode::Fixed => "done\n".to_string(),
    };
    (200, None, out)
}

/// A scratch directory the shell runs in, so a redirect in a payload writes nowhere that matters.
fn shell_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("apz-dast-shell");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn go(mode: Mode, to: &str) -> (u16, Option<String>, String) {
    match mode {
        Mode::Vulnerable => (302, Some(to.to_string()), String::new()),
        Mode::Fixed if to.starts_with('/') => (302, Some(to.to_string()), String::new()),
        Mode::Fixed => (400, None, "only local redirects".into()),
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Undoes the query encoding: `%XX` bytes and `+` as a space.
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("00");
                out.push(u8::from_str_radix(hex, 16).unwrap_or(b'?'));
                i += 2;
            }
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn classes(hits: &[Hit]) -> Vec<&str> {
    hits.iter().map(|h| h.id.as_str()).collect()
}

#[test]
fn a_seeded_sql_injection_is_found_and_the_parameterised_route_is_not_flagged() {
    let vulnerable = start(Mode::Vulnerable);
    let hits = dast::scan_route(
        &vulnerable,
        "/users",
        &params(&[("name", "Alice")]),
        &Http::new(),
    )
    .unwrap();
    // The SQLite error text repeats the value unescaped, so the same answer also shows reflected markup: a true finding.
    assert!(classes(&hits).contains(&"sql-injection"), "{hits:?}");
    let sql = hits.iter().find(|h| h.id == "sql-injection").unwrap();
    assert!(sql.detail.starts_with("error-based"), "{}", sql.detail);
    assert_eq!(sql.subject, "GET /users parameter name");

    let fixed = start(Mode::Fixed);
    let hits = dast::scan_route(
        &fixed,
        "/users",
        &params(&[("name", "Alice")]),
        &Http::new(),
    )
    .unwrap();
    assert!(hits.is_empty(), "{hits:?}");
}

#[test]
fn reflected_markup_is_found_and_escaped_markup_is_not() {
    let hits = dast::scan_route(
        &start(Mode::Vulnerable),
        "/echo",
        &params(&[("text", "Hello")]),
        &Http::new(),
    )
    .unwrap();
    assert_eq!(classes(&hits), ["reflected-xss"], "{hits:?}");
    let hits = dast::scan_route(
        &start(Mode::Fixed),
        "/echo",
        &params(&[("text", "Hello")]),
        &Http::new(),
    )
    .unwrap();
    assert!(hits.is_empty(), "{hits:?}");
}

// The payloads read `/etc/passwd`, which only a Unix host has.
#[cfg(unix)]
#[test]
fn a_file_read_outside_the_directory_is_found_and_a_file_name_only_is_not() {
    let hits = dast::scan_route(
        &start(Mode::Vulnerable),
        "/file",
        &params(&[("name", "index.html")]),
        &Http::new(),
    )
    .unwrap();
    assert_eq!(classes(&hits), ["path-traversal"], "{hits:?}");
    let hits = dast::scan_route(
        &start(Mode::Fixed),
        "/file",
        &params(&[("name", "index.html")]),
        &Http::new(),
    )
    .unwrap();
    assert!(hits.is_empty(), "{hits:?}");
}

#[test]
fn a_shell_fed_by_a_parameter_is_found_and_a_literal_echo_is_not() {
    let hits = dast::scan_route(
        &start(Mode::Vulnerable),
        "/run",
        &params(&[("arg", "Alice")]),
        &Http::new(),
    )
    .unwrap();
    assert_eq!(classes(&hits), ["command-injection"], "{hits:?}");
    let hits = dast::scan_route(
        &start(Mode::Fixed),
        "/run",
        &params(&[("arg", "Alice")]),
        &Http::new(),
    )
    .unwrap();
    assert!(hits.is_empty(), "{hits:?}");
}

#[test]
fn a_redirect_to_another_host_is_found_and_a_local_one_is_not() {
    let hits = dast::scan_route(
        &start(Mode::Vulnerable),
        "/go",
        &params(&[("to", "/home")]),
        &Http::new(),
    )
    .unwrap();
    assert_eq!(classes(&hits), ["open-redirect"], "{hits:?}");
    let hits = dast::scan_route(
        &start(Mode::Fixed),
        "/go",
        &params(&[("to", "/home")]),
        &Http::new(),
    )
    .unwrap();
    assert!(hits.is_empty(), "{hits:?}");
}

/// A fake answering as an SQL flaw does when the database evaluates the condition and no error shows: a true condition
/// answers as the value does, a false one answers with nothing.
struct BooleanFlaw;

impl Fetch for BooleanFlaw {
    fn get(&self, url: &str) -> Result<Reply, String> {
        let body = if url.contains("AND%20%271%27%3D%271") {
            "Alice\n"
        } else if url.contains("AND%20%271%27%3D%272") {
            ""
        } else {
            "Alice\n"
        };
        Ok(Reply {
            status: 200,
            location: None,
            body: body.into(),
            millis: 3,
        })
    }
}

#[test]
fn a_boolean_flaw_without_errors_is_found_by_the_true_and_false_conditions() {
    let hits = dast::scan_route(
        "http://h",
        "/users",
        &params(&[("name", "Alice")]),
        &BooleanFlaw,
    )
    .unwrap();
    assert_eq!(classes(&hits), ["sql-injection"], "{hits:?}");
    assert!(
        hits[0].detail.starts_with("boolean-based"),
        "{}",
        hits[0].detail
    );
}

#[test]
fn a_declared_route_runs_through_the_stages_and_its_hit_is_an_outcome() {
    let vulnerable = start(Mode::Vulnerable);
    let text = r#"
name = "web"

[[service]]
name = "api"
image = "api:1"
ports = [8080]

[[security.dast]]
service = "api"
path = "/users"
params = { name = "Alice" }
"#;
    let topology = spec::parse(text).unwrap();
    let services = BTreeMap::from([("api".to_string(), vulnerable)]);
    let cx = Context {
        tree: None,
        services: &services,
        path_var: "",
    };
    let outcomes = security::run(&topology.security, &cx);
    let dast_outcome = outcomes
        .iter()
        .find(|o| o.stage == "dast:api/users")
        .expect("the route is a stage");
    assert!(dast_outcome.ran, "{dast_outcome:?}");
    assert!(
        classes(&dast_outcome.hits).contains(&"sql-injection"),
        "{:?}",
        dast_outcome.hits
    );
}

#[test]
fn the_escaping_and_decoding_round_trip() {
    assert_eq!(decode("a%20b+c%27"), "a b c'");
    assert_eq!(
        escape("<a href=\"x\">'&"),
        "&lt;a href=&quot;x&quot;&gt;&#39;&amp;"
    );
}
