//! The one choke point for what agents and the owner's MCP client do on a workstation. Every operation a station
//! receives goes through `agent_op`, and every lease opens and closes through `lease_open` and `lease_release`.
//! Each checks, in order: the station is known, online, not revoked or draining; a screen operation names this
//! agent's active pixel lease on the station, with the session the operation names; the laws allow the call
//! (`tools.jhrl`, T-014). Each attempt is audited in the station audit, refused or not, and `hooks` sees it after.
//!
//! The rules live in `run_op` over the `Hub` trait, so the tests run them without a database.

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use apz_station::{AppsOp, Op};
use serde_json::{Value, json};

use super::audit::StationAudit;
use super::hooks;
use super::leases::{Kind, Lease, LeaseView, State};
use super::link::StationLink;
use super::ops::{self, Failure, RPC_TIMEOUT};
use super::registry::Station;
use super::sync;
use super::takeover;
use crate::laws::OWNER;
use crate::state::App;

/// How long a pixel lease waits for its session before it answers with where it stands in the queue.
const GRANT_WAIT: Duration = Duration::from_secs(25);
/// How often that wait looks again.
const GRANT_POLL: Duration = Duration::from_millis(250);
/// The longest a command or script in an audit row may be before it is cut.
const AUDIT_TEXT_CHARS: usize = 500;
/// The refusal of a screenshot or an input while the owner holds the screen (takeover.rs).
const OWNER_HAS_SCREEN: &str = "the owner has the screen; wait";

/// What the checks need from the hub. `AppHub` is the real one; the tests use a fake.
pub(crate) trait Hub {
    fn station(&self, id: &str) -> Option<Station>;
    /// The station's live link, when it is online.
    fn link(&self, id: &str) -> Option<Arc<StationLink>>;
    fn lease(&self, id: &str) -> Option<Lease>;
    /// Whether the owner's takeover refuses the agent's screenshots on `session` (`agent_waits`, `release_lease`).
    fn pixel_paused(&self, station: &str, session: u64) -> bool;
    /// Whether the owner's takeover refuses the agent's pointer and keyboard on `session` (every policy).
    fn pointer_paused(&self, station: &str, session: u64) -> bool;
    /// The laws' refusal of a call (`Laws::refusal`), `None` when it may run.
    fn refusal(&self, cid: u64, role: &str, tool: &str, input: &Value) -> Option<String>;
    /// Writes one entry to the station audit.
    fn audit(
        &self,
        station: &str,
        cid: u64,
        actor: &str,
        event: StationAudit,
    ) -> Result<(), String>;
}

struct AppHub<'a>(&'a App);

impl Hub for AppHub<'_> {
    fn station(&self, id: &str) -> Option<Station> {
        self.0.stations.station(id)
    }

    fn link(&self, id: &str) -> Option<Arc<StationLink>> {
        if self.0.stations.is_online(id, apz_plaza::now_ms()) {
            self.0.stations.live(id)
        } else {
            None
        }
    }

    fn lease(&self, id: &str) -> Option<Lease> {
        self.0.stations.leases.get(id)
    }

    fn pixel_paused(&self, station: &str, session: u64) -> bool {
        takeover::pixel_paused(self.0, station, session)
    }

    fn pointer_paused(&self, station: &str, session: u64) -> bool {
        takeover::pointer_paused(self.0, station, session)
    }

    fn refusal(&self, cid: u64, role: &str, tool: &str, input: &Value) -> Option<String> {
        self.0.laws.refusal(cid, role, tool, input)
    }

    fn audit(
        &self,
        station: &str,
        cid: u64,
        actor: &str,
        event: StationAudit,
    ) -> Result<(), String> {
        ops::audit(self.0, Some(station), Some(cid), actor, event)
    }
}

/// The role an action runs under: the agent's, or the owner's for the owner's MCP client (cid 0).
pub(crate) fn role_of(app: &App, cid: u64) -> Result<String, Failure> {
    if cid == 0 {
        return Ok(OWNER.to_string());
    }
    app.agents
        .get(cid)
        .map(|agent| agent.role)
        .ok_or_else(|| (404, format!("no agent {cid}")))
}

/// Who an action is recorded as acting: the agent, or the owner.
fn actor_of(cid: u64) -> String {
    if cid == 0 {
        OWNER.to_string()
    } else {
        format!("agent:{cid}")
    }
}

