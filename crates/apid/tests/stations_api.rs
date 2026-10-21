//! The workstation API, end to end on a scratch hub: join codes (issued, used once, revoked before use), a station's
//! name, drain, revoke, settings, the audit, and the 404s for a station that is not there. A workstation joins from
//! another machine, so its join is sent the way the front door serves a caller from elsewhere (the relay preface,
//! peer 0.0.0.0); the owner's calls come from loopback, where the caller is the owner.

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use apz_station::{AccountMode, Facts, Form, Os};
use serde_json::{Value, json};

use common::{Hub, Scratch, text};

/// A workstation's facts: a Linux desktop with the given hostname and machine id, neither the hub's.
fn facts(hostname: &str, machine: &str) -> Facts {
    Facts {
        os: Os::Linux,
        os_version: "6.8".into(),
        arch: "x86_64".into(),
        hostname: hostname.into(),
        form: Form::Desktop,
        cpus: 8,
        ram_mb: 16_384,
        gpus: Vec::new(),
        displays: Vec::new(),
        account: AccountMode::AgentUser {
            name: "agent".into(),
        },
        apps: 0,
        virtual_displays: false,
        ui_tree: false,
        machine_id: Some(machine.into()),
    }
}

/// A join from another machine: the connection opens with the relay preface, so the server serves it as from
/// elsewhere. A loopback caller is the hub's own machine and the join route refuses it before the code is read.
fn join_from_elsewhere(
    hub: &Hub,
    code: &str,
    name: &str,
    hostname: &str,
    machine: &str,
) -> (u16, Value) {
    let body = json!({ "code": code, "facts": facts(hostname, machine), "name": name }).to_string();
    let mut stream = TcpStream::connect(("127.0.0.1", hub.port)).expect("connect to the hub");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("a read timeout");
    let head = format!(
        "POST /v1/stations/join HTTP/1.1\r\nHost: hub.example:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        hub.port,
        body.len()
    );
    stream
        .write_all(apz_http::RELAY_PREFACE)
        .expect("send the preface");
    stream.write_all(head.as_bytes()).expect("send the request");
    let mut reply = String::new();
    let _ = stream.read_to_string(&mut reply);
    let status = reply
        .split(' ')
        .nth(1)
        .and_then(|word| word.parse().ok())
        .unwrap_or(0);
    let value = reply
        .split_once("\r\n\r\n")
        .and_then(|(_, body)| serde_json::from_str(body).ok())
        .unwrap_or(Value::Null);
    (status, value)
}

/// A station joined with a fresh code: its id.
fn joined_station(hub: &Hub, hostname: &str) -> String {
    let (status, issued) = hub.call(
        "POST",
        "/v1/stations/codes",
        Some(json!({ "label": hostname })),
    );
    assert_eq!(status, 201, "{issued}");
    let (status, joined) = join_from_elsewhere(
        hub,
        &text(&issued["token"]),
        hostname,
        hostname,
        &format!("{hostname}-machine"),
    );
    assert_eq!(status, 201, "{joined}");
    text(&joined["station"])
}

/// The row of code `code_id` in the code list (Null when it is not listed).
fn code_row(hub: &Hub, code_id: &str) -> Value {
    let (status, codes) = hub.call("GET", "/v1/stations/codes", None);
    assert_eq!(status, 200, "{codes}");
    codes
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["id"] == code_id).cloned())
        .unwrap_or(Value::Null)
}

#[test]
fn a_code_joins_one_station_once() {
    let scratch = Scratch::new("stations-join-once");
    let hub = Hub::start(&scratch, None, None);

    let (status, issued) = hub.call(
        "POST",
        "/v1/stations/codes",
        Some(json!({ "label": "Lab", "ttl_minutes": 10 })),
    );
    assert_eq!(status, 201, "{issued}");
    let code_id = text(&issued["id"]);
    let code = text(&issued["token"]);
    assert_eq!(code_row(&hub, &code_id)["state"], "active");

    let (status, joined) = join_from_elsewhere(&hub, &code, "Lab box", "lab-box", "lab-machine-1");
    assert_eq!(status, 201, "{joined}");
    let station = text(&joined["station"]);
    assert!(!station.is_empty(), "{joined}");
    assert!(!text(&joined["token"]).is_empty(), "{joined}");

    // A used code joins no second station: the join is refused, and the station list still has one.
    let (status, again) = join_from_elsewhere(&hub, &code, "Again", "lab-again", "lab-machine-2");
    assert_eq!(status, 403, "{again}");
    let (status, views) = hub.call("GET", "/v1/stations", None);
    assert_eq!(status, 200, "{views}");
    assert_eq!(views.as_array().map(Vec::len), Some(1), "{views}");
    assert_eq!(views[0]["id"], station.as_str(), "{views}");

    // The code list shows the code as used by that join.
    assert_eq!(code_row(&hub, &code_id)["state"], "used");
}

