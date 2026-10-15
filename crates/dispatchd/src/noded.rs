//! The node daemon's ledger and fleet register, reached over the loopback.
//!
//! `noded` is the one daemon the dispatcher reads *and* writes: his ledger
//! (`GET /api/v0/tasks`) is the work, his register (`GET /api/v0/hub`, the
//! hub's own status proxied so a browser's one-origin policy is not in the
//! way) is who could do it, and his task endpoints are how a pass binds a task
//! to an agent and ends it.
//!
//! Everything here is the node's contract as it stands: a non-2xx answer is a
//! refusal carrying the node's own words (never a silently empty list), and a
//! task id is checked to be an id before it is put into a request path, so a
//! malformed id from a malformed answer cannot reshape the request.

use apz_http::auth::InternalToken;
use serde::Deserialize;

use crate::dispatch::Ledger;
use crate::http::{self, Answer, TIMEOUT, Target};
use crate::views::{Agent, Outcome, Task};

/// The node daemon: the ledger and the register behind one client.
#[derive(Debug, Clone)]
pub struct Noded {
    target: Target,
    auth: InternalToken,
}

impl Noded {
    /// A client for the node at `url` (`http://127.0.0.1:8899`), sending `auth`
    /// on every call.
    pub fn new(url: &str, auth: InternalToken) -> Result<Self, String> {
        Ok(Self {
            target: Target::parse(url)?,
            auth,
        })
    }

    /// Where this client reaches the node.
    pub fn target(&self) -> Target {
        self.target
    }
}

impl Ledger for Noded {
    fn tasks(&self) -> Result<Vec<Task>, String> {
        let answer = http::get(&self.target, "/api/v0/tasks", &self.auth, TIMEOUT)?;
        if answer.status() != 200 {
            return Err(refused("the ledger", &answer));
        }
        let view: TasksView = serde_json::from_slice(answer.body())
            .map_err(|err| format!("the task list could not be read: {err}"))?;
        Ok(view.tasks)
    }

    fn agents(&self) -> Result<Vec<Agent>, String> {
        let answer = http::get(&self.target, "/api/v0/hub", &self.auth, TIMEOUT)?;
        if answer.status() != 200 {
            return Err(refused("the fleet register", &answer));
        }
        let view: RegistryView = serde_json::from_slice(answer.body())
            .map_err(|err| format!("the fleet register could not be read: {err}"))?;
        Ok(view.agents)
    }

    fn claim(&self, id: &str, cid: u64) -> Result<(), String> {
        let path = format!("/api/v0/tasks/{}/claim", task_path(id)?);
        let body = serde_json::json!({ "agent_cid": cid }).to_string();
        expect_ok(
            &self.target,
            &self.auth,
            &path,
            body.as_bytes(),
            "the claim",
        )
    }

    fn finish(&self, id: &str, outcome: Outcome, note: &str) -> Result<(), String> {
        let path = format!("/api/v0/tasks/{}/finish", task_path(id)?);
        let body = serde_json::json!({ "outcome": outcome.as_str(), "note": note }).to_string();
        expect_ok(
            &self.target,
            &self.auth,
            &path,
            body.as_bytes(),
            "the finish",
        )
    }
}

/// The list endpoint's body: `{"tasks": [...]}`.
#[derive(Debug, Deserialize)]
struct TasksView {
    #[serde(default)]
    tasks: Vec<Task>,
}

/// The hub's status body, as the node proxies it: the `agents` this client
/// reads, and everything else the fleet's consoles read.
#[derive(Debug, Deserialize)]
struct RegistryView {
    #[serde(default)]
    agents: Vec<Agent>,
}

/// A task id as it may appear in a request path.
///
/// The node's ids are `t<seq>`; anything outside `[A-Za-z0-9]` is not one, and
/// is refused here rather than spliced into a request line — an id comes from
/// the daemon's own answer, but a request is built from it, so it is checked
/// before it is used.
fn task_path(id: &str) -> Result<&str, String> {
    id_path(id, "task")
}

/// An id as it may appear in a request path: the node's ids are alphanumeric
/// (`t<seq>`, `m<seq>`), so anything outside that is refused before a request
/// is built from it.
fn id_path<'a>(id: &'a str, what: &str) -> Result<&'a str, String> {
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(format!("{id:?} is not a {what} id"));
    }
    Ok(id)
}

/// Sends one mutating request and insists on a 2xx answer.
fn expect_ok(
    target: &Target,
    auth: &InternalToken,
    path: &str,
    body: &[u8],
    what: &str,
) -> Result<(), String> {
    let answer = http::post(target, path, auth, body, TIMEOUT)?;
    if (200..300).contains(&answer.status()) {
        Ok(())
    } else {
        Err(refused(what, &answer))
    }
}

