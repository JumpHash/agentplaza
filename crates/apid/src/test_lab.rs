//! The test lab (W56, plan 22.5): the nodes that take qualification runs, the capacity they give the lab, and the queue
//! of runs that reserve it.
//!
//! A node gives the lab its test share (`nodes::Node::test_share`, in percent): that share of its CPUs and memory. A
//! qualification run (`LabRun`) asks for CPUs, memory and capabilities; it is admitted when the lab has that capacity
//! free on nodes that have the capabilities, and it holds its reservations (one piece per node) until it finishes.
//! Runs queue by channel: a yank or a hotfix first, then stable, beta and nightly; first come, first served within a
//! channel. The queue is strict: a run that does not fit holds back every run behind it.
//!
//! Owner routes:
//!   GET  /v1/test-lab                  → the lab's nodes and capacity, the queue, what runs and the last runs that ended
//!   POST /v1/test-lab/runs             {project, channel, cpus, memory_mib, needs?} → 201 the run (queued, or running
//!                                       when it fits now); the qualification recipe (10.12) queues its runs the same way
//!   GET  /v1/test-lab/runs/{id}        → one run, with its reservations and its place in the queue (null once it runs)
//!   POST /v1/test-lab/runs/{id}/cancel → ends a queued or running run as cancelled and admits the queue again
//!
//! What a run observes (its collector's OTLP and logs, its assertions) is in `lab_observe`: a run keeps its change and
//! topology, and the hash of the ingest token its collector posts with.

use std::collections::BTreeSet;
use std::sync::Mutex;

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, TableDef};
use serde::{Deserialize, Serialize};

use crate::lab_observe::{LabLog, LabSpan, LabVerdict};
use crate::nodes::Machine;
use crate::state::App;

/// The channels, first to last. A yank and a hotfix share the first rank.
pub const CHANNELS: [&str; 5] = ["yank", "hotfix", "stable", "beta", "nightly"];
/// How many runs that ended the overview lists.
const RECENT_DONE: usize = 20;

/// A channel's rank: 0 for a yank or a hotfix, 1 stable, 2 beta, 3 nightly (None for an unknown channel).
pub fn rank(channel: &str) -> Option<u8> {
    match channel {
        "yank" | "hotfix" => Some(0),
        "stable" => Some(1),
        "beta" => Some(2),
        "nightly" => Some(3),
        _ => None,
    }
}

/// A piece of a node's lab capacity that a run holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct Reservation {
    pub node: String,
    pub cpus: u32,
    pub memory_mib: u64,
}

/// A qualification run in the lab: what it asks for, its state and what it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct LabRun {
    /// `q<seq>`.
    pub id: String,
    pub project: String,
    /// yank | hotfix | stable | beta | nightly (see [`rank`]).
    pub channel: String,
    /// queued | running | done.
    pub state: String,
    pub cpus: u32,
    pub memory_mib: u64,
    /// The capabilities its nodes must have (`arch:x86_64`, `browser:chromium`, `phone`, ...).
    #[serde(default)]
    #[table(list = "test_lab_run_need")]
    pub needs: Vec<String>,
    /// What it holds while it runs; kept after it ends as the record of where it ran.
    #[serde(default)]
    #[table(child = "test_lab_reservation")]
    pub reservations: Vec<Reservation>,
    pub enqueued_ms: u64,
    pub started_ms: Option<u64>,
    pub ended_ms: Option<u64>,
    /// passed | failed | cancelled; None until it is done.
    pub outcome: Option<String>,
    /// The change a failing assertion of the run is a finding on (empty when none).
    #[serde(default)]
    pub change: String,
    /// The topology the run observes (its name in the project's `.plaza/topologies`), whose assertions assess checks.
    #[serde(default)]
    pub topology: String,
    /// The release the run qualifies (empty when none): its performance is compared with the last qualified stable
    /// release's (PLAN 22.8, `lab_performance`).
    #[serde(default)]
    pub release: String,
    /// The SHA-256 (hex) of the ingest token its collector posts with; the token itself is given once, when queued.
    #[serde(default)]
    pub token_sha: String,
    /// The log lines and spans the run kept (its collector's observations), and those refused past the cap.
    #[serde(default)]
    pub observed: u64,
    #[serde(default)]
    pub dropped: u64,
}

