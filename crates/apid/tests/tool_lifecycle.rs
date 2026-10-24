//! An agent-built tool from proposal to use, end to end on a scratch hub (PLAN 14.1, Engine 5.5). A project's planner
//! writes a tool that uses a connector's secret and proposes it (`tool_package`); its tests report passed; the planner
//! approves; the owner answers the connector card; the tool is published to its project. The project's planner sees it
//! in its profile and calls it: the hub runs the component on the glue tier, the key goes out on the request, and the
//! component never sees it. A second project's planner sees neither the tool nor can call it.
//!
//! noded and the agent gateway are stood in for as in `work_engine.rs`. The connector's API is a fake HTTP server here,
//! reached through `APZ_GLUE_STAND_IN`, which records the request heads so the test can read the key's header.

mod common;

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{Value, json};

use common::{FakeDaemon, Hub, Scratch, text};

/// The CID noded gives the first VM it starts; each next one gets the next.
const FIRST_CID: u64 = 500;
const WAIT: Duration = Duration::from_secs(20);
/// The connector's key, as the owner sets it; it must reach the fake API and never the component.
const KEY: &str = "fc-SENTINEL-E2E";
const COMPONENT: &str = include_str!("fixtures/glue_fetch.wat");

/// The VMs noded started, as (cid, role).
type Vms = Arc<Mutex<Vec<(u64, String)>>>;

/// noded and the agent gateway, sharing the VMs noded started.
fn daemons(vms: &Vms) -> (FakeDaemon, FakeDaemon) {
    let started = Arc::clone(vms);
    let noded = FakeDaemon::serve(move |method, path, body| {
        let mut vms = started.lock().unwrap_or_else(|p| p.into_inner());
        if method == "GET" && path.starts_with("/v1/runs") {
            let list: Vec<Value> = vms
                .iter()
                .enumerate()
                .map(|(i, (cid, role))| json!({ "id": i, "guest_cid": cid, "role": role }))
                .collect();
            (
                200,
                json!({ "runs": [{ "id": "r1", "ended_at": null, "vms": list }] }).to_string(),
            )
        } else if method == "POST" && path == "/v1/runs" {
            let role = serde_json::from_str::<Value>(body)
                .ok()
                .and_then(|b| b["role"].as_str().map(str::to_string))
                .unwrap_or_default();
            let cid = FIRST_CID + vms.iter().filter(|(cid, _)| *cid >= FIRST_CID).count() as u64;
            vms.push((cid, role));
            let vm = json!({ "id": vms.len() - 1, "guest_cid": cid, "state": "running" });
            (200, json!({ "id": "r1", "vms": [vm] }).to_string())
        } else {
            (200, "{}".into())
        }
    });
    let reported = Arc::clone(vms);
    let gateway = FakeDaemon::serve(move |_, path, _| {
        if path.starts_with("/v1/status") {
            let vms = reported.lock().unwrap_or_else(|p| p.into_inner());
            let agents: Vec<Value> = vms
                .iter()
                .map(|(cid, role)| json!({ "cid": cid, "role": role }))
                .collect();
            (200, json!({ "agents": agents }).to_string())
        } else {
            (200, r#"{"ok":true}"#.into())
        }
    });
    (noded, gateway)
}

/// The connector's fake API: it records each request's head (its request line and headers) and answers 200.
struct FakeApi {
    port: u16,
    heads: Arc<Mutex<Vec<String>>>,
}

fn fake_api() -> FakeApi {
    let listener =
        TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).expect("bind the fake API");
    let port = listener.local_addr().expect("its address").port();
    let heads = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&heads);
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if stream.read_exact(&mut byte).is_err() {
                    break;
                }
                head.push(byte[0]);
            }
            let head = String::from_utf8_lossy(&head).into_owned();
            let mut length = 0;
            for line in head.lines() {
                let lower = line.to_ascii_lowercase();
                if let Some(value) = lower.strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; length];
            let _ = stream.read_exact(&mut body);
            log.lock().unwrap_or_else(|p| p.into_inner()).push(head);
            let answer = r#"{"status":"scraped"}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                answer.len()
            );
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    FakeApi { port, heads }
}

