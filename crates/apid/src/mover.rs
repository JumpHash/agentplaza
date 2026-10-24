//! Stopping, resuming and rebalancing the plaza's machines.
//!
//! The owner stops a node (`POST /v1/nodes/{id}/stop`) to take it out of service, `n0` included. No new
//! agent is placed on it and no agent moves to it, the work queued for its agents waits (`holds`), and
//! the mover thread moves each of its agents to the machine with the most room for it
//! (`placement::target_for`), one at a time. An agent in the middle of a turn moves when the turn ends;
//! a stop `now` interrupts the turn instead (`scheduler::Queues::interrupt`), and the same work starts
//! again on the agent's new machine, with its workspace as it was. An agent that cannot leave yet waits,
//! with the reason on the node's view, and is tried again on the next pass. A stopping node with no
//! agent left on it is stopped: its machine can be turned off, and the node stays out of service,
//! online or not, until the owner resumes it.
//!
//! A VM moves only within its pool (plan 17.2): the machines that share its architecture, hypervisor and CPU
//! baseline, since a snapshot does not translate across them. Where no other machine of its pool has room, a
//! step session hands over (`work_loop::hand_over_for_pool`): it ends with a handover, and its step starts again
//! in a new session on a machine with room.
//!
//! A resumed node (`POST /v1/nodes/{id}/resume`) takes agents again, and a rebalance follows once it
//! reports: the idle agents are spread over the machines in service, each machine's share of them in
//! proportion to its memory ([`plan`]). `POST /v1/nodes/rebalance` runs one at any time.
//!
//! Routes (owner):
//!   POST /v1/nodes/{id}/stop    {now?}        → NodeView
//!   POST /v1/nodes/{id}/resume  {rebalance?}  → NodeView (rebalance defaults to true)
//!   POST /v1/nodes/{id}/drain   {on}          → the older name: on stops the node, off resumes it
//!   POST /v1/nodes/rebalance    {dry_run?}    → {moves: [{cid, from, to}], started}

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agents::Status;
use crate::fleet;
use crate::nodes::{self, Pool, Waiting};
use crate::placement::{self, ADMIT_MIB};
use crate::state::App;
use crate::work_loop;

/// How often the mover looks at the stopping nodes while one still runs agents.
const PASS: Duration = Duration::from_secs(3);
/// How long the mover waits for a kick when no node is stopping.
const IDLE: Duration = Duration::from_secs(30);
/// How long an agent whose move failed waits before it is tried again.
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// A rebalance moves at most this many agents.
pub const MAX_MOVES: usize = 16;
/// A resumed node gets its rebalance when it reports, if it does within this long.
const REBALANCE_WAIT_MS: u64 = 15 * 60 * 1000;
/// Memory a planned move counts, on the agent's new node and off its old one: what a new agent reserves.
const AGENT_MIB: u64 = placement::RESERVE_MIB;

/// What the mover keeps between its passes.
struct Book {
    /// Per stopping node: the agents that cannot leave it yet, and why.
    waiting: BTreeMap<String, BTreeMap<u64, String>>,
    /// Stopping nodes whose agents' turns are interrupted (a stop `now`), so their work moves at once.
    now: BTreeSet<String>,
    /// Agents whose move failed, and when they are tried again.
    retry_at: BTreeMap<u64, Instant>,
    /// Nodes resumed and waiting for their rebalance: when it was asked for (ms).
    rebalance_for: BTreeMap<String, u64>,
    /// The moves of a rebalance the owner asked for, run on the next pass.
    planned: Vec<Planned>,
    /// A kick is waiting.
    kicked: bool,
}

static BOOK: Mutex<Book> = Mutex::new(Book {
    waiting: BTreeMap::new(),
    now: BTreeSet::new(),
    retry_at: BTreeMap::new(),
    rebalance_for: BTreeMap::new(),
    planned: Vec::new(),
    kicked: false,
});
static SIGNAL: Condvar = Condvar::new();

