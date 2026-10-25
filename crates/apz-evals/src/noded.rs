//! The stand-in noded: the hub asks it for VMs (`POST /v1/runs`, `{n, role, …}`), and it starts a real harness process
//! for each one. The harness is told its command line (`--cmdline`), and dials vsockd's socket for its CID
//! (`plaza.connect=uds:`), so vsockd serves it as a guest. A VM stops when the hub stops it, and the rest end with
//! the suite.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use serde_json::{Value, json};

/// The first guest CID the stand-in hands out; vsockd serves each CID it is told to.
pub const FIRST_CID: u64 = 500;
/// The vsock port the harnesses dial (vsockd's `--port`).
pub const PORT: u32 = 5000;

/// Where the harnesses come from and where they keep their logs.
#[derive(Debug, Clone)]
pub struct Harness {
    pub binary: PathBuf,
    pub uds_dir: PathBuf,
    pub logs: PathBuf,
}

/// A VM the stand-in started.
struct Vm {
    cid: u64,
    role: String,
    child: Option<Child>,
}

/// The VMs started so far; a VM's index is its place here.
#[derive(Default)]
struct State {
    vms: Vec<Vm>,
}

/// The stand-in on loopback. It serves until dropped, and then its harnesses are killed.
pub struct Noded {
    state: Arc<Mutex<State>>,
}

impl Noded {
    /// Serves on `port` in a thread of its own; each connection gets one.
    pub fn serve(port: u16, harness: Harness) -> io::Result<Noded> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let state = Arc::new(Mutex::new(State::default()));
        let shared = Arc::clone(&state);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (shared, harness) = (Arc::clone(&shared), harness.clone());
                thread::spawn(move || serve_one(stream, &shared, &harness));
            }
        });
        Ok(Noded { state })
    }
}

impl Drop for Noded {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        for vm in state.vms.iter_mut() {
            stop(vm);
        }
    }
}