/// What the node's non-2xx answer is reported as: the status and the node's
/// own words, so a caller sees the daemon's reason rather than a paraphrase.
fn refused(what: &str, answer: &Answer) -> String {
    format!("{what} answered {}: {}", answer.status(), answer.refusal())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Write;
    use std::net::{SocketAddr, TcpListener};
    use std::sync::mpsc::{Sender, channel};
    use std::thread;

    /// A task id is an id before it is a path: the daemon's own ids pass, and
    /// anything that would reshape the request does not.
    #[test]
    fn a_task_id_is_an_id_before_it_is_a_path() {
        for good in ["t1", "t300", "abc123"] {
            assert_eq!(task_path(good), Ok(good));
        }
        for bad in [
            "",
            "../x",
            "t1/finish",
            "t 1",
            "t1?x=1",
            "t1#x",
            "t1\n",
            "t1%2f",
        ] {
            assert!(task_path(bad).is_err(), "should have been refused: {bad:?}");
        }
    }

    /// A refusal carries the daemon's status and its own words.
    #[test]
    fn a_refusal_carries_the_daemons_words() {
        let answer = Answer::for_test(409, br#"{"error":"a task may not go from done to failed"}"#);
        assert_eq!(
            refused("the finish", &answer),
            "the finish answered 409: a task may not go from done to failed"
        );
        // An answer that is not the daemon's error shape still says something.
        let answer = Answer::for_test(500, b"boom");
        assert_eq!(
            refused("the ledger", &answer),
            "the ledger answered 500: boom"
        );
    }

    /// The ledger and register paths end to end against a stub node: the
    /// client asks for the right paths, reads the daemon's own shapes, and
    /// sends the claim and finish bodies the ledger's contract takes.
    #[test]
    fn the_client_speaks_the_nodes_contract() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the test binds a port");
        let addr = listener.local_addr().expect("a bound address");
        let (requests, received) = channel();
        let served = thread::spawn(move || {
            for _ in 0..4 {
                let (stream, _peer) = listener.accept().expect("the client connects");
                serve_one(stream, &requests);
            }
        });
        let noded = Noded::new(
            &format!("http://127.0.0.1:{}", addr.port()),
            InternalToken::none(),
        )
        .expect("a loopback URL");

        let tasks = noded.tasks().expect("the ledger answers");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "t1");
        assert_eq!(tasks[0].roled(), Some("builder"));
        let (line, body) = received.recv().expect("a request");
        assert_eq!(line, "GET /api/v0/tasks HTTP/1.1");
        assert!(body.is_empty());

        let agents = noded.agents().expect("the register answers");
        assert_eq!(agents.len(), 1);
        assert_eq!((agents[0].cid, agents[0].role.as_str()), (300, "builder"));
        assert_eq!(
            received.recv().expect("a request").0,
            "GET /api/v0/hub HTTP/1.1"
        );

        noded.claim("t1", 300).expect("the claim is taken");
        let (line, body) = received.recv().expect("a request");
        assert_eq!(line, "POST /api/v0/tasks/t1/claim HTTP/1.1");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).expect("the claim body is JSON"),
            serde_json::json!({ "agent_cid": 300 })
        );

        noded
            .finish(
                "t1",
                Outcome::Done,
                "artifacts: build-notes.txt (2 tool call(s))",
            )
            .expect("the finish is taken");
        let (line, body) = received.recv().expect("a request");
        assert_eq!(line, "POST /api/v0/tasks/t1/finish HTTP/1.1");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).expect("the finish body is JSON"),
            serde_json::json!({
                "outcome": "done",
                "note": "artifacts: build-notes.txt (2 tool call(s))",
            })
        );

        served.join().expect("the stub node finishes");
    }

    /// Serves one request against the stub node's fixed answers, and reports
    /// what it was asked for.
    fn serve_one(mut stream: std::net::TcpStream, requests: &Sender<(String, String)>) {
        let request = crate::testing::read_http_request(&mut stream);
        let line = request.lines().next().unwrap_or_default().to_string();
        let path = line.split(' ').nth(1).unwrap_or_default().to_string();
        let body = request
            .split_once("\r\n\r\n")
            .map(|(_, body)| body.to_string())
            .unwrap_or_default();
        requests.send((line, body)).expect("the test is listening");

        let answer = match path.as_str() {
            "/api/v0/tasks" => r#"{"tasks":[{"id":"t1","goal":"do t1","role":"builder","state":"open"}]}"#
                .to_string(),
            "/api/v0/hub" => {
                r#"{"node":{"cids":[300]},"agents":[{"cid":300,"role":"builder","connected_at_ms":1,"last_heartbeat_ms":2,"calls":0}],"sessions_active":1}"#
                    .to_string()
            }
            _ => r#"{"id":"t1","state":"claimed"}"#.to_string(),
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
            answer.len()
        );
        stream
            .write_all(response.as_bytes())
            .expect("the answer is written");
    }

    /// A stub node that refuses is a refusal naming the status and the node's
    /// reason — never a silently empty ledger.
    #[test]
    fn a_refusing_node_is_an_error_not_an_empty_ledger() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the test binds a port");
        let addr: SocketAddr = listener.local_addr().expect("a bound address");
        let served = thread::spawn(move || {
            let (mut stream, _peer) = listener.accept().expect("the client connects");
            let _ = crate::testing::read_http_request(&mut stream);
            let answer = r#"{"error":"no such endpoint"}"#;
            let response = format!(
                "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                answer.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("the answer is written");
        });
        let noded = Noded::new(
            &format!("http://127.0.0.1:{}", addr.port()),
            InternalToken::none(),
        )
        .expect("a loopback URL");
        let err = noded.tasks().expect_err("the ledger refused");
        assert_eq!(err, "the ledger answered 404: no such endpoint");
        served.join().expect("the stub finishes");
    }
}
