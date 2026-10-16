//! The relay's HTTP surface on `127.0.0.1:--http-port` (default 19080):
//!
//! | route | does |
//! |---|---|
//! | `GET /v1/status` | node facts, the served CIDs, and one row per connected agent |
//! | `GET /healthz` | `ok` |
//! | `POST /v1/cids/{cid}` | starts serving the guest CID → `{cid, served: true}` (idempotent) |
//! | `DELETE /v1/cids/{cid}` | stops serving it → `{cid, served: false}`; 404 when it was not served |
//! | `POST /v1/agents/{cid}/send` | `{message: HostMessage}`: write one host line to the agent |
//! | `POST /v1/agents/{cid}/tell` | `{name, args?, timeout_ms?}` → the agent's `{ok, output}` |
//! | `POST /v1/agents/{cid}/stop` | `{reason?}` → `{cid, stopped, calls}` or `{cid, stopped: false, reason}` |
//!
//! A CID the relay does not serve is 404; a served CID with no session, 409. A
//! tool the agent's role does not grant is 403 `{ok: false, output}` and never
//! reaches the guest; an agent that does not answer in time is 504. A CID path
//! segment below 3 is 400: CIDs 0 to 2 are the hypervisor, local and host, never
//! a guest.
//!
//! With `--internal-token-file`, every route but `GET /healthz` needs the
//! machine's internal token (`x-apz-internal`), as the node daemons' APIs do.

use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use apz_http::auth::InternalToken;
use apz_http::{Handle, Limits, Request, Response, Router, Server};
use apz_proto::wire::{HostMessage, MAX_LINE_BYTES};
use serde::{Deserialize, Serialize};

use crate::hub::{Hub, SendOutcome, StopOutcome, TellOutcome};
use crate::{Registry, RemoveError};

/// The longest a `/tell` may wait for its agent (and its default).
pub const MAX_TELL_TIMEOUT_MS: u64 = 5_000;
/// How long `/send` and `/stop` wait for the session.
const SESSION_WAIT: Duration = Duration::from_secs(5);
/// Bounds what a caller may push into a session's log and wire line.
const MAX_TOOL_NAME: usize = 256;
const MAX_STOP_REASON: usize = 256;
const DEFAULT_STOP_REASON: &str = "the operator stopped this session";
/// CIDs 0 to 2 are the hypervisor, the local loopback and the host.
const FIRST_GUEST_CID: u32 = 3;

/// The daemon's own facts, as `/v1/status` reports them. The served CIDs
/// change at run time, so they come from the [`Registry`], not from here.
#[derive(Debug)]
pub struct NodeFacts {
    pub port: u32,
    pub started: Instant,
    pub max_sessions: usize,
    pub keepalive_idle_s: u64,
}

struct Surface {
    hub: Arc<Hub>,
    registry: Arc<Registry>,
    facts: NodeFacts,
}

/// Serves the surface until the handle is dropped. `token` guards every route
/// but `GET /healthz`, a liveness probe that reveals nothing.
pub fn serve(
    port: u16,
    hub: Arc<Hub>,
    registry: Arc<Registry>,
    facts: NodeFacts,
    token: InternalToken,
) -> io::Result<Handle> {
    let surface = Surface {
        hub,
        registry,
        facts,
    };
    let router = router();
    let guarded = token.guard(move |request| router.handle(&surface, request));
    Server::bind(port)?
        .limits(Limits {
            max_body: MAX_LINE_BYTES,
            ..Limits::default()
        })
        .spawn(move |request| {
            if request.method == "GET" && request.path == "/healthz" {
                return Response::text(200, "ok");
            }
            guarded(request)
        })
}

fn router() -> Router<Surface> {
    Router::<Surface>::new()
        .get("/v1/metrics", |_, _, _| {
            Response::new(200, "text/plain; version=0.0.4", crate::metrics::render())
        })
        .get("/v1/status", |surface, _, _| {
            Response::new(
                200,
                apz_http::JSON,
                status_json(surface, surface.facts.started.elapsed().as_secs()),
            )
        })
        .post("/v1/cids/{cid}", |surface, _, params| {
            serve_cid(surface, params.get("cid"))
        })
        .delete("/v1/cids/{cid}", |surface, _, params| {
            unserve_cid(surface, params.get("cid"))
        })
        // apid: a guest's egress rules changed, so the decisions kept for it go.
        .post("/v1/egress/forget/{cid}", |_, _, params| {
            match params.get("cid").parse::<u32>() {
                Ok(cid) => Response::json(
                    200,
                    &serde_json::json!({ "forgotten": crate::egress::forget(cid) }),
                ),
                Err(_) => Response::error(404, "not a CID"),
            }
        })
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
        .post("/v1/agents/{cid}/quiet", |surface, request, params| {
            with_cid(surface, params.get("cid"), |cid| {
                quiet(surface, cid, request)
            })
        })
}