fn stop(vm: &mut Vm) {
    if let Some(mut child) = vm.child.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn serve_one(mut stream: TcpStream, state: &Mutex<State>, harness: &Harness) {
    let Ok(request) = read_request(&stream) else {
        return;
    };
    let (status, body) = handle(state, harness, &request);
    let text = body.to_string();
    let reply = format!(
        "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );
    let _ = stream.write_all(reply.as_bytes());
    let _ = stream.shutdown(Shutdown::Both);
}

/// One request: its method, its path and its body.
struct Request {
    method: String,
    path: String,
    body: String,
}

fn read_request(stream: &TcpStream) -> io::Result<Request> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut words = line.split_whitespace();
    let method = words.next().unwrap_or_default().to_string();
    let path = words.next().unwrap_or("/").to_string();
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header)?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    Ok(Request {
        method,
        path,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

/// The answer to one request.
fn handle(state: &Mutex<State>, harness: &Harness, request: &Request) -> (u16, Value) {
    let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
    let parts: Vec<&str> = request.path.trim_matches('/').split('/').collect();
    match (request.method.as_str(), parts.as_slice()) {
        ("POST", ["v1", "runs"]) => {
            let body: Value = serde_json::from_str(&request.body).unwrap_or(Value::Null);
            let role = body["role"].as_str().unwrap_or_default().to_string();
            let count = body["n"].as_u64().unwrap_or(1).max(1);
            let mut vms = Vec::new();
            for _ in 0..count {
                match start(&mut state, harness, &role) {
                    Ok(vm) => vms.push(vm),
                    Err(err) => return (500, json!({ "error": err })),
                }
            }
            (200, json!({ "id": "r1", "vms": vms }))
        }
        ("GET", ["v1", "runs"]) => {
            let list: Vec<Value> = state
                .vms
                .iter()
                .enumerate()
                .map(|(id, vm)| json!({ "id": id, "guest_cid": vm.cid, "role": vm.role }))
                .collect();
            (
                200,
                json!({ "runs": [{ "id": "r1", "ended_at": null, "vms": list }] }),
            )
        }
        ("POST", ["v1", "runs", _, "vms", vm, "stop"]) => {
            if let Some(vm) = vm.parse::<usize>().ok().and_then(|i| state.vms.get_mut(i)) {
                stop(vm);
            }
            (200, json!({}))
        }
        _ => (200, json!({})),
    }
}

/// Starts a harness for a new VM of `role`, and says so as noded does.
fn start(state: &mut State, harness: &Harness, role: &str) -> Result<Value, String> {
    let index = state.vms.len();
    let cid = FIRST_CID + index as u64;
    let socket = harness.uds_dir.join(format!("vm{cid}_{PORT}"));
    let flags = format!(
        "plaza.keepalive=1 plaza.role={role} plaza.vm={index} plaza.port={PORT} plaza.connect=uds:{}",
        socket.display()
    );
    let log = File::create(harness.logs.join(format!("vm{cid}.log")))
        .map_err(|err| format!("the VM's log: {err}"))?;
    let echo = log
        .try_clone()
        .map_err(|err| format!("the VM's log: {err}"))?;
    let child = Command::new(&harness.binary)
        .arg("--cmdline")
        .arg(flags)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(echo))
        .spawn()
        .map_err(|err| format!("cannot start the harness: {err}"))?;
    state.vms.push(Vm {
        cid,
        role: role.to_string(),
        child: Some(child),
    });
    Ok(json!({ "id": index, "guest_cid": cid, "state": "running" }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn request(method: &str, path: &str, body: &str) -> Request {
        Request {
            method: method.into(),
            path: path.into(),
            body: body.into(),
        }
    }

    /// A harness that is `sleep` (it rejects the command line and exits at once): the VM's bookkeeping is the same.
    fn harness(dir: &Path, binary: &str) -> Harness {
        Harness {
            binary: PathBuf::from(binary),
            uds_dir: dir.to_path_buf(),
            logs: dir.to_path_buf(),
        }
    }

    #[test]
    fn a_run_starts_a_harness_per_vm_names_its_socket_and_a_stop_ends_it() {
        let dir = std::env::temp_dir().join(format!("apz-noded-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let state = Mutex::new(State::default());
        let harness = harness(&dir, "/bin/sleep");

        let (status, body) = handle(
            &state,
            &harness,
            &request("POST", "/v1/runs", r#"{"n":2,"role":"builder"}"#),
        );
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["vms"][1]["guest_cid"], FIRST_CID + 1, "{body}");
        let (_, list) = handle(&state, &harness, &request("GET", "/v1/runs", ""));
        assert_eq!(
            list["runs"][0]["vms"][0],
            json!({ "id": 0, "guest_cid": FIRST_CID, "role": "builder" }),
            "{list}"
        );
        let log = fs::read_to_string(dir.join(format!("vm{FIRST_CID}.log"))).unwrap_or_default();
        assert!(log.is_empty() || !log.contains("panicked"), "{log}");

        let (status, _) = handle(
            &state,
            &harness,
            &request("POST", "/v1/runs/r1/vms/0/stop", ""),
        );
        assert_eq!(status, 200);
        assert!(
            state.lock().unwrap().vms[0].child.is_none(),
            "the VM is stopped"
        );
        let (status, body) = handle(&state, &harness, &request("POST", "/v1/nothing", ""));
        assert_eq!((status, body), (200, json!({})));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_harness_that_cannot_start_fails_the_run_and_starts_no_vm() {
        let dir = std::env::temp_dir().join(format!("apz-noded-missing-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let state = Mutex::new(State::default());
        let (status, body) = handle(
            &state,
            &harness(&dir, "/no/such/agent-harness"),
            &request("POST", "/v1/runs", r#"{"role":"builder"}"#),
        );
        assert_eq!(status, 500);
        assert!(
            body["error"]
                .as_str()
                .unwrap_or_default()
                .contains("cannot start the harness"),
            "{body}"
        );
        assert!(state.lock().unwrap().vms.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