/// Sends `op` to `station` for `cid` acting under `role`, under `lease` when it names one. Every attempt is audited,
/// refused or not; an audit that cannot be written is a 500, and says whether the operation ran first.
pub(crate) fn run_op(
    hub: &dyn Hub,
    cid: u64,
    role: &str,
    station: &str,
    lease: Option<&str>,
    op: Op,
) -> Result<Value, Failure> {
    let summary = summary(&op);
    let outcome = send(hub, cid, role, station, lease, op);
    let event = StationAudit::AgentOp {
        op: summary,
        lease: lease.map(str::to_string),
        ok: outcome.is_ok(),
        error: outcome.as_ref().err().map(|(_, message)| message.clone()),
    };
    match hub.audit(station, cid, &actor_of(cid), event) {
        Ok(()) => outcome,
        Err(e) if outcome.is_ok() => Err((
            500,
            format!("the action ran, but the station audit could not be written: {e}"),
        )),
        Err(e) => Err((500, format!("the station audit could not be written: {e}"))),
    }
}

/// The checks and the send, without the audit.
fn send(
    hub: &dyn Hub,
    cid: u64,
    role: &str,
    station: &str,
    lease: Option<&str>,
    op: Op,
) -> Result<Value, Failure> {
    let Some(tool) = law_name(&op) else {
        return Err((
            403,
            format!("no agent may send {} to a workstation", tag(&op)),
        ));
    };
    let target = hub
        .station(station)
        .ok_or((404, "no such workstation".to_string()))?;
    if target.revoked {
        return Err((409, "this workstation was revoked".into()));
    }
    if target.draining {
        return Err((409, "this workstation is draining".into()));
    }
    let link = hub
        .link(station)
        .ok_or((409, "this workstation is offline".to_string()))?;
    if let Some(session) = pixel_session(&op) {
        let held = lease.and_then(|id| hub.lease(id));
        let ours = held.is_some_and(|held| {
            held.station == station
                && held.agent == cid
                && held.kind.holds_session()
                && held.state == State::Active
                && held.session == Some(session)
        });
        if !ours {
            return Err((
                403,
                "a screen operation needs your active pixel lease on this workstation (station_lease), and its session".into(),
            ));
        }
        // The owner has the screen: the screenshots and the pointer and keyboard wait (takeover.rs). An accessibility
        // operation (the UI tree, a UI action) is not a pixel one, so it goes on under every policy.
        let blocked = match &op {
            Op::Screenshot { .. } => hub.pixel_paused(station, session),
            Op::Input { .. } => {
                hub.pixel_paused(station, session) || hub.pointer_paused(station, session)
            }
            _ => false,
        };
        if blocked {
            return Err((409, OWNER_HAS_SCREEN.into()));
        }
    }
    let input = json!({"station": station, "op": &op});
    if let Some(why) = hub.refusal(cid, role, tool, &input) {
        return Err((403, why));
    }
    let wait = timeout(&op);
    link.request(op, wait).map_err(|e| (502, e))
}

/// The laws' name for an operation: the workstation tool it belongs to (`tools.jhrl` permits these names). `None` for
/// the operations no agent sends: the freeze, settings, sync, the live stream, the session plumbing that
/// `station_lease` does, and the app focus and quit that no tool offers.
pub(super) fn law_name(op: &Op) -> Option<&'static str> {
    use apz_station::InputAction;
    Some(match op {
        Op::Screenshot { .. } => "station_screenshot",
        Op::Input { action, .. } => match action {
            InputAction::Move { .. }
            | InputAction::Click { .. }
            | InputAction::Down { .. }
            | InputAction::Up { .. } => "station_click",
            InputAction::Type { .. } => "station_type",
            InputAction::Key { .. } => "station_key",
            InputAction::Scroll { .. } => "station_scroll",
            InputAction::Drag { .. } => "station_drag",
        },
        Op::UiTree { .. } => "station_ui_tree",
        Op::UiAct { .. } => "station_ui_act",
        Op::Term(_) => "station_shell",
        Op::Run(_) => "station_run",
        Op::Files(_) => "station_files",
        Op::Apps(AppsOp::List) => "station_app_list",
        Op::Apps(AppsOp::Launch { .. }) => "station_app_launch",
        Op::Script { .. } => "app_script",
        Op::Facts
        | Op::SessionOpen { .. }
        | Op::SessionClose { .. }
        | Op::Apps(AppsOp::Focus { .. } | AppsOp::Quit { .. })
        | Op::Stream { .. }
        | Op::Freeze { .. }
        | Op::Sync(_)
        | Op::Settings(_) => return None,
    })
}

/// The session a screen operation names; `None` for the operations that need no lease.
fn pixel_session(op: &Op) -> Option<u64> {
    match op {
        Op::Screenshot { session, .. }
        | Op::Input { session, .. }
        | Op::UiTree { session, .. }
        | Op::UiAct { session, .. } => Some(*session),
        _ => None,
    }
}

