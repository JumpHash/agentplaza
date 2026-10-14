//! `apz-workstation join`: enrolls this machine with a hub using a join code.
//!
//! The daemon posts `{code, facts, name}` to `POST /v1/stations/join`. The hub answers with the station's id and
//! its token, and both are stored in the state directory with the hub's address and the optional certificate
//! pin. The token is never printed. A machine that is already enrolled refuses until it is left.

use std::path::Path;
use std::time::Duration;

use apz_http::client::{Target, request_json};
use apz_station::{AccountMode, Facts};
use serde_json::{Value, json};

use crate::state::{self, Record};

const TIMEOUT: Duration = Duration::from_secs(15);

/// What `join` was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinRequest {
    /// `https://host:port` (or `http://` for a loopback hub).
    pub hub: String,
    pub code: String,
    pub name: String,
    pub account: AccountMode,
    /// The hub's certificate pin, base64url, when the hub uses a certificate the public roots do not sign.
    pub pin: Option<String>,
}

/// Enrolls the machine, storing the record in `dir`, and returns it.
pub fn join(dir: &Path, request: &JoinRequest, facts: &Facts) -> Result<Record, String> {
    if state::load_record(dir)?.is_some() {
        return Err(
            "this machine is already a workstation: run `apz-workstation leave` first".to_string(),
        );
    }
    let hub = request.hub.trim_end_matches('/').to_string();
    let target = Target {
        base: hub.clone(),
        pin: match &request.pin {
            Some(pin) => Some(state::parse_pin(pin)?),
            None => None,
        },
        bearer: None,
        headers: Vec::new(),
    };
    let body = json!({
        "code": request.code,
        "facts": facts,
        "name": request.name,
    });
    let (status, reply) = request_json(&target, "POST", "/v1/stations/join", Some(&body), TIMEOUT)
        .map_err(|e| format!("cannot reach the hub: {e}"))?;
    if status != 200 && status != 201 {
        return Err(hub_error(status, &reply));
    }
    let station = text_field(&reply, "station")
        .ok_or_else(|| "the hub's join reply has no station".to_string())?;
    let token = text_field(&reply, "token")
        .ok_or_else(|| "the hub's join reply has no token".to_string())?;
    let record = Record {
        hub,
        pin: request.pin.clone(),
        station,
        token,
        name: request.name.clone(),
        account: request.account.clone(),
    };
    state::save_record(dir, &record)?;
    Ok(record)
}

/// The hub's `error` when it sent one, else its status.
fn hub_error(status: u16, body: &Value) -> String {
    match body.get("error").and_then(Value::as_str) {
        Some(message) => message.to_string(),
        None => format!("the hub answered {status}"),
    }
}

fn text_field(reply: &Value, key: &str) -> Option<String> {
    reply
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_station::{Form, Os};
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    fn facts() -> Facts {
        Facts {
            os: Os::Linux,
            os_version: "test".into(),
            arch: "x86_64".into(),
            hostname: "desk".into(),
            form: Form::Desktop,
            cpus: 4,
            ram_mb: 8192,
            gpus: vec![],
            displays: vec![],
            account: AccountMode::AgentUser {
                name: "agentws".into(),
            },
            apps: 0,
            virtual_displays: false,
            ui_tree: false,
        }
    }

    fn request(hub: String) -> JoinRequest {
        JoinRequest {
            hub,
            code: "JOIN-1234".into(),
            name: "desk".into(),
            account: AccountMode::AgentUser {
                name: "agentws".into(),
            },
            pin: None,
        }
    }

    /// A one-shot HTTP server that answers the first request with `status` and `body`, and sends the request
    /// it read back on the channel.
    fn one_shot(status: u16, body: &'static str) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let (mut conn, _) = listener.accept().expect("a connection");
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = conn.read(&mut chunk).unwrap_or(0);
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some(head_end) = text.find("\r\n\r\n") {
                    let length = text[..head_end]
                        .lines()
                        .find_map(|line| {
                            let line = line.to_ascii_lowercase();
                            line.strip_prefix("content-length: ")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if buf.len() >= head_end + 4 + length {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let _ = tx.send(String::from_utf8_lossy(&buf).to_string());
            let reply = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = conn.write_all(reply.as_bytes());
        });
        (format!("http://127.0.0.1:{port}"), rx)
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("apz-ws-join-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn join_posts_the_code_facts_and_name_and_stores_the_station() {
        let (hub, seen) = one_shot(201, r#"{"station":"ws-7","token":"tok-7"}"#);
        let dir = scratch("posts");
        let record = join(&dir, &request(hub), &facts()).expect("joins");
        assert_eq!(record.station, "ws-7");
        assert_eq!(state::load_record(&dir).unwrap(), Some(record));

        let sent = seen.recv().expect("the request");
        assert!(sent.starts_with("POST /v1/stations/join "), "{sent}");
        let body = sent.split("\r\n\r\n").nth(1).expect("a body");
        let value: Value = serde_json::from_str(body).expect("JSON");
        assert_eq!(value["code"], "JOIN-1234");
        assert_eq!(value["name"], "desk");
        assert_eq!(value["facts"]["os"], "linux");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hub_refusal_is_its_error_and_stores_nothing() {
        let (hub, _seen) = one_shot(403, r#"{"error":"the join code expired"}"#);
        let dir = scratch("refused");
        let err = join(&dir, &request(hub), &facts()).unwrap_err();
        assert_eq!(err, "the join code expired");
        assert_eq!(state::load_record(&dir).unwrap(), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_machine_that_is_joined_refuses_a_second_join_before_dialling() {
        let dir = scratch("twice");
        let record = Record {
            hub: "http://127.0.0.1:1".into(),
            pin: None,
            station: "ws-1".into(),
            token: "t".into(),
            name: "n".into(),
            account: AccountMode::AgentUser {
                name: "agentws".into(),
            },
        };
        state::save_record(&dir, &record).unwrap();
        let err = join(&dir, &request("http://127.0.0.1:1".into()), &facts()).unwrap_err();
        assert!(err.contains("already a workstation"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pin_that_is_not_thirty_two_bytes_is_refused_before_dialling() {
        let dir = scratch("pin");
        let mut bad = request("http://127.0.0.1:1".into());
        bad.pin = Some(apz_trust::b64(&[1u8; 8]));
        assert!(join(&dir, &bad, &facts()).is_err());
        assert_eq!(state::load_record(&dir).unwrap(), None);
    }
}
