//! The relay's HTTP surface on `127.0.0.1:--http-port` (default 19080):
//!
//! | route | does |
//! |---|---|
//! | `GET /v1/status` | node facts and one row per connected agent |
//! | `GET /healthz` | `ok` |
//! | `POST /v1/agents/{cid}/send` | `{message: HostMessage}`: write one host line to the agent |
//! | `POST /v1/agents/{cid}/tell` | `{name, args?, timeout_ms?}` → the agent's `{ok, output}` |
//! | `POST /v1/agents/{cid}/stop` | `{reason?}` → `{cid, stopped, calls}` or `{cid, stopped: false, reason}` |
//!
//! A CID the relay does not serve is 404; a served CID with no session, 409. A
//! tool the agent's role does not grant is 403 `{ok: false, output}` and never
//! reaches the guest; an agent that does not answer in time is 504.

use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use apz_http::{Handle, Limits, Request, Response, Router, Server};
use apz_proto::wire::{HostMessage, MAX_LINE_BYTES};
use serde::{Deserialize, Serialize};

use crate::hub::{Hub, SendOutcome, StopOutcome, TellOutcome};

/// The longest a `/tell` may wait for its agent (and its default).
pub const MAX_TELL_TIMEOUT_MS: u64 = 5_000;
/// How long `/send` and `/stop` wait for the session.
const SESSION_WAIT: Duration = Duration::from_secs(5);
/// Bounds what a caller may push into a session's log and wire line.
const MAX_TOOL_NAME: usize = 256;
const MAX_STOP_REASON: usize = 256;
const DEFAULT_STOP_REASON: &str = "the operator stopped this session";

/// The daemon's own facts, as `/v1/status` reports them.
#[derive(Debug)]
pub struct NodeFacts {
    pub cids: Vec<u32>,
    pub port: u32,
    pub started: Instant,
    pub max_sessions: usize,
    pub keepalive_idle_s: u64,
}

struct Surface {
    hub: Arc<Hub>,
    facts: NodeFacts,
}

/// Serves the surface until the handle is dropped.
pub fn serve(port: u16, hub: Arc<Hub>, facts: NodeFacts) -> io::Result<Handle> {
    let surface = Surface { hub, facts };
    let router = router();
    Server::bind(port)?
        .limits(Limits {
            max_body: MAX_LINE_BYTES,
            ..Limits::default()
        })
        .spawn(move |request| router.handle(&surface, request))
}

fn router() -> Router<Surface> {
    Router::<Surface>::new()
        .get("/v1/status", |surface, _, _| {
            Response::new(
                200,
                apz_http::JSON,
                status_json(surface, surface.facts.started.elapsed().as_secs()),
            )
        })
        .get("/healthz", |_, _, _| Response::text(200, "ok"))
        .post("/v1/agents/{cid}/send", |surface, request, params| {
            with_cid(surface, params.get("cid"), |cid| {
                send(surface, cid, request)
            })
        })
        .post("/v1/agents/{cid}/tell", |surface, request, params| {
            with_cid(surface, params.get("cid"), |cid| {
                tell(surface, cid, request)
            })
        })
        .post("/v1/agents/{cid}/stop", |surface, request, params| {
            with_cid(surface, params.get("cid"), |cid| {
                stop(surface, cid, request)
            })
        })
}

#[derive(Serialize)]
struct StatusBody<'a> {
    node: NodeView<'a>,
    agents: Vec<crate::hub::Row>,
    sessions_active: usize,
}

#[derive(Serialize)]
struct NodeView<'a> {
    cids: &'a [u32],
    port: u32,
    uptime_s: u64,
    max_sessions: usize,
    keepalive_idle_s: u64,
}

fn status_json(surface: &Surface, uptime_s: u64) -> String {
    let snapshot = surface.hub.snapshot();
    let facts = &surface.facts;
    let body = StatusBody {
        node: NodeView {
            cids: &facts.cids,
            port: facts.port,
            uptime_s,
            max_sessions: facts.max_sessions,
            keepalive_idle_s: facts.keepalive_idle_s,
        },
        agents: snapshot.rows,
        sessions_active: snapshot.sessions_active,
    };
    serde_json::to_string(&body)
        .unwrap_or_else(|err| serde_json::json!({ "error": err.to_string() }).to_string())
}

/// Resolves the path's CID: a number the daemon serves, or a 404.
fn with_cid(surface: &Surface, segment: &str, handle: impl FnOnce(u32) -> Response) -> Response {
    match segment.parse::<u32>() {
        Ok(cid) if surface.facts.cids.contains(&cid) => handle(cid),
        _ => Response::error(404, &format!("no agent with guest CID {segment}")),
    }
}

fn not_connected(cid: u32) -> Response {
    Response::error(409, &format!("agent with guest CID {cid} is not connected"))
}

#[derive(Deserialize)]
struct SendRequest {
    message: HostMessage,
}