/// The tool names in the latest profile the gateway was sent for `cid`.
fn profile_tools(gateway: &FakeDaemon, cid: u64) -> Vec<String> {
    let suffix = format!("/agents/{cid}/send");
    gateway
        .seen()
        .into_iter()
        .filter(|(method, path, _)| method == "POST" && path.ends_with(&suffix))
        .filter_map(|(_, _, body)| {
            let body: Value = serde_json::from_str(&body).ok()?;
            let message = &body["message"];
            (message["type"] == "profile").then(|| {
                message["profile"]["tools"]
                    .as_array()
                    .map(|tools| tools.iter().map(|t| text(&t["name"])).collect())
                    .unwrap_or_default()
            })
        })
        .next_back()
        .unwrap_or_default()
}

/// The VM's tool request `name` with `input`, as the hub decides it: the reply.
fn tool_request(hub: &Hub, cid: u64, work: u64, call: &str, name: &str, input: Value) -> Value {
    let message =
        json!({ "type": "tool_request", "work": work, "call": call, "name": name, "input": input });
    let (status, body) = hub.guest(cid, "message", message);
    assert_eq!(status, 200, "{body}");
    body["reply"].clone()
}

/// The planner session of project `pid`, once it has its VM: its cid.
fn planner_cid(hub: &Hub, pid: &str) -> u64 {
    let sessions = hub.wait_for(&format!("/v1/sessions?project={pid}"), WAIT, |s| {
        s.as_array().is_some_and(|s| {
            s.iter()
                .any(|s| s["role"] == "planner" && s["cid"].is_u64())
        })
    });
    sessions
        .as_array()
        .and_then(|s| {
            s.iter()
                .find(|s| s["role"] == "planner")
                .and_then(|s| s["cid"].as_u64())
        })
        .unwrap_or_else(|| panic!("a planner session: {sessions}\n{}", hub.log()))
}

