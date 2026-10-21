//! The workstations' actions with the rest of the hub: the owner's changes and their audit, a daemon's hello
//! and events, the grants of queued pixel leases, STOP ALL's freeze, and the ticker that announces a station
//! coming online or going offline.
//!
//! Every action that changes a station or a lease writes one row to the station audit first and answers with
//! it: an audit that cannot be written is a 500, as the IDE audit's is. A daemon's hello and events run on the
//! link's own thread, so anything that waits on that link's answers is spawned on another thread.

use std::collections::BTreeMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use apz_http::ws::Socket;
use apz_station::{Event, Facts, Notice, Op, SessionKind, StationSettings};
use serde_json::{Value, json};

use super::hooks;
use super::leases::{self, Kind, Lease, LeaseView, State};
use super::link::{self, Hooks, StationLink};
use super::recordings;
use super::registry::{self, Refusal, Station};
use super::takeover;
use crate::state::App;

/// How long a daemon has to answer a settings, freeze or close request.
pub const RPC_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a pixel session may take to open (the display and the session come up first).
pub const OPEN_TIMEOUT: Duration = Duration::from_secs(20);
/// How often the ticker looks for stations that came online or went offline.
const TICK: Duration = Duration::from_secs(5);
/// A takeover is audited once per session in this window; the owner's typing is not a row per key.
pub const TAKEOVER_AUDIT_MS: u64 = 30_000;

/// An action's failure: the HTTP status and the message.
pub type Failure = (u16, String);

fn now() -> u64 {
    apz_plaza::now_ms()
}

fn server_error(message: impl ToString) -> Failure {
    (500, message.to_string())
}

/// Writes one row to the station audit: `kind` by `actor`, on `station` (None: the registry as a whole) and
/// for `agent` (a CID) when one is involved. `data` is merged into the row.
pub fn audit(
    app: &App,
    station: Option<&str>,
    agent: Option<u64>,
    kind: &str,
    actor: &str,
    data: Value,
) -> Result<(), String> {
    app.db
        .append_station_audit(&audit_row(station, agent, kind, actor, data))
}

/// The station audit row [`audit`] writes, for a caller that writes it to its own database handle.
pub fn audit_row(
    station: Option<&str>,
    agent: Option<u64>,
    kind: &str,
    actor: &str,
    data: Value,
) -> Value {
    let mut row =
        json!({"at_ms": now(), "actor": actor, "station": station, "agent": agent, "kind": kind});
    if let (Some(map), Value::Object(extra)) = (row.as_object_mut(), data) {
        map.extend(extra);
    }
    row
}

fn announce(app: &App, id: &str, action: &str) {
    app.bus
        .publish("station", &json!({"id": id, "action": action}));
}

