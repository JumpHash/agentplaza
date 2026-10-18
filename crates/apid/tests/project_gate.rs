//! The project gate, end to end on a scratch hub. An agent on the bench makes no model or tool call and takes no
//! chat; a project's task goes only to its agents; a paused project holds them; an archived one sends them back to
//! the bench. The hub boots as a child process with everything in a temp directory, an empty env file and ports
//! nothing listens on. The agents' VMs are played by calls to the internal API, as the agent hub makes them.

use std::io::{BufRead, BufReader, Read};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

/// The apid binary built for this test run.
const APID: &str = env!("CARGO_BIN_EXE_apid");
/// Ports the owner's own hub and daemons use. The test never picks one of them.
const OWNER_PORTS: [u16; 3] = [7700, 8899, 19080];
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const CALL_TIMEOUT: Duration = Duration::from_secs(10);
const ATTEMPTS: u32 = 3;

/// A temp directory removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("apid-project-gate-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create the scratch directory");
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A child apid, its output collected on a reader thread. Killed on drop.
struct Hub {
    child: Child,
    output: Arc<Mutex<String>>,
    port: u16,
    /// The internal token the agent hub sends; apid writes it to its state dir.
    token: String,
}

impl Drop for Hub {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Hub {
    fn log(&self) -> String {
        self.output
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// The owner's call (a loopback request is the owner's on this hub).
    fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        self.send(method, path, &[], body)
    }

    /// A call from agent `cid`'s VM, as the agent hub relays it.
    fn guest(&self, cid: u64, what: &str, body: Value) -> (u16, Value) {
        let path = format!("/internal/v1/agents/{cid}/{what}");
        self.send(
            "POST",
            &path,
            &[("x-apz-internal", &self.token)],
            Some(body),
        )
    }

    fn send(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<Value>,
    ) -> (u16, Value) {
        let body = body.map(|body| body.to_string());
        let reply = apz_http::client::send_with(
            self.port,
            method,
            path,
            headers,
            body.as_deref().map(str::as_bytes),
            CALL_TIMEOUT,
        )
        .unwrap_or_else(|err| panic!("{method} {path}: {err}\n{}", self.log()));
        let value = serde_json::from_slice(&reply.body).unwrap_or(Value::Null);
        (reply.status, value)
    }

    /// Agent `cid`'s model call, as its harness sends one mid-turn: the hub's answer.
    fn model_call(&self, cid: u64) -> Value {
        let message = json!({
            "type": "model_call",
            "req": 1,
            "work": 1,
            "request": { "model": "", "messages": [] },
        });
        let (status, body) = self.guest(cid, "message", message);
        assert_eq!(status, 200, "{body}");
        body["reply"].clone()
    }

    /// Agent `cid`'s request for a tool no role is granted, as its harness sends one: the hub's decision. The
    /// gate answers before the grants do, and a granted tool is never reached, so nothing is run or checkpointed.
    fn tool_request(&self, cid: u64) -> Value {
        let message = json!({
            "type": "tool_request",
            "work": 1,
            "call": "toolu_1",
            "name": "no_such_tool",
            "input": {},
        });
        let (status, body) = self.guest(cid, "message", message);
        assert_eq!(status, 200, "{body}");
        body["reply"].clone()
    }
}

/// Three loopback ports the OS just handed out, none of them the owner's.
fn free_ports() -> [u16; 3] {
    let mut held = Vec::new();
    let mut ports: Vec<u16> = Vec::new();
    while ports.len() < 3 {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .expect("bind a loopback port");
        let port = listener
            .local_addr()
            .expect("the listener's address")
            .port();
        if !OWNER_PORTS.contains(&port) && !ports.contains(&port) {
            ports.push(port);
        }
        held.push(listener);
    }
    drop(held);
    [ports[0], ports[1], ports[2]]
}

fn capture<R: Read + Send + 'static>(reader: R, sink: Arc<Mutex<String>>) {
    thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            let mut text = sink.lock().unwrap_or_else(|p| p.into_inner());
            text.push_str(&line);
            text.push('\n');
        }
    });
}

