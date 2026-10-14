//! The workstation routes. The owner's routes are behind the owner gate (`hub_auth::gate`: the console and the
//! local machine pass). `POST /v1/stations/join` and `GET /v1/stations/link` are public by design: each checks
//! its own credential (the join code, the station's bearer token). The lease routes under `/internal/` are
//! for the agents, checked by `internal::authorized` as every agent route is.
//!
//!   POST   /v1/stations/codes                 {label?, ttl_minutes?} → 201 {id, token, expires_ms} (the token is shown once)
//!   GET    /v1/stations/codes                 → [CodeView]
//!   DELETE /v1/stations/codes/{id}            → revokes an unused code
//!   POST   /v1/stations/join                  {code, facts, name?} → 201 {station, token}   (public; another machine only)
//!   GET    /v1/stations/link                  (WebSocket, subprotocol apz.station.v1, Bearer station token; public)
//!   GET    /v1/stations/audit?station=&limit= → [audit row], newest first (every station's when `station` is absent)
//!   GET    /v1/stations/recordings?station=&limit=&before= → [Recording], newest first (`before` is a start in ms)
//!   GET    /v1/stations/recordings/{id}       → Recording + `events` (its newest actions, oldest first)
//!   GET    /v1/stations/recordings/{id}/frames/{seq} → the frame's JPEG (seq counts from 1)
//!   DELETE /v1/stations/recordings/{id}       → {id, deleted} (not while its lease is open)
//!   GET    /v1/stations/recording-defaults    → RecordingDefaults (for the stations that join from now on)
//!   PUT    /v1/stations/recording-defaults    RecordingDefaults → the saved defaults
//!   GET    /v1/stations                       → [StationView]
//!   GET    /v1/stations/{id}                  → StationView
//!   POST   /v1/stations/{id}/drain            → StationView (no new work; the link closes once its answers are in)
//!   POST   /v1/stations/{id}/undrain          → StationView
//!   POST   /v1/stations/{id}/revoke           → StationView (the token stops working; the link is ended)
//!   PATCH  /v1/stations/{id}                  {name} → StationView
//!   GET    /v1/stations/{id}/settings         → StationSettings
//!   PUT    /v1/stations/{id}/settings         StationSettings (a partial object takes the defaults) → {settings, pushed, push_error}
//!   POST   /v1/stations/{id}/leases           {agent, task?, kind: "shared"|"pixel"} → 201 LeaseView
//!   GET    /v1/stations/{id}/leases           → [LeaseView] (queued leases carry their position)
//!   GET    /v1/stations/{id}/sync             → the work-folder sync's state: files, bytes, last_sync_ms, linked, conflicts
//!   GET    /v1/stations/{id}/files?prefix=    → [FileView] of the mirrored work folder (sync.rs)
//!   GET    /v1/stations/{id}/files/{path}     → the file's content (its last version the hub holds)
//!   PUT    /v1/stations/{id}/files/{path}     raw bytes → 201 FileView (the next pass pushes it to the station)
//!   DELETE /v1/stations/{id}/files/{path}     → FileView, deleted (the next pass removes it on the station)
//!   POST   /v1/stations/{id}/files/{path}/deliver  {title?, description?, task?} → 201, the file into the Library
//!   DELETE /v1/stations/{id}/leases/{lease}   → the released lease
//!   POST   /internal/v1/agents/{cid}/stations/{id}/leases           {task?, kind} → 201 LeaseView (the agent's own)
//!   DELETE /internal/v1/agents/{cid}/stations/{id}/leases/{lease}   → the lease, when it is that agent's
//!   WS     /v1/stations/{id}/screen?session=&fps=&scale=            (live view, apz.station.screen.v1; screen.rs)
//!   GET    /v1/stations/{id}/thumbnail?session=                     → one JPEG, cached 5 s (screen.rs)
//!   WS     /v1/stations/{id}/control?session=                       (takeover and input, apz.station.control.v1; control.rs)

use std::time::Duration;

use apz_http::ws::{self, Limits, Origins};
use apz_http::{Request, Response, Router};
use apz_station::{Facts, StationSettings};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use super::act;
use super::leases::Kind;
use super::ops::{self, Failure};
use super::recordings;
use super::sync;
use crate::deliverables::guess_type;
use crate::settings::RecordingDefaults;
use crate::state::App;