fn send(surface: &Surface, cid: u32, request: &Request) -> Response {
    let message = match request.json::<SendRequest>() {
        Ok(SendRequest { message }) => message,
        Err(err) => return Response::error(400, &format!("invalid send request: {err}")),
    };
    match message {
        HostMessage::ToolCall { .. } => {
            return Response::error(400, "drive a tool with /tell, which numbers and gates it");
        }
        HostMessage::Welcome { .. } => {
            return Response::error(400, "the session's handshake is the hub's");
        }
        _ => {}
    }
    match surface.hub.send(cid, message, SESSION_WAIT) {
        SendOutcome::Sent => Response::json(200, &serde_json::json!({ "cid": cid, "sent": true })),
        SendOutcome::NotConnected => not_connected(cid),
        SendOutcome::TimedOut => Response::error(504, "the session did not take the line in time"),
    }
}

#[derive(Deserialize)]
struct TellRequest {
    name: String,
    #[serde(default = "empty_object")]
    args: serde_json::Value,
    timeout_ms: Option<u64>,
}

fn empty_object() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

fn tell(surface: &Surface, cid: u32, request: &Request) -> Response {
    let tell: TellRequest = match request.json() {
        Ok(tell) => tell,
        Err(err) => return Response::error(400, &format!("invalid tell request: {err}")),
    };
    if tell.name.is_empty() || tell.name.len() > MAX_TOOL_NAME {
        return Response::error(
            400,
            &format!("a tell needs a tool name of 1..={MAX_TOOL_NAME} bytes"),
        );
    }
    if !tell.args.is_object() {
        return Response::error(400, "args must be a JSON object");
    }
    let timeout_ms = tell.timeout_ms.unwrap_or(MAX_TELL_TIMEOUT_MS);
    if !(1..=MAX_TELL_TIMEOUT_MS).contains(&timeout_ms) {
        return Response::error(
            400,
            &format!("timeout_ms must be 1..={MAX_TELL_TIMEOUT_MS}"),
        );
    }
    let answer = |ok: bool, output: &str| serde_json::json!({ "ok": ok, "output": output });
    match surface.hub.tell(
        cid,
        &tell.name,
        tell.args,
        Duration::from_millis(timeout_ms),
    ) {
        TellOutcome::Answered { ok, output } => Response::json(200, &answer(ok, &output)),
        TellOutcome::NotConnected => not_connected(cid),
        TellOutcome::TimedOut => {
            Response::error(504, &format!("agent did not answer within {timeout_ms} ms"))
        }
    }
}

#[derive(Deserialize)]
struct StopRequest {
    reason: Option<String>,
}