/// Starts apid with everything under `dir` and waits until it listens. `None` when it does not come up (a port
/// taken in between, say); the caller tries again.
fn boot(dir: &Path) -> Option<Hub> {
    let ports = free_ports();
    let state = dir.join("state");
    let config = dir.join("config");
    std::fs::create_dir_all(&config).expect("create the config directory");
    let env_file = dir.join("empty.env");
    std::fs::write(&env_file, "").expect("write the empty env file");
    let mut command = apz_os::runner::command(Path::new(APID));
    command
        .arg("--port")
        .arg(ports[0].to_string())
        .arg("--hub-port")
        .arg(ports[1].to_string())
        .arg("--noded-port")
        .arg(ports[2].to_string())
        .arg("--state-dir")
        .arg(&state)
        .arg("--vault-key")
        .arg(config.join("vault.key"))
        .arg("--env-file")
        .arg(&env_file)
        .arg("--roles")
        .arg(dir.join("no-roles.json"))
        .arg("--prompts")
        .arg(dir.join("no-prompts"))
        .arg("--console")
        .arg(dir.join("no-console"))
        // Nothing the owner has on this machine: every location apid reads is inside the scratch dir.
        .env("HOME", dir)
        .env("XDG_STATE_HOME", dir.join("xdg-state"))
        .env("XDG_CONFIG_HOME", dir.join("xdg-config"))
        .env("XDG_DATA_HOME", dir.join("xdg-data"))
        .env("APZ_SHARE", dir.join("no-share"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().ok()?;
    let output = Arc::new(Mutex::new(String::new()));
    capture(
        child.stdout.take().expect("piped stdout"),
        Arc::clone(&output),
    );
    capture(
        child.stderr.take().expect("piped stderr"),
        Arc::clone(&output),
    );
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        let listening = output
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains("apid: listening on");
        if listening {
            break;
        }
        if matches!(child.try_wait(), Ok(Some(_))) || Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let token = std::fs::read_to_string(state.join("internal.token"))
        .expect("apid writes its internal token")
        .trim()
        .to_string();
    Some(Hub {
        child,
        output,
        port: ports[0],
        token,
    })
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_string()
}

#[test]
fn only_a_projects_agents_work_and_only_while_it_runs() {
    let scratch = Scratch::new();
    let hub = (1..=ATTEMPTS)
        .find_map(|attempt| boot(&scratch.0.join(format!("attempt-{attempt}"))))
        .expect("a scratch hub comes up");

    // Two agents say hello. Neither is on a project, so both are on the bench, and their prompt says so.
    for cid in [300, 301] {
        let (status, body) = hub.guest(cid, "hello", json!({ "role": "builder", "vm": 0 }));
        assert_eq!(status, 200, "{body}");
        let system = text(&body["profile"]["system"]);
        assert!(system.contains("You are on the bench"), "{system}");
    }
    let (status, agent) = hub.call("GET", "/v1/agents/300", None);
    assert_eq!(status, 200, "{agent}");
    assert_eq!(agent["project"], Value::Null);
    assert!(text(&agent["held"]).contains("on the bench"), "{agent}");

    // On the bench: no model call, no tool call, no chat.
    let reply = hub.model_call(300);
    assert_eq!(reply["type"], "model_result", "{reply}");
    assert_eq!(reply["ok"], false, "{reply}");
    assert!(text(&reply["error"]).contains("on the bench"), "{reply}");
    let decision = hub.tool_request(300);
    assert_eq!(decision["decision"], "deny", "{decision}");
    assert!(
        text(&decision["output"]).contains("on the bench"),
        "{decision}"
    );
    let (status, refused) = hub.call("POST", "/v1/agents/300/chat", Some(json!({ "text": "hi" })));
    assert_eq!(status, 409, "{refused}");
    assert!(
        text(&refused["error"]).contains("on the bench"),
        "{refused}"
    );

    // Every task belongs to a project.
    let (status, refused) = hub.call("POST", "/v1/tasks", Some(json!({ "goal": "Loose" })));
    assert_eq!(status, 400, "{refused}");
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Gate", "goal": "Check the gate" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    let (status, task) = hub.call(
        "POST",
        "/v1/tasks",
        Some(json!({ "goal": "First", "project": pid })),
    );
    assert_eq!(status, 201, "{task}");
    let tid = text(&task["id"]);
    let claim = |cid: u64| {
        hub.call(
            "POST",
            &format!("/v1/tasks/{tid}/claim"),
            Some(json!({ "agent_cid": cid })),
        )
    };
    let (status, refused) = claim(300);
    assert_eq!(status, 409, "a bench agent takes no task: {refused}");

    // On the project, 300 works; 301 stays on the bench and still takes nothing.
    let (status, member) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/members"),
        Some(json!({ "cid": 300 })),
    );
    assert_eq!(status, 200, "{member}");
    assert_eq!(member["project"], pid.as_str());
    assert_eq!(member["held"], Value::Null);
    let (_, on) = hub.call("GET", &format!("/v1/agents?project={pid}"), None);
    let cids = |rows: &Value| -> Vec<u64> {
        rows.as_array()
            .map(|rows| rows.iter().filter_map(|r| r["cid"].as_u64()).collect())
            .unwrap_or_default()
    };
    assert_eq!(cids(&on), [300]);
    let (_, bench) = hub.call("GET", "/v1/agents?project=bench", None);
    assert_eq!(cids(&bench), [301]);
    let (status, refused) = claim(301);
    assert_eq!(
        status, 409,
        "a project's task goes only to its agents: {refused}"
    );
    let (status, claimed) = claim(300);
    assert_eq!(status, 200, "{claimed}");
    assert_eq!(claimed["state"], "claimed");
    // A member's call passes the gate: only the grants refuse it now. (Its model calls are not tried here: with no
    // keys they would go to the provider's gateway.)
    let decision = hub.tool_request(300);
    assert_eq!(decision["decision"], "deny", "{decision}");
    let output = text(&decision["output"]);
    assert!(
        !output.contains("bench") && !output.contains("paused"),
        "a member's tool call passes the gate: {decision}"
    );

    // A paused project holds its agents mid-turn too.
    let (status, paused) = hub.call("POST", &format!("/v1/projects/{pid}/pause"), None);
    assert_eq!(status, 200, "{paused}");
    assert!(text(&hub.model_call(300)["error"]).contains("paused"));
    let (status, refused) = hub.call("POST", "/v1/agents/300/chat", Some(json!({ "text": "hi" })));
    assert_eq!(status, 409, "{refused}");
    let (status, _) = hub.call("POST", &format!("/v1/projects/{pid}/resume"), None);
    assert_eq!(status, 200);

    // An archived project sends its agents back to the bench.
    let (status, archived) = hub.call("POST", &format!("/v1/projects/{pid}/archive"), None);
    assert_eq!(status, 200, "{archived}");
    assert_eq!(archived["archived"], true);
    let (_, agent) = hub.call("GET", "/v1/agents/300", None);
    assert_eq!(agent["project"], Value::Null, "{agent}");
    assert!(text(&hub.model_call(300)["error"]).contains("on the bench"));
    let (status, refused) = hub.call(
        "POST",
        &format!("/v1/projects/{pid}/members"),
        Some(json!({ "cid": 300 })),
    );
    assert_eq!(
        status, 409,
        "an archived project takes no agents: {refused}"
    );
}