/// Who the owner's routes record as the actor of an action.
const OWNER: &str = "owner";

pub fn routes(router: Router<App>) -> Router<App> {
    let router = router
        .post("/v1/stations/codes", |app, req, _| issue_code_route(app, req))
        .get("/v1/stations/codes", |app, _, _| Response::json(200, &app.stations.codes(apz_plaza::now_ms())))
        .delete("/v1/stations/codes/{id}", |app, _, p| code_revoke_route(app, p.get("id")))
        .post("/v1/stations/join", |app, req, _| join_route(app, req))
        .get("/v1/stations/link", |app, req, _| link_route(app, req))
        .get("/v1/stations/audit", |app, req, _| audit_route(app, req))
        .get("/v1/stations/recordings", |app, req, _| recordings_list_route(app, req))
        .get("/v1/stations/recordings/{id}", |app, _, p| recording_show_route(app, p.get("id")))
        .get("/v1/stations/recordings/{id}/frames/{seq}", |app, _, p| recording_frame_route(app, p.get("id"), p.get("seq")))
        .delete("/v1/stations/recordings/{id}", |app, _, p| recording_delete_route(app, p.get("id")))
        .get("/v1/stations/recording-defaults", |app, _, _| Response::json(200, &app.settings.get().workstation_recordings))
        .route("PUT", "/v1/stations/recording-defaults", |app, req, _| recording_defaults_route(app, req))
        .get("/v1/stations", |app, _, _| Response::json(200, &app.stations.views(apz_plaza::now_ms())))
        .get("/v1/stations/{id}", |app, _, p| show_route(app, p.get("id")))
        .post("/v1/stations/{id}/drain", |app, _, p| drain_route(app, p.get("id"), true))
        .post("/v1/stations/{id}/undrain", |app, _, p| drain_route(app, p.get("id"), false))
        .post("/v1/stations/{id}/revoke", |app, _, p| revoke_route(app, p.get("id")))
        .route("PATCH", "/v1/stations/{id}", |app, req, p| rename_route(app, req, p.get("id")))
        .get("/v1/stations/{id}/settings", |app, _, p| settings_route(app, p.get("id")))
        .route("PUT", "/v1/stations/{id}/settings", |app, req, p| put_settings_route(app, req, p.get("id")))
        .post("/v1/stations/{id}/leases", |app, req, p| open_lease_route(app, req, p.get("id"), None, OWNER))
        .get("/v1/stations/{id}/leases", |app, _, p| leases_route(app, p.get("id")))
        .delete("/v1/stations/{id}/leases/{lease}", |app, _, p| release_route(app, p.get("id"), p.get("lease"), None, OWNER))
        .get("/v1/stations/{id}/sync", |app, _, p| sync_state_route(app, p.get("id")))
        .get("/v1/stations/{id}/files", |app, req, p| sync_files_route(app, req, p.get("id")))
        .get("/v1/stations/{id}/files/{path..}", |app, _, p| sync_read_route(app, p.get("id"), p.get("path")))
        .route("PUT", "/v1/stations/{id}/files/{path..}", |app, req, p| sync_put_route(app, req, p.get("id"), p.get("path")))
        .delete("/v1/stations/{id}/files/{path..}", |app, _, p| sync_delete_route(app, p.get("id"), p.get("path")))
        .post("/v1/stations/{id}/files/{path..}", |app, req, p| sync_post_route(app, req, p.get("id"), p.get("path")))
        .post("/internal/v1/agents/{cid}/stations/{id}/leases", |app, req, p| {
            crate::internal::authorized(app, req, p.get("cid"), |cid| Ok(agent_open_lease_route(app, req, p.get("id"), cid)))
        })
        .delete("/internal/v1/agents/{cid}/stations/{id}/leases/{lease}", |app, req, p| {
            crate::internal::authorized(app, req, p.get("cid"), |cid| Ok(agent_release_route(app, p.get("id"), p.get("lease"), cid)))
        });
    // The live views and the takeover socket (screen.rs, control.rs).
    super::control::routes(super::screen::routes(router))
}