/// The lab's state of record: its runs, and what each run observed (its logs, spans and verdicts, child rows keyed by
/// their run).
#[derive(Default, apz_store::Table)]
#[table(name = "test_lab")]
pub(crate) struct Persisted {
    #[table(child = "test_lab_run")]
    pub(crate) runs: Vec<LabRun>,
    #[table(child = "test_lab_log")]
    pub(crate) logs: Vec<LabLog>,
    #[table(child = "test_lab_span")]
    pub(crate) spans: Vec<LabSpan>,
    #[table(child = "test_lab_verdict")]
    pub(crate) verdicts: Vec<LabVerdict>,
}

/// The lab's table: `test_lab`, with its runs and their reservations and needs.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(Persisted)
}

/// The lab's runs, in memory (plaza.db is the record; the saver writes what changed).
#[derive(Default)]
pub struct Store {
    pub(crate) inner: Mutex<Persisted>,
    /// The last cumulative total of each counter a run's collector reported (not kept: a restart counts the first total
    /// of each as an increase).
    pub(crate) totals: Mutex<std::collections::BTreeMap<String, f64>>,
}

impl Store {
    /// The lab's rows, as they are now.
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        let p = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        RowSet::one(def(), &*p)
    }
    /// Replaces the runs with what plaza.db holds (at start).
    pub fn load(&self, conn: &Connection) -> Result<(), StoreError> {
        let stored = apz_store::table::load_one::<Persisted>(conn, def())?.unwrap_or_default();
        *self.inner.lock().unwrap_or_else(|e| e.into_inner()) = stored;
        Ok(())
    }
    /// Every run, in the order they were made.
    pub fn runs(&self) -> Vec<LabRun> {
        self.inner
            .lock()
            .map(|p| p.runs.clone())
            .unwrap_or_default()
    }
}

/// A node's lab capacity: what it gives the lab, and what it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabNode {
    pub id: String,
    /// The lab's share of its CPUs and of its memory (its test share of each).
    pub cpus: u64,
    pub memory_mib: u64,
    pub capabilities: BTreeSet<String>,
}

/// What is free on one node for the lab now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub node: String,
    pub cpus: u64,
    pub memory_mib: u64,
    pub capabilities: BTreeSet<String>,
}

/// The capabilities a node's machine reports, in the names the runs use (pure).
pub fn capabilities_of(machine: &Machine) -> BTreeSet<String> {
    let mut caps = BTreeSet::new();
    if let Some(arch) = &machine.arch {
        caps.insert(format!("arch:{arch}"));
    }
    if let Some(os) = &machine.os {
        caps.insert(format!("os:{os}"));
    }
    if !machine.gpus.is_empty() {
        caps.insert("gpu".to_string());
    }
    for browser in &machine.browsers {
        caps.insert(format!("browser:{browser}"));
    }
    if machine.mac.as_ref().is_some_and(|mac| mac.simulators > 0) {
        caps.insert("simulator".to_string());
    }
    if !machine.phones.is_empty() {
        caps.insert("phone".to_string());
    }
    if machine.traffic_shaping == Some(true) {
        caps.insert("traffic-shaping".to_string());
    }
    caps
}

/// A node's share of `total` in percent (pure).
fn share_of(total: u64, percent: u32) -> u64 {
    total.saturating_mul(u64::from(percent.min(100))) / 100
}