fn stop(surface: &Surface, cid: u32, request: &Request) -> Response {
    // An empty body is a stop with the default reason.
    let reason = match request.body.is_empty() {
        true => None,
        false => match request.json::<StopRequest>() {
            Ok(stop) => stop.reason,
            Err(err) => return Response::error(400, &format!("invalid stop request: {err}")),
        },
    };
    let reason = match reason.as_deref() {
        None | Some("") => DEFAULT_STOP_REASON,
        Some(reason) if reason.len() > MAX_STOP_REASON => {
            return Response::error(
                400,
                &format!("the reason is longer than {MAX_STOP_REASON} bytes"),
            );
        }
        Some(reason) => reason,
    };
    match surface.hub.stop(cid, reason, SESSION_WAIT) {
        StopOutcome::Ended { calls } => Response::json(
            200,
            &serde_json::json!({ "cid": cid, "stopped": true, "calls": calls }),
        ),
        StopOutcome::StillUp => Response::json(
            200,
            &serde_json::json!({
                "cid": cid,
                "stopped": false,
                "reason": format!("the agent did not end within {} ms", SESSION_WAIT.as_millis()),
            }),
        ),
        StopOutcome::NotConnected => not_connected(cid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apid::Apid;
    use crate::hub::tests::hub_with;
    use crate::hub::{Command, ToolReply};
    use std::thread;

    fn surface(hub: Hub) -> Surface {
        let facts = NodeFacts {
            cids: vec![10, 11],
            port: 5000,
            started: Instant::now(),
            max_sessions: 64,
            keepalive_idle_s: 300,
        };
        Surface {
            hub: Arc::new(hub),
            facts,
        }
    }

    fn post(surface: &Surface, path: &str, body: &str) -> (u16, serde_json::Value) {
        let request = Request {
            method: "POST".into(),
            path: path.into(),
            body: body.into(),
            ..Request::default()
        };
        let response = router().handle(surface, &request);
        (
            response.status,
            serde_json::from_slice(response.bytes()).unwrap_or_default(),
        )
    }

    #[test]
    fn the_status_shape_is_frozen() {
        let anchor = Instant::now();
        let log = Arc::new(crate::hub::tests::Recording::default());
        let surface = surface(Hub::started_at(log, Apid::off(), anchor, 1_700_000_000_000));
        surface
            .hub
            .connect(10, "builder", anchor + Duration::from_millis(250));
        surface.hub.note_calls(10, 2);
        assert_eq!(
            status_json(&surface, 42),
            concat!(
                r#"{"node":{"cids":[10,11],"port":5000,"uptime_s":42,"max_sessions":64,"keepalive_idle_s":300},"#,
                r#""agents":[{"cid":10,"role":"builder","connected_at_ms":1700000000250,"last_heartbeat_ms":1700000000250,"busy":false,"calls":2}],"#,
                r#""sessions_active":1}"#,
            )
        );
    }

    #[test]
    fn requests_outside_the_contract_are_refused_by_status() {
        let surface = surface(hub_with(None).1);
        for (path, body, status) in [
            ("/v1/agents/12/tell", r#"{"name":"echo"}"#, 404),
            ("/v1/agents/x/stop", "", 404),
            ("/v1/agents/10/tell", r#"{"name":"echo"}"#, 409),
            ("/v1/agents/10/stop", "", 409),
            ("/v1/agents/10/tell", r#"{"name":""}"#, 400),
            ("/v1/agents/10/tell", r#"{"name":"echo","args":[1]}"#, 400),
            (
                "/v1/agents/10/tell",
                r#"{"name":"echo","timeout_ms":5001}"#,
                400,
            ),
            ("/v1/agents/10/tell", "not json", 400),
            (
                "/v1/agents/10/send",
                r#"{"message":{"type":"tool_call","call":1,"name":"x","args":{}}}"#,
                400,
            ),
            (
                "/v1/agents/10/send",
                r#"{"message":{"type":"cancel","work":1,"reason":"x"}}"#,
                409,
            ),
            (
                "/v1/agents/10/stop",
                &format!(r#"{{"reason":"{}"}}"#, "x".repeat(MAX_STOP_REASON + 1)),
                400,
            ),
        ] {
            assert_eq!(post(&surface, path, body).0, status, "{path} {body:.60}");
        }
    }

    #[test]
    fn a_tell_is_trusted_and_its_tool_is_dispatched_with_the_answer_returned() {
        let surface = surface(hub_with(None).1);
        let (_token, steering) = surface.hub.connect(10, "librarian", Instant::now());
        let agent = thread::spawn(move || {
            let mut dispatched = Vec::new();
            while let Ok(Command::Call { name, reply, .. }) =
                steering.recv_timeout(Duration::from_millis(500))
            {
                if let Some(reply) = reply {
                    let _ = reply.send(ToolReply {
                        ok: true,
                        output: format!("ran {name}"),
                    });
                }
                dispatched.push(name);
            }
            dispatched
        });
        assert_eq!(
            post(&surface, "/v1/agents/10/tell", r#"{"name":"write_file"}"#),
            (
                200,
                serde_json::json!({ "ok": true, "output": "ran write_file" })
            )
        );
        assert_eq!(agent.join().unwrap(), ["write_file"]);
    }

    #[test]
    fn the_owners_ide_tools_pass_the_hub_whatever_the_role() {
        // The owner's `ide_*` tools go to the agent as any operator tool does, with their arguments.
        let surface = surface(hub_with(None).1);
        let (_token, steering) = surface.hub.connect(10, "librarian", Instant::now());
        let agent = thread::spawn(move || {
            let mut dispatched = Vec::new();
            while let Ok(Command::Call { name, args, reply }) =
                steering.recv_timeout(Duration::from_millis(500))
            {
                if let Some(reply) = reply {
                    let _ = reply.send(ToolReply {
                        ok: true,
                        output: format!("ran {name} {args}"),
                    });
                }
                dispatched.push(name);
            }
            dispatched
        });
        assert_eq!(
            post(
                &surface,
                "/v1/agents/10/tell",
                r#"{"name":"ide_list","args":{"path":"src"}}"#
            ),
            (
                200,
                serde_json::json!({ "ok": true, "output": r#"ran ide_list {"path":"src"}"# })
            )
        );
        assert_eq!(
            post(
                &surface,
                "/v1/agents/10/tell",
                r#"{"name":"ide_run_poll","args":{"run":"run-1"}}"#
            )
            .0,
            200
        );
        assert_eq!(agent.join().unwrap(), ["ide_list", "ide_run_poll"]);
    }

    #[test]
    fn the_surface_serves_over_loopback() {
        let handle = serve(
            0,
            Arc::new(hub_with(None).1),
            surface(hub_with(None).1).facts,
        )
        .expect("binds");
        let reply = apz_http::client::send(
            handle.port(),
            "GET",
            "/healthz",
            None,
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!((reply.status, reply.body.as_slice()), (200, &b"ok"[..]));
        let (status, body) =
            apz_http::client::get_json(handle.port(), "/v1/status", Duration::from_secs(5))
                .unwrap();
        assert_eq!((status, body["sessions_active"].as_u64()), (200, Some(0)));
    }
}