#[derive(Default, Deserialize)]
struct DeliverBody {
    title: Option<String>,
    description: Option<String>,
    task: Option<String>,
}

fn sync_state_route(app: &App, id: &str) -> Response {
    match sync::state(app, id) {
        Ok(state) => Response::json(200, &state),
        Err(failure) => fail(failure),
    }
}

fn sync_files_route(app: &App, req: &Request, id: &str) -> Response {
    let prefix = req.query_param("prefix").unwrap_or("");
    match sync::list(app, id, prefix) {
        Ok(files) => Response::json(200, &files),
        Err(failure) => fail(failure),
    }
}

fn sync_read_route(app: &App, id: &str, path: &str) -> Response {
    match sync::read(app, id, path) {
        Ok((bytes, _)) => Response::new(200, guess_type(path), bytes).with_header("x-content-type-options", "nosniff"),
        Err(failure) => fail(failure),
    }
}

fn sync_put_route(app: &App, req: &Request, id: &str, path: &str) -> Response {
    match sync::put(app, id, path, &req.body, OWNER) {
        Ok(file) => Response::json(201, &file),
        Err(failure) => fail(failure),
    }
}

fn sync_delete_route(app: &App, id: &str, path: &str) -> Response {
    match sync::remove(app, id, path, OWNER) {
        Ok(file) => Response::json(200, &file),
        Err(failure) => fail(failure),
    }
}

