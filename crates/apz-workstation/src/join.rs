//! `apz-workstation join`: enrolls this machine with a hub using a join code.
//!
//! The daemon posts `{code, facts, name}` to `POST /v1/stations/join`. The hub answers with the station's id and
//! its token, and both are stored in the state directory with the hub's address and the optional certificate
//! pin. The token is never printed. A machine that is already enrolled refuses until it is left.
//!
//! A workstation VM has no NIC (rule 0010): its hub is `vsock://2:<port>`, the node's relay, and the join config
//! (the hub's address, its pin and the code) comes from the node's config port, once ([`join_from_node`]), so the
//! code never reaches the kernel command line or an argument list.

use std::io::Read;
use std::path::Path;
use std::time::Duration;

use apz_http::client::{Target, request_json, request_json_over};
use apz_http::ws::Duplex;
use apz_station::vsock::{self, NodeConfig, TLS_NAME};
use apz_station::{AccountMode, Facts};
use serde_json::{Value, json};

use crate::state::{self, Record};
use crate::vsock::VsockStream;

const TIMEOUT: Duration = Duration::from_secs(15);
const JOIN_PATH: &str = "/v1/stations/join";
/// The most a node's join config may be. A config is a short JSON object; anything longer is not one.
const MAX_CONFIG: u64 = 4096;

/// What `join` was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinRequest {
    /// `https://host:port` (or `http://` for a loopback hub), or `vsock://<cid>:<port>` through a node's relay.
    pub hub: String,
    pub code: String,
    pub name: String,
    pub account: AccountMode,
    /// The hub's certificate pin, base64url, when the hub uses a certificate the public roots do not sign.
    pub pin: Option<String>,
}

/// Refuses when this machine is already a workstation, before anything is dialled.
fn ensure_not_joined(dir: &Path) -> Result<(), String> {
    if state::load_record(dir)?.is_some() {
        return Err(
            "this machine is already a workstation: run `apz-workstation leave` first".to_string(),
        );
    }
    Ok(())
}

