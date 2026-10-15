//! Agents sleep when they have nothing to do, and wake the moment something needs them.
//!
//! CubeSandbox pauses a sandbox when its proxy has seen no traffic for a while and resumes it on the
//! next request. The hub knows more than traffic: an agent's work queue, its running engines and
//! terminals, its open IDE sessions, the approvals it waits on, its leases. So an agent sleeps when
//! all of those are empty and it has been idle for `idle_sleep_min` minutes (a setting; 0 = never),
//! unless its role is in `sleep_never_roles`. Sleeping is a snapshot of its VM and a stop
//! (noded `POST /v1/runs/{run}/vms/{vm}/sleep`); the agent shows as `asleep`, its memory is free.
//!
//! It wakes **before** the work is delivered: the scheduler's dispatch, a chat, a `tell`, an IDE
//! request and the owner's wake all call [`wake`] / [`ensure_awake`], which restore the VM from its
//! sleep snapshot (noded `.../wake`); the harness reconnects within moments, its hello marks it idle,
//! and the queued work goes out (`Scheduler::dispatch_idle`).
//!
//! POST /v1/agents/{cid}/sleep → {ok}      (the owner puts it to sleep now)
//! POST /v1/agents/{cid}/wake  → {ok}      (the owner wakes it)

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use apz_http::{Params, Request, Response, Router};
use serde_json::{Value, json};

use crate::agents::Status;
use crate::lifecycle::place;
use crate::state::App;

/// How often the sweeper looks at the roster.
const SWEEP_EVERY: Duration = Duration::from_secs(30);

/// How often `ensure_awake` checks whether the agent is back.
const POLL_EVERY: Duration = Duration::from_millis(50);

/// How long a route that needs the agent waits for it to wake before it answers 503.
const WAKE_WAIT: Duration = Duration::from_secs(15);

/// What the sleep state holds: when each agent last did something, and which agents are being put to
/// sleep or woken right now.
struct Book {
    /// cid → when it last did something (ms). An agent with no entry counts from when it connected.
    touched: BTreeMap<u64, u64>,
    /// Agents whose sleep is in flight. A wake for them waits: the sleep ends with a wake if work came in.
    sleeping: BTreeSet<u64>,
    /// Agents whose wake is in flight (one noded call at a time per agent).
    waking: BTreeSet<u64>,
}

static BOOK: Mutex<Book> = Mutex::new(Book {
    touched: BTreeMap::new(),
    sleeping: BTreeSet::new(),
    waking: BTreeSet::new(),
});

/// The hub, for the wake threads: a wake is spawned from a `&App` (the scheduler's dispatch), which
/// cannot be moved into a thread. Set once by [`spawn_sweeper`], which runs at start.
static HUB: OnceLock<Weak<App>> = OnceLock::new();

fn book() -> MutexGuard<'static, Book> {
    BOOK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The answer when an agent is still waking after the wait.
fn waking_response() -> Response {
    Response::error(503, "the agent is waking up, try again")
}

/// Routes: `POST /v1/agents/{cid}/sleep`, `POST /v1/agents/{cid}/wake`.
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .post("/v1/agents/{cid}/sleep", |app, req, p| {
            sleep_route(app, req, p)
        })
        .post("/v1/agents/{cid}/wake", |app, req, p| {
            wake_route(app, req, p)
        })
}

/// Records activity for `cid` (work started or ended, a chat, an IDE call, a tool call): the idle
/// clock restarts.
pub fn touch(app: &App, cid: u64) {
    let _ = app;
    book().touched.insert(cid, apz_plaza::now_ms());
}

/// Starts waking `cid` if it is asleep (returns at once; idempotent while a wake is in flight).
pub fn wake(app: &App, cid: u64) {
    if app
        .agents
        .get(cid)
        .is_none_or(|agent| agent.status != Status::Asleep)
    {
        return;
    }
    {
        let mut book = book();
        if book.sleeping.contains(&cid) || !book.waking.insert(cid) {
            return;
        }
    }
    let Some(hub) = HUB.get().and_then(Weak::upgrade) else {
        book().waking.remove(&cid);
        eprintln!("apid: cannot wake agent {cid}: the sleep sweeper is not running");
        return;
    };
    std::thread::spawn(move || {
        let result = noded_post(&hub, cid, "wake");
        book().waking.remove(&cid);
        match result {
            Ok(_) => {
                hub.transcripts.record(&hub.bus, cid, 0, "wake", json!({}));
            }
            Err(err) => eprintln!("apid: could not wake agent {cid}: {err}"),
        }
    });
}

