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

use serde::Deserialize;

use crate::dispatch::{Council, Ledger, Social};
use crate::http::{self, Answer, TIMEOUT, Target};
use crate::views::{
    Agent, Board, CastVote, Contact, ContactsView, Motion, Motions, NewMotion, NewPost, Outcome,
    Post, ReadAck, Task,
};

/// The node daemon: the ledger and the register behind one client.
#[derive(Debug, Clone)]
pub struct Noded {
    target: Target,
}

impl Noded {
    /// A client for the node at `url` (`http://127.0.0.1:8899`).
    pub fn new(url: &str) -> Result<Self, String> {
        Ok(Self {
            target: Target::parse(url)?,
        })
    }

    /// Where this client reaches the node.
    pub fn target(&self) -> Target {
        self.target
    }
}

impl Ledger for Noded {
    fn tasks(&self) -> Result<Vec<Task>, String> {
        let answer = http::get(&self.target, "/api/v0/tasks", TIMEOUT)?;
        if answer.status() != 200 {
            return Err(refused("the ledger", &answer));
        }
        let view: TasksView = serde_json::from_slice(answer.body())
            .map_err(|err| format!("the task list could not be read: {err}"))?;
        Ok(view.tasks)
    }

    fn agents(&self) -> Result<Vec<Agent>, String> {
        let answer = http::get(&self.target, "/api/v0/hub", TIMEOUT)?;
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
        expect_ok(&self.target, &path, body.as_bytes(), "the claim")
    }

    fn finish(&self, id: &str, outcome: Outcome, note: &str) -> Result<(), String> {
        let path = format!("/api/v0/tasks/{}/finish", task_path(id)?);
        let body = serde_json::json!({ "outcome": outcome.as_str(), "note": note }).to_string();
        expect_ok(&self.target, &path, body.as_bytes(), "the finish")
    }
}

/// The node's social surface, as `docs/agents/social.md` spells it: the board
/// with its read cursor, the contact lists, and the conversations.
///
/// The one reading rule is the same everywhere here: a request the node
/// refuses is a refusal carrying the node's own words — never a silently empty
/// board, an empty contact list, or a post that was not recorded. The
/// dispatcher's social pass reads the first two calls (the newest posts, and
/// the cursor advance); the rest is the contract's surface, reached through
/// this same client so the plaza is not half-wired.
impl Social for Noded {
    fn board(&self) -> Result<Board, String> {
        let answer = http::get(&self.target, "/api/v0/board", TIMEOUT)?;
        if answer.status() != 200 {
            return Err(refused("the board", &answer));
        }
        serde_json::from_slice(answer.body())
            .map_err(|err| format!("the board could not be read: {err}"))
    }

    fn read_board(&self, cid: u64, up_to: &str) -> Result<ReadAck, String> {
        let body = serde_json::json!({ "agent_cid": cid, "up_to": up_to }).to_string();
        let answer = http::post(&self.target, "/api/v0/board/read", body.as_bytes(), TIMEOUT)?;
        if answer.status() != 200 {
            return Err(refused("the board read", &answer));
        }
        serde_json::from_slice(answer.body())
            .map_err(|err| format!("the board read's answer could not be read: {err}"))
    }

    fn post(&self, post: &NewPost) -> Result<Post, String> {
        let body = serde_json::json!({
            "agent_cid": post.agent_cid,
            "subject": post.subject,
            "body": post.body,
            "task": post.task,
            "posted_by": post.posted_by,
        })
        .to_string();
        let answer = http::post(&self.target, "/api/v0/board", body.as_bytes(), TIMEOUT)?;
        if answer.status() != 201 {
            return Err(refused("the post", &answer));
        }
        serde_json::from_slice(answer.body())
            .map_err(|err| format!("the post the board recorded could not be read: {err}"))
    }

    fn contacts(&self, cid: u64) -> Result<Vec<Contact>, String> {
        let path = format!("/api/v0/agents/{cid}/contacts");
        let answer = http::get(&self.target, &path, TIMEOUT)?;
        if answer.status() != 200 {
            return Err(refused("the contact list", &answer));
        }
        let view: ContactsView = serde_json::from_slice(answer.body())
            .map_err(|err| format!("the contact list could not be read: {err}"))?;
        Ok(view.contacts)
    }

    fn add_contact(&self, cid: u64, peer_cid: u64) -> Result<(), String> {
        let path = format!("/api/v0/agents/{cid}/contacts");
        let body = serde_json::json!({ "peer_cid": peer_cid }).to_string();
        expect_ok(&self.target, &path, body.as_bytes(), "the contact")
    }