fn book() -> MutexGuard<'static, Book> {
    BOOK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Starts the mover's next pass now (a stop, a resume, a turn that ended on a stopping node).
pub fn kick() {
    book().kicked = true;
    SIGNAL.notify_all();
}

/// Waits for a kick, up to `timeout`.
fn await_kick(timeout: Duration) {
    let guard = book();
    let (mut guard, _) = SIGNAL
        .wait_timeout_while(guard, timeout, |book| !book.kicked)
        .unwrap_or_else(PoisonError::into_inner);
    guard.kicked = false;
}

/// Whether agent `cid` is away, so nothing starts on it: it is moving, or its node is stopped and it
/// leaves first.
pub fn holds(app: &App, cid: u64) -> bool {
    placement::is_moving(cid) || app.nodes.is_stopped(&fleet::node_of(app, cid))
}

/// The agents that cannot leave stopping node `node` yet, each with the reason.
pub fn waiting(node: &str) -> Vec<Waiting> {
    book()
        .waiting
        .get(node)
        .map(|agents| {
            agents
                .iter()
                .map(|(&cid, reason)| Waiting {
                    cid,
                    reason: reason.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The mover thread: a pass over the stopping nodes and the rebalances due, every [`PASS`] while a
/// stopping node runs agents, else on a kick or every [`IDLE`].
pub fn spawn(app: Arc<App>) {
    let spawned = std::thread::Builder::new()
        .name("apid-mover".into())
        .spawn(move || {
            loop {
                let busy = evacuate(&app);
                rebalance_due(&app);
                await_kick(if busy { PASS } else { IDLE });
            }
        });
    if let Err(err) = spawned {
        eprintln!("apid: mover: cannot start: {err}");
    }
}

/// One pass over the stopping nodes: each agent that can leave now is moved, one at a time, and the
/// others are noted with why they wait. True while a stopping node still runs agents.
fn evacuate(app: &App) -> bool {
    let stopped = app.nodes.stopped_ids();
    {
        let mut book = book();
        book.waiting.retain(|node, _| stopped.contains(node));
        book.now.retain(|node| stopped.contains(node));
    }
    if stopped.is_empty() {
        return false;
    }
    let agents = app
        .agents
        .select(|a| (a.status != Status::Offline).then(|| (a.cid, a.role.clone())));
    let mut busy = false;
    for node in &stopped {
        let on: Vec<&(u64, String)> = agents
            .iter()
            .filter(|(cid, _)| fleet::node_of(app, *cid) == *node)
            .collect();
        if on.is_empty() {
            continue;
        }
        busy = true;
        let now = book().now.contains(node);
        let mut seen = BTreeSet::new();
        for (cid, role) in on {
            // The owner may have resumed it meanwhile.
            if !app.nodes.is_stopped(node) {
                break;
            }
            seen.insert(*cid);
            let retry = book().retry_at.get(cid).copied();
            if retry.is_some_and(|at| Instant::now() < at) {
                continue;
            }
            let (reason, moved) = match leave(app, *cid, role, node, now) {
                Ok(to) => {
                    book().retry_at.remove(cid);
                    eprintln!("apid: stop of {node}: agent {cid} moved to {to}");
                    (None, true)
                }
                Err(Wait::Turn(reason)) => (Some(reason), false),
                Err(Wait::Failed(reason)) => {
                    book().retry_at.insert(*cid, Instant::now() + RETRY_AFTER);
                    eprintln!("apid: stop of {node}: agent {cid} stays for now: {reason}");
                    (Some(reason), false)
                }
            };
            // Each outcome shows at once: a pass of real moves takes a while.
            if note(node, *cid, reason) || moved {
                publish(app, node);
            }
        }
        // An agent that left the node another way (moved by the owner, gone offline) waits for nothing.
        let dropped = book().waiting.get_mut(node).is_some_and(|waiting| {
            let before = waiting.len();
            waiting.retain(|cid, _| seen.contains(cid));
            waiting.len() != before
        });
        if dropped {
            publish(app, node);
        }
    }
    busy
}

/// Notes why agent `cid` waits on `node` (None: it no longer does); true when that changed.
fn note(node: &str, cid: u64, reason: Option<String>) -> bool {
    let mut book = book();
    let waiting = book.waiting.entry(node.to_string()).or_default();
    match reason {
        Some(reason) => waiting.insert(cid, reason.clone()).as_ref() != Some(&reason),
        None => waiting.remove(&cid).is_some(),
    }
}

/// Why an agent cannot leave yet: its turn first, or a move that did not happen (tried again after
/// [`RETRY_AFTER`]).
enum Wait {
    Turn(String),
    Failed(String),
}

/// Moves agent `cid` off `node` to the node with the most room for it; Ok is where it went. With `now`
/// its turn in flight is interrupted first, and its work resumes where it goes.
fn leave(app: &App, cid: u64, role: &str, node: &str, now: bool) -> Result<String, Wait> {
    if placement::is_moving(cid) {
        return Err(Wait::Turn("moving now".into()));
    }
    if app.work.current(cid).is_some() {
        if now {
            app.work.interrupt(
                app,
                cid,
                "its machine is stopping; the work resumes on another machine",
            );
            return Err(Wait::Turn("its turn is interrupted to move".into()));
        }
        return Err(Wait::Turn("finishing its turn first".into()));
    }
    let needs = placement::agent_volumes(app, cid, node).map_err(Wait::Failed)?;
    let Some(target) = placement::target_for(app, role, node, &needs) else {
        // No machine in its pool has room: a step session hands over, and its step starts again in a new session
        // on a machine with room (plan 17.2). Any other agent waits for room, as before.
        if work_loop::hand_over_for_pool(app, cid) {
            return Err(Wait::Turn(
                "its session hands over: its step starts again on another machine".into(),
            ));
        }
        return Err(Wait::Failed(
            "no machine in its pool has room for it, with its volumes".into(),
        ));
    };
    placement::move_agent(app, cid, &target)
        .map(|moved| moved.to)
        .map_err(|refusal| Wait::Failed(error_of(&refusal)))
}

/// The message of an error response (`{error}`), or its whole text.
fn error_of(resp: &Response) -> String {
    let text = placement::body_text(resp);
    serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|body| body["error"].as_str().map(str::to_string))
        .unwrap_or(text)
}

/// Runs the rebalance the owner asked for, and the rebalance of each resumed node that reports now.
fn rebalance_due(app: &App) {
    let planned = std::mem::take(&mut book().planned);
    if !planned.is_empty() {
        run(app, &planned);
    }
    if book().rebalance_for.is_empty() {
        return;
    }
    let reporting: BTreeSet<String> = fleet::reports(app)
        .into_iter()
        .map(|(node, _, _)| node)
        .collect();
    let now_ms = apz_plaza::now_ms();
    let due: Vec<String> = {
        let mut book = book();
        book.rebalance_for
            .retain(|_, asked| now_ms.saturating_sub(*asked) < REBALANCE_WAIT_MS);
        let due: Vec<String> = book
            .rebalance_for
            .keys()
            .filter(|node| reporting.contains(*node))
            .cloned()
            .collect();
        for node in &due {
            book.rebalance_for.remove(node);
        }
        due
    };
    if !due.is_empty() {
        run(app, &plan_now(app));
    }
}

/// Carries out planned moves, one at a time. A step whose agent is no longer where the plan found it is
/// skipped; a move that is refused (the agent got work, the target was stopped) is logged.
fn run(app: &App, plan: &[Planned]) {
    let mut touched = BTreeSet::new();
    for step in plan {
        if fleet::node_of(app, step.cid) != step.from {
            continue;
        }
        match placement::move_agent(app, step.cid, &step.to) {
            Ok(moved) => {
                eprintln!(
                    "apid: rebalance: agent {} moved {} → {} ({} bytes, {} ms)",
                    step.cid, moved.from, moved.to, moved.bytes, moved.ms
                );
                touched.insert(step.from.clone());
                touched.insert(step.to.clone());
            }
            Err(refusal) => eprintln!(
                "apid: rebalance: agent {} stays on {}: {}",
                step.cid,
                step.from,
                error_of(&refusal)
            ),
        }
    }
    for node in touched {
        publish(app, &node);
    }
}

/// Publishes node `node`'s view as a `node` event.
fn publish(app: &App, node: &str) {
    if let Some(view) = nodes::view_by_id(app, node) {
        app.bus.publish("node", &view);
    }
}

/// One planned move.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Planned {
    pub cid: u64,
    pub from: String,
    pub to: String,
}

/// A machine in service, as a rebalance sees it.
#[derive(Debug, Clone)]
pub struct Host {
    pub node: String,
    pub total_mib: u64,
    /// Free memory now, less what is reserved on it.
    pub free_mib: u64,
}

/// An agent that is not offline, as a rebalance sees it.
#[derive(Debug, Clone)]
pub struct Guest {
    pub cid: u64,
    pub node: String,
    /// Idle, asleep or dozing, with no work in flight or queued and no move or sleep under way.
    pub movable: bool,
    /// Not asleep: its VM holds memory where it runs.
    pub awake: bool,
}

/// The moves that spread the agents over the hosts, each host's share in proportion to its memory
/// (pure). Each move takes a movable agent from the host most over its share to the host most under
/// it with room for one more ([`ADMIT_MIB`] free, after [`AGENT_MIB`] for each move already planned
/// onto it), and only while the two are more than one agent apart, so every move narrows the spread and
/// the plan ends. An awake agent goes before an asleep one (it frees memory where it was), then the
/// lowest CID; an agent moves once at most. Agents on a machine that is not a host stay where they are
/// and do not count. At most `max` moves.
pub fn plan(hosts: &[Host], guests: &[Guest], max: usize) -> Vec<Planned> {
    let total: u64 = hosts.iter().map(|host| host.total_mib).sum();
    if hosts.len() < 2 || total == 0 {
        return Vec::new();
    }
    let on: Vec<&Guest> = guests
        .iter()
        .filter(|guest| hosts.iter().any(|host| host.node == guest.node))
        .collect();
    let agents = on.len() as f64;
    let mut count: BTreeMap<&str, f64> =
        hosts.iter().map(|host| (host.node.as_str(), 0.0)).collect();
    for guest in &on {
        *count.entry(guest.node.as_str()).or_default() += 1.0;
    }
    let mut free: BTreeMap<&str, u64> = hosts
        .iter()
        .map(|host| (host.node.as_str(), host.free_mib))
        .collect();
    let mut moved: BTreeSet<u64> = BTreeSet::new();
    let mut planned = Vec::new();
    while planned.len() < max {
        let surplus =
            |host: &Host| count[host.node.as_str()] - agents * host.total_mib as f64 / total as f64;
        let movable = |host: &Host| {
            on.iter()
                .filter(|guest| guest.movable && guest.node == host.node)
                .filter(|guest| !moved.contains(&guest.cid))
                .min_by_key(|guest| (!guest.awake, guest.cid))
                .copied()
        };
        let Some(source) = hosts
            .iter()
            .filter(|host| movable(host).is_some())
            .max_by(|a, b| {
                surplus(a)
                    .total_cmp(&surplus(b))
                    .then_with(|| b.node.cmp(&a.node))
            })
        else {
            break;
        };
        let Some(target) = hosts
            .iter()
            .filter(|host| host.node != source.node && free[host.node.as_str()] >= ADMIT_MIB)
            .min_by(|a, b| {
                surplus(a)
                    .total_cmp(&surplus(b))
                    .then_with(|| a.node.cmp(&b.node))
            })
        else {
            break;
        };
        if surplus(source) - surplus(target) <= 1.0 + 1e-9 {
            break;
        }
        let Some(guest) = movable(source) else {
            break;
        };
        moved.insert(guest.cid);
        *count.entry(source.node.as_str()).or_default() -= 1.0;
        *count.entry(target.node.as_str()).or_default() += 1.0;
        if let Some(room) = free.get_mut(target.node.as_str()) {
            *room = room.saturating_sub(AGENT_MIB);
        }
        if let Some(room) = free.get_mut(source.node.as_str()) {
            *room = room.saturating_add(AGENT_MIB);
        }
        planned.push(Planned {
            cid: guest.cid,
            from: source.node.clone(),
            to: target.node.clone(),
        });
    }
    planned
}

/// The moves of a rebalance, planned in each pool on its own (plan 17.2): a VM moves only within its pool, so
/// the hosts of one pool are planned together with the agents on them, and the pools share `max`. A host with
/// no pool is planned for nothing.
pub fn plan_in_pools(
    hosts: Vec<(Option<Pool>, Host)>,
    guests: &[Guest],
    max: usize,
) -> Vec<Planned> {
    let mut pools: BTreeMap<Pool, Vec<Host>> = BTreeMap::new();
    for (pool, host) in hosts {
        if let Some(pool) = pool {
            pools.entry(pool).or_default().push(host);
        }
    }
    let mut planned = Vec::new();
    for members in pools.values() {
        let room = max - planned.len();
        planned.extend(plan(members, guests, room));
    }
    planned
}

/// The rebalance [`plan`] for the plaza as it is: the nodes in service that report their memory, planned in
/// each pool (see [`plan_in_pools`]), and every agent that is not offline.
pub fn plan_now(app: &App) -> Vec<Planned> {
    let stopped = app.nodes.stopped_ids();
    let at = Instant::now();
    let hosts: Vec<(Option<Pool>, Host)> = fleet::reports(app)
        .into_iter()
        .filter(|(node, _, _)| !stopped.contains(node))
        .filter_map(|(node, _, _)| {
            let (total, used) = placement::memory_of(app, &node)?;
            let pool = placement::pool_of(app, &node);
            let host = Host {
                free_mib: total
                    .saturating_sub(used)
                    .saturating_sub(placement::reserved_mib(&node, at)),
                total_mib: total,
                node,
            };
            Some((pool, host))
        })
        .collect();
    let agents = app
        .agents
        .select(|a| (a.status != Status::Offline).then_some((a.cid, a.status)));
    let guests: Vec<Guest> = agents
        .into_iter()
        .map(|(cid, status)| Guest {
            cid,
            node: fleet::node_of(app, cid),
            movable: matches!(status, Status::Idle | Status::Asleep | Status::Dozing)
                && app.work.current(cid).is_none()
                && app.work.queued(cid).is_empty()
                && !placement::is_moving(cid)
                && !crate::sleep::in_transition(cid),
            awake: status != Status::Asleep,
        })
        .collect();
    plan_in_pools(hosts, &guests, MAX_MOVES)
}

/// Routes: stop, resume and drain a node; rebalance the plaza.
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .post("/v1/nodes/{id}/stop", stop_route)
        .doc(
            RouteDoc::new("Stop a node: its agents move to the other machines")
                .errors(&[400, 404, 409]),
        )
        .post("/v1/nodes/{id}/resume", resume_route)
        .doc(
            RouteDoc::new("Resume a stopped node, and spread the agents onto it")
                .errors(&[400, 404, 409]),
        )
        .post("/v1/nodes/{id}/drain", drain_route)
        .doc(
            RouteDoc::new("Stop (on) or resume (off) a node: the older name of both")
                .errors(&[400, 404, 409]),
        )
        .post("/v1/nodes/rebalance", rebalance_route)
        .doc(RouteDoc::new("Spread the idle agents over the machines").errors(&[400]))
}

#[derive(Default, Deserialize)]
struct StopBody {
    /// Interrupt the turns in flight: their work resumes on the agents' new machines.
    #[serde(default)]
    now: bool,
}

#[derive(Deserialize)]
struct ResumeBody {
    /// Spread the agents onto the node once it reports (true when left out).
    #[serde(default = "yes")]
    rebalance: bool,
}

impl Default for ResumeBody {
    fn default() -> Self {
        ResumeBody { rebalance: true }
    }
}

fn yes() -> bool {
    true
}

#[derive(Default, Deserialize)]
struct DrainBody {
    on: Option<bool>,
}

#[derive(Default, Deserialize)]
struct RebalanceBody {
    /// Only say which moves a rebalance would make.
    #[serde(default)]
    dry_run: bool,
}

/// `POST /v1/nodes/{id}/stop {now?}`.
fn stop_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    match nodes::parse_or_default::<StopBody>(req) {
        Ok(body) => stop(app, p.get("id"), body.now),
        Err(refusal) => refusal,
    }
}

/// `POST /v1/nodes/{id}/resume {rebalance?}`.
fn resume_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    match nodes::parse_or_default::<ResumeBody>(req) {
        Ok(body) => resume(app, p.get("id"), body.rebalance),
        Err(refusal) => refusal,
    }
}