/// Wakes `cid` if it is asleep and waits until it is connected again, up to `timeout`. Ok when the
/// agent is idle or working (or gone: the caller's own checks answer for that); Err when it is still
/// waking at the timeout.
pub fn ensure_awake(app: &App, cid: u64, timeout: Duration) -> Result<(), String> {
    if !app
        .agents
        .get(cid)
        .is_some_and(|agent| agent.status == Status::Asleep)
    {
        touch(app, cid);
        return Ok(());
    }
    wake(app, cid);
    let deadline = Instant::now() + timeout;
    loop {
        match app.agents.get(cid).map(|agent| agent.status) {
            Some(Status::Idle | Status::Working) => {
                touch(app, cid);
                return Ok(());
            }
            None | Some(Status::Offline) => return Ok(()),
            Some(Status::Asleep) => {}
        }
        if Instant::now() >= deadline {
            return Err(format!("agent {cid} is still waking after {timeout:?}"));
        }
        std::thread::sleep(POLL_EVERY);
    }
}

/// The entry points' wake: [`ensure_awake`] for the usual wait, answering 503 when the agent is not
/// back in time.
pub fn awake_or_503(app: &App, cid: u64) -> Result<(), Response> {
    ensure_awake(app, cid, WAKE_WAIT).map_err(|_| waking_response())
}

/// Starts the sleep sweeper: every 30 s, sleeps the agents that qualify (see [`should_sleep`]) and
/// wakes the asleep ones that have queued work (a wake that failed, or work restored after a restart).
pub fn spawn_sweeper(app: Arc<App>) {
    let _ = HUB.set(Arc::downgrade(&app));
    std::thread::spawn(move || {
        loop {
            sweep_once(&app);
            std::thread::sleep(SWEEP_EVERY);
        }
    });
}

/// One sweep over the roster.
fn sweep_once(app: &App) {
    // Work that waits for an idle agent is sent now (a hello or a restart can leave it behind).
    app.work.dispatch_idle(app);
    let settings = app.settings.get();
    let stop_all = app.control.paused();
    let approvals: BTreeSet<u64> = app
        .approvals
        .list()
        .into_iter()
        .filter(|item| item.state == crate::approvals::State::Pending)
        .map(|item| item.cid)
        .collect();
    for agent in app.agents.list() {
        let cid = agent.cid;
        let busy = app.work.current(cid).is_some() || !app.work.queued(cid).is_empty();
        match agent.status {
            Status::Asleep => {
                // Queued work for an asleep agent (a failed wake, or work restored after a restart) gets
                // another wake, but not while STOP ALL holds the plaza.
                if busy && !stop_all {
                    wake(app, cid);
                }
            }
            Status::Working => touch(app, cid),
            Status::Idle if busy => touch(app, cid),
            Status::Idle => {
                let last = book()
                    .touched
                    .get(&cid)
                    .copied()
                    .unwrap_or(agent.connected_at_ms);
                let check = SleepCheck {
                    status: agent.status,
                    busy,
                    approval_pending: approvals.contains(&cid),
                    idle_ms: apz_plaza::now_ms().saturating_sub(last),
                    idle_sleep_min: settings.idle_sleep_min,
                    role: &agent.role,
                    never_roles: &settings.sleep_never_roles,
                    stop_all,
                    placed: place(app, cid).is_ok(),
                };
                if should_sleep(&check) {
                    if let Err(res) = put_to_sleep(app, cid) {
                        eprintln!(
                            "apid: agent {cid} stays awake: its sleep was refused ({})",
                            res.status
                        );
                    }
                }
            }
            Status::Offline => {}
        }
    }
}