/// The free capacity of every lab node after the reservations of the running runs (pure). A run that is not running
/// holds nothing.
pub fn free_slots(nodes: &[LabNode], runs: &[LabRun]) -> Vec<Slot> {
    let mut slots: Vec<Slot> = nodes
        .iter()
        .map(|node| Slot {
            node: node.id.clone(),
            cpus: node.cpus,
            memory_mib: node.memory_mib,
            capabilities: node.capabilities.clone(),
        })
        .collect();
    for run in runs.iter().filter(|run| run.state == "running") {
        for held in &run.reservations {
            take(
                &mut slots,
                &held.node,
                u64::from(held.cpus),
                held.memory_mib,
            );
        }
    }
    slots
}

/// Takes `cpus` and `memory_mib` off node `node`'s slot, down to zero (pure).
fn take(slots: &mut [Slot], node: &str, cpus: u64, memory_mib: u64) {
    if let Some(slot) = slots.iter_mut().find(|slot| slot.node == node) {
        slot.cpus = slot.cpus.saturating_sub(cpus);
        slot.memory_mib = slot.memory_mib.saturating_sub(memory_mib);
    }
}

/// The reservations that give a run its CPUs and memory from the free slots (pure): only nodes with every capability the
/// run needs count, the one with the most free memory first, a piece from each until the run is covered. None when the
/// free capacity does not cover it.
pub fn plan(
    slots: &[Slot],
    cpus: u32,
    memory_mib: u64,
    needs: &[String],
) -> Option<Vec<Reservation>> {
    let mut left_cpus = u64::from(cpus);
    let mut left_memory = memory_mib;
    let mut usable: Vec<&Slot> = slots
        .iter()
        .filter(|slot| needs.iter().all(|need| slot.capabilities.contains(need)))
        .filter(|slot| slot.cpus > 0 || slot.memory_mib > 0)
        .collect();
    usable.sort_by(|a, b| b.memory_mib.cmp(&a.memory_mib).then(a.node.cmp(&b.node)));
    let mut out = Vec::new();
    for slot in usable {
        if left_cpus == 0 && left_memory == 0 {
            break;
        }
        let take_cpus = left_cpus.min(slot.cpus);
        let take_memory = left_memory.min(slot.memory_mib);
        if take_cpus == 0 && take_memory == 0 {
            continue;
        }
        out.push(Reservation {
            node: slot.node.clone(),
            cpus: u32::try_from(take_cpus).unwrap_or(u32::MAX),
            memory_mib: take_memory,
        });
        left_cpus -= take_cpus;
        left_memory -= take_memory;
    }
    (left_cpus == 0 && left_memory == 0).then_some(out)
}