/// `POST /v1/nodes/{id}/drain {on}`: on (the default) stops the node, off resumes it.
fn drain_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    match nodes::parse_or_default::<DrainBody>(req) {
        Ok(DrainBody { on: Some(false) }) => resume(app, p.get("id"), true),
        Ok(_) => stop(app, p.get("id"), false),
        Err(refusal) => refusal,
    }
}

/// Takes node `id` out of service; its agents leave on the mover's next pass.
pub(crate) fn stop(app: &App, id: &str, now: bool) -> Response {
    if let Err((status, message)) = app.nodes.set_stopped(id, true) {
        return Response::error(status, &message);
    }
    {
        let mut book = book();
        if now {
            book.now.insert(id.to_string());
        }
        book.rebalance_for.remove(id);
    }
    save(app);
    kick();
    answer(app, id)
}

/// Puts node `id` back in service: its agents' held work goes out, and with `rebalance` the agents are
/// spread onto it once it reports.
pub(crate) fn resume(app: &App, id: &str, rebalance: bool) -> Response {
    if let Err((status, message)) = app.nodes.set_stopped(id, false) {
        return Response::error(status, &message);
    }
    {
        let mut book = book();
        book.now.remove(id);
        book.waiting.remove(id);
        if rebalance {
            book.rebalance_for
                .insert(id.to_string(), apz_plaza::now_ms());
        }
    }
    save(app);
    app.work.dispatch_idle(app);
    kick();
    answer(app, id)
}