#[derive(Serialize)]
struct StatusBody {
    node: NodeView,
    agents: Vec<crate::hub::Row>,
    sessions_active: usize,
    cids: Vec<u32>,
}

#[derive(Serialize)]
struct NodeView {
    cids: Vec<u32>,
    port: u32,
    uptime_s: u64,
    max_sessions: usize,
    keepalive_idle_s: u64,
}

fn status_json(surface: &Surface, uptime_s: u64) -> String {
    let snapshot = surface.hub.snapshot();
    let facts = &surface.facts;
    let cids = surface.registry.served_cids();
    let body = StatusBody {
        node: NodeView {
            cids: cids.clone(),
            port: facts.port,
            uptime_s,
            max_sessions: facts.max_sessions,
            keepalive_idle_s: facts.keepalive_idle_s,
        },
        agents: snapshot.rows,
        sessions_active: snapshot.sessions_active,
        cids,
    };
    serde_json::to_string(&body)
        .unwrap_or_else(|err| serde_json::json!({ "error": err.to_string() }).to_string())
}

/// Resolves the path's CID for the `/v1/cids` routes: a number of 3 or more.
fn guest_cid(segment: &str) -> Result<u32, Response> {
    let cid = segment
        .parse::<u32>()
        .map_err(|_| Response::error(400, &format!("{segment} is not a guest CID")))?;
    if cid < FIRST_GUEST_CID {
        return Err(Response::error(
            400,
            &format!(
                "CID {cid} is not a guest: CIDs 0 to 2 are reserved, guests start at {FIRST_GUEST_CID}"
            ),
        ));
    }
    Ok(cid)
}

fn serve_cid(surface: &Surface, segment: &str) -> Response {
    let cid = match guest_cid(segment) {
        Ok(cid) => cid,
        Err(response) => return response,
    };
    match surface.registry.serve(cid) {
        Ok(_) => Response::json(200, &serde_json::json!({ "cid": cid, "served": true })),
        Err(err) => Response::error(409, &err),
    }
}

fn unserve_cid(surface: &Surface, segment: &str) -> Response {
    let cid = match guest_cid(segment) {
        Ok(cid) => cid,
        Err(response) => return response,
    };
    match surface.registry.remove(cid) {
        Ok(()) => Response::json(200, &serde_json::json!({ "cid": cid, "served": false })),
        Err(RemoveError::NotServed) => {
            Response::error(404, &format!("guest CID {cid} is not served"))
        }
    }
}