#[test]
fn a_tool_goes_from_proposal_to_use_in_its_project_and_not_in_another() {
    let vms: Vms = Arc::default();
    let (noded, gateway) = daemons(&vms);
    let api = fake_api();
    let role = |name: &str, title: &str| json!({ "role": name, "title": title, "tools": ["read", "run", "tool_package"] });
    let roles = json!({ "roles": {
        "orchestrator": role("orchestrator", "Orchestrator"),
        "planner": role("planner", "Planner"),
        "builder": role("builder", "Builder"),
        "reviewer": role("reviewer", "Reviewer"),
    } });
    let scratch = Scratch::new("tool-lifecycle");
    let no_weft = scratch.0.join("no-weft");
    let stand_in = format!("127.0.0.1:{}", api.port);
    let hub = Hub::start_with_env(
        &scratch,
        Some(gateway.port),
        Some(noded.port),
        Some(&roles),
        &[
            ("WEFT_BIN", no_weft.to_str().expect("a path")),
            ("APZ_GLUE_STAND_IN", stand_in.as_str()),
        ],
    );

    // The first project opens, and its planner's VM says hello.
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Mail", "goal": "Send the digests.", "kind": "other" })),
    );
    assert_eq!(status, 201, "{project}");
    let first = text(&project["id"]);
    let planner = planner_cid(&hub, &first);
    let (status, _) = hub.guest(planner, "hello", json!({ "role": "planner", "vm": 1 }));
    assert_eq!(status, 200);

    // Write and propose: the planner's `tool_package` call is allowed, and its report is stored as a pending version.
    let component = wat::parse_str(COMPONENT).expect("the fixture is valid WAT");
    let package = json!({ "package": {
        "name": "fetcher",
        "description": "Scrapes a page through the connector.",
        "command": [],
        "requires": [],
        "input_schema": { "type": "object" },
        "publish": true,
        "connector": "firecrawl",
        "files": [{
            "path": "component.wasm",
            "content_b64": base64::engine::general_purpose::STANDARD.encode(&component),
        }],
    } });
    let reply = tool_request(
        &hub,
        planner,
        1,
        "toolu_pkg",
        "tool_package",
        json!({ "name": "fetcher" }),
    );
    assert_eq!(reply["decision"], "allow", "{reply}");
    let (status, body) = hub.guest(
        planner,
        "message",
        json!({ "type": "tool_report", "work": 1, "call": "toolu_pkg", "ok": true, "output": package.to_string() }),
    );
    assert_eq!(status, 200, "{body}");
    let (_, library) = hub.call("GET", "/v1/library", None);
    let entry = library
        .as_array()
        .and_then(|l| l.iter().find(|e| e["name"] == "fetcher"))
        .unwrap_or_else(|| panic!("the proposal is in the library: {library}"));
    assert_eq!(
        (entry["state"].as_str(), entry["project"].as_str()),
        (Some("pending"), Some(first.as_str())),
        "{entry}"
    );

    // The tests run in the proposal's test fork and report passed; the owner's key goes into the vault.
    let (status, body) = hub.call(
        "POST",
        "/v1/library/fetcher/1/tests",
        Some(json!({ "passed": true })),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = hub.call(
        "PUT",
        "/v1/connectors/firecrawl/secret",
        Some(json!({ "field": "api_key", "value": KEY })),
    );
    assert_eq!(status, 200, "{body}");

    // The planner approves; the tool uses a connector's secret, so the owner's connector card opens. Before the answer
    // the planner's profile does not offer the tool.
    let (status, body) = hub.call(
        "POST",
        "/v1/library/fetcher/1/review",
        Some(json!({ "approve": true, "note": "the scrape is what the digest needs" })),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["state"], "awaiting_owner", "{body}");
    assert!(!profile_tools(&gateway, planner).contains(&"x_fetcher".to_string()));
    let (_, cards) = hub.call("GET", "/v1/cards", None);
    let card = cards
        .as_array()
        .and_then(|c| {
            c.iter()
                .find(|c| c["subject"] == "tool:fetcher:1" && c["state"] == "open")
        })
        .cloned()
        .unwrap_or_else(|| panic!("the connector card for the tool: {cards}\n{}", hub.log()));
    let (status, body) = hub.call(
        "POST",
        &format!("/v1/cards/{}", text(&card["id"])),
        Some(json!({ "choice": "approve" })),
    );
    assert_eq!(status, 200, "{body}");

    // Published to its project: the planner's profile now offers it, and the project's chat is told.
    let (_, library) = hub.call("GET", "/v1/library", None);
    let entry = library
        .as_array()
        .and_then(|l| l.iter().find(|e| e["name"] == "fetcher"))
        .cloned()
        .unwrap_or_default();
    assert_eq!(entry["state"], "published", "{entry}");
    assert!(
        profile_tools(&gateway, planner).contains(&"x_fetcher".to_string()),
        "the planner's profile offers it: {:?}",
        profile_tools(&gateway, planner)
    );

    // Use: the planner calls it, the hub runs the component on the glue tier, and the key goes out on the request only.
    let reply = tool_request(
        &hub,
        planner,
        1,
        "toolu_call",
        "x_fetcher",
        json!({ "method": "POST", "url": "https://api.firecrawl.dev/v2/scrape", "body": "{}" }),
    );
    assert_eq!(reply["decision"], "done", "{reply}");
    let output = text(&reply["output"]);
    assert!(output.contains(r#""status":200"#), "{output}");
    assert!(output.contains("scraped"), "{output}");
    assert!(
        !output.contains("SENTINEL"),
        "the key never reaches the reply: {output}"
    );
    {
        let heads = api.heads.lock().unwrap_or_else(|p| p.into_inner());
        assert_eq!(
            heads.len(),
            1,
            "one request reached the connector's API: {heads:?}"
        );
        assert!(heads[0].starts_with("POST /v2/scrape "), "{}", heads[0]);
        assert!(
            heads[0].contains(&format!("Bearer {KEY}")),
            "the request carries the vaulted key: {}",
            heads[0]
        );
    }

    // A second project: its planner sees neither the tool nor can call it.
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Notes", "goal": "Keep the notes.", "kind": "other" })),
    );
    assert_eq!(status, 201, "{project}");
    let second = text(&project["id"]);
    let other = planner_cid(&hub, &second);
    assert_ne!(other, planner);
    let (status, _) = hub.guest(other, "hello", json!({ "role": "planner", "vm": 1 }));
    assert_eq!(status, 200);
    assert!(
        !profile_tools(&gateway, other).contains(&"x_fetcher".to_string()),
        "the other project's profile does not offer it: {:?}",
        profile_tools(&gateway, other)
    );
    let reply = tool_request(
        &hub,
        other,
        1,
        "toolu_other",
        "x_fetcher",
        json!({ "method": "GET", "url": "https://api.firecrawl.dev/v2/scrape" }),
    );
    assert_ne!(reply["decision"], "done", "{reply}");
    assert_eq!(api.heads.lock().unwrap_or_else(|p| p.into_inner()).len(), 1);
}
