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

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::Duration;

use apz_http::{Params, Request, Response, Router};
use serde_json::{Value, json};

use crate::agents::Status;
use crate::lifecycle::place;
use crate::state::App;

/// How often the sweeper looks at the roster.
const SWEEP_EVERY: Duration = Duration::from_secs(30);

/// How long a route that needs the agent waits for it to wake before it answers 503.
const WAKE_WAIT: Duration = Duration::from_secs(15);

/// How many threads put idle agents to sleep at once in one sweep. Each sleep is a noded snapshot
/// that can take a while, so they run side by side, but not all at once.
const SLEEP_PARALLEL: usize = 4;

/// When one wake started and how far it got: the request, the worker picking it up, and noded's answer.
struct WakeTimes {
    requested: std::time::Instant,
    dequeued: Option<std::time::Instant>,
    node_done: Option<std::time::Instant>,
}

/// What the sleep state holds: when each agent last did something, and which agents are being put to
/// sleep or woken right now.
struct Book {
    /// cid → when it last did something (ms). An agent with no entry counts from when it connected.
    touched: BTreeMap<u64, u64>,
    /// Agents whose sleep is in flight. A wake for them waits: the sleep ends with a wake if work came in.
    sleeping: BTreeSet<u64>,
    /// Agents whose wake is in flight (one noded call at a time per agent).
    waking: BTreeSet<u64>,
    /// Wakes in flight, timed until the agent's hello ends them (see [`woke`]).
    wake_started: BTreeMap<u64, WakeTimes>,
}

static BOOK: Mutex<Book> = Mutex::new(Book {
    touched: BTreeMap::new(),
    sleeping: BTreeSet::new(),
    waking: BTreeSet::new(),
    wake_started: BTreeMap::new(),
});