/// Resolves the path's CID: a number the daemon serves, or a 404.
fn with_cid(surface: &Surface, segment: &str, handle: impl FnOnce(u32) -> Response) -> Response {
    match segment.parse::<u32>() {
        Ok(cid) if surface.registry.is_served(cid) => handle(cid),
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
        // Everything else is pushed as it is: the agent loop's decisions, work, profiles and
        // stops, and a v4 `permission_decision`, which apid sends when the owner decides a held
        // engine call.
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

#[derive(Deserialize)]
struct QuietRequest {
    quiet: bool,
}

/// Marks a connected agent quiet (a dozing VM, not reaped for its silence) or loud again. Only a
/// connected session can be quiet: the flag ends with the session.
fn quiet(surface: &Surface, cid: u32, request: &Request) -> Response {
    let QuietRequest { quiet } = match request.json() {
        Ok(quiet) => quiet,
        Err(err) => return Response::error(400, &format!("invalid quiet request: {err}")),
    };
    if surface.hub.set_quiet(cid, quiet) {
        Response::json(200, &serde_json::json!({ "cid": cid, "quiet": quiet }))
    } else {
        not_connected(cid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apid::Apid;
    use crate::egress;
    use crate::hub::tests::hub_with;
    use crate::hub::{Command, ToolReply};
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::thread;

    /// A fresh socket directory for each registry a test builds.
    fn socket_dir() -> std::path::PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "plaza-vsockd-http-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, AtomicOrdering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        crate::create_uds_dir(&dir).unwrap();
        dir
    }

    /// A surface whose registry serves CIDs 10 and 11 (egress off).
    fn surface(hub: Hub) -> Surface {
        let hub = Arc::new(hub);
        let registry = Arc::new(Registry::new(
            socket_dir(),
            5000,
            None,
            Arc::clone(&hub),
            crate::DEFAULT_MAX_SESSIONS,
        ));
        for cid in [10, 11] {
            registry.serve(cid).expect("binds");
        }
        Surface {
            hub,
            registry,
            facts: NodeFacts {
                port: 5000,
                started: Instant::now(),
                max_sessions: 64,
                keepalive_idle_s: 300,
            },
        }
    }

    fn call(surface: &Surface, method: &str, path: &str, body: &str) -> (u16, serde_json::Value) {
        let request = Request {
            method: method.into(),
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

    fn post(surface: &Surface, path: &str, body: &str) -> (u16, serde_json::Value) {
        call(surface, "POST", path, body)
    }

    #[test]
    fn the_status_shape_is_frozen() {
        let anchor = Instant::now();
        let log = Arc::new(crate::hub::tests::Recording::default());
        let surface = surface(Hub::started_at(log, Apid::off(), anchor, 1_700_000_000_000));
        surface
            .hub
            .connect(10, "builder", anchor + Duration::from_millis(250))
            .unwrap();
        surface.hub.note_calls(10, 2);
        assert_eq!(
            status_json(&surface, 42),
            concat!(
                r#"{"node":{"cids":[10,11],"port":5000,"uptime_s":42,"max_sessions":64,"keepalive_idle_s":300},"#,
                r#""agents":[{"cid":10,"role":"builder","connected_at_ms":1700000000250,"last_heartbeat_ms":1700000000250,"busy":false,"calls":2}],"#,
                r#""sessions_active":1,"cids":[10,11]}"#,
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
                "/v1/agents/10/send",
                r#"{"message":{"type":"permission_decision","work":1,"call":"toolu_1.e1","allow":true}}"#,
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
    fn quiet_is_set_on_a_connected_agent_and_refused_otherwise() {
        let surface = surface(hub_with(None).1);
        for (path, body, status) in [
            ("/v1/agents/12/quiet", r#"{"quiet":true}"#, 404),
            ("/v1/agents/10/quiet", r#"{"quiet":true}"#, 409),
            ("/v1/agents/10/quiet", "", 400),
            ("/v1/agents/10/quiet", "{}", 400),
            ("/v1/agents/10/quiet", r#"{"quiet":"yes"}"#, 400),
            ("/v1/agents/10/quiet", "not json", 400),
        ] {
            assert_eq!(post(&surface, path, body).0, status, "{path} {body:.60}");
        }
        let (token, _steering, _waker) =
            surface.hub.connect(10, "builder", Instant::now()).unwrap();
        assert_eq!(
            post(&surface, "/v1/agents/10/quiet", r#"{"quiet":true}"#),
            (200, serde_json::json!({ "cid": 10, "quiet": true }))
        );
        assert!(surface.hub.is_quiet(10, token));
        assert_eq!(
            post(&surface, "/v1/agents/10/quiet", r#"{"quiet":false}"#),
            (200, serde_json::json!({ "cid": 10, "quiet": false }))
        );
        assert!(!surface.hub.is_quiet(10, token));
    }

    #[test]
    fn a_tell_is_trusted_and_its_tool_is_dispatched_with_its_args_and_answer_returned() {
        let surface = surface(hub_with(None).1);
        let (_token, steering, _waker) = surface
            .hub
            .connect(10, "librarian", Instant::now())
            .unwrap();
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
        // Any tool goes to the agent, the owner's `ide_*` ones included, with its arguments (`{}` when absent).
        assert_eq!(
            post(&surface, "/v1/agents/10/tell", r#"{"name":"write_file"}"#),
            (
                200,
                serde_json::json!({ "ok": true, "output": "ran write_file {}" })
            )
        );
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
        assert_eq!(agent.join().unwrap(), ["write_file", "ide_list"]);
    }

    #[test]
    fn cids_are_served_and_unserved_over_http_and_a_low_cid_is_refused() {
        let surface = surface(hub_with(None).1);
        assert_eq!(
            post(&surface, "/v1/cids/50", ""),
            (200, serde_json::json!({ "cid": 50, "served": true }))
        );
        // Idempotent: serving it again is the same answer and changes nothing.
        assert_eq!(
            post(&surface, "/v1/cids/50", ""),
            (200, serde_json::json!({ "cid": 50, "served": true }))
        );
        let (status, body) = call(&surface, "GET", "/v1/status", "");
        assert_eq!(
            (status, &body["cids"]),
            (200, &serde_json::json!([10, 11, 50]))
        );
        assert_eq!(body["node"]["cids"], serde_json::json!([10, 11, 50]));
        // A run-time CID is routed like a start-up one: served, so 409 with no session.
        assert_eq!(
            post(&surface, "/v1/agents/50/tell", r#"{"name":"echo"}"#).0,
            409
        );

        for path in ["/v1/cids/2", "/v1/cids/0", "/v1/cids/x"] {
            assert_eq!(post(&surface, path, "").0, 400, "{path}");
            assert_eq!(call(&surface, "DELETE", path, "").0, 400, "{path}");
        }

        assert_eq!(
            call(&surface, "DELETE", "/v1/cids/50", ""),
            (200, serde_json::json!({ "cid": 50, "served": false }))
        );
        assert_eq!(call(&surface, "DELETE", "/v1/cids/50", "").0, 404);
        assert_eq!(
            post(&surface, "/v1/agents/50/tell", r#"{"name":"echo"}"#).0,
            404
        );
        let (_, body) = call(&surface, "GET", "/v1/status", "");
        assert_eq!(body["cids"], serde_json::json!([10, 11]));
    }

    #[test]
    fn with_egress_on_a_run_time_cid_stops_both_of_its_listeners() {
        let hub = Arc::new(hub_with(None).1);
        let policy = Arc::new(egress::Policy::new(Vec::new()));
        let registry = Arc::new(Registry::new(
            socket_dir(),
            5000,
            Some((policy, Apid::off())),
            Arc::clone(&hub),
            crate::DEFAULT_MAX_SESSIONS,
        ));
        let surface = Surface {
            hub,
            registry,
            facts: NodeFacts {
                port: 5000,
                started: Instant::now(),
                max_sessions: 64,
                keepalive_idle_s: 300,
            },
        };
        assert_eq!(post(&surface, "/v1/cids/70", "").0, 200);
        assert_eq!(call(&surface, "DELETE", "/v1/cids/70", "").0, 200);
        let (_, body) = call(&surface, "GET", "/v1/status", "");
        assert_eq!(body["cids"], serde_json::json!([]));
    }

    #[test]
    fn the_surface_serves_over_loopback() {
        let surface = surface(hub_with(None).1);
        let handle = serve(
            0,
            Arc::clone(&surface.hub),
            Arc::clone(&surface.registry),
            surface.facts,
            InternalToken::none(),
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

    #[test]
    fn with_a_token_file_every_route_but_healthz_needs_the_token() {
        let path =
            std::env::temp_dir().join(format!("plaza-vsockd-http-token-{}", std::process::id()));
        std::fs::write(&path, "s3cret\n").expect("the token file");
        let surface = surface(hub_with(None).1);
        let handle = serve(
            0,
            Arc::clone(&surface.hub),
            Arc::clone(&surface.registry),
            surface.facts,
            InternalToken::file(&path),
        )
        .expect("binds");
        let port = handle.port();
        let get = |headers: &[(&str, &str)]| {
            apz_http::client::send_with(
                port,
                "GET",
                "/v1/status",
                headers,
                None,
                Duration::from_secs(5),
            )
            .unwrap()
            .status
        };
        assert_eq!(get(&[]), 401);
        assert_eq!(get(&[(apz_http::auth::HEADER, "wrong")]), 401);
        assert_eq!(get(&[(apz_http::auth::HEADER, "s3cret")]), 200);
        let healthz =
            apz_http::client::send(port, "GET", "/healthz", None, Duration::from_secs(5)).unwrap();
        assert_eq!(healthz.status, 200);
        let _ = std::fs::remove_file(&path);
    }
}