#[test]
fn the_owner_renames_drains_and_revokes_a_station() {
    let scratch = Scratch::new("stations-owner");
    let hub = Hub::start(&scratch, None, None);
    let station = joined_station(&hub, "lab-owner");
    let path = format!("/v1/stations/{station}");

    // Rename: the answer and the view show the new name.
    let (status, renamed) = hub.call("PATCH", &path, Some(json!({ "name": "Front room" })));
    assert_eq!(status, 200, "{renamed}");
    assert_eq!(renamed["name"], "Front room", "{renamed}");
    let (status, view) = hub.call("GET", &path, None);
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["name"], "Front room", "{view}");

    // Drain: the view says draining; undrain takes it back.
    let (status, drained) = hub.call("POST", &format!("{path}/drain"), None);
    assert_eq!(status, 200, "{drained}");
    assert_eq!(drained["status"], "draining", "{drained}");
    assert_eq!(drained["draining"], json!(true), "{drained}");
    let (status, undrained) = hub.call("POST", &format!("{path}/undrain"), None);
    assert_eq!(status, 200, "{undrained}");
    assert_eq!(undrained["draining"], json!(false), "{undrained}");
    assert_ne!(undrained["status"], "draining", "{undrained}");

    // Revoke: the view says revoked. The registry does not refuse a second revoke or a drain after it (ops.rs
    // writes the row and answers the view), and the view stays revoked, since revoked is what it shows first.
    let (status, revoked) = hub.call("POST", &format!("{path}/revoke"), None);
    assert_eq!(status, 200, "{revoked}");
    assert_eq!(revoked["status"], "revoked", "{revoked}");
    assert_eq!(revoked["revoked"], json!(true), "{revoked}");
    let (status, again) = hub.call("POST", &format!("{path}/revoke"), None);
    assert_eq!(status, 200, "{again}");
    assert_eq!(again["status"], "revoked", "{again}");
    let (status, after) = hub.call("POST", &format!("{path}/drain"), None);
    assert_eq!(status, 200, "{after}");
    assert_eq!(after["status"], "revoked", "{after}");

    // The audit lists these actions for the station, newest first, and the owner is the actor of the newest.
    let (status, rows) = hub.call(
        "GET",
        &format!("/v1/stations/audit?station={station}"),
        None,
    );
    assert_eq!(status, 200, "{rows}");
    let kinds: Vec<&str> = rows
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row["kind"].as_str())
        .collect();
    assert_eq!(
        kinds,
        [
            "drain", "revoke", "revoke", "undrain", "drain", "rename", "join"
        ],
        "{rows}"
    );
    assert_eq!(rows[0]["actor"], "owner", "{rows}");
}

#[test]
fn settings_round_trip() {
    let scratch = Scratch::new("stations-settings");
    let hub = Hub::start(&scratch, None, None);
    let station = joined_station(&hub, "lab-settings");
    let path = format!("/v1/stations/{station}/settings");

    let (status, before) = hub.call("GET", &path, None);
    assert_eq!(status, 200, "{before}");
    let mut changed = before.clone();
    changed["retention_days"] = json!(7);
    changed["record"] = json!(false);
    let (status, answer) = hub.call("PUT", &path, Some(changed));
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["settings"]["retention_days"], 7, "{answer}");
    // No link is up, so the settings wait for the station's next link and the answer says so.
    assert_eq!(answer["pushed"], json!(false), "{answer}");

    let (status, after) = hub.call("GET", &path, None);
    assert_eq!(status, 200, "{after}");
    assert_eq!(after["retention_days"], 7, "{after}");
    assert_eq!(after["record"], json!(false), "{after}");
    // What was not changed is what it was.
    assert_eq!(after["record_fps"], before["record_fps"], "{after}");
    assert_eq!(after["work_folder"], before["work_folder"], "{after}");
}

#[test]
fn a_code_can_be_revoked_before_use() {
    let scratch = Scratch::new("stations-code-revoke");
    let hub = Hub::start(&scratch, None, None);
    let (status, issued) = hub.call(
        "POST",
        "/v1/stations/codes",
        Some(json!({ "label": "Spare" })),
    );
    assert_eq!(status, 201, "{issued}");
    let code_id = text(&issued["id"]);
    let code = text(&issued["token"]);

    let (status, revoked) = hub.call("DELETE", &format!("/v1/stations/codes/{code_id}"), None);
    assert_eq!(status, 200, "{revoked}");
    assert_eq!(revoked["revoked"], json!(true), "{revoked}");
    assert_eq!(code_row(&hub, &code_id)["state"], "revoked");

    // A join with the revoked code is refused, and no station comes of it.
    let (status, refused) = join_from_elsewhere(&hub, &code, "Spare box", "spare-box", "spare-1");
    assert_eq!(status, 403, "{refused}");
    let (status, views) = hub.call("GET", "/v1/stations", None);
    assert_eq!(status, 200, "{views}");
    assert_eq!(views, json!([]), "{views}");

    // A code that is not there cannot be revoked.
    let (status, missing) = hub.call("DELETE", "/v1/stations/codes/j-no-such", None);
    assert_eq!(status, 404, "{missing}");
}

#[test]
fn an_unknown_station_is_404() {
    let scratch = Scratch::new("stations-unknown");
    let hub = Hub::start(&scratch, None, None);
    let path = "/v1/stations/w-no-such";

    let (status, body) = hub.call("GET", path, None);
    assert_eq!(status, 404, "{body}");
    let (status, body) = hub.call("PATCH", path, Some(json!({ "name": "Ghost" })));
    assert_eq!(status, 404, "{body}");
    let (status, body) = hub.call("POST", &format!("{path}/drain"), None);
    assert_eq!(status, 404, "{body}");
    let (status, body) = hub.call("POST", &format!("{path}/revoke"), None);
    assert_eq!(status, 404, "{body}");
    let (status, body) = hub.call("GET", &format!("{path}/settings"), None);
    assert_eq!(status, 404, "{body}");
}