/// The seq of a run id (`q12` is 12), for ordering within a channel.
fn seq_of(id: &str) -> u64 {
    id.strip_prefix('q')
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Admits the queued runs in queue order against the free slots (pure): each admitted run gets its reservations and
/// `started_ms = now`, and the scan stops at the first run that does not fit, so the runs behind it wait. Returns the ids
/// admitted, in the order they were.
pub fn schedule(runs: &mut [LabRun], nodes: &[LabNode], now: u64) -> Vec<String> {
    let mut slots = free_slots(nodes, runs);
    let mut queued: Vec<usize> = runs
        .iter()
        .enumerate()
        .filter(|(_, run)| run.state == "queued")
        .map(|(i, _)| i)
        .collect();
    queued.sort_by_key(|&i| {
        (
            rank(&runs[i].channel).unwrap_or(u8::MAX),
            seq_of(&runs[i].id),
        )
    });
    let mut admitted = Vec::new();
    for i in queued {
        let Some(reserved) = plan(&slots, runs[i].cpus, runs[i].memory_mib, &runs[i].needs) else {
            break;
        };
        for held in &reserved {
            take(
                &mut slots,
                &held.node,
                u64::from(held.cpus),
                held.memory_mib,
            );
        }
        runs[i].state = "running".into();
        runs[i].started_ms = Some(now);
        runs[i].reservations = reserved;
        admitted.push(runs[i].id.clone());
    }
    admitted
}

/// The lab's nodes now: each node in service with a test share, its lab share of its CPUs and memory, and its
/// capabilities. A node that is offline, out of service or has no test share is not in the lab.
pub(crate) fn lab_nodes(app: &App) -> Vec<LabNode> {
    crate::nodes::views(app)
        .into_iter()
        .filter(|view| view.online && !view.node.draining && view.node.test_share > 0)
        .map(|view| {
            let machine = if view.node.id == crate::fleet::LOCAL {
                crate::nodes::local_node(app).machine
            } else {
                view.node.machine.clone()
            };
            let share = view.node.test_share;
            LabNode {
                id: view.node.id.clone(),
                cpus: share_of(machine.cpus.unwrap_or(0), share),
                memory_mib: share_of(machine.memory_mib.unwrap_or(0), share),
                capabilities: capabilities_of(&machine),
            }
        })
        .collect()
}

/// Queues a qualification run and admits what fits. `channel` is one of [`CHANNELS`]; it asks for `cpus` CPUs and
/// `memory_mib` MiB on nodes with `needs`. `change` names the change its failing steps are findings on, `topology` the
/// topology it observes (both may be empty). Returns the run as it is after admission, and the ingest token its
/// collector posts with (the only time the token is given; the run keeps its hash).
pub fn enqueue(
    app: &App,
    project: &str,
    channel: &str,
    cpus: u32,
    memory_mib: u64,
    needs: Vec<String>,
    (change, topology): (&str, &str),
    release: &str,
) -> Result<(LabRun, String), String> {
    if project.trim().is_empty() {
        return Err("a run needs its project".into());
    }
    let release = release.trim();
    if !release.is_empty()
        && app
            .releases
            .get(release)
            .is_none_or(|r| r.project != project)
    {
        return Err(format!("{release} is not a release of this project"));
    }
    if rank(channel).is_none() {
        return Err(format!(
            "the channel must be one of {}",
            CHANNELS.join(", ")
        ));
    }
    if cpus == 0 || memory_mib == 0 {
        return Err("a run asks for at least one CPU and some memory".into());
    }
    let now = apz_plaza::now_ms();
    let nodes = lab_nodes(app);
    let token = crate::lab_observe::new_token();
    let mut p = app.test_lab.inner.lock().map_err(|e| e.to_string())?;
    let seq = crate::nodes::next_seq(p.runs.iter().map(|run| run.id.as_str()), 'q');
    let id = format!("q{seq}");
    p.runs.push(LabRun {
        id: id.clone(),
        project: project.trim().to_string(),
        channel: channel.to_string(),
        state: "queued".into(),
        cpus,
        memory_mib,
        needs,
        reservations: Vec::new(),
        enqueued_ms: now,
        started_ms: None,
        ended_ms: None,
        outcome: None,
        change: change.trim().to_string(),
        topology: topology.trim().to_string(),
        release: release.to_string(),
        token_sha: crate::lab_observe::token_hash(&token),
        observed: 0,
        dropped: 0,
    });
    schedule(&mut p.runs, &nodes, now);
    let run = p
        .runs
        .iter()
        .find(|run| run.id == id)
        .cloned()
        .ok_or_else(|| "the run was not kept".to_string())?;
    Ok((run, token))
}

/// Ends a queued or running run with `outcome` (passed | failed | cancelled), releases what it held, and admits the
/// queue again. A run that is done already is refused.
pub fn finish(app: &App, id: &str, outcome: &str) -> Result<LabRun, String> {
    if !matches!(outcome, "passed" | "failed" | "cancelled") {
        return Err("the outcome must be passed, failed or cancelled".into());
    }
    let now = apz_plaza::now_ms();
    let nodes = lab_nodes(app);
    let mut p = app.test_lab.inner.lock().map_err(|e| e.to_string())?;
    let run = p
        .runs
        .iter_mut()
        .find(|run| run.id == id)
        .ok_or_else(|| "no such run".to_string())?;
    if run.state == "done" {
        return Err("that run is done already".into());
    }
    run.state = "done".into();
    run.ended_ms = Some(now);
    run.outcome = Some(outcome.to_string());
    let done = run.clone();
    schedule(&mut p.runs, &nodes, now);
    Ok(done)
}

/// The lab's nodes as the Lab view lists them.
#[derive(Debug, Clone, Serialize)]
pub struct LabNodeView {
    pub id: String,
    pub name: String,
    pub label: String,
    pub test_share: u32,
    pub work_share: u32,
    pub online: bool,
    pub cpus: u64,
    pub memory_mib: u64,
    pub lab_cpus: u64,
    pub lab_memory_mib: u64,
    pub free_cpus: u64,
    pub free_memory_mib: u64,
    pub capabilities: Vec<String>,
}

/// `GET /v1/test-lab`: every node with a role, the queue, the running runs and the last runs that ended.
#[derive(Debug, Clone, Serialize)]
pub struct Overview {
    pub nodes: Vec<LabNodeView>,
    /// The queued runs, in the order they will be admitted.
    pub queue: Vec<LabRun>,
    pub running: Vec<LabRun>,
    pub done: Vec<LabRun>,
}

/// The overview (pure over the nodes' views, the lab's capacity and the runs).
pub fn overview(nodes: &[crate::nodes::NodeView], lab: &[LabNode], runs: &[LabRun]) -> Overview {
    let slots = free_slots(lab, runs);
    let node_views = nodes
        .iter()
        .map(|view| {
            let node = &view.node;
            let lab_node = lab.iter().find(|l| l.id == node.id);
            let slot = slots.iter().find(|s| s.node == node.id);
            LabNodeView {
                id: node.id.clone(),
                name: node.name.clone(),
                label: node.label.clone(),
                test_share: node.test_share,
                work_share: view.work_share,
                online: view.online,
                cpus: node.machine.cpus.unwrap_or(0),
                memory_mib: node.machine.memory_mib.unwrap_or(0),
                lab_cpus: lab_node.map_or(0, |l| l.cpus),
                lab_memory_mib: lab_node.map_or(0, |l| l.memory_mib),
                free_cpus: slot.map_or(0, |s| s.cpus),
                free_memory_mib: slot.map_or(0, |s| s.memory_mib),
                capabilities: lab_node
                    .map(|l| l.capabilities.iter().cloned().collect())
                    .unwrap_or_default(),
            }
        })
        .collect();
    let mut queued: Vec<&LabRun> = runs.iter().filter(|r| r.state == "queued").collect();
    queued.sort_by_key(|run| (rank(&run.channel).unwrap_or(u8::MAX), seq_of(&run.id)));
    let mut done: Vec<LabRun> = runs.iter().filter(|r| r.state == "done").cloned().collect();
    done.sort_by_key(|run| std::cmp::Reverse(run.ended_ms.unwrap_or(0)));
    done.truncate(RECENT_DONE);
    Overview {
        nodes: node_views,
        queue: queued.iter().map(|run| (*run).clone()).collect(),
        running: runs
            .iter()
            .filter(|r| r.state == "running")
            .cloned()
            .collect(),
        done,
    }
}

/// A run as `GET /v1/test-lab/runs/{id}` shows it: the run and its 1-based place in the queue (null once it runs).
#[derive(Debug, Clone, Serialize)]
pub struct RunView {
    #[serde(flatten)]
    pub run: LabRun,
    pub place: Option<usize>,
}

/// The run `id` with its place in the queue, from the runs (pure).
pub fn run_view(runs: &[LabRun], id: &str) -> Option<RunView> {
    let run = runs.iter().find(|run| run.id == id)?.clone();
    let mut queued: Vec<&LabRun> = runs.iter().filter(|r| r.state == "queued").collect();
    queued.sort_by_key(|run| (rank(&run.channel).unwrap_or(u8::MAX), seq_of(&run.id)));
    let place = queued
        .iter()
        .position(|queued| queued.id == id)
        .map(|at| at + 1);
    Some(RunView { run, place })
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/test-lab", |app, _, _| {
            let nodes = crate::nodes::views(app);
            Response::json(200, &overview(&nodes, &lab_nodes(app), &app.test_lab.runs()))
        })
        .doc(RouteDoc::new(
            "The test lab: its nodes and their capacity, the queue, the running runs and the last runs that ended",
        ))
        .post("/v1/test-lab/runs", enqueue_route)
        .doc(RouteDoc::new("Queue a qualification run in the lab").status(201).errors(&[400]))
        .get("/v1/test-lab/runs/{id}", run_route)
        .doc(RouteDoc::new("A qualification run in the lab, with its reservations and its place in the queue").errors(&[404]))
        .post("/v1/test-lab/runs/{id}/cancel", cancel_route)
        .doc(RouteDoc::new("Cancel a queued or running run").errors(&[404, 409]))
}