/// `POST .../files/{path}/deliver` (the router's wildcard takes the path, so the suffix is checked here).
fn sync_post_route(app: &App, req: &Request, id: &str, path: &str) -> Response {
    let Some(file) = path.strip_suffix("/deliver") else {
        return Response::error(404, "POST a file's path ending in /deliver to put it in the Library");
    };
    let body: DeliverBody = match parse_or_default(req) {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    match sync::deliver(app, id, file, body.title, body.description, body.task, OWNER) {
        Ok(item) => Response::json(201, &json!({"id": item.id, "name": item.name, "size": item.size, "kind": item.kind, "audience": item.audience})),
        Err(failure) => fail(failure),
    }
}

/// An agent opens its own lease through the choke point (`act::lease_open`): the laws and the station's rules apply.
fn agent_open_lease_route(app: &App, req: &Request, id: &str, cid: u64) -> Response {
    let body: OpenBody = match parse(req) {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    if body.agent.is_some_and(|named| named != cid) {
        return Response::error(400, "the agent in the body is not the agent in the path");
    }
    match act::lease_open(app, cid, id, body.kind, body.task) {
        Ok(view) => Response::json(201, &view),
        Err(failure) => fail(failure),
    }
}

/// An agent releases its own lease through the choke point (`act::lease_release`).
fn agent_release_route(app: &App, id: &str, lease: &str, cid: u64) -> Response {
    match act::lease_release(app, cid, id, lease) {
        Ok(released) => Response::json(200, &released),
        Err(failure) => fail(failure),
    }
}

fn fail((status, message): Failure) -> Response {
    Response::error(status, &message)
}

fn parse<T: DeserializeOwned>(req: &Request) -> Result<T, Response> {
    req.json::<T>().map_err(|e| Response::error(400, &format!("bad body: {e}")))
}

fn parse_or_default<T: DeserializeOwned + Default>(req: &Request) -> Result<T, Response> {
    if req.body.is_empty() { Ok(T::default()) } else { parse(req) }
}

/// The view of a station as it stands now, or a 404.
fn view_or_404(app: &App, id: &str) -> Response {
    match app.stations.station(id) {
        Some(station) => Response::json(200, &app.stations.view(station, apz_plaza::now_ms())),
        None => Response::error(404, "no such workstation"),
    }
}

#[derive(Default, Deserialize)]
struct IssueBody {
    label: Option<String>,
    ttl_minutes: Option<u64>,
}

fn issue_code_route(app: &App, req: &Request) -> Response {
    let body: IssueBody = match parse_or_default(req) {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    match ops::issue_code(app, &body.label.unwrap_or_default(), body.ttl_minutes.unwrap_or(0), OWNER) {
        Ok((id, token, expires_ms)) => Response::json(201, &json!({"id": id, "token": token, "expires_ms": expires_ms})),
        Err(failure) => fail(failure),
    }
}

fn code_revoke_route(app: &App, id: &str) -> Response {
    match ops::revoke_code(app, id, OWNER) {
        Ok(()) => Response::json(200, &json!({"id": id, "revoked": true})),
        Err(failure) => fail(failure),
    }
}

#[derive(Deserialize)]
struct JoinBody {
    code: String,
    facts: Facts,
    #[serde(default)]
    name: Option<String>,
}

fn join_route(app: &App, req: &Request) -> Response {
    let body: JoinBody = match parse(req) {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    // A workstation is another machine. A request from this host (loopback, or a peer nobody can name) is refused
    // before the code is looked at, so a join from the hub's own machine never spends one.
    if req.peer.is_none_or(|peer| peer.is_loopback()) {
        return Response::error(403, "a workstation joins from another machine; this request came from the hub's own host");
    }
    let source = crate::nodes::source_of(req);
    match ops::join(app, &source, &body.code, body.facts, body.name.as_deref()) {
        Ok((station, token)) => Response::json(201, &json!({"station": station, "token": token})),
        Err(failure) => fail(failure),
    }
}

/// The daemon's link: its bearer token is checked before the upgrade, then the socket runs on its own thread.
fn link_route(app: &App, req: &Request) -> Response {
    let bearer = req.header("authorization").and_then(|value| {
        let (scheme, token) = value.split_once(' ')?;
        scheme.eq_ignore_ascii_case("bearer").then_some(token.trim())
    });
    let Some(bearer) = bearer else {
        return Response::error(401, "the workstation's token is sent as a Bearer token").with_header("www-authenticate", "Bearer");
    };
    let station = match ops::accept_link(app, bearer) {
        Ok(station) => station,
        Err(failure) => return fail(failure),
    };
    let weak = app.handle();
    let id = station.id;
    // A screenshot answer is one message of up to 32 MiB (the daemon's own limit), so this socket accepts frames and
    // messages that size. The link pings every 10 s and ends after 45 s of silence. Other sockets keep their limits.
    let limits = Limits {
        max_frame: 32 << 20,
        max_message: 32 << 20,
        ping_every: Duration::from_secs(10),
        dead_after: Duration::from_secs(45),
    };
    ws::accept(req, Origins::None, Some(apz_station::SUBPROTOCOL), limits, move |socket| ops::run_link(weak, id, socket))
}

fn show_route(app: &App, id: &str) -> Response {
    view_or_404(app, id)
}

/// `GET /v1/stations/audit?station=<id>&limit=<n>`: the station audit, newest first; every station's when `station`
/// is absent. `limit` is 100 by default and at most 1000.
fn audit_route(app: &App, req: &Request) -> Response {
    let limit = req.query_param("limit").and_then(|l| l.parse::<i64>().ok()).unwrap_or(100).clamp(0, 1000);
    let station = req.query_param("station").filter(|s| !s.is_empty());
    match app.db.station_audit(station, limit) {
        Ok(rows) => Response::json(200, &rows),
        Err(reason) => Response::error(500, &reason),
    }
}

/// The recordings' list: `station`, `limit` (50 by default, at most 200) and `before` (a start in ms) are optional.
fn recordings_list_route(app: &App, req: &Request) -> Response {
    let limit = req.query_param("limit").and_then(|l| l.parse::<i64>().ok()).unwrap_or(50).clamp(1, 200);
    let station = req.query_param("station").filter(|s| !s.is_empty());
    let before = match req.query_param("before") {
        Some(text) => match text.parse::<u64>() {
            Ok(ms) => Some(ms),
            Err(_) => return Response::error(400, "before is a time in milliseconds"),
        },
        None => None,
    };
    match recordings::list(app, station, before, limit) {
        Ok(rows) => Response::json(200, &rows),
        Err(reason) => Response::error(500, &reason),
    }
}

fn recording_show_route(app: &App, id: &str) -> Response {
    match recordings::detail(app, id) {
        Ok(Some(detail)) => Response::json(200, &detail),
        Ok(None) => Response::error(404, "no such recording"),
        Err(reason) => Response::error(500, &reason),
    }
}

fn recording_frame_route(app: &App, id: &str, seq: &str) -> Response {
    let Ok(seq) = seq.parse::<u64>() else {
        return Response::error(400, "a frame's seq is a number, counted from 1");
    };
    match recordings::frame(app, id, seq) {
        Ok(Some(jpeg)) => Response::new(200, "image/jpeg", jpeg).with_header("x-content-type-options", "nosniff"),
        Ok(None) => Response::error(404, "no such frame"),
        Err(reason) => Response::error(500, &reason),
    }
}

fn recording_delete_route(app: &App, id: &str) -> Response {
    match recordings::remove(app, id, OWNER) {
        Ok(rec) => Response::json(200, &json!({"id": rec.id, "deleted": true})),
        Err(failure) => fail(failure),
    }
}

fn recording_defaults_route(app: &App, req: &Request) -> Response {
    let defaults: RecordingDefaults = match parse(req) {
        Ok(defaults) => defaults,
        Err(refusal) => return refusal,
    };
    match recordings::set_defaults(app, defaults, OWNER) {
        Ok(saved) => Response::json(200, &saved),
        Err(failure) => fail(failure),
    }
}

fn drain_route(app: &App, id: &str, on: bool) -> Response {
    match ops::drain(app, id, on, OWNER) {
        Ok(_) => view_or_404(app, id),
        Err(failure) => fail(failure),
    }
}

fn revoke_route(app: &App, id: &str) -> Response {
    match ops::revoke(app, id, OWNER) {
        Ok(_) => view_or_404(app, id),
        Err(failure) => fail(failure),
    }
}

#[derive(Deserialize)]
struct RenameBody {
    name: String,
}

fn rename_route(app: &App, req: &Request, id: &str) -> Response {
    let body: RenameBody = match parse(req) {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    match ops::rename(app, id, &body.name, OWNER) {
        Ok(_) => view_or_404(app, id),
        Err(failure) => fail(failure),
    }
}

fn settings_route(app: &App, id: &str) -> Response {
    match app.stations.station(id) {
        Some(station) => Response::json(200, &station.settings),
        None => Response::error(404, "no such workstation"),
    }
}

fn put_settings_route(app: &App, req: &Request, id: &str) -> Response {
    let settings: StationSettings = match parse(req) {
        Ok(settings) => settings,
        Err(refusal) => return refusal,
    };
    match ops::put_settings(app, id, settings, OWNER) {
        Ok(answer) => Response::json(200, &answer),
        Err(failure) => fail(failure),
    }
}

#[derive(Deserialize)]
struct OpenBody {
    agent: Option<u64>,
    task: Option<String>,
    kind: Kind,
}

/// Opens a lease. On the internal route `cid` is the agent making the call and must match the body's, if any; on
/// the owner's route the body names the agent.
fn open_lease_route(app: &App, req: &Request, id: &str, cid: Option<u64>, actor: &str) -> Response {
    let body: OpenBody = match parse(req) {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    let agent = match (cid, body.agent) {
        (Some(cid), Some(named)) if named != cid => {
            return Response::error(400, "the agent in the body is not the agent in the path");
        }
        (Some(cid), _) => cid,
        (None, Some(agent)) => agent,
        (None, None) => return Response::error(400, "name the agent: {\"agent\": <cid>, \"kind\": \"pixel\" | \"shared\"}"),
    };
    match ops::open_lease(app, id, agent, body.task, body.kind, actor) {
        Ok(view) => Response::json(201, &view),
        Err(failure) => fail(failure),
    }
}

fn leases_route(app: &App, id: &str) -> Response {
    if app.stations.station(id).is_none() {
        return Response::error(404, "no such workstation");
    }
    Response::json(200, &app.stations.leases.views(id))
}

fn release_route(app: &App, id: &str, lease: &str, cid: Option<u64>, actor: &str) -> Response {
    match ops::release_lease(app, id, lease, cid, actor) {
        Ok(released) => Response::json(200, &released),
        Err(failure) => fail(failure),
    }
}