    fn say(&self, cid: u64, to_cid: u64, text: &str) -> Result<(), String> {
        let path = format!("/api/v0/agents/{cid}/say");
        let body = serde_json::json!({ "to_cid": to_cid, "text": text }).to_string();
        expect_ok(&self.target, &path, body.as_bytes(), "the say")
    }
}

/// The node's council surface, as `docs/agents/council.md` spells it: the
/// motions the council holds, the proposal a seat makes, and the vote it
/// casts.
///
/// The one reading rule is the same as everywhere else here: a request the node
/// refuses is a refusal carrying the node's own words — never a silently empty
/// motions board, a motion that was not recorded, or a vote that did not land.
/// A decided motion's `409` and a role-less voter's `403` are read back the way
/// the daemon said them, so the pass can log exactly what the council refused.
impl Council for Noded {
    fn motions(&self) -> Result<Motions, String> {
        let answer = http::get(&self.target, "/api/v0/motions", TIMEOUT)?;
        if answer.status() != 200 {
            return Err(refused("the motions", &answer));
        }
        serde_json::from_slice(answer.body())
            .map_err(|err| format!("the motions could not be read: {err}"))
    }

    fn propose(&self, motion: &NewMotion) -> Result<Motion, String> {
        // `needs_owner` is deliberately absent: the daemon derives it from the
        // kind, and a caller's claim about it is refused.
        let body = serde_json::json!({
            "proposer_cid": motion.proposer_cid,
            "kind": motion.kind,
            "text": motion.text,
        })
        .to_string();
        let answer = http::post(&self.target, "/api/v0/motions", body.as_bytes(), TIMEOUT)?;
        if answer.status() != 201 {
            return Err(refused("the proposal", &answer));
        }
        serde_json::from_slice(answer.body())
            .map_err(|err| format!("the motion the council recorded could not be read: {err}"))
    }

    fn vote(&self, id: &str, cast: &CastVote) -> Result<Motion, String> {
        let path = format!("/api/v0/motions/{}/vote", motion_path(id)?);
        let body = serde_json::json!({
            "agent_cid": cast.agent_cid,
            "vote": cast.vote,
            "reason": cast.reason,
        })
        .to_string();
        let answer = http::post(&self.target, &path, body.as_bytes(), TIMEOUT)?;
        if answer.status() != 200 {
            return Err(refused("the vote", &answer));
        }
        serde_json::from_slice(answer.body())
            .map_err(|err| format!("the motion the vote answered with could not be read: {err}"))
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

/// A motion id as it may appear in a request path.
///
/// The node's ids are `m<seq>`, checked the same way a task's is: a malformed
/// id from a malformed answer cannot reshape the vote's request.
fn motion_path(id: &str) -> Result<&str, String> {
    id_path(id, "motion")
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
fn expect_ok(target: &Target, path: &str, body: &[u8], what: &str) -> Result<(), String> {
    let answer = http::post(target, path, body, TIMEOUT)?;
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
        let noded =
            Noded::new(&format!("http://127.0.0.1:{}", addr.port())).expect("a loopback URL");

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
        let noded =
            Noded::new(&format!("http://127.0.0.1:{}", addr.port())).expect("a loopback URL");
        let err = noded.tasks().expect_err("the ledger refused");
        assert_eq!(err, "the ledger answered 404: no such endpoint");
        served.join().expect("the stub finishes");
    }

    /// The social client speaks the node's own contract: the board, the cursor
    /// advance, the post, the contacts and the say — each at its own path, each
    /// with the body the endpoint takes, each read back from the node's own
    /// shape.
    #[test]
    fn the_social_client_speaks_the_nodes_contract() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the test binds a port");
        let addr = listener.local_addr().expect("a bound address");
        let (requests, received) = channel();
        let served = thread::spawn(move || {
            for _ in 0..6 {
                let (stream, _peer) = listener.accept().expect("the client connects");
                serve_social(stream, &requests);
            }
        });
        let noded =
            Noded::new(&format!("http://127.0.0.1:{}", addr.port())).expect("a loopback URL");

        let board = noded.board().expect("the board answers");
        assert_eq!(board.newest().map(|post| post.id.as_str()), Some("p2"));
        assert_eq!(board.unread_of(300), Some(0));
        let (line, body) = received.recv().expect("a request");
        assert_eq!(line, "GET /api/v0/board HTTP/1.1");
        assert!(body.is_empty());

        let ack = noded.read_board(300, "p2").expect("the cursor advances");
        assert_eq!(
            (ack.agent_cid, ack.up_to.as_str(), ack.unread),
            (300, "p2", 0)
        );
        let (line, body) = received.recv().expect("a request");
        assert_eq!(line, "POST /api/v0/board/read HTTP/1.1");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).expect("the read body is JSON"),
            serde_json::json!({ "agent_cid": 300, "up_to": "p2" })
        );