/// Drops `cid`'s wake timing and its in-flight count, when the wake gave up before the agent's hello.
fn forget_wake(cid: u64) {
    let removed = book().wake_started.remove(&cid).is_some();
    if removed {
        crate::metrics::WAKES_IN_FLIGHT.add(-1);
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

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

/// Starts waking `cid` if it is asleep (returns at once; idempotent while a wake is in flight). An
/// interactive wake: a person or a tool call is waiting for the agent.
pub fn wake(app: &App, cid: u64) {
    wake_with(app, cid, WakePriority::Interactive);
}

/// Starts waking `cid` if it is asleep, in `priority`'s lane (returns at once; idempotent while a
/// wake is in flight).
pub fn wake_with(app: &App, cid: u64, priority: WakePriority) {
    if app
        .agents
        .get(cid)
        .is_none_or(|agent| agent.status != Status::Asleep)
    {
        return;
    }
    // An agent being moved to another node is woken there by the move, not here: a wake now would
    // restore it on the node it is leaving while its files are copied away.
    if crate::placement::is_moving(cid) {
        return;
    }
    {
        let mut book = book();
        if book.sleeping.contains(&cid) || !book.waking.insert(cid) {
            return;
        }
        book.wake_started.insert(
            cid,
            WakeTimes {
                requested: std::time::Instant::now(),
                dequeued: None,
                node_done: None,
            },
        );
    }
    crate::metrics::WAKES.inc();
    crate::metrics::WAKES_IN_FLIGHT.add(1);
    if HUB.get().and_then(Weak::upgrade).is_none() {
        book().waking.remove(&cid);
        forget_wake(cid);
        eprintln!("apid: cannot wake agent {cid}: the sleep sweeper is not running");
        return;
    }
    if !enqueue(cid, priority) {
        book().waking.remove(&cid);
        forget_wake(cid);
        crate::metrics::WAKES_REFUSED.inc();
        eprintln!("apid: cannot wake agent {cid}: {WAKE_QUEUE} wakes are already queued");
    }
}

/// How many threads run wakes. A wake is one noded call that restores a VM, so a few at a time is
/// enough; the rest wait in the queue.
const WAKE_WORKERS: usize = 8;

/// How many wakes may wait for a worker. A wake beyond this is refused: the agent stays asleep and
/// the next request or dispatch asks again.
const WAKE_QUEUE: usize = 4096;

/// How many wakes may run against one node at a time. A node restores its VMs a few at a time; a
/// wake beyond this waits, and the wakes for the other nodes go ahead.
const WAKES_PER_NODE: usize = 4;

/// How urgent a wake is. An interactive wake (the owner, a chat, a tool call waiting on the agent)
/// is taken before a background one (the scheduler's dispatch of queued work nobody is waiting on).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakePriority {
    Interactive,
    Background,
}

/// The wakes waiting for a worker, in two FIFO lanes, and how many wakes run against each node.
/// Pure: [`WAKES`] wraps it in a lock, and the tests drive it directly.
struct WakeQueue {
    capacity: usize,
    interactive: VecDeque<u64>,
    background: VecDeque<u64>,
    /// Wakes running per node. A node with none has no entry.
    running: BTreeMap<String, usize>,
}

impl WakeQueue {
    const fn new(capacity: usize) -> Self {
        Self {
            capacity,
            interactive: VecDeque::new(),
            background: VecDeque::new(),
            running: BTreeMap::new(),
        }
    }

    fn queued(&self) -> usize {
        self.interactive.len() + self.background.len()
    }

    /// Queues `cid` in its lane. False when the queue is full: the wake is refused.
    fn push(&mut self, cid: u64, priority: WakePriority) -> bool {
        if self.queued() >= self.capacity {
            return false;
        }
        match priority {
            WakePriority::Interactive => self.interactive.push_back(cid),
            WakePriority::Background => self.background.push_back(cid),
        }
        true
    }

    /// Takes the next wake a worker may run: the first in the interactive lane whose node has a free
    /// slot, else the first in the background lane, each lane in arrival order. A wake whose node is
    /// full stays where it is, so it keeps its place at the front. Returns the agent and its node;
    /// the node's slot is held until [`WakeQueue::finish`].
    fn admit(&mut self, node_of: &dyn Fn(u64) -> String) -> Option<(u64, String)> {
        admit_from(&mut self.interactive, &mut self.running, node_of)
            .or_else(|| admit_from(&mut self.background, &mut self.running, node_of))
    }

    /// Gives back the slot a finished wake held on `node`.
    fn finish(&mut self, node: &str) {
        if let Some(n) = self.running.get_mut(node) {
            *n -= 1;
            if *n == 0 {
                self.running.remove(node);
            }
        }
    }
}

/// The first wake in `lane` whose node has a free slot, taken out of the lane and counted against
/// that node.
fn admit_from(
    lane: &mut VecDeque<u64>,
    running: &mut BTreeMap<String, usize>,
    node_of: &dyn Fn(u64) -> String,
) -> Option<(u64, String)> {
    let (pos, node) = lane.iter().enumerate().find_map(|(pos, &cid)| {
        let node = node_of(cid);
        (running.get(&node).copied().unwrap_or(0) < WAKES_PER_NODE).then_some((pos, node))
    })?;
    let cid = lane.remove(pos)?;
    *running.entry(node.clone()).or_insert(0) += 1;
    Some((cid, node))
}

/// The wakes waiting for a worker. Locked for a push, an admission or a finish, never across a noded call.
static WAKES: Mutex<WakeQueue> = Mutex::new(WakeQueue::new(WAKE_QUEUE));
/// Signalled when a wake is queued or a node's slot frees, so an idle worker looks again.
static WAKE_READY: Condvar = Condvar::new();
/// Set once the wake workers have started.
static WORKERS: OnceLock<()> = OnceLock::new();

fn wakes() -> MutexGuard<'static, WakeQueue> {
    WAKES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Queues `cid` in `priority`'s lane; false when the queue is full. The first call starts the workers.
fn enqueue(cid: u64, priority: WakePriority) -> bool {
    WORKERS.get_or_init(|| {
        for i in 0..WAKE_WORKERS {
            std::thread::Builder::new()
                .name(format!("apid-wake-{i}"))
                .spawn(wake_worker)
                .expect("apid: cannot start a wake worker");
        }
    });
    let pushed = wakes().push(cid, priority);
    if pushed {
        WAKE_READY.notify_all();
    }
    pushed
}

/// The wakes waiting for a worker, by lane: (interactive, background). Read by `GET /v1/metrics`.
pub fn queued_by_lane() -> (usize, usize) {
    let queue = wakes();
    (queue.interactive.len(), queue.background.len())
}

/// A wake a worker has admitted. It holds its node's slot until it is dropped.
struct Admitted {
    cid: u64,
    node: String,
}

impl Drop for Admitted {
    fn drop(&mut self) {
        wakes().finish(&self.node);
        WAKE_READY.notify_all();
    }
}

/// Waits until a wake can run (its node has a free slot), then takes it. The node is resolved from
/// the roster now, the same way the noded call that follows will find it.
fn next_wake(app: &App) -> Admitted {
    let node_of = |cid: u64| crate::fleet::node_of(app, cid);
    let mut queue = wakes();
    loop {
        if let Some((cid, node)) = queue.admit(&node_of) {
            return Admitted { cid, node };
        }
        queue = WAKE_READY
            .wait(queue)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

/// A wake worker: runs admitted wakes, one at a time, for the life of the process.
fn wake_worker() {
    let Some(hub) = HUB.get().and_then(Weak::upgrade) else {
        return;
    };
    loop {
        let admitted = next_wake(&hub);
        run_wake(admitted.cid, &admitted.node);
    }
}

/// Runs one wake on a worker, against `node`: noded restores `cid`'s VM, then the wake is recorded.
fn run_wake(cid: u64, node: &str) {
    let dequeued = std::time::Instant::now();
    if let Some(times) = book().wake_started.get_mut(&cid) {
        times.dequeued = Some(dequeued);
    }
    let Some(hub) = HUB.get().and_then(Weak::upgrade) else {
        book().waking.remove(&cid);
        forget_wake(cid);
        eprintln!("apid: cannot wake agent {cid}: the sleep sweeper is not running");
        return;
    };
    let started = std::time::Instant::now();
    let result = noded_post(&hub, node, cid, "wake");
    crate::metrics::WAKE_NODE.observe(started.elapsed());
    book().waking.remove(&cid);
    match result {
        Ok(_) => {
            if let Some(times) = book().wake_started.get_mut(&cid) {
                times.node_done = Some(std::time::Instant::now());
            }
            hub.transcripts.record(&hub.bus, cid, 0, "wake", json!({}));
        }
        Err(err) => {
            forget_wake(cid);
            eprintln!("apid: could not wake agent {cid}: {err}");
        }
    }
}

/// The agent's harness said hello: its wake, if one was in flight, is over. Times it end to end and
/// writes one `timing op=wake` line on stderr.
pub fn woke(cid: u64) {
    let Some(times) = book().wake_started.remove(&cid) else {
        return;
    };
    crate::metrics::WAKES_IN_FLIGHT.add(-1);
    let now = std::time::Instant::now();
    let total = now.duration_since(times.requested);
    crate::metrics::WAKE.observe(total);
    let queue = times
        .dequeued
        .map_or(Duration::ZERO, |d| d.duration_since(times.requested));
    let node = match (times.dequeued, times.node_done) {
        (Some(d), Some(n)) => n.duration_since(d),
        _ => Duration::ZERO,
    };
    let reconnect = match times.node_done {
        Some(n) => {
            let elapsed = now.duration_since(n);
            crate::metrics::WAKE_RECONNECT.observe(elapsed);
            elapsed
        }
        None => Duration::ZERO,
    };
    eprintln!(
        "timing op=wake id={cid} total_ms={:.1} queue_ms={:.1} node_ms={:.1} reconnect_ms={:.1}",
        ms(total),
        ms(queue),
        ms(node),
        ms(reconnect)
    );
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
    app.agents.wait_until(cid, timeout, |agent| {
        !matches!(agent.map(|a| a.status), Some(Status::Asleep))
    });
    match app.agents.get(cid).map(|agent| agent.status) {
        Some(Status::Idle | Status::Working) => {
            touch(app, cid);
            Ok(())
        }
        None | Some(Status::Offline) => Ok(()),
        Some(Status::Asleep) => Err(format!("agent {cid} is still waking after {timeout:?}")),
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
    let mut on_roster = BTreeSet::new();
    let mut to_sleep: Vec<u64> = Vec::new();
    for agent in app.agents.list() {
        let cid = agent.cid;
        on_roster.insert(cid);
        let busy = app.work.current(cid).is_some() || !app.work.queued(cid).is_empty();
        match agent.status {
            Status::Asleep => {
                // Queued work for an asleep agent (a failed wake, or work restored after a restart) gets
                // another wake, but not while STOP ALL holds the plaza.
                if busy && !stop_all {
                    wake_with(app, cid, WakePriority::Background);
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
                let mut check = SleepCheck {
                    status: agent.status,
                    busy,
                    approval_pending: approvals.contains(&cid),
                    idle_ms: apz_plaza::now_ms().saturating_sub(last),
                    idle_sleep_min: settings.idle_sleep_min,
                    role: &agent.role,
                    never_roles: &settings.sleep_never_roles,
                    stop_all,
                    // Only an agent that passes the cheap checks asks noded for its VM.
                    placed: true,
                };
                if should_sleep(&check) {
                    check.placed = place(app, cid).is_ok();
                    if should_sleep(&check) {
                        to_sleep.push(cid);
                    }
                }
            }
            Status::Offline => {}
        }
    }
    // The sleeps run side by side: each is a noded snapshot that can take a while.
    if !to_sleep.is_empty() {
        std::thread::scope(|scope| {
            for chunk in to_sleep.chunks(to_sleep.len().div_ceil(SLEEP_PARALLEL).max(1)) {
                scope.spawn(move || {
                    for &cid in chunk {
                        if let Err(res) = put_to_sleep(app, cid) {
                            eprintln!(
                                "apid: agent {cid} stays awake: its sleep was refused ({})",
                                res.status
                            );
                        }
                    }
                });
            }
        });
    }
    let stale = prune_book(&mut book(), &on_roster, std::time::Instant::now());
    crate::metrics::WAKES_IN_FLIGHT.add(-(stale as i64));
}

/// Drops the book's entries that no longer matter: a `touched` clock for a cid off the roster, and a
/// wake timing whose request is older than twice [`WAKE_WAIT`] (its hello never came). Returns how
/// many wake timings it dropped, so the caller can take them off the in-flight count.
fn prune_book(book: &mut Book, on_roster: &BTreeSet<u64>, now: std::time::Instant) -> usize {
    book.touched.retain(|cid, _| on_roster.contains(cid));
    let before = book.wake_started.len();
    book.wake_started
        .retain(|_, times| now.saturating_duration_since(times.requested) <= 2 * WAKE_WAIT);
    before - book.wake_started.len()
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
    let started = std::time::Instant::now();
    let result = noded_post(app, &crate::fleet::node_of(app, cid), cid, "sleep");
    let elapsed = started.elapsed();
    book().sleeping.remove(&cid);
    match result {
        Ok(body) => {
            crate::metrics::SLEEPS.inc();
            crate::metrics::SLEEP.observe(elapsed);
            eprintln!("timing op=sleep id={cid} total_ms={:.1}", ms(elapsed));
            app.transcripts.record(
                &app.bus,
                cid,
                0,
                "sleep",
                json!({ "snapshot": body["snapshot"] }),
            );
            // Work that arrived while the sleep was in flight waits for a wake now.
            if app.work.current(cid).is_some() || !app.work.queued(cid).is_empty() {
                wake_with(app, cid, WakePriority::Background);
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

/// Asks noded on `node` to run `action` (`sleep` or `wake`) on the agent's VM; the answer's body on
/// success. This calls noded directly rather than through `lifecycle::noded_call`, which discards the
/// body (the snapshot) and gives no text to log when noded refuses.
fn noded_post(app: &App, node: &str, cid: u64, action: &str) -> Result<Value, String> {
    let (run, vm) = place(app, cid).map_err(|_| format!("agent {cid} has no VM on this node"))?;
    let path = format!("/v1/runs/{run}/vms/{vm}/{action}");
    match crate::fleet::noded(app, node, "POST", &path, Some(&json!({}))) {
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
    /// The node of each agent in the queue tests: cid 6 is on "b", the others on "a".
    fn node_a_b(cid: u64) -> String {
        if cid == 6 { "b" } else { "a" }.to_string()
    }

    #[test]
    fn interactive_wakes_are_taken_before_background_ones() {
        let mut queue = WakeQueue::new(8);
        assert!(queue.push(1, WakePriority::Background));
        assert!(queue.push(2, WakePriority::Background));
        assert!(queue.push(3, WakePriority::Interactive));
        let order: Vec<u64> =
            std::iter::from_fn(|| queue.admit(&node_a_b).map(|(cid, _)| cid)).collect();
        assert_eq!(order, vec![3, 1, 2]);
    }

    #[test]
    fn a_wake_beyond_the_bound_is_refused() {
        let mut queue = WakeQueue::new(2);
        assert!(queue.push(1, WakePriority::Background));
        assert!(queue.push(2, WakePriority::Interactive));
        assert!(
            !queue.push(3, WakePriority::Interactive),
            "full: interactive is refused too"
        );
        assert!(!queue.push(4, WakePriority::Background));
        assert_eq!(queue.queued(), 2);
        // Taking one out makes room again.
        assert_eq!(queue.admit(&node_a_b), Some((2, "a".to_string())));
        assert!(queue.push(5, WakePriority::Background));
    }

    #[test]
    fn a_full_nodes_wake_waits_while_another_nodes_goes_ahead() {
        let mut queue = WakeQueue::new(16);
        for cid in 1..=5 {
            assert!(queue.push(cid, WakePriority::Interactive));
        }
        assert!(queue.push(6, WakePriority::Background));
        // Four wakes take node a's slots. The fifth (cid 5) waits, and node b's background wake goes
        // ahead of it.
        for expected in [1, 2, 3, 4] {
            assert_eq!(queue.admit(&node_a_b).map(|(cid, _)| cid), Some(expected));
        }
        assert_eq!(queue.admit(&node_a_b), Some((6, "b".to_string())));
        assert_eq!(queue.admit(&node_a_b), None, "node a is full: cid 5 waits");
        assert_eq!(queue.queued(), 1);
        // A finished wake frees its slot, and cid 5 is taken.
        queue.finish("a");
        assert_eq!(queue.admit(&node_a_b), Some((5, "a".to_string())));
    }

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
    fn prune_book_drops_what_is_off_the_roster_or_stale() {
        let now = std::time::Instant::now();
        let mut state = Book {
            touched: BTreeMap::new(),
            sleeping: BTreeSet::new(),
            waking: BTreeSet::new(),
            wake_started: BTreeMap::new(),
        };
        state.touched.insert(1, 10);
        state.touched.insert(2, 20);
        state.wake_started.insert(
            1,
            WakeTimes {
                requested: now - Duration::from_secs(31),
                dequeued: None,
                node_done: None,
            },
        );
        state.wake_started.insert(
            2,
            WakeTimes {
                requested: now,
                dequeued: None,
                node_done: None,
            },
        );
        let on_roster = BTreeSet::from([1]);
        assert_eq!(prune_book(&mut state, &on_roster, now), 1);
        assert_eq!(state.touched.keys().copied().collect::<Vec<_>>(), vec![1]);
        assert_eq!(
            state.wake_started.keys().copied().collect::<Vec<_>>(),
            vec![2]
        );
    }

    #[test]
    fn a_wake_is_timed_until_the_agent_says_hello() {
        let cid = 9_000_001;
        let now = std::time::Instant::now();
        book().wake_started.insert(
            cid,
            WakeTimes {
                requested: now,
                dequeued: Some(now),
                node_done: Some(now),
            },
        );
        let before = crate::metrics::WAKE.count();
        woke(cid);
        assert!(!book().wake_started.contains_key(&cid));
        assert_eq!(crate::metrics::WAKE.count(), before + 1);
    }

    #[test]
    fn idle_agents_sleep_unless_a_condition_keeps_them_awake() {
        // an_idle_agent_past_its_idle_minutes_sleeps: an idle agent past its idle minutes sleeps.
        {
            let never = vec!["orchestrator".to_string()];
            assert!(should_sleep(&check(&never)));
        }

        // each_condition_keeps_an_agent_awake: each condition that keeps an agent awake holds the sleep.
        {
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

        // a_role_that_never_sleeps_stays_awake: a role on the never-sleep list stays awake.
        {
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
}