/// The hub's hostname: what a machine reporting the same name is refused as. Empty when it cannot be read.
fn hub_host() -> String {
    let host = crate::nodes::local_summary()["host"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_string();
    if !host.is_empty() {
        return host;
    }
    std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default()
}

/// The public URL and the certificate pin a join code names.
pub(super) fn hub_pin(app: &App) -> Result<(String, [u8; 32]), Failure> {
    let url = app
        .hub_auth
        .config
        .lock()
        .map_err(|_| server_error("hub config unavailable"))?
        .public_url
        .clone();
    Ok((url, app.hub_auth.cert_sha256))
}

/// Issues a join code: its id, its text (shown once) and its expiry.
pub fn issue_code(
    app: &App,
    label: &str,
    ttl_minutes: u64,
    actor: &str,
) -> Result<(String, String, u64), Failure> {
    let (url, cert) = hub_pin(app)?;
    let (id, text, expires_ms) = app
        .stations
        .issue_code(&url, cert, label, ttl_minutes, now())
        .map_err(|e| (400, e))?;
    audit(
        app,
        None,
        None,
        "code_issue",
        actor,
        json!({"code": id, "label": label, "expires_ms": expires_ms}),
    )
    .map_err(server_error)?;
    Ok((id, text, expires_ms))
}

pub fn revoke_code(app: &App, id: &str, actor: &str) -> Result<(), Failure> {
    app.stations.revoke_code(id).map_err(|e| (404, e))?;
    audit(app, None, None, "code_revoke", actor, json!({"code": id})).map_err(server_error)
}

/// Enrolls a workstation with a join code: its id and its token (shown once). The caller has checked the
/// peer is another machine.
pub fn join(
    app: &App,
    source: &str,
    code: &str,
    facts: Facts,
    name: Option<&str>,
) -> Result<(String, String), Failure> {
    match app
        .stations
        .redeem(source, code, facts, name, &hub_host(), now())
    {
        Ok((station, token)) => {
            let station = super::vms::adopt(app, station);
            let defaults = app.settings.get().workstation_recordings;
            if let Err(reason) = app
                .stations
                .set_settings(&station.id, recordings::join_settings(&defaults))
            {
                app.stations.undo_join(&station.id);
                return Err(server_error(reason));
            }
            let row = json!({"name": station.name, "code": station.code, "address": source, "hostname": station.facts.hostname});
            if let Err(e) = audit(app, Some(&station.id), None, "join", "station", row) {
                app.stations.undo_join(&station.id);
                return Err(server_error(format!(
                    "the workstation was not enrolled: its record cannot be written ({e})"
                )));
            }
            announce(app, &station.id, "join");
            Ok((station.id, token))
        }
        Err(Refusal::RateLimited) => Err((429, crate::nodes::RATE_LIMITED.into())),
        Err(Refusal::Invalid(message)) => Err((403, message)),
        Err(Refusal::OwnHost(message)) => {
            let _ = audit(
                app,
                None,
                None,
                "join_refused",
                "station",
                json!({"address": source}),
            );
            Err((403, message))
        }
    }
}

/// Revokes a station: its token stops working, its link is told and ended, and its leases end.
pub fn revoke(app: &App, id: &str, actor: &str) -> Result<Station, Failure> {
    let station = app.stations.revoke(id).map_err(|e| (404, e))?;
    if let Some(link) = app.stations.live(id) {
        link.notify(Notice::Revoked);
        link.kill(1000, "revoked");
    }
    for lease in app.stations.leases.drop_station(id) {
        recordings::end(app, id, &lease.id);
        hooks::lease_ended(app, id, &lease, now());
        let _ = audit(
            app,
            Some(id),
            Some(lease.agent),
            "lease_ended",
            actor,
            json!({"lease": lease.id, "reason": "revoked"}),
        );
        app.bus.publish("lease_ended", &lease);
    }
    audit(
        app,
        Some(id),
        None,
        "revoke",
        actor,
        json!({"name": station.name}),
    )
    .map_err(server_error)?;
    announce(app, id, "revoked");
    Ok(station)
}

/// Starts (`on`) or ends a drain. A draining station takes no new work and its link closes once its answers are
/// in; it may link again once the drain ends.
pub fn drain(app: &App, id: &str, on: bool, actor: &str) -> Result<Station, Failure> {
    let station = app.stations.set_draining(id, on).map_err(|e| (404, e))?;
    if let Some(link) = app.stations.live(id) {
        link.set_draining(on);
        if on {
            link.notify(Notice::Drain);
        }
    }
    let action = if on { "drain" } else { "undrain" };
    audit(app, Some(id), None, action, actor, json!({})).map_err(server_error)?;
    announce(app, id, action);
    Ok(station)
}

pub fn rename(app: &App, id: &str, name: &str, actor: &str) -> Result<Station, Failure> {
    app.stations
        .station(id)
        .ok_or((404, "no such workstation".to_string()))?;
    if !registry::valid_name(name) {
        return Err((400, "the name must be 1 to 64 printable characters".into()));
    }
    let station = app.stations.rename(id, name).map_err(|e| (404, e))?;
    audit(
        app,
        Some(id),
        None,
        "rename",
        actor,
        json!({"name": station.name}),
    )
    .map_err(server_error)?;
    Ok(station)
}

/// Stores a station's settings and pushes them to its live link. The answer says whether the push landed: a
/// station that is not linked gets them when it links.
pub fn put_settings(
    app: &App,
    id: &str,
    settings: StationSettings,
    actor: &str,
) -> Result<Value, Failure> {
    recordings::check_fps(settings.record_fps)?;
    app.stations
        .set_settings(id, settings.clone())
        .map_err(|e| (404, e))?;
    let (pushed, push_error) = push_settings(app, id, &settings);
    audit(
        app,
        Some(id),
        None,
        "settings",
        actor,
        json!({"settings": settings}),
    )
    .map_err(server_error)?;
    announce(app, id, "settings");
    Ok(json!({"settings": settings, "pushed": pushed, "push_error": push_error}))
}

fn push_settings(app: &App, id: &str, settings: &StationSettings) -> (bool, Option<String>) {
    match app.stations.live(id) {
        Some(link) => match link.request(Op::Settings(settings.clone()), RPC_TIMEOUT) {
            Ok(_) => (true, None),
            Err(reason) => (false, Some(reason)),
        },
        None => (
            false,
            Some("the workstation is not linked; it gets these settings when it links".into()),
        ),
    }
}

/// Opens a lease for `agent` on `station`. A shared lease is active at once. A pixel lease is granted when the
/// machine has a session free, and queued otherwise; the answer is the lease as it stands.
pub fn open_lease(
    app: &App,
    station_id: &str,
    agent: u64,
    task: Option<String>,
    kind: Kind,
    actor: &str,
) -> Result<LeaseView, Failure> {
    let station = app
        .stations
        .station(station_id)
        .ok_or((404, "no such workstation".to_string()))?;
    if station.revoked {
        return Err((409, "this workstation was revoked".into()));
    }
    if station.draining {
        return Err((409, "this workstation is draining".into()));
    }
    if app.control.paused() {
        return Err((
            409,
            "STOP ALL holds the plaza: resume it before a lease".into(),
        ));
    }
    if !app.stations.is_online(station_id, now()) {
        return Err((409, "this workstation is offline".into()));
    }
    super::vms::before_lease(app, station_id)?;
    let lease = app
        .stations
        .leases
        .open(station_id, agent, task.clone(), kind, now())
        .map_err(|e| (409, e))?;
    if let Err(e) = audit(
        app,
        Some(station_id),
        Some(agent),
        "lease_open",
        actor,
        json!({"lease": lease.id, "kind": kind, "task": task}),
    ) {
        app.stations.leases.remove(&lease.id);
        return Err(server_error(e));
    }
    if kind == Kind::Pixel {
        grant_next(app, station_id);
    }
    let view = lease_view(app, &lease.id).ok_or((
        502,
        "the workstation could not open a pixel session; the station audit says why".to_string(),
    ))?;
    if view.lease.state == State::Queued {
        app.bus.publish("lease_waiting", &view);
    } else if kind == Kind::Shared {
        // A pixel lease's recording begins when its session is granted (grant_next); a shared one is active now.
        recordings::begin(app, station_id, &view);
        app.bus.publish("lease_granted", &view);
    }
    Ok(view)
}

/// The lease as the station lists it now (with its queue position), or None once it has ended.
fn lease_view(app: &App, lease_id: &str) -> Option<LeaseView> {
    let station = app.stations.leases.get(lease_id)?.station;
    app.stations
        .leases
        .views(&station)
        .into_iter()
        .find(|v| v.lease.id == lease_id)
}

/// Releases a lease: a pixel session it holds is closed, and the next queued lease is granted. `agent` is
/// set on the internal route, where a lease may only be released by the agent that holds it.
pub fn release_lease(
    app: &App,
    station_id: &str,
    lease_id: &str,
    agent: Option<u64>,
    actor: &str,
) -> Result<Lease, Failure> {
    release_with(app, station_id, lease_id, agent, actor, "released")
}

/// Releases the lease of a session the owner took under the `release_lease` policy (takeover.rs). The bus says why
/// (`lease_released` with reason `owner_takeover`), so the scheduler can put the task back in the queue.
pub fn release_for_takeover(app: &App, station_id: &str, lease_id: &str) -> Result<Lease, Failure> {
    release_with(app, station_id, lease_id, None, "owner", OWNER_TAKEOVER)
}

/// The reason a takeover gives for a lease's release (`lease_released`).
pub const OWNER_TAKEOVER: &str = "owner_takeover";

fn release_with(
    app: &App,
    station_id: &str,
    lease_id: &str,
    agent: Option<u64>,
    actor: &str,
    reason: &str,
) -> Result<Lease, Failure> {
    let lease = app
        .stations
        .leases
        .get(lease_id)
        .filter(|l| l.station == station_id)
        .ok_or((404, "no such lease".to_string()))?;
    if agent.is_some_and(|cid| cid != lease.agent) {
        return Err((403, "that lease belongs to another agent".into()));
    }
    app.stations
        .leases
        .remove(lease_id)
        .ok_or((404, "no such lease".to_string()))?;
    recordings::end(app, station_id, lease_id);
    hooks::lease_ended(app, station_id, &lease, now());
    let close_error = match (lease.kind, lease.session, app.stations.live(station_id)) {
        (Kind::Pixel, Some(session), Some(link)) => link
            .request(Op::SessionClose { session }, RPC_TIMEOUT)
            .err(),
        _ => None,
    };
    audit(app, Some(station_id), Some(lease.agent), "lease_released", actor, json!({"lease": lease.id, "session": lease.session, "close_error": close_error, "reason": reason}))
        .map_err(server_error)?;
    let mut data = serde_json::to_value(&lease).unwrap_or(Value::Null);
    if let Value::Object(map) = &mut data {
        map.insert("reason".into(), json!(reason));
    }
    app.bus.publish("lease_released", &data);
    // The owner holds the screen after a takeover, so the queue waits for the give back (takeover.rs grants it then).
    if reason != OWNER_TAKEOVER {
        grant_next(app, station_id);
    }
    Ok(lease)
}

/// Grants queued pixel leases of `station` while the machine has room: each opens its session on the daemon.
/// Stops at STOP ALL, at a station with no link (the leases wait), at a failed open (its lease is dropped), and on a
/// machine with one session while the owner holds a pixel session there (takeover.rs `pixel_held`).
pub fn grant_next(app: &App, station_id: &str) {
    loop {
        if app.control.paused() || app.stations.is_resetting(station_id) {
            return;
        }
        let Some(station) = app.stations.station(station_id) else {
            return;
        };
        let capacity = leases::pixel_capacity(&station.facts);
        if capacity == Some(1) && takeover::pixel_held(app, station_id) {
            return;
        }
        let Some(lease) = app.stations.leases.grant_next(station_id, capacity) else {
            return;
        };
        let Some(link) = app.stations.live(station_id) else {
            app.stations.leases.requeue(&lease.id);
            return;
        };
        let opened = link
            .request(
                Op::SessionOpen {
                    agent: lease.agent,
                    kind: SessionKind::Pixel,
                },
                OPEN_TIMEOUT,
            )
            .and_then(|answer| {
                answer["session"]
                    .as_u64()
                    .ok_or_else(|| "the station's answer names no session".to_string())
            });
        match opened {
            Ok(session) => {
                if app.stations.leases.granted(&lease.id, session, now()) {
                    let _ = audit(
                        app,
                        Some(station_id),
                        Some(lease.agent),
                        "lease_granted",
                        "station",
                        json!({"lease": lease.id, "session": session}),
                    );
                    if let Some(view) = lease_view(app, &lease.id) {
                        recordings::begin(app, station_id, &view);
                        app.bus.publish("lease_granted", &view);
                    }
                } else {
                    // Released while it was opening: the session it opened is given back.
                    let _ = link.request(Op::SessionClose { session }, RPC_TIMEOUT);
                }
            }
            Err(reason) => {
                app.stations.leases.remove(&lease.id);
                let _ = audit(
                    app,
                    Some(station_id),
                    Some(lease.agent),
                    "lease_failed",
                    "station",
                    json!({"lease": lease.id, "reason": reason}),
                );
                app.bus.publish(
                    "lease_failed",
                    &json!({"lease": lease.id, "station": station_id, "reason": reason}),
                );
            }
        }
    }
}

/// `grant_next` on its own thread, for a caller that holds a link's thread or a takeover's lock.
pub fn schedule_grant(app: &App, station_id: &str) {
    let weak = app.handle();
    let station_id = station_id.to_string();
    std::thread::spawn(move || {
        if let Some(app) = weak.upgrade() {
            grant_next(&app, &station_id);
        }
    });
}

/// STOP ALL (`on`) and its end: every live station is frozen, or thawed and its queue granted again. Each
/// request runs on its own thread, so the owner's stop is not held up by a slow daemon.
pub fn freeze_all(app: &App, on: bool, actor: &str) {
    for link in app.stations.live_links() {
        let weak = app.handle();
        let actor = actor.to_string();
        std::thread::spawn(move || {
            let station = link.station().to_string();
            let answer = link.request(Op::Freeze { on }, RPC_TIMEOUT);
            let Some(app) = weak.upgrade() else { return };
            let (kind, data) = match &answer {
                Ok(_) => (if on { "freeze" } else { "unfreeze" }, json!({})),
                Err(reason) => ("freeze_failed", json!({"reason": reason})),
            };
            let _ = audit(&app, Some(&station), None, kind, &actor, data);
            if !on {
                grant_next(&app, &station);
            }
        });
    }
}

/// Starts the ticker: a station that came online or went offline since the last tick is announced on the bus.
pub fn spawn(app: Arc<App>) {
    recordings::spawn_sampler(&app);
    recordings::spawn_sweep(&app);
    std::thread::spawn(move || {
        let mut seen: BTreeMap<String, bool> = BTreeMap::new();
        loop {
            std::thread::sleep(TICK);
            for station in app.stations.list() {
                let online = app.stations.is_online(&station.id, now());
                if seen
                    .insert(station.id.clone(), online)
                    .is_some_and(|was| was != online)
                {
                    announce(&app, &station.id, if online { "online" } else { "offline" });
                }
            }
            // The owner's input takes whose owner has gone quiet end here (takeover.rs).
            takeover::sweep(&app);
        }
    });
}

/// A link's bearer token, checked before the socket is upgraded: a station that is draining may not link.
pub fn accept_link(app: &App, bearer: &str) -> Result<Station, Failure> {
    let station = app.stations.authenticate(bearer).map_err(|e| (401, e))?;
    if station.draining {
        return Err((
            503,
            "this workstation is draining; it can link again once the drain ends".into(),
        ));
    }
    Ok(station)
}

/// Serves one station's link on the socket until it ends. The link is attached on the daemon's hello; a link
/// that ends is detached (and not the replacement that took its place).
pub fn run_link(weak: Weak<App>, station: String, mut socket: Socket) {
    let Some(app) = weak.upgrade() else {
        socket.close(1001, "the hub is shutting down");
        return;
    };
    let id = app.stations.next_link_id();
    let (link, outgoing) = StationLink::new(&station, id, now());
    let result = link::serve(&mut socket, &link, &outgoing, &Live(app.as_ref()));
    link.finish();
    app.stations.detach(&station, id);
    app.stations.seen(&station, now());
    if let Err(reason) = result {
        eprintln!("apid: workstation {station} link ended: {reason}");
    }
}

/// The hub's side of a live link.
struct Live<'a>(&'a App);