        let post = NewPost {
            agent_cid: 300,
            subject: "available for t1".to_string(),
            body: "builder can serve t1".to_string(),
            task: Some("t1".to_string()),
            posted_by: "dispatchd".to_string(),
        };
        let recorded = noded.post(&post).expect("the post is recorded");
        assert_eq!(recorded.id, "p3");
        let (line, body) = received.recv().expect("a request");
        assert_eq!(line, "POST /api/v0/board HTTP/1.1");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).expect("the post body is JSON"),
            serde_json::json!({
                "agent_cid": 300,
                "subject": "available for t1",
                "body": "builder can serve t1",
                "task": "t1",
                "posted_by": "dispatchd",
            })
        );

        let contacts = noded.contacts(300).expect("the list answers");
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].peer_cid, 310);
        assert_eq!(
            received.recv().expect("a request").0,
            "GET /api/v0/agents/300/contacts HTTP/1.1"
        );

        noded.add_contact(300, 310).expect("the contact is taken");
        let (line, body) = received.recv().expect("a request");
        assert_eq!(line, "POST /api/v0/agents/300/contacts HTTP/1.1");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).expect("the contact body is JSON"),
            serde_json::json!({ "peer_cid": 310 })
        );

        noded.say(300, 310, "hello").expect("the say is taken");
        let (line, body) = received.recv().expect("a request");
        assert_eq!(line, "POST /api/v0/agents/300/say HTTP/1.1");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).expect("the say body is JSON"),
            serde_json::json!({ "to_cid": 310, "text": "hello" })
        );

        served.join().expect("the stub node finishes");
    }

    /// Every social call reports a refusal in the node's own words, never as an
    /// empty board, an empty list, or a post that was not made.
    #[test]
    fn a_social_refusal_carries_the_nodes_words() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the test binds a port");
        let addr = listener.local_addr().expect("a bound address");
        let served = thread::spawn(move || {
            for _ in 0..6 {
                let (mut stream, _peer) = listener.accept().expect("the client connects");
                let _ = crate::testing::read_http_request(&mut stream);
                let answer = r#"{"error":"the agent is not connected"}"#;
                let response = format!(
                    "HTTP/1.1 409 Conflict\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                    answer.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("the answer is written");
            }
        });
        let noded =
            Noded::new(&format!("http://127.0.0.1:{}", addr.port())).expect("a loopback URL");
        let post = NewPost {
            agent_cid: 300,
            subject: "available for t1".to_string(),
            body: "builder can serve t1".to_string(),
            task: None,
            posted_by: "dispatchd".to_string(),
        };

        assert_eq!(
            noded.board().expect_err("refused"),
            "the board answered 409: the agent is not connected"
        );
        assert_eq!(
            noded.read_board(300, "p1").expect_err("refused"),
            "the board read answered 409: the agent is not connected"
        );
        assert_eq!(
            noded.post(&post).expect_err("refused"),
            "the post answered 409: the agent is not connected"
        );
        assert_eq!(
            noded.contacts(300).expect_err("refused"),
            "the contact list answered 409: the agent is not connected"
        );
        assert_eq!(
            noded.add_contact(300, 310).expect_err("refused"),
            "the contact answered 409: the agent is not connected"
        );
        assert_eq!(
            noded.say(300, 310, "hello").expect_err("refused"),
            "the say answered 409: the agent is not connected"
        );

        served.join().expect("the stub finishes");
    }

    /// Serves one request against the social stub node's fixed answers.
    fn serve_social(mut stream: std::net::TcpStream, requests: &Sender<(String, String)>) {
        let request = crate::testing::read_http_request(&mut stream);
        let line = request.lines().next().unwrap_or_default().to_string();
        let mut parts = line.split(' ');
        let method = parts.next().unwrap_or_default().to_string();
        let path = parts.next().unwrap_or_default().to_string();
        let body = request
            .split_once("\r\n\r\n")
            .map(|(_, body)| body.to_string())
            .unwrap_or_default();
        requests.send((line, body)).expect("the test is listening");

        let (status, answer) = match (method.as_str(), path.as_str()) {
            ("GET", "/api/v0/board") => (
                "200 OK",
                r#"{"posts":[
                     {"id":"p2","agent_cid":320,"role":"mayor","subject":"hiring for t1","body":"do t1","task":"t1","posted_by":"operator","created_at":2},
                     {"id":"p1","agent_cid":310,"role":"librarian","subject":"found work","body":"for a builder","task":null,"posted_by":"operator","created_at":1}
                   ],"total":2,"limit":256,"truncated":false,
                   "unread":[{"agent_cid":300,"count":0},{"agent_cid":310,"count":2}]}"#
                    .to_string(),
            ),
            ("POST", "/api/v0/board") => (
                "201 Created",
                r#"{"id":"p3","agent_cid":300,"role":"builder","subject":"available for t1","body":"builder can serve t1","task":"t1","posted_by":"dispatchd","created_at":3}"#
                    .to_string(),
            ),
            ("POST", "/api/v0/board/read") => (
                "200 OK",
                r#"{"agent_cid":300,"up_to":"p2","unread":0}"#.to_string(),
            ),
            ("GET", "/api/v0/agents/300/contacts") => (
                "200 OK",
                r#"{"agent_cid":300,"contacts":[{"peer_cid":310,"role":"librarian","since":7,"kind":"met"}],"total":1,"limit":256,"truncated":false}"#
                    .to_string(),
            ),
            ("POST", "/api/v0/agents/300/contacts") | ("POST", "/api/v0/agents/300/say") => {
                ("200 OK", "{}".to_string())
            }
            (_, other) => (
                "404 Not Found",
                format!(r#"{{"error":"no endpoint {other}"}}"#),
            ),
        };
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
            answer.len()
        );
        stream
            .write_all(response.as_bytes())
            .expect("the answer is written");
    }

    /// A motion id is an id before it is a path, exactly as a task's is.
    #[test]
    fn a_motion_id_is_an_id_before_it_is_a_path() {
        for good in ["m1", "m300", "abc123"] {
            assert_eq!(motion_path(good), Ok(good));
        }
        for bad in ["", "../x", "m1/vote", "m 1", "m1?x=1", "m1#x", "m1\n"] {
            assert!(
                motion_path(bad).is_err(),
                "should have been refused: {bad:?}"
            );
        }
    }

    /// The council client speaks the node's own contract: the motions board,
    /// the proposal, and the vote — each at its own path, each with the body
    /// the endpoint takes, each read back from the node's own shape.
    #[test]
    fn the_council_client_speaks_the_nodes_contract() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the test binds a port");
        let addr = listener.local_addr().expect("a bound address");
        let (requests, received) = channel();
        let served = thread::spawn(move || {
            for _ in 0..3 {
                let (stream, _peer) = listener.accept().expect("the client connects");
                serve_council(stream, &requests);
            }
        });
        let noded =
            Noded::new(&format!("http://127.0.0.1:{}", addr.port())).expect("a loopback URL");

        let motions = noded.motions().expect("the motions answer");
        assert_eq!(motions.motions.len(), 1);
        assert_eq!(motions.motions[0].id, "m1");
        assert_eq!(motions.motions[0].kind, "priority");
        assert_eq!(motions.motions[0].state, "open");
        assert!(motions.motions[0].is_open());
        assert_eq!(motions.total, 1);
        let (line, body) = received.recv().expect("a request");
        assert_eq!(line, "GET /api/v0/motions HTTP/1.1");
        assert!(body.is_empty());

        let motion = NewMotion {
            proposer_cid: 105,
            kind: "priority".to_string(),
            text: "priority: t1 — do t1".to_string(),
        };
        let recorded = noded.propose(&motion).expect("the motion is recorded");
        assert_eq!(recorded.id, "m2");
        let (line, body) = received.recv().expect("a request");
        assert_eq!(line, "POST /api/v0/motions HTTP/1.1");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).expect("the propose body is JSON"),
            serde_json::json!({
                "proposer_cid": 105,
                "kind": "priority",
                "text": "priority: t1 — do t1",
            })
        );

        let cast = CastVote {
            agent_cid: 105,
            vote: "yes".to_string(),
            reason: "the record: priority 'priority: t1 — do t1' names open task t1".to_string(),
        };
        let after = noded.vote("m1", &cast).expect("the vote is taken");
        assert_eq!(after.id, "m1");
        assert_eq!((after.tally.yes, after.tally.voted), (1, 1));
        let (line, body) = received.recv().expect("a request");
        assert_eq!(line, "POST /api/v0/motions/m1/vote HTTP/1.1");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).expect("the vote body is JSON"),
            serde_json::json!({
                "agent_cid": 105,
                "vote": "yes",
                "reason": "the record: priority 'priority: t1 — do t1' names open task t1",
            })
        );

        served.join().expect("the stub node finishes");
    }

    /// Every council call reports a refusal in the node's own words, never as an
    /// empty board, a motion that was not recorded, or a vote that did not land.
    #[test]
    fn a_council_refusal_carries_the_nodes_words() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the test binds a port");
        let addr = listener.local_addr().expect("a bound address");
        let served = thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _peer) = listener.accept().expect("the client connects");
                let _ = crate::testing::read_http_request(&mut stream);
                let answer = r#"{"error":"motion m1 is carried, not open: a decided motion takes no further vote"}"#;
                let response = format!(
                    "HTTP/1.1 409 Conflict\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                    answer.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("the answer is written");
            }
        });
        let noded =
            Noded::new(&format!("http://127.0.0.1:{}", addr.port())).expect("a loopback URL");
        let motion = NewMotion {
            proposer_cid: 105,
            kind: "priority".to_string(),
            text: "priority: t1".to_string(),
        };
        let cast = CastVote {
            agent_cid: 105,
            vote: "yes".to_string(),
            reason: "r".to_string(),
        };

        assert_eq!(
            noded.motions().expect_err("refused"),
            "the motions answered 409: motion m1 is carried, not open: a decided motion takes no further vote"
        );
        assert_eq!(
            noded.propose(&motion).expect_err("refused"),
            "the proposal answered 409: motion m1 is carried, not open: a decided motion takes no further vote"
        );
        assert_eq!(
            noded.vote("m1", &cast).expect_err("refused"),
            "the vote answered 409: motion m1 is carried, not open: a decided motion takes no further vote"
        );

        served.join().expect("the stub finishes");
    }

    /// Serves one request against the council stub node's fixed answers.
    fn serve_council(mut stream: std::net::TcpStream, requests: &Sender<(String, String)>) {
        let request = crate::testing::read_http_request(&mut stream);
        let line = request.lines().next().unwrap_or_default().to_string();
        let mut parts = line.split(' ');
        let method = parts.next().unwrap_or_default().to_string();
        let path = parts.next().unwrap_or_default().to_string();
        let body = request
            .split_once("\r\n\r\n")
            .map(|(_, body)| body.to_string())
            .unwrap_or_default();
        requests.send((line, body)).expect("the test is listening");

        let (status, answer) = match (method.as_str(), path.as_str()) {
            ("GET", "/api/v0/motions") => (
                "200 OK",
                r#"{"motions":[{"id":"m1","proposer_cid":105,"proposer_role":"finance",
                     "kind":"priority","text":"priority: t1 — do t1","needs_owner":false,
                     "state":"open","votes":[],"tally":{"yes":0,"no":0,"abstain":0,"voted":0,"seats":4},
                     "created_at":1,"updated_at":1}],"total":1,"limit":256,"truncated":false}"#
                    .to_string(),
            ),
            ("POST", "/api/v0/motions") => (
                "201 Created",
                r#"{"id":"m2","proposer_cid":105,"proposer_role":"finance","kind":"priority",
                    "text":"priority: t1 — do t1","needs_owner":false,"state":"open","votes":[],
                    "tally":{"yes":0,"no":0,"abstain":0,"voted":0,"seats":4},"created_at":2,"updated_at":2}"#
                    .to_string(),
            ),
            ("POST", "/api/v0/motions/m1/vote") => (
                "200 OK",
                r#"{"id":"m1","proposer_cid":105,"proposer_role":"finance","kind":"priority",
                    "text":"priority: t1 — do t1","needs_owner":false,"state":"open",
                    "votes":[{"seat":"finance","agent_cid":105,"vote":"yes","reason":"r","at":3}],
                    "tally":{"yes":1,"no":0,"abstain":0,"voted":1,"seats":4},"created_at":1,"updated_at":3}"#
                    .to_string(),
            ),
            (_, other) => (
                "404 Not Found",
                format!(r#"{{"error":"no endpoint {other}"}}"#),
            ),
        };
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
            answer.len()
        );
        stream
            .write_all(response.as_bytes())
            .expect("the answer is written");
    }
}