/// How long the answer may take: a script runs for its own timeout, everything else is quick.
fn timeout(op: &Op) -> Duration {
    match op {
        Op::Script { timeout_s, .. } => RPC_TIMEOUT + Duration::from_secs(u64::from(*timeout_s)),
        _ => RPC_TIMEOUT,
    }
}

/// The operation's tag (its `op` field).
fn tag(op: &Op) -> String {
    serde_json::to_value(op)
        .ok()
        .and_then(|value| value["op"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// The operation as the audit keeps it: its fields, with the typed text, the file contents and the values replaced by
/// their sizes, and long commands and script code cut short.
pub(super) fn summary(op: &Op) -> Value {
    let mut value = serde_json::to_value(op).unwrap_or(Value::Null);
    redact(&mut value);
    value
}

fn redact(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map.iter_mut() {
                let shrunk = match (key.as_str(), inner.as_str()) {
                    ("text" | "value" | "data_b64", Some(text)) => {
                        Some(json!({"bytes": text.len()}))
                    }
                    ("code" | "command", Some(text)) if text.chars().count() > AUDIT_TEXT_CHARS => {
                        Some(Value::String(text.chars().take(AUDIT_TEXT_CHARS).collect()))
                    }
                    _ => None,
                };
                match shrunk {
                    Some(shrunk) => *inner = shrunk,
                    None => redact(inner),
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact),
        _ => {}
    }
}

/// The one way an operation reaches a station: an agent's (its cid), or the owner's (cid 0, the owner's MCP client).
pub fn agent_op(
    app: &App,
    cid: u64,
    station: &str,
    lease: Option<&str>,
    op: Op,
) -> Result<Value, Failure> {
    let role = role_of(app, cid)?;
    let result = run_op(&AppHub(app), cid, &role, station, lease, op.clone());
    hooks::after_agent_op(app, station, lease, &op, &result);
    result
}

/// An agent puts a mirrored file of `station` into the Library (`station_files` with `deliver`): credited to the agent,
/// and to the task of its lease on the station when it holds one. Through `run_deliver`, so the laws and the audit apply.
pub fn agent_deliver(app: &App, cid: u64, station: &str, path: &str) -> Result<Value, Failure> {
    let role = role_of(app, cid)?;
    let name = match cid {
        0 => OWNER.to_string(),
        _ => app
            .agents
            .get(cid)
            .map(|agent| agent.label())
            .unwrap_or_else(|| format!("agent {cid}")),
    };
    let task = app
        .stations
        .leases
        .views(station)
        .into_iter()
        .find(|view| view.lease.agent == cid)
        .and_then(|view| view.lease.task);
    run_deliver(&AppHub(app), cid, &role, station, path, || {
        let item = sync::deliver(
            app,
            station,
            path,
            None,
            None,
            task.clone(),
            (cid, name.as_str()),
        )?;
        Ok(
            json!({"id": item.id, "name": item.name, "size": item.size, "kind": item.kind, "audience": item.audience, "task": item.task}),
        )
    })
}

/// The checks and the copy of a delivery, audited like an operation: `perform` does the copy once the laws allow it.
pub(crate) fn run_deliver(
    hub: &dyn Hub,
    cid: u64,
    role: &str,
    station: &str,
    path: &str,
    perform: impl FnOnce() -> Result<Value, Failure>,
) -> Result<Value, Failure> {
    let outcome = deliver_checks(hub, cid, role, station, path).and_then(|()| perform());
    let event = StationAudit::AgentOp {
        op: json!({"op": "deliver", "path": path}),
        lease: None,
        ok: outcome.is_ok(),
        error: outcome.as_ref().err().map(|(_, message)| message.clone()),
    };
    match hub.audit(station, cid, &actor_of(cid), event) {
        Ok(()) => outcome,
        Err(e) if outcome.is_ok() => Err((
            500,
            format!("the action ran, but the station audit could not be written: {e}"),
        )),
        Err(e) => Err((500, format!("the station audit could not be written: {e}"))),
    }
}

/// The station is known and not revoked, and the laws allow the file tool's call with `deliver` (the laws name it
/// `station_files`, as they do the file operations).
fn deliver_checks(
    hub: &dyn Hub,
    cid: u64,
    role: &str,
    station: &str,
    path: &str,
) -> Result<(), Failure> {
    let target = hub
        .station(station)
        .ok_or((404, "no such workstation".to_string()))?;
    if target.revoked {
        return Err((409, "this workstation was revoked".into()));
    }
    let input = json!({"station": station, "deliver": true, "path": path});
    match hub.refusal(cid, role, "station_files", &input) {
        Some(why) => Err((403, why)),
        None => Ok(()),
    }
}

/// Opens a lease of `kind` on `station` for `cid`: the laws decide, then the station's rules (`ops::open_lease`).
/// A pixel lease that is queued waits up to `GRANT_WAIT` for its session; the answer is the lease as it stands then.
pub fn lease_open(
    app: &App,
    cid: u64,
    station: &str,
    kind: Kind,
    task: Option<String>,
    device: Option<String>,
) -> Result<LeaseView, Failure> {
    let role = role_of(app, cid)?;
    let input = json!({"station": station, "kind": kind, "task": task, "device": device});
    if let Some(why) = app.laws.refusal(cid, &role, "station_lease", &input) {
        return Err(refused(
            app,
            station,
            cid,
            json!({"op": "lease", "kind": kind}),
            why,
        ));
    }
    let view = ops::open_lease(app, station, cid, task, kind, device, &actor_of(cid))?;
    Ok(wait_for_grant(app, station, view))
}

/// Releases a lease on `station`: the laws decide, then `ops::release_lease`. An agent releases only its own; the
/// owner's client (cid 0) may release any.
pub fn lease_release(app: &App, cid: u64, station: &str, lease: &str) -> Result<Lease, Failure> {
    let role = role_of(app, cid)?;
    let input = json!({"station": station, "lease": lease});
    if let Some(why) = app.laws.refusal(cid, &role, "station_release", &input) {
        return Err(refused(
            app,
            station,
            cid,
            json!({"op": "release", "lease": lease}),
            why,
        ));
    }
    ops::release_lease(
        app,
        station,
        lease,
        (cid != 0).then_some(cid),
        &actor_of(cid),
    )
}

/// A lease the laws refused: audited as a refused operation, and answered with the law's reason.
fn refused(app: &App, station: &str, cid: u64, op: Value, why: String) -> Failure {
    let event = StationAudit::AgentOp {
        op,
        lease: None,
        ok: false,
        error: Some(why.clone()),
    };
    match ops::audit(app, Some(station), Some(cid), &actor_of(cid), event) {
        Ok(()) => (403, why),
        Err(e) => (500, e),
    }
}

fn wait_for_grant(app: &App, station: &str, view: LeaseView) -> LeaseView {
    let started = Instant::now();
    let id = view.lease.id.clone();
    let mut current = view;
    while current.lease.state != State::Active && started.elapsed() < GRANT_WAIT {
        thread::sleep(GRANT_POLL);
        match app
            .stations
            .leases
            .views(station)
            .into_iter()
            .find(|seen| seen.lease.id == id)
        {
            Some(next) => current = next,
            // The lease ended while it waited: released, or its session failed to open.
            None => break,
        }
    }
    current
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::thread::{self, JoinHandle};

    use apz_station::{FilesOp, HubFrame, ImageFormat, StationFrame, StationSettings};

    use super::*;
    use crate::laws::Laws;
    use crate::stations::leases::Leases;
    use crate::stations::link::tests::{Accept, Daemon, facts, greet, start};
    use crate::stations::tools::{Call, parse};

    /// A rule an owner might add: no clicks, for any agent.
    const NO_CLICKS: &str = r#"law "O-001" "No clicks" {
  tier statute
  enforcement preventive
  severity info
  statement "Test rule: no agent may click."
  forbid tool.call
  principal is Agent
  resource is Tool
  when resource.name == "station_click"
}
"#;

    /// A rule against the file tool, deliveries included.
    const NO_FILES: &str = r#"law "O-002" "No files" {
  tier statute
  enforcement preventive
  severity info
  statement "Test rule: no agent may use the file tool."
  forbid tool.call
  principal is Agent
  resource is Tool
  when resource.name == "station_files"
}
"#;

    /// The hub's pieces for station `w1`: its record, its link when online, a lease table, the laws, and the audit
    /// rows the test reads.
    struct FakeHub {
        station: Station,
        link: Option<Arc<StationLink>>,
        leases: Leases,
        laws: Laws,
        audit: Mutex<Vec<Value>>,
        audit_broken: bool,
        /// The owner holds the screen under `agent_waits` or `release_lease`: the agent's pixels are refused.
        screen_taken: bool,
        /// The owner holds the session under any policy: the agent's pointer and keyboard are refused.
        pointer_taken: bool,
    }

    impl Hub for FakeHub {
        fn station(&self, id: &str) -> Option<Station> {
            (id == self.station.id).then(|| self.station.clone())
        }

        fn link(&self, id: &str) -> Option<Arc<StationLink>> {
            if id == self.station.id {
                self.link.clone()
            } else {
                None
            }
        }

        fn lease(&self, id: &str) -> Option<Lease> {
            self.leases.get(id)
        }

        fn pixel_paused(&self, _station: &str, _session: u64) -> bool {
            self.screen_taken
        }

        fn pointer_paused(&self, _station: &str, _session: u64) -> bool {
            self.pointer_taken
        }

        fn refusal(&self, cid: u64, role: &str, tool: &str, input: &Value) -> Option<String> {
            self.laws.refusal(cid, role, tool, input)
        }

        fn audit(
            &self,
            station: &str,
            cid: u64,
            actor: &str,
            event: StationAudit,
        ) -> Result<(), String> {
            if self.audit_broken {
                return Err("disk full".into());
            }
            let data = serde_json::to_value(&event).unwrap_or(Value::Null);
            self.audit.lock().unwrap().push(
                json!({"station": station, "agent": cid, "kind": data["kind"], "actor": actor, "data": data}),
            );
            Ok(())
        }
    }

    /// The built-in laws, with no owner rules.
    fn laws() -> Laws {
        Laws::builtin()
    }

    fn station(draining: bool, revoked: bool) -> Station {
        Station {
            id: "w1".into(),
            name: "desk".into(),
            version: "0.0.8".into(),
            code: "j1".into(),
            token_sha256: String::new(),
            joined_ms: 0,
            last_seen_ms: 0,
            facts: facts(),
            settings: StationSettings::default(),
            draining,
            revoked,
            address: String::new(),
            sync_pause: None,
        }
    }

    fn hub(link: Option<Arc<StationLink>>) -> FakeHub {
        FakeHub {
            station: station(false, false),
            link,
            leases: Leases::default(),
            laws: laws(),
            audit: Mutex::new(Vec::new()),
            audit_broken: false,
            screen_taken: false,
            pointer_taken: false,
        }
    }

    /// An online station: its daemon has said hello, and the test plays the daemon.
    fn online() -> (Arc<StationLink>, Daemon) {
        let (link, daemon, _served) = start("w1", Arc::new(Accept));
        greet(&link, &daemon);
        (link, daemon)
    }

    /// Answers the next request the hub sends with `result`, on another thread. Joins to the op it was asked and the
    /// daemon back.
    fn answer(daemon: Daemon, result: Value) -> JoinHandle<(Op, Daemon)> {
        thread::spawn(move || {
            let HubFrame::Request { id, op } = daemon.hears() else {
                panic!("a request")
            };
            daemon.say(&StationFrame::Response {
                id,
                ok: true,
                result: Some(result),
                error: None,
            });
            (op, daemon)
        })
    }

    /// Opens a pixel lease for `cid` on `w1` and grants it `session`; returns its id.
    fn pixel_lease(hub: &FakeHub, cid: u64, session: u64) -> String {
        let lease = hub.leases.open("w1", cid, None, Kind::Pixel, 0).unwrap();
        assert_eq!(
            hub.leases
                .grant_next("w1", None)
                .expect("a session is free")
                .id,
            lease.id
        );
        assert!(hub.leases.granted(&lease.id, session, 0));
        lease.id
    }

    fn screenshot(session: u64) -> Op {
        Op::Screenshot {
            session,
            scale: 0.5,
            grid: false,
            format: ImageFormat::Png,
        }
    }

    fn click(session: u64) -> Op {
        Op::Input {
            session,
            action: apz_station::InputAction::Click {
                x: 1,
                y: 2,
                button: apz_station::Button::Left,
                count: 1,
            },
        }
    }

    fn last_audit(hub: &FakeHub) -> Value {
        hub.audit
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("an audit row")
    }

    #[test]
    fn a_screen_operation_needs_the_pixel_lease() {
        let (link, daemon) = online();
        let hub = hub(Some(link));
        let (status, message) = run_op(&hub, 7, "builder", "w1", None, screenshot(9)).unwrap_err();
        assert_eq!(status, 403);
        assert!(message.contains("pixel lease"), "{message}");
        assert!(
            daemon.hears.try_recv().is_err(),
            "nothing was sent to the station"
        );
        assert_eq!(last_audit(&hub)["data"]["ok"], false);
    }

    #[test]
    fn an_owner_screen_lease_runs_the_screen_operations_on_its_session() {
        let (link, daemon) = online();
        let hub = hub(Some(link));
        let lease = hub
            .leases
            .open("w1", 7, None, Kind::OwnerScreen, 0)
            .unwrap();
        assert!(
            hub.leases
                .grant_next("w1", None)
                .is_some_and(|granted| granted.id == lease.id)
        );
        assert!(hub.leases.granted(&lease.id, 11, 0));
        let asked = answer(
            daemon,
            json!({"w": 1, "h": 1, "format": "png", "data_b64": "AA=="}),
        );
        run_op(&hub, 7, "builder", "w1", Some(&lease.id), screenshot(11)).unwrap();
        let (op, _) = asked.join().unwrap();
        assert!(matches!(op, Op::Screenshot { session: 11, .. }));
    }

    #[test]
    fn a_lease_of_another_agent_or_of_another_session_is_refused_and_the_held_one_runs() {
        let (link, daemon) = online();
        let hub = hub(Some(link));
        let theirs = pixel_lease(&hub, 8, 9);
        let mine = pixel_lease(&hub, 7, 10);
        let (status, _) =
            run_op(&hub, 7, "builder", "w1", Some(&theirs), screenshot(9)).unwrap_err();
        assert_eq!(status, 403);
        let (status, message) =
            run_op(&hub, 7, "builder", "w1", Some(&mine), screenshot(9)).unwrap_err();
        assert_eq!(status, 403);
        assert!(message.contains("pixel lease"), "{message}");
        assert!(
            daemon.hears.try_recv().is_err(),
            "nothing was sent for either refusal"
        );
        let asked = answer(
            daemon,
            json!({"w": 1, "h": 1, "format": "png", "data_b64": "AA=="}),
        );
        let shot = run_op(&hub, 7, "builder", "w1", Some(&mine), screenshot(10)).unwrap();
        assert_eq!(shot["w"], 1);
        let (op, _) = asked.join().unwrap();
        assert!(matches!(op, Op::Screenshot { session: 10, .. }));
        assert_eq!(last_audit(&hub)["data"]["ok"], true);
    }

    #[test]
    fn the_owners_screen_and_pointer_takeovers_pause_only_what_they_take() {
        // The owner has the screen: the agent's screenshots and clicks wait, and the refused click is audited.
        {
            let (link, daemon) = online();
            let mut hub = hub(Some(link));
            let lease = pixel_lease(&hub, 7, 9);
            hub.screen_taken = true;
            let (status, message) =
                run_op(&hub, 7, "builder", "w1", Some(&lease), screenshot(9)).unwrap_err();
            assert_eq!(
                (status, message.as_str()),
                (409, "the owner has the screen; wait")
            );
            let (status, message) =
                run_op(&hub, 7, "builder", "w1", Some(&lease), click(9)).unwrap_err();
            assert_eq!(
                (status, message.as_str()),
                (409, "the owner has the screen; wait")
            );
            assert!(
                daemon.hears.try_recv().is_err(),
                "nothing reached the station"
            );
            assert_eq!(
                last_audit(&hub)["data"]["ok"],
                false,
                "the refused click is audited"
            );
            assert!(
                last_audit(&hub)["data"]["error"]
                    .as_str()
                    .unwrap()
                    .contains("the owner has the screen")
            );

            hub.screen_taken = false;
            let asked = answer(
                daemon,
                json!({"w": 1, "h": 1, "format": "png", "data_b64": "AA=="}),
            );
            assert!(
                run_op(&hub, 7, "builder", "w1", Some(&lease), screenshot(9)).is_ok(),
                "back when the owner gives it back"
            );
            asked.join().unwrap();
        }
        // Under a pointer pause the screenshots go on, and only the pointer waits.
        {
            let (link, daemon) = online();
            let mut hub = hub(Some(link));
            let lease = pixel_lease(&hub, 7, 9);
            hub.pointer_taken = true;
            let asked = answer(
                daemon,
                json!({"w": 1, "h": 1, "format": "png", "data_b64": "AA=="}),
            );
            assert!(
                run_op(&hub, 7, "builder", "w1", Some(&lease), screenshot(9)).is_ok(),
                "screenshots continue"
            );
            asked.join().unwrap();
            let (status, message) =
                run_op(&hub, 7, "builder", "w1", Some(&lease), click(9)).unwrap_err();
            assert_eq!(
                (status, message.as_str()),
                (409, "the owner has the screen; wait"),
                "the pointer waits"
            );
        }
    }

    #[test]
    fn the_laws_refuse_before_anything_is_sent() {
        let (link, daemon) = online();
        let hub = hub(Some(link));
        let lease = pixel_lease(&hub, 7, 9);
        hub.laws
            .add_rule("No clicks", NO_CLICKS, true, 0)
            .expect("the rule compiles");
        let (status, message) =
            run_op(&hub, 7, "builder", "w1", Some(&lease), click(9)).unwrap_err();
        assert_eq!(status, 403);
        assert!(message.starts_with("refused by law O-001"), "{message}");
        assert!(daemon.hears.try_recv().is_err(), "nothing was sent");
        assert_eq!(last_audit(&hub)["data"]["ok"], false);
        assert!(
            last_audit(&hub)["data"]["error"]
                .as_str()
                .unwrap()
                .contains("O-001")
        );
    }

    #[test]
    fn an_unknown_offline_revoked_or_draining_station_is_refused() {
        let (link, daemon) = online();
        let offline = hub(None);
        let (status, message) =
            run_op(&offline, 7, "builder", "w1", None, Op::Apps(AppsOp::List)).unwrap_err();
        assert_eq!(
            (status, message.as_str()),
            (409, "this workstation is offline")
        );
        let mut revoked = hub(Some(Arc::clone(&link)));
        revoked.station = station(false, true);
        assert_eq!(
            run_op(&revoked, 7, "builder", "w1", None, Op::Apps(AppsOp::List))
                .unwrap_err()
                .0,
            409
        );
        let mut draining = hub(Some(Arc::clone(&link)));
        draining.station = station(true, false);
        assert_eq!(
            run_op(&draining, 7, "builder", "w1", None, Op::Apps(AppsOp::List))
                .unwrap_err()
                .0,
            409
        );
        let known = hub(Some(link));
        let (status, _) =
            run_op(&known, 7, "builder", "w9", None, Op::Apps(AppsOp::List)).unwrap_err();
        assert_eq!(status, 404);
        assert_eq!(
            offline.audit.lock().unwrap().len(),
            1,
            "a refused attempt is audited"
        );
        assert!(
            daemon.hears.try_recv().is_err(),
            "nothing was sent to a station that is not online"
        );
    }

    #[test]
    fn an_operation_no_agent_sends_is_refused() {
        let (link, daemon) = online();
        let hub = hub(Some(link));
        let (status, message) =
            run_op(&hub, 7, "builder", "w1", None, Op::Freeze { on: false }).unwrap_err();
        assert_eq!(status, 403);
        assert!(message.contains("freeze"), "{message}");
        assert!(daemon.hears.try_recv().is_err());
    }

    #[test]
    fn every_attempt_is_audited_with_its_summary_and_no_file_contents() {
        let (link, daemon) = online();
        let hub = hub(Some(link));
        let asked = answer(daemon, json!({"path": "notes.md", "hash": "abc"}));
        let op = Op::Files(FilesOp::Write {
            path: "notes.md".into(),
            text: "the secret body".into(),
            expected_hash: None,
        });
        run_op(&hub, 7, "builder", "w1", None, op).expect("written");
        asked.join().unwrap();
        let row = last_audit(&hub);
        assert_eq!(row["kind"], "agent_op");
        assert_eq!(row["station"], "w1");
        assert_eq!(row["agent"], 7);
        assert_eq!(row["actor"], "agent:7");
        assert_eq!(row["data"]["ok"], true);
        assert_eq!(row["data"]["op"]["op"], "files");
        assert_eq!(row["data"]["op"]["path"], "notes.md");
        assert_eq!(row["data"]["op"]["text"]["bytes"], 15);
        assert!(!row.to_string().contains("the secret body"), "{row}");
    }

    #[test]
    fn a_delivery_runs_its_copy_once_and_is_audited_as_an_operation() {
        let hub = hub(None);
        let copies = Mutex::new(0);
        let out = run_deliver(&hub, 7, "builder", "w1", "out/render.png", || {
            *copies.lock().unwrap() += 1;
            Ok(json!({"id": "f1"}))
        })
        .expect("delivered");
        assert_eq!(out["id"], "f1");
        assert_eq!(*copies.lock().unwrap(), 1);
        let row = last_audit(&hub);
        assert_eq!(row["kind"], "agent_op");
        assert_eq!(row["actor"], "agent:7");
        assert_eq!(
            row["data"]["op"],
            json!({"op": "deliver", "path": "out/render.png"})
        );
        assert_eq!(row["data"]["ok"], true);
    }

    #[test]
    fn a_delivery_the_laws_refuse_copies_nothing_and_is_audited_as_refused() {
        let hub = hub(None);
        let copies = Mutex::new(0);
        let copy = || {
            *copies.lock().unwrap() += 1;
            Ok(json!({}))
        };
        assert!(
            run_deliver(&hub, 7, "builder", "w1", "a.txt", copy).is_ok(),
            "allowed without a rule"
        );
        hub.laws
            .add_rule("No files", NO_FILES, true, 0)
            .expect("the rule compiles");
        let (status, message) = run_deliver(&hub, 7, "builder", "w1", "a.txt", copy).unwrap_err();
        assert_eq!(status, 403);
        assert!(message.starts_with("refused by law O-002"), "{message}");
        assert_eq!(
            *copies.lock().unwrap(),
            1,
            "the refused delivery copied nothing"
        );
        assert_eq!(last_audit(&hub)["data"]["ok"], false);
        assert!(
            last_audit(&hub)["data"]["error"]
                .as_str()
                .unwrap()
                .contains("O-002")
        );
    }

    #[test]
    fn a_delivery_that_fails_is_audited_and_a_revoked_station_is_refused() {
        let known = hub(None);
        let (status, message) = run_deliver(&known, 7, "builder", "w1", "gone.txt", || {
            Err((404, "no file gone.txt in the work folder".into()))
        })
        .unwrap_err();
        assert_eq!(
            (status, message.as_str()),
            (404, "no file gone.txt in the work folder")
        );
        assert_eq!(last_audit(&known)["data"]["ok"], false);

        let mut revoked = hub(None);
        revoked.station = station(false, true);
        let (status, _) =
            run_deliver(&revoked, 7, "builder", "w1", "a.txt", || Ok(json!({}))).unwrap_err();
        assert_eq!(status, 409);
        let (status, _) =
            run_deliver(&known, 7, "builder", "w9", "a.txt", || Ok(json!({}))).unwrap_err();
        assert_eq!(status, 404);
    }

    #[test]
    fn a_failed_audit_of_a_delivery_is_a_500_that_says_the_copy_ran() {
        let mut hub = hub(None);
        hub.audit_broken = true;
        let (status, message) =
            run_deliver(&hub, 7, "builder", "w1", "a.txt", || Ok(json!({}))).unwrap_err();
        assert_eq!(status, 500);
        assert!(message.starts_with("the action ran"), "{message}");
    }

    #[test]
    fn a_failed_audit_is_a_500_that_says_the_action_ran() {
        let (link, daemon) = online();
        let mut hub = hub(Some(link));
        hub.audit_broken = true;
        let asked = answer(daemon, json!({"entries": []}));
        let (status, message) =
            run_op(&hub, 7, "builder", "w1", None, Op::Apps(AppsOp::List)).unwrap_err();
        asked.join().unwrap();
        assert_eq!(status, 500);
        assert!(message.contains("the action ran"), "{message}");
    }

    #[test]
    fn station_files_and_app_script_happy_paths_run_through_the_hub() {
        // Station files: a directory listing comes back, and the station is asked for a list.
        {
            let (link, daemon) = online();
            let hub = hub(Some(link));
            let Call::Op { station, lease, op } = parse(
                "station_files",
                &json!({"station": "w1", "cmd": "list", "path": "."}),
            )
            .unwrap() else {
                panic!("an operation");
            };
            let asked = answer(
                daemon,
                json!({"entries": [{"name": "a.txt", "kind": "file"}]}),
            );
            let listed = run_op(&hub, 7, "builder", &station, lease.as_deref(), op).unwrap();
            assert_eq!(listed["entries"][0]["name"], "a.txt");
            let (op, _) = asked.join().unwrap();
            assert_eq!(
                op,
                Op::Files(FilesOp::List {
                    path: ".".into(),
                    cursor: None
                })
            );
        }
        // An app script runs through its bridge: the exit code comes back, and the bridge is asked.
        {
            let (link, daemon) = online();
            let hub = hub(Some(link));
            let Call::Op { station, lease, op } = parse(
                "app_script",
                &json!({"station": "w1", "bridge": "cli", "script": "ffmpeg -version", "timeout_ms": 30000}),
            )
            .unwrap() else {
                panic!("an operation");
            };
            let asked = answer(
                daemon,
                json!({"exit_code": 0, "output": "ffmpeg version 7", "timed_out": false}),
            );
            let ran = run_op(&hub, 7, "builder", &station, lease.as_deref(), op).unwrap();
            assert_eq!(ran["exit_code"], 0);
            let (op, _) = asked.join().unwrap();
            assert_eq!(
                op,
                Op::Script {
                    bridge: apz_station::Bridge::Cli,
                    app: None,
                    code: "ffmpeg -version".into(),
                    timeout_s: 30
                }
            );
        }
    }
}