/// Enrolls the machine, storing the record in `dir`, and returns it.
pub fn join(dir: &Path, request: &JoinRequest, facts: &Facts) -> Result<Record, String> {
    ensure_not_joined(dir)?;
    let hub = request.hub.trim_end_matches('/').to_string();
    let pin = match &request.pin {
        Some(pin) => Some(state::parse_pin(pin)?),
        None => None,
    };
    let body = json!({
        "code": request.code,
        "facts": facts,
        "name": request.name,
    });
    let (status, reply) = post_join(&hub, pin, &body)?;
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

/// Enrolls the machine with the join config the node serves on `source` (`vsock://2:<port>`), and returns the record.
/// The config is read once: the machine is checked first, so a join that would be refused does not use it up.
pub fn join_from_node(
    dir: &Path,
    source: &str,
    name: String,
    account: AccountMode,
    facts: &Facts,
) -> Result<Record, String> {
    ensure_not_joined(dir)?;
    let config = fetch_config(source)?;
    let request = JoinRequest {
        hub: config.hub,
        code: config.code,
        name,
        account,
        pin: Some(config.pin),
    };
    join(dir, &request, facts)
}

/// Reads the node's join config from `source`, a `vsock://` address of the node's config port.
pub fn fetch_config(source: &str) -> Result<NodeConfig, String> {
    let (cid, port) = vsock::parse_vsock(source)
        .ok_or_else(|| "the node's config address is not a vsock:// address".to_string())?;
    let mut stream =
        VsockStream::connect(cid, port).map_err(|e| format!("cannot reach the node: {e}"))?;
    stream
        .set_read_timeout(Some(TIMEOUT))
        .map_err(|e| format!("cannot reach the node: {e}"))?;
    read_config(&mut stream)
}

/// Reads and checks a join config from `reader`. The text is never echoed in an error: it holds the code.
pub fn read_config(reader: impl Read) -> Result<NodeConfig, String> {
    let mut text = Vec::new();
    reader
        .take(MAX_CONFIG)
        .read_to_end(&mut text)
        .map_err(|e| format!("the node's join config could not be read: {e}"))?;
    let config: NodeConfig = serde_json::from_slice(&text)
        .map_err(|_| "the node's join config is not readable".to_string())?;
    if config.code.is_empty() {
        return Err("the node's join config has no join code".to_string());
    }
    if vsock::parse_vsock(&config.hub).is_none() {
        return Err("the node's join config names no vsock:// hub".to_string());
    }
    Ok(config)
}

/// Posts the join to the hub: over TCP for an http or https hub, or, for a vsock hub, through the node's relay with
/// the hub's TLS inside. A vsock hub has no host name, so it must be pinned.
fn post_join(hub: &str, pin: Option<[u8; 32]>, body: &Value) -> Result<(u16, Value), String> {
    if let Some((cid, port)) = vsock::parse_vsock(hub) {
        let pin = pin.ok_or_else(|| "a vsock hub needs the hub's certificate pin".to_string())?;
        let stream =
            VsockStream::connect(cid, port).map_err(|e| format!("cannot reach the node: {e}"))?;
        let target = Target {
            base: String::new(),
            pin: Some(pin),
            bearer: None,
            headers: Vec::new(),
        };
        return request_json_over(
            stream,
            Some(TLS_NAME),
            &target,
            "POST",
            JOIN_PATH,
            Some(body),
            TIMEOUT,
        )
        .map_err(|e| format!("cannot reach the hub: {e}"));
    }
    let target = Target {
        base: hub.to_string(),
        pin,
        bearer: None,
        headers: Vec::new(),
    };
    request_json(&target, "POST", JOIN_PATH, Some(body), TIMEOUT)
        .map_err(|e| format!("cannot reach the hub: {e}"))
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
            machine_id: None,
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
    fn a_join_config_is_read_and_a_broken_one_never_echoes_its_code() {
        let good = r#"{"hub":"vsock://2:5100","pin":"cGlu","code":"apzj1.SECRET"}"#;
        let config = read_config(good.as_bytes()).expect("a config");
        assert_eq!(
            (config.hub.as_str(), config.pin.as_str()),
            ("vsock://2:5100", "cGlu")
        );
        assert_eq!(config.code, "apzj1.SECRET");

        let truncated = r#"{"hub":"vsock://2:5100","pin":"cGlu","code":"apzj1.SECRET""#;
        let err = read_config(truncated.as_bytes()).unwrap_err();
        assert!(!err.contains("SECRET"), "{err}");

        let tcp_hub = r#"{"hub":"https://hub.example:7443","pin":"cGlu","code":"c"}"#;
        assert!(
            read_config(tcp_hub.as_bytes())
                .unwrap_err()
                .contains("vsock://")
        );
        let no_code = r#"{"hub":"vsock://2:5100","pin":"cGlu","code":""}"#;
        assert!(
            read_config(no_code.as_bytes())
                .unwrap_err()
                .contains("no join code")
        );
        let huge = vec![b' '; MAX_CONFIG as usize + 10];
        assert!(read_config(&huge[..]).is_err());
    }

    #[test]
    fn joining_from_a_node_refuses_an_enrolled_machine_before_reading_the_config() {
        let dir = scratch("from-node-twice");
        let record = Record {
            hub: "vsock://2:5100".into(),
            pin: Some("cGlu".into()),
            station: "ws-2".into(),
            token: "t".into(),
            name: "n".into(),
            account: AccountMode::AgentUser {
                name: "agentws".into(),
            },
        };
        state::save_record(&dir, &record).unwrap();
        // The source is never dialled: the refusal comes first, so the one-shot config is kept.
        let err = join_from_node(
            &dir,
            "vsock://2:5101",
            "desk".into(),
            AccountMode::AgentUser {
                name: "agentws".into(),
            },
            &facts(),
        )
        .unwrap_err();
        assert!(err.contains("already a workstation"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn join_refuses_a_bad_pin_before_dialling() {
        // a_vsock_hub_without_a_pin_is_refused_before_anything_is_dialled
        {
            let dir = scratch("vsock-no-pin");
            let err = join(&dir, &request("vsock://2:5100".into()), &facts()).unwrap_err();
            assert!(err.contains("pin"), "{err}");
            assert_eq!(state::load_record(&dir).unwrap(), None);
        }
        // a_pin_that_is_not_thirty_two_bytes_is_refused_before_dialling
        {
            let dir = scratch("pin");
            let mut bad = request("http://127.0.0.1:1".into());
            bad.pin = Some(apz_trust::b64(&[1u8; 8]));
            assert!(join(&dir, &bad, &facts()).is_err());
            assert_eq!(state::load_record(&dir).unwrap(), None);
        }
    }
}