/// `POST /v1/nodes/rebalance {dry_run?}`: the moves a rebalance makes now, started unless `dry_run`.
fn rebalance_route(app: &App, req: &Request, _p: &Params<'_>) -> Response {
    let body = match nodes::parse_or_default::<RebalanceBody>(req) {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    let moves = plan_now(app);
    let started = !body.dry_run && !moves.is_empty();
    if started {
        book().planned = moves.clone();
        kick();
    }
    Response::json(200, &json!({ "moves": moves, "started": started }))
}

/// The node's view after a change, published as a `node` event too.
fn answer(app: &App, id: &str) -> Response {
    match nodes::view_by_id(app, id) {
        Some(view) => {
            app.bus.publish("node", &view);
            Response::json(200, &view)
        }
        None => Response::error(404, &format!("no node {id}")),
    }
}

/// Saves the node store now (the saver's write, not its next pass), so a stop outlives a restart.
fn save(app: &App) {
    crate::store::saver::save_now(app, "nodes");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(node: &str, total_mib: u64, free_mib: u64) -> Host {
        Host {
            node: node.into(),
            total_mib,
            free_mib,
        }
    }

    fn guest(cid: u64, node: &str, movable: bool, awake: bool) -> Guest {
        Guest {
            cid,
            node: node.into(),
            movable,
            awake,
        }
    }

    fn moves(plan: &[Planned]) -> Vec<(u64, &str, &str)> {
        plan.iter()
            .map(|step| (step.cid, step.from.as_str(), step.to.as_str()))
            .collect()
    }

    #[test]
    fn a_resumed_empty_machine_takes_its_share_of_the_agents() {
        // Two equal machines, four agents on n0: two move to n1, awake ones first.
        let hosts = [host("n0", 8192, 4096), host("n1", 8192, 8192)];
        let guests = [
            guest(200, "n0", true, false),
            guest(201, "n0", true, true),
            guest(202, "n0", true, false),
            guest(203, "n0", true, true),
        ];
        assert_eq!(
            moves(&plan(&hosts, &guests, MAX_MOVES)),
            [(201, "n0", "n1"), (203, "n0", "n1")]
        );
    }

    #[test]
    fn shares_follow_memory_and_a_one_agent_difference_is_left_alone() {
        // n1 has three times n0's memory: of eight agents, six belong there.
        let hosts = [host("n0", 4096, 2048), host("n1", 12288, 12288)];
        let guests: Vec<Guest> = (0..8).map(|i| guest(200 + i, "n0", true, true)).collect();
        assert_eq!(plan(&hosts, &guests, MAX_MOVES).len(), 6);
        // Three agents on two equal machines: two and one is as even as it gets.
        let even = [host("n0", 8192, 8192), host("n1", 8192, 8192)];
        let three = [
            guest(1, "n0", true, true),
            guest(2, "n0", true, true),
            guest(3, "n1", true, true),
        ];
        assert!(plan(&even, &three, MAX_MOVES).is_empty());
    }

    #[test]
    fn busy_agents_stay_and_a_full_machine_takes_none() {
        let hosts = [host("n0", 8192, 4096), host("n1", 8192, 8192)];
        let busy = [
            guest(1, "n0", false, true),
            guest(2, "n0", false, true),
            guest(3, "n0", false, true),
        ];
        assert!(
            plan(&hosts, &busy, MAX_MOVES).is_empty(),
            "nothing working moves"
        );
        let full = [host("n0", 8192, 4096), host("n1", 8192, ADMIT_MIB - 1)];
        let idle = [
            guest(1, "n0", true, true),
            guest(2, "n0", true, true),
            guest(3, "n0", true, true),
        ];
        assert!(plan(&full, &idle, MAX_MOVES).is_empty());
        // Each planned move counts against the target's room: room for two moves, then for one.
        let tight = [
            host("n0", 8192, 4096),
            host("n1", 8192, ADMIT_MIB + AGENT_MIB),
        ];
        let four: Vec<Guest> = (0..4).map(|i| guest(i, "n0", true, true)).collect();
        assert_eq!(plan(&tight, &four, MAX_MOVES).len(), 2);
        let tighter = [host("n0", 8192, 4096), host("n1", 8192, ADMIT_MIB)];
        assert_eq!(plan(&tighter, &four, MAX_MOVES).len(), 1);
    }

    #[test]
    fn agents_elsewhere_do_not_count_and_the_plan_stops_at_its_limit() {
        // An agent on a stopped machine (not a host) is neither moved nor counted.
        let hosts = [host("n0", 8192, 8192), host("n1", 8192, 8192)];
        let guests = [
            guest(1, "n2", true, true),
            guest(2, "n0", true, true),
            guest(3, "n1", true, true),
        ];
        assert!(plan(&hosts, &guests, MAX_MOVES).is_empty());
        let many: Vec<Guest> = (0..40).map(|i| guest(i, "n0", true, true)).collect();
        assert_eq!(plan(&hosts, &many, 5).len(), 5);
        assert!(
            plan(&hosts[..1], &many, MAX_MOVES).is_empty(),
            "one machine"
        );
        assert!(plan(&[], &many, MAX_MOVES).is_empty());
    }

    #[test]
    fn three_machines_even_out_from_the_most_loaded_one() {
        let hosts = [
            host("n0", 8192, 8192),
            host("n1", 8192, 8192),
            host("n2", 8192, 8192),
        ];
        let guests: Vec<Guest> = (0..6)
            .map(|i| guest(i, "n1", true, true))
            .chain([guest(10, "n0", true, true)])
            .collect();
        let planned = plan(&hosts, &guests, MAX_MOVES);
        let mut after: BTreeMap<&str, usize> = BTreeMap::new();
        for g in &guests {
            let node = planned
                .iter()
                .find(|step| step.cid == g.cid)
                .map_or(g.node.as_str(), |step| step.to.as_str());
            *after.entry(node).or_default() += 1;
        }
        let counts: Vec<usize> = after.values().copied().collect();
        assert!(
            counts.iter().max().unwrap() - counts.iter().min().unwrap() <= 1,
            "{after:?}"
        );
        assert!(planned.iter().all(|step| step.from == "n1"), "{planned:?}");
    }

    #[test]
    fn a_rebalance_plans_each_pool_on_its_own() {
        let arm = Pool::of(
            Some("aarch64"),
            Some("kvm"),
            Some("arm64 midr=0x41:0xd0c:0x3:1"),
        )
        .unwrap();
        let x86 = Pool::of(
            Some("x86_64"),
            Some("kvm"),
            Some("x86_64 vendor=GenuineIntel"),
        )
        .unwrap();
        // Two arm64 machines, and an x86_64 one with all the room: the agents on n0 spread over the arm64 pool only.
        let hosts = vec![
            (Some(arm.clone()), host("n0", 8192, 4096)),
            (Some(arm), host("n1", 8192, 8192)),
            (Some(x86), host("x0", 8192, 8192)),
        ];
        let guests = [
            guest(200, "n0", true, false),
            guest(201, "n0", true, true),
            guest(202, "n0", true, false),
            guest(203, "n0", true, true),
        ];
        assert_eq!(
            moves(&plan_in_pools(hosts, &guests, MAX_MOVES)),
            [(201, "n0", "n1"), (203, "n0", "n1")]
        );
    }

    #[test]
    fn a_machine_with_no_pool_takes_no_rebalance() {
        let hosts = vec![
            (None, host("n0", 8192, 4096)),
            (None, host("n1", 8192, 8192)),
        ];
        let guests = [guest(200, "n0", true, true), guest(201, "n0", true, true)];
        assert!(plan_in_pools(hosts, &guests, MAX_MOVES).is_empty());
    }

    #[test]
    fn a_stopped_node_moves_its_agents_only_within_their_pool() {
        // n1 (arm64) is stopped; n2 is the other arm64 machine with less room, n3 an x86_64 one with all the room.
        let arm = Pool::of(
            Some("aarch64"),
            Some("kvm"),
            Some("arm64 midr=0x41:0xd0c:0x3:1"),
        )
        .unwrap();
        let x86 = Pool::of(
            Some("x86_64"),
            Some("kvm"),
            Some("x86_64 vendor=GenuineIntel"),
        )
        .unwrap();
        let candidate = |node: &str, free_mib: u64, pool: &Pool| placement::Candidate {
            node: node.into(),
            free_mib,
            running_vms: 0,
            warm: false,
            pool: Some(pool.clone()),
            base: false,
            disk_free_gib: None,
            idle_cpus: None,
        };
        let fleet = vec![candidate("n2", 4096, &arm), candidate("n3", 16384, &x86)];
        let movable = placement::in_pool(fleet, "n1", Some(&arm));
        assert_eq!(placement::pick(&movable).as_deref(), Some("n2"));
    }
}