#[test]
fn a_task_from_before_projects_moves_into_one_with_its_links() {
    let scratch = Scratch::new();
    let hub = (1..=ATTEMPTS)
        .find_map(|attempt| boot(&scratch.0.join(format!("attempt-{attempt}"))))
        .expect("a scratch hub comes up");
    let (status, project) = hub.call(
        "POST",
        "/v1/projects",
        Some(json!({ "title": "Adopt", "goal": "Take in the old tasks" })),
    );
    assert_eq!(status, 201, "{project}");
    let pid = text(&project["id"]);
    // No route makes a task outside projects any more, so the move's refusals are what this hub can show.
    let (status, refused) = hub.call("POST", "/v1/tasks/t9/move", Some(json!({ "project": pid })));
    assert_eq!(status, 404, "{refused}");
    let (status, refused) = hub.call(
        "POST",
        "/v1/tasks/t9/move",
        Some(json!({ "project": "no-such-project" })),
    );
    assert_eq!(status, 404, "{refused}");
    let (status, task) = hub.call(
        "POST",
        "/v1/tasks",
        Some(json!({ "goal": "In the project", "project": pid })),
    );
    assert_eq!(status, 201, "{task}");
    let (status, refused) = hub.call(
        "POST",
        &format!("/v1/tasks/{}/move", text(&task["id"])),
        Some(json!({ "project": pid })),
    );
    assert_eq!(status, 409, "a task never leaves its project: {refused}");
    let (status, suggested) = hub.call(
        "GET",
        &format!("/v1/projects/{pid}/suggested-members"),
        None,
    );
    assert_eq!(status, 200, "{suggested}");
    assert_eq!(suggested, json!([]));
}