/// What the sweeper knows about one agent when it asks whether to put it to sleep.
pub struct SleepCheck<'a> {
    pub status: Status,
    /// Work is in flight or waiting for it.
    pub busy: bool,
    /// An approval for the agent waits on the owner.
    pub approval_pending: bool,
    /// Milliseconds since its last activity (see [`touch`]), or since it connected.
    pub idle_ms: u64,
    pub idle_sleep_min: u32,
    pub role: &'a str,
    pub never_roles: &'a [String],
    /// STOP ALL holds the plaza.
    pub stop_all: bool,
    /// Its VM is placed on this node (`lifecycle::place` succeeds).
    pub placed: bool,
}

/// Whether the sweeper puts this agent to sleep: idle, with nothing to do, nothing waiting on the
/// owner, its idle time at least `idle_sleep_min` minutes (0 never), its role not one that never
/// sleeps, the plaza not held, and a VM to snapshot.
pub fn should_sleep(check: &SleepCheck) -> bool {
    check.status == Status::Idle
        && !check.busy
        && !check.approval_pending
        && !check.stop_all
        && check.placed
        && check.idle_sleep_min > 0
        && check.idle_ms >= u64::from(check.idle_sleep_min) * 60_000
        && !check.never_roles.iter().any(|role| role == check.role)
}

/// Puts an idle agent to sleep: its VM is snapshotted and stopped (noded `.../sleep`). The agent is
/// asleep from the first step, so the session ending is not an offline agent (`agents::gone`). A
/// refused sleep puts it back to idle and sends its waiting work.
pub(crate) fn put_to_sleep(app: &App, cid: u64) -> Result<(), Response> {
    place(app, cid)?;
    book().sleeping.insert(cid);
    // Idle is checked and left under the roster lock, so a delivery cannot slip in between.
    let mut flipped = false;
    app.agents.update(cid, |agent| {
        if agent.status == Status::Idle {
            agent.status = Status::Asleep;
            flipped = true;
        }
    });
    if !flipped {
        book().sleeping.remove(&cid);
        return Err(Response::error(409, &format!("agent {cid} is not idle")));
    }
    publish(app, cid);
    // Work that came in after the check: it goes out after the wake below.
    if app.work.current(cid).is_some() || !app.work.queued(cid).is_empty() {
        book().sleeping.remove(&cid);
        app.agents.update(cid, |agent| {
            if agent.status == Status::Asleep {
                agent.status = Status::Idle;
            }
        });
        publish(app, cid);
        app.work.dispatch_idle(app);
        return Err(Response::error(409, &format!("agent {cid} has work")));
    }
    let result = noded_post(app, cid, "sleep");
    book().sleeping.remove(&cid);
    match result {
        Ok(body) => {
            app.transcripts.record(
                &app.bus,
                cid,
                0,
                "sleep",
                json!({ "snapshot": body["snapshot"] }),
            );
            // Work that arrived while the sleep was in flight waits for a wake now.
            if app.work.current(cid).is_some() || !app.work.queued(cid).is_empty() {
                wake(app, cid);
            }
            Ok(())
        }
        Err(err) => {
            app.agents.update(cid, |agent| {
                if agent.status == Status::Asleep {
                    agent.status = Status::Idle;
                }
            });
            publish(app, cid);
            app.work.dispatch_idle(app);
            Err(Response::error(
                502,
                &format!("noded refused the sleep: {err}"),
            ))
        }
    }
}

/// Publishes the agent on the bus after a status change.
fn publish(app: &App, cid: u64) {
    if let Some(agent) = app.agents.get(cid) {
        app.bus.publish("agent", &agent);
    }
}