impl Hooks for Live<'_> {
    fn hello(&self, link: &Arc<StationLink>, version: &str, facts: Facts) -> Result<(), String> {
        let app = self.0;
        let id = link.station();
        let station = app.stations.station(id).ok_or("no such workstation")?;
        if station.revoked {
            return Err("this workstation was revoked".into());
        }
        if !registry::may_join_facts(&facts, &hub_host(), apz_os::machine::id().as_deref()) {
            return Err("this machine is the hub's own host, or its hostname cannot be told apart from the hub's".into());
        }
        let facts = super::vms::stamp_form(app, id, facts);
        app.stations.hello(id, version, facts, now())?;
        if let Some(old) = app.stations.attach(Arc::clone(link)) {
            old.kill(1000, "replaced by a newer link");
        }
        announce(app, id, "online");
        // A freeze if STOP ALL holds the plaza (first, so a station that restarted unfrozen is frozen before
        // anything else reaches it), its settings, and the queue: all of them need this link's answers.
        let weak = app.handle();
        let link = Arc::clone(link);
        let id = id.to_string();
        std::thread::spawn(move || {
            let Some(app) = weak.upgrade() else { return };
            if app.control.paused()
                && let Err(reason) = link.request(Op::Freeze { on: true }, RPC_TIMEOUT)
            {
                eprintln!("apid: workstation {id}: STOP ALL was not replayed on link: {reason}");
            }
            if let Err(reason) = link.request(Op::Settings(station.settings.clone()), RPC_TIMEOUT) {
                eprintln!(
                    "apid: workstation {id}: its settings were not applied on link: {reason}"
                );
            }
            grant_next(&app, &id);
            // The work folder: a station that was away may have changed files, and the hub may have written some.
            super::sync::kick(&app, &id);
        });
        Ok(())
    }

    fn event(&self, link: &Arc<StationLink>, event: Event) {
        let app = self.0;
        let id = link.station();
        match event {
            Event::Facts(facts) => {
                if !registry::may_join_facts(&facts, &hub_host(), apz_os::machine::id().as_deref())
                {
                    link.kill(1008, "this machine is the hub's own host, or its hostname cannot be told apart from the hub's");
                    return;
                }
                app.stations.set_facts(id, facts);
            }
            Event::SessionEnded { session, reason } => {
                takeover::session_ended(app, id, session);
                if let Some(lease) = app.stations.leases.by_session(id, session) {
                    app.stations.leases.remove(&lease.id);
                    recordings::end(app, id, &lease.id);
                    hooks::lease_ended(app, id, &lease, now());
                    let _ = audit(
                        app,
                        Some(id),
                        Some(lease.agent),
                        "lease_ended",
                        "station",
                        json!({"lease": lease.id, "session": session, "reason": reason}),
                    );
                    app.bus.publish("lease_ended", &lease);
                    schedule_grant(app, id);
                }
            }
            Event::OwnerInput { session } => {
                takeover::owner_input(app, id, session);
                let Some(lease) = app.stations.leases.by_session(id, session) else {
                    return;
                };
                recordings::on_owner_input(app, &lease.id);
                if app.stations.takeover_due(session, now(), TAKEOVER_AUDIT_MS) {
                    let _ = audit(
                        app,
                        Some(id),
                        Some(lease.agent),
                        "owner_input",
                        "owner",
                        json!({"lease": lease.id, "session": session}),
                    );
                }
                app.bus.publish("lease_owner_input", &lease);
            }
            Event::Log { level, message } => {
                eprintln!("apid: workstation {id}: {level:?}: {message}")
            }
            // The station's manifest is asked for by the pass, so a change is only a reason to run one.
            Event::SyncChanged { .. } => super::sync::kick(app, id),
            Event::Frame { .. } => {}
        }
    }
}