/// The body of `POST /v1/test-lab/runs`.
#[derive(Debug, Deserialize)]
struct EnqueueBody {
    project: String,
    channel: String,
    cpus: u32,
    memory_mib: u64,
    #[serde(default)]
    needs: Vec<String>,
    #[serde(default)]
    change: String,
    #[serde(default)]
    topology: String,
    #[serde(default)]
    release: String,
}

fn enqueue_route(app: &App, req: &Request, _p: &Params<'_>) -> Response {
    let body: EnqueueBody = match crate::nodes::parse(req) {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    match enqueue(
        app,
        &body.project,
        &body.channel,
        body.cpus,
        body.memory_mib,
        body.needs,
        (&body.change, &body.topology),
        &body.release,
    ) {
        Ok((run, token)) => {
            // The run's collector posts with this token (`X-Apz-Lab-Token`); it is not given again.
            let mut view = serde_json::to_value(&run).unwrap_or_default();
            view["ingest_token"] = serde_json::Value::String(token);
            Response::json(201, &view)
        }
        Err(message) => Response::error(400, &message),
    }
}

fn run_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    match run_view(&app.test_lab.runs(), p.get("id")) {
        Some(view) => Response::json(200, &view),
        None => Response::error(404, "no such run"),
    }
}

fn cancel_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    if app.test_lab.runs().iter().all(|run| run.id != id) {
        return Response::error(404, "no such run");
    }
    match finish(app, id, "cancelled") {
        Ok(run) => Response::json(200, &run),
        Err(message) => Response::error(409, &message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn node(id: &str, cpus: u64, memory_mib: u64, list: &[&str]) -> LabNode {
        LabNode {
            id: id.into(),
            cpus,
            memory_mib,
            capabilities: caps(list),
        }
    }

    fn run(id: &str, channel: &str, cpus: u32, memory_mib: u64, state: &str) -> LabRun {
        LabRun {
            id: id.into(),
            project: "p".into(),
            channel: channel.into(),
            state: state.into(),
            cpus,
            memory_mib,
            needs: Vec::new(),
            reservations: Vec::new(),
            enqueued_ms: 0,
            started_ms: None,
            ended_ms: None,
            outcome: None,
            change: String::new(),
            topology: String::new(),
            release: String::new(),
            token_sha: String::new(),
            observed: 0,
            dropped: 0,
        }
    }

    #[test]
    fn plan_covers_a_run_from_several_nodes_with_the_capability() {
        let nodes = [
            node("n1", 4, 8192, &["arch:x86_64"]),
            node("n2", 2, 2048, &["arch:x86_64", "browser:chromium"]),
            node("n3", 8, 16384, &["arch:aarch64"]),
        ];
        let slots = free_slots(&nodes, &[]);
        let needs = vec!["arch:x86_64".to_string()];
        let reserved = plan(&slots, 5, 6000, &needs).expect("covered by n1 and n2");
        assert_eq!(
            reserved.iter().map(|r| r.node.as_str()).collect::<Vec<_>>(),
            ["n1", "n2"]
        );
        assert_eq!(reserved.iter().map(|r| u64::from(r.cpus)).sum::<u64>(), 5);
        assert_eq!(reserved.iter().map(|r| r.memory_mib).sum::<u64>(), 6000);
        // The aarch64 node has room but not the capability: a run for chromium needs n2.
        let chromium = vec!["browser:chromium".to_string()];
        assert!(plan(&slots, 1, 512, &chromium).is_some_and(|r| r.iter().all(|x| x.node == "n2")));
        assert!(plan(&slots, 7, 1024, &["browser:firefox".to_string()]).is_none());
        assert!(
            plan(&slots, 20, 1, &needs).is_none(),
            "more CPUs than the lab has"
        );
    }

    #[test]
    fn a_run_queues_behind_a_higher_priority_run_until_capacity_frees() {
        let nodes = [node("n1", 4, 8192, &["os:linux"])];
        let mut stable = run("q1", "stable", 4, 8192, "running");
        stable.reservations = vec![Reservation {
            node: "n1".into(),
            cpus: 4,
            memory_mib: 8192,
        }];
        let mut runs = vec![
            stable,
            run("q2", "nightly", 4, 8192, "queued"),
            run("q3", "yank", 4, 8192, "queued"),
        ];
        // The stable run holds the whole node: the nightly and the yank wait, though the yank outranks everything.
        assert!(schedule(&mut runs, &nodes, 10).is_empty());
        assert_eq!(runs[1].state, "queued");
        assert_eq!(runs[2].state, "queued");
        // It ends: the yank (rank 0) is admitted before the nightly that was queued earlier.
        runs[0].state = "done".into();
        runs[0].outcome = Some("passed".into());
        assert_eq!(schedule(&mut runs, &nodes, 20), vec!["q3".to_string()]);
        assert_eq!(runs[1].state, "queued");
        assert_eq!(runs[2].state, "running");
        assert_eq!(
            runs[2].reservations,
            vec![Reservation {
                node: "n1".into(),
                cpus: 4,
                memory_mib: 8192
            }]
        );
    }

    #[test]
    fn the_queue_is_strict_a_run_that_does_not_fit_holds_back_the_rest() {
        let nodes = [node("n1", 4, 8192, &[])];
        let mut runs = vec![
            run("q1", "beta", 8, 1024, "queued"),
            run("q2", "beta", 1, 512, "queued"),
        ];
        assert!(schedule(&mut runs, &nodes, 1).is_empty());
        assert!(runs.iter().all(|r| r.state == "queued"));
    }

    #[test]
    fn a_channel_orders_by_rank_then_arrival() {
        assert_eq!(rank("yank"), rank("hotfix"));
        assert!(rank("hotfix") < rank("stable"));
        assert!(rank("stable") < rank("beta"));
        assert!(rank("beta") < rank("nightly"));
        assert_eq!(rank("weekly"), None);
    }

    #[test]
    fn the_run_view_gives_its_place_in_the_queue() {
        let runs = vec![
            run("q1", "stable", 4, 8192, "running"),
            run("q2", "nightly", 1, 512, "queued"),
            run("q3", "yank", 1, 512, "queued"),
        ];
        assert_eq!(run_view(&runs, "q3").and_then(|v| v.place), Some(1));
        assert_eq!(run_view(&runs, "q2").and_then(|v| v.place), Some(2));
        assert_eq!(run_view(&runs, "q1").and_then(|v| v.place), None);
        assert!(run_view(&runs, "q9").is_none());
    }

    #[test]
    fn capabilities_read_the_machine() {
        let machine = Machine {
            arch: Some("aarch64".into()),
            os: Some("linux".into()),
            browsers: vec!["chromium".into()],
            phones: vec!["R58N1".into()],
            traffic_shaping: Some(true),
            ..Machine::default()
        };
        assert_eq!(
            capabilities_of(&machine),
            caps(&[
                "arch:aarch64",
                "os:linux",
                "browser:chromium",
                "phone",
                "traffic-shaping"
            ])
        );
        assert_eq!(share_of(8192, 25), 2048);
        assert_eq!(share_of(8192, 250), 8192);
    }
}