/// Asks noded to run `action` (`sleep` or `wake`) on the agent's VM; the answer's body on success.
/// This calls noded directly rather than through `lifecycle::noded_call`, which discards the body
/// (the snapshot) and gives no text to log when noded refuses.
fn noded_post(app: &App, cid: u64, action: &str) -> Result<Value, String> {
    let (run, vm) = place(app, cid).map_err(|_| format!("agent {cid} has no VM on this node"))?;
    let path = format!("/v1/runs/{run}/vms/{vm}/{action}");
    match crate::fleet::noded_for(app, cid, "POST", &path, Some(&json!({}))) {
        Ok((code, body)) if (200..300).contains(&code) => Ok(body),
        Ok((code, body)) => Err(format!("noded {action} answered {code}: {body}")),
        Err(err) => Err(format!("noded unreachable: {err}")),
    }
}

/// `POST /v1/agents/{cid}/sleep`: the owner puts an idle agent to sleep now. 409 when it has work
/// in flight or queued, or is not idle.
fn sleep_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    let agent = match crate::agents::agent_at(app, p) {
        Ok(agent) => agent,
        Err(res) => return res,
    };
    if app.work.current(agent.cid).is_some() || !app.work.queued(agent.cid).is_empty() {
        return Response::error(
            409,
            &format!("agent {} has work in flight or queued", agent.cid),
        );
    }
    if agent.status != Status::Idle {
        return Response::error(
            409,
            &format!("agent {} is {:?}, not idle", agent.cid, agent.status),
        );
    }
    match put_to_sleep(app, agent.cid) {
        Ok(()) => Response::json(200, &json!({ "ok": true })),
        Err(res) => res,
    }
}

/// `POST /v1/agents/{cid}/wake`: the owner wakes an asleep agent, and waits for it to be back.
fn wake_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    let agent = match crate::agents::agent_at(app, p) {
        Ok(agent) => agent,
        Err(res) => return res,
    };
    if agent.status != Status::Asleep {
        return Response::error(409, &format!("agent {} is not asleep", agent.cid));
    }
    match awake_or_503(app, agent.cid) {
        Ok(()) => Response::json(200, &json!({ "ok": true })),
        Err(res) => res,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check<'a>(never: &'a [String]) -> SleepCheck<'a> {
        SleepCheck {
            status: Status::Idle,
            busy: false,
            approval_pending: false,
            idle_ms: 10 * 60_000,
            idle_sleep_min: 10,
            role: "worker",
            never_roles: never,
            stop_all: false,
            placed: true,
        }
    }

    #[test]
    fn an_idle_agent_past_its_idle_minutes_sleeps() {
        let never = vec!["orchestrator".to_string()];
        assert!(should_sleep(&check(&never)));
    }

    #[test]
    fn each_condition_keeps_an_agent_awake() {
        let never = vec!["orchestrator".to_string()];
        let mut c = check(&never);
        c.status = Status::Working;
        assert!(!should_sleep(&c), "a working agent stays awake");
        let mut c = check(&never);
        c.status = Status::Asleep;
        assert!(!should_sleep(&c), "an asleep agent is already asleep");
        let mut c = check(&never);
        c.busy = true;
        assert!(!should_sleep(&c), "work in flight or queued keeps it awake");
        let mut c = check(&never);
        c.approval_pending = true;
        assert!(!should_sleep(&c), "a pending approval keeps it awake");
        let mut c = check(&never);
        c.stop_all = true;
        assert!(!should_sleep(&c), "STOP ALL holds the sleep");
        let mut c = check(&never);
        c.placed = false;
        assert!(
            !should_sleep(&c),
            "an agent with no VM here has nothing to snapshot"
        );
        let mut c = check(&never);
        c.idle_ms = 10 * 60_000 - 1;
        assert!(!should_sleep(&c), "not idle long enough yet");
        let mut c = check(&never);
        c.idle_sleep_min = 0;
        c.idle_ms = u64::MAX / 2;
        assert!(!should_sleep(&c), "0 means agents never sleep on their own");
    }

    #[test]
    fn a_role_that_never_sleeps_stays_awake() {
        let never = vec!["orchestrator".to_string()];
        let mut c = check(&never);
        c.role = "orchestrator";
        assert!(!should_sleep(&c));
        let none: Vec<String> = Vec::new();
        let mut c = check(&none);
        c.role = "orchestrator";
        assert!(
            should_sleep(&c),
            "with no never-role the orchestrator may sleep"
        );
    }
}
