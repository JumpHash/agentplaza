//! Where agents run: the node a new agent is placed on, and the move of an agent to another node. A
//! stopped node (`mover`) takes neither: its agents are moved off by the mover, one at a time.
//!
//! A move carries the agent's VM between two noded. The VM is put to sleep (`sleep::put_to_sleep`),
//! its files are exported on the source (`POST /v1/runs/{run}/vms/{vm}/export`), copied in
//! [`CHUNK`]-byte pieces to an import on the target, and finished there into a sleeping run. Then the
//! placement is recorded (`nodes::Store::set_override`, saved with the node store), the target's
//! gateway serves the CID, the agent wakes on the target, and the source lets go of it (its gateway
//! and its run). Any failure before the placement is recorded aborts the import and wakes the agent
//! where it was. Work queued for the agent waits while it moves (`scheduler`'s dispatch holds a moving
//! agent) and goes out once it is awake on the target; work in flight refuses the move.
//!
//! Routes (owner):
//!   POST /v1/agents/{cid}/move  {node} → {ok, from, to, bytes, ms}

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use apz_http::{Body, Params, Request, Response, RouteDoc, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::agents::Status;
use crate::base_recipe::BaseKind;
use crate::fleet::{self, LOCAL};
use crate::nodes::{MachineMac, Node, OFFLINE_AFTER_MS, Pool};
use crate::project_stack;
use crate::sleep;
use crate::state::App;

/// A node needs at least this much free memory (less what is reserved) to take a new agent: VMs boot
/// with a small usable memory and grow through their balloon.
pub const ADMIT_MIB: u64 = 256;
/// Memory set aside on the chosen node for a new agent until its VM shows up in the node's measured
/// use.
pub const RESERVE_MIB: u64 = 256;
/// A warm template for the role saves a cold boot: it counts as this much free memory when placing.
pub const WARM_BONUS_MIB: i64 = 1024;
/// A spreading weight per VM a node already runs, in MiB of score: the measured used memory already
/// counts the VMs themselves, so this only nudges new agents toward the emptier nodes.
pub const VM_PENALTY_MIB: i64 = 64;
/// A spreading weight per idle CPU, in MiB of score: a node with more CPUs free takes the agent over a busier one.
pub const CPU_MIB: i64 = 64;
/// Free space a node needs to take a fork (16.9: a fork owns its own written blocks, typically 1 to 5 GB). A node
/// that did not say how much it has free is not kept out.
pub const FORK_DISK_GIB: u64 = 5;
/// How long a move waits for the agent to be back after its wake.
const WAKE_WAIT: Duration = Duration::from_secs(60);
/// How long a reservation holds its memory back on a node (see [`reserved_mib`]).
const RESERVE_FOR: Duration = Duration::from_secs(60);
/// How long a node's template list is reused before it is asked again (see [`templates_of`]).
const TEMPLATES_FRESH: Duration = Duration::from_secs(30);

/// Recent placements: the node each new agent went to, and when.
static RESERVED: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());
/// The local node's pool as it last answered (`pool_of`): a session's end reads it without asking noded again.
static LOCAL_POOL: Mutex<Option<Pool>> = Mutex::new(None);
/// Each node's template list, and when it was fetched.
static TEMPLATES: Mutex<BTreeMap<String, (Instant, Value)>> = Mutex::new(BTreeMap::new());

/// One node's standing for a placement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub node: String,
    /// Memory not in use now, in MiB.
    pub free_mib: u64,
    /// VMs running on it now.
    pub running_vms: u64,
    /// It holds a warm template for the role.
    pub warm: bool,
    /// Its pool (see `nodes::Pool`); None when it reports no CPU baseline.
    pub pool: Option<Pool>,
    /// It holds the project's base (see [`base_key`]) that a fork of the project starts from; false for a
    /// placement with no project.
    pub base: bool,
    /// Free space on its state filesystem, in GiB; None when it did not say.
    pub disk_free_gib: Option<u64>,
    /// CPUs not in use now (its CPU count less its load); None when it did not say.
    pub idle_cpus: Option<u64>,
}

/// A candidate's score; the higher wins. Free memory, less the VMs it runs, plus the warm bonus and the idle CPUs.
pub fn score(candidate: &Candidate) -> i64 {
    let free = i64::try_from(candidate.free_mib).unwrap_or(i64::MAX);
    let vms = i64::try_from(candidate.running_vms).unwrap_or(i64::MAX);
    let bonus = if candidate.warm { WARM_BONUS_MIB } else { 0 };
    let idle = candidate
        .idle_cpus
        .map_or(0, |cpus| i64::try_from(cpus).unwrap_or(i64::MAX));
    free - vms.saturating_mul(VM_PENALTY_MIB) + bonus + idle.saturating_mul(CPU_MIB)
}

/// Whether a candidate can take an agent (pure): at least [`ADMIT_MIB`] free, and [`FORK_DISK_GIB`] free on its
/// disk when it says how much it has.
fn admitted(candidate: &Candidate) -> bool {
    candidate.free_mib >= ADMIT_MIB
        && candidate
            .disk_free_gib
            .is_none_or(|free| free >= FORK_DISK_GIB)
}

/// The node a new agent goes to, from the candidates (pure). Only an [`admitted`] node is eligible. The
/// node holding the project's base wins; then the highest score; a tie goes to `n0`, then to the earlier
/// candidate. With no eligible candidate the agent goes to `n0`, where noded says whether it fits.
pub fn choose(candidates: &[Candidate]) -> String {
    pick(candidates).unwrap_or_else(|| LOCAL.to_string())
}

/// The best of the eligible candidates as [`choose`] ranks them (pure); None when no candidate is [`admitted`].
pub fn pick(candidates: &[Candidate]) -> Option<String> {
    let mut best: Option<((bool, i64), &Candidate)> = None;
    for candidate in candidates.iter().filter(|candidate| admitted(candidate)) {
        let rank = (candidate.base, score(candidate));
        let better = match best {
            None => true,
            Some((top, held)) => {
                rank > top || (rank == top && candidate.node == LOCAL && held.node != LOCAL)
            }
        };
        if better {
            best = Some((rank, candidate));
        }
    }
    best.map(|(_, candidate)| candidate.node.clone())
}

/// The node a base build of `project` runs its VM on, in `pool` (`any` for a project that declares none), with the
/// `needs` volumes (base_recipe.rs). The node that holds the project's base is preferred, as a fork's is (3.5). The
/// node's room is reserved, as a new agent's is.
pub fn base_node(app: &App, project: &str, pool: &str, needs: &[&str]) -> Option<String> {
    let declared = declared_pools(app, project);
    let needs: Vec<String> = needs.iter().map(|volume| volume.to_string()).collect();
    let candidates: Vec<Candidate> = candidates(app, "", &needs, Some(project))
        .into_iter()
        .filter(|candidate| {
            pool == "any"
                || candidate
                    .pool
                    .as_ref()
                    .is_some_and(|p| format!("{}-{}", p.arch, p.hypervisor) == pool)
        })
        .collect();
    let chosen = fork_node(candidates, &declared)?;
    reserve(&chosen);
    Some(chosen)
}

/// The key of the base a project's forks start from (16.1): the project's `build` base. A node that holds it
/// reports it among its `bases` (17.1).
pub fn base_key(project: &str) -> String {
    format!("{project}/{}", BaseKind::Build.name())
}

/// The candidates on the declared pools (17.2) of a project (pure): every candidate when none is declared,
/// otherwise those whose pool is one of them.
pub fn in_pools(candidates: Vec<Candidate>, declared: &[project_stack::Pool]) -> Vec<Candidate> {
    if declared.is_empty() {
        return candidates;
    }
    candidates
        .into_iter()
        .filter(|candidate| {
            candidate.pool.as_ref().is_some_and(|pool| {
                declared.iter().any(|declared| {
                    declared.arch.name() == pool.arch
                        && declared.hypervisor.name() == pool.hypervisor
                })
            })
        })
        .collect()
}

/// The idle CPUs of a node with `cpus` CPUs at `cpu_pct` load (pure); None when either is unknown.
pub fn idle_cpus(cpus: Option<u64>, cpu_pct: Option<f64>) -> Option<u64> {
    let (cpus, pct) = (cpus?, cpu_pct?);
    let idle = cpus as f64 * (100.0 - pct.clamp(0.0, 100.0)) / 100.0;
    Some(idle.floor() as u64)
}

/// The node a new agent of `role` goes to: among the nodes that report and are in service (`n0`
/// included), the best by [`choose`], with the memory reserved on each node (see [`reserved_mib`])
/// taken off its free memory. The chosen node is then reserved for the new agent. A node whose
/// machine stats did not answer is left out. For a session's fork (`project`) the candidates are its
/// declared pools' (see [`in_pools`]) and the node holding its base is preferred; a project that declares
/// pools gets no node outside them, so None when none of them has room.
pub fn choose_node(app: &App, role: &str, project: Option<&str>) -> Option<String> {
    let needs = app
        .settings
        .get()
        .role_volumes
        .get(role)
        .cloned()
        .unwrap_or_default();
    let declared = project
        .map(|project| declared_pools(app, project))
        .unwrap_or_default();
    let chosen = fork_node(candidates(app, role, &needs, project), &declared)?;
    reserve(&chosen);
    Some(chosen)
}

/// Whether a fork of `project` has a node to start on now (see [`fork_node`]), reserving nothing: a project that
/// declares no pool always has one (n0 takes it, as it does any agent).
pub fn has_room_for_fork(app: &App, role: &str, project: &str) -> bool {
    let declared = declared_pools(app, project);
    if declared.is_empty() {
        return true;
    }
    let needs = app
        .settings
        .get()
        .role_volumes
        .get(role)
        .cloned()
        .unwrap_or_default();
    fork_node(candidates(app, role, &needs, Some(project)), &declared).is_some()
}

/// The node a fork goes to among the candidates (pure): by [`choose`] when the project declares no pool; otherwise the
/// best candidate on a declared pool, and None when none of them has room.
pub fn fork_node(candidates: Vec<Candidate>, declared: &[project_stack::Pool]) -> Option<String> {
    if declared.is_empty() {
        return Some(choose(&candidates));
    }
    pick(&in_pools(candidates, declared))
}

/// Whether every node that reports is full for a new agent of `role`: no candidate has [`ADMIT_MIB`] free (see
/// [`pick`]). The work loop waits to start a task while this holds (PLAN 7.1). False when no node reports, since
/// [`choose`] then falls back to n0 and noded decides, as it always has.
pub fn fleet_full(app: &App, role: &str) -> bool {
    full_of(&candidates(app, role, &[], None))
}

/// [`fleet_full`] over the candidates (pure).
fn full_of(candidates: &[Candidate]) -> bool {
    !candidates.is_empty() && pick(candidates).is_none()
}

/// The node an agent of `role` leaving `from` moves to: the best of the other nodes in service in `from`'s
/// pool by [`pick`] that hold the `needs` volumes (see [`lacking_volumes`]), reserved for it as a new agent's
/// would be. None when none in the pool has room.
pub fn target_for(app: &App, role: &str, from: &str, needs: &[String]) -> Option<String> {
    let others = in_pool(
        candidates(app, role, needs, None),
        from,
        pool_of(app, from).as_ref(),
    );
    let chosen = pick(&others)?;
    reserve(&chosen);
    Some(chosen)
}

/// The data volumes `held` lacks of `needs` (pure), in the order `needs` lists them.
pub fn lacking_volumes(held: &[String], needs: &[String]) -> Vec<String> {
    needs
        .iter()
        .filter(|name| !held.contains(name))
        .cloned()
        .collect()
}

/// The names of the data volumes node `node` holds (its `GET /v1/volumes`); an error when it did not
/// answer.
fn volumes_on(app: &App, node: &str) -> Result<Vec<String>, String> {
    let body = answered(
        &format!("{node} GET /v1/volumes"),
        fleet::noded(app, node, "GET", "/v1/volumes", None),
    )?;
    Ok(body
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|volume| volume["name"].as_str().map(str::to_string))
        .collect())
}

/// The data volumes agent `cid`'s run carries on `node`, as the run there names them.
pub fn agent_volumes(app: &App, cid: u64, node: &str) -> Result<Vec<String>, String> {
    let (run, _) = crate::lifecycle::place(app, cid).map_err(|resp| body_text(&resp))?;
    run_volumes(app, node, &run)
}

/// The data volumes run `run` carries on `node`, from its entry in the node's run list.
fn run_volumes(app: &App, node: &str, run: &str) -> Result<Vec<String>, String> {
    let body = answered(
        &format!("{node} GET /v1/runs"),
        fleet::noded(app, node, "GET", "/v1/runs", None),
    )?;
    let entry = body["runs"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry["id"].as_str() == Some(run))
        .ok_or_else(|| format!("{node} has no run {run}"))?;
    Ok(entry["volumes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|name| name.as_str().map(str::to_string))
        .collect())
}

/// The nodes that report and are in service, each as a candidate for an agent of `role` that needs the
/// `needs` volumes: a node that does not hold them all, or that did not say which it holds, is left out. A test-only
/// node (its test-lab share is the whole of it, see `Node::test_share`) takes no agent. On a node that does both, only
/// its work share of the memory counts. With a `project`, each candidate says whether it holds the project's base.
fn candidates(app: &App, role: &str, needs: &[String], project: Option<&str>) -> Vec<Candidate> {
    let stopped = app.nodes.stopped_ids();
    let now = Instant::now();
    let base = project.map(base_key);
    fleet::reports(app)
        .into_iter()
        .filter(|(node, _, _)| !stopped.contains(node))
        .filter(|(node, _, _)| app.nodes.test_share(node) < 100)
        .filter(|(node, _, _)| {
            needs.is_empty()
                || volumes_on(app, node).is_ok_and(|held| lacking_volumes(&held, needs).is_empty())
        })
        .filter_map(|(node, _status, runs)| {
            let (total, used) = memory_of(app, &node)?;
            let work = work_share_of(total, app.nodes.test_share(&node));
            let facts = facts_of(app, &node);
            Some(Candidate {
                free_mib: work
                    .saturating_sub(used)
                    .saturating_sub(reserved_mib(&node, now)),
                running_vms: running_vms(&runs),
                warm: warm_for(&templates_of(app, &node), role),
                pool: pool_of(app, &node),
                base: base.as_ref().is_some_and(|key| facts.bases.contains(key)),
                disk_free_gib: facts.storage_free_gib,
                idle_cpus: facts.idle_cpus,
                node,
            })
        })
        .collect()
}

/// The memory of a node's `total_mib` that agents may use, given its test-lab share in percent (pure): the rest is the
/// test lab's (`test_lab`).
pub fn work_share_of(total_mib: u64, test_share: u32) -> u64 {
    let work = 100u64.saturating_sub(u64::from(test_share.min(100)));
    total_mib.saturating_mul(work) / 100
}

/// The nodes that could build a project's targets (targets.rs): every candidate in service, with its pool and whether it
/// holds the project's base (the Android SDK lives there).
pub(crate) fn build_candidates(app: &App, project: &str) -> Vec<Candidate> {
    candidates(app, "", &[], Some(project))
}

/// The pools a project declares in its `.plaza/project.toml` (17.2): none when it declares none, or when its
/// declaration does not read yet (a project with no repo, or Weft out of reach).
fn declared_pools(app: &App, project: &str) -> Vec<project_stack::Pool> {
    project_stack::read(app, project)
        .ok()
        .flatten()
        .map(|stack| stack.pools)
        .unwrap_or_default()
}

/// A node's placement facts beyond its memory (pure data): the base keys it holds, its free disk in GiB and
/// its idle CPUs.
#[derive(Debug, Default, PartialEq, Eq)]
struct Facts {
    bases: Vec<String>,
    storage_free_gib: Option<u64>,
    idle_cpus: Option<u64>,
}

/// A node's facts (17.1): what it reported (see [`reported_facts_of`]), and the bases the builds on this hub held on it
/// (base_vm.rs), which a node's own report does not list.
fn facts_of(app: &App, node: &str) -> Facts {
    let mut facts = reported_facts_of(app, node);
    for key in crate::base_vm::held_on(node) {
        if !facts.bases.contains(&key) {
            facts.bases.push(key);
        }
    }
    facts
}

/// A node's facts as it reported them (17.1): `n0`'s from noded's machine view and stats asked now, a joined node's from
/// its last heartbeat. Empty when they did not answer, so the node has no base and an unknown disk.
fn reported_facts_of(app: &App, node: &str) -> Facts {
    if node == LOCAL {
        let machine = fleet::noded(app, LOCAL, "GET", "/v1/machine", None);
        let stats = fleet::noded(app, LOCAL, "GET", "/v1/machine/stats", None);
        let (Ok((200, machine)), Ok((200, stats))) = (machine, stats) else {
            return Facts::default();
        };
        return Facts {
            bases: machine["bases"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|key| key.as_str().map(str::to_string))
                .collect(),
            storage_free_gib: machine["storage_free_gib"].as_u64(),
            idle_cpus: idle_cpus(machine["cpus"].as_u64(), stats["cpu_pct"].as_f64()),
        };
    }
    let Some(record) = app.nodes.get(node) else {
        return Facts::default();
    };
    Facts {
        bases: record.machine.bases.clone(),
        storage_free_gib: record.machine.storage_free_gib,
        idle_cpus: idle_cpus(
            record.machine.cpus,
            record
                .machine
                .stats
                .as_ref()
                .and_then(|stats| stats.cpu_pct),
        ),
    }
}

/// The pool of node `node` (see `nodes::Pool`): `n0`'s from noded's machine view, a joined node's from its
/// last heartbeat. None when the node reports no CPU baseline, or does not answer.
pub(crate) fn pool_of(app: &App, node: &str) -> Option<Pool> {
    if node == LOCAL {
        let Ok((200, machine)) = fleet::noded(app, LOCAL, "GET", "/v1/machine", None) else {
            return None;
        };
        let pool = Pool::of(
            machine["arch"].as_str(),
            machine["backend"].as_str(),
            machine["cpu_baseline"].as_str(),
        );
        *LOCAL_POOL.lock().unwrap_or_else(PoisonError::into_inner) = pool.clone();
        return pool;
    }
    app.nodes.get(node)?.machine.pool()
}

/// The pool of node `node` as last known, asking no node: a joined node's from its heartbeat, the local node's from the
/// last answer `pool_of` had. A session's end records its node's pool this way.
pub(crate) fn known_pool(app: &App, node: &str) -> Option<Pool> {
    if node == LOCAL {
        return LOCAL_POOL
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
    }
    app.nodes.get(node)?.machine.pool()
}

/// The candidates an agent may move to from a node of pool `from_pool` (pure): the others of that pool. A node
/// with no pool takes no move, and no node is in its pool.
pub fn in_pool(candidates: Vec<Candidate>, from: &str, from_pool: Option<&Pool>) -> Vec<Candidate> {
    let Some(pool) = from_pool else {
        return Vec::new();
    };
    candidates
        .into_iter()
        .filter(|candidate| candidate.node != from && candidate.pool.as_ref() == Some(pool))
        .collect()
}

/// Why a move of an agent from a node of pool `from_pool` to `target` (of pool `target_pool`) is refused (pure):
/// a VM moves only within its pool (plan 17.2), and a node with no pool is in none.
pub fn pool_refusal(
    target: &str,
    from_pool: Option<&Pool>,
    target_pool: Option<&Pool>,
) -> Option<(u16, String)> {
    match (from_pool, target_pool) {
        (Some(from), Some(to)) if from == to => None,
        _ => Some((
            409,
            format!("node {target} is not in the agent's pool: a VM moves only within its pool"),
        )),
    }
}

/// Sets [`RESERVE_MIB`] aside on `node` for an agent placed or moved there.
fn reserve(node: &str) {
    RESERVED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((node.to_string(), Instant::now()));
}

/// Memory reserved on `node` at `now`: [`RESERVE_MIB`] for each agent placed there less than
/// [`RESERVE_FOR`] ago. Older reservations are dropped.
pub(crate) fn reserved_mib(node: &str, now: Instant) -> u64 {
    let mut reserved = RESERVED.lock().unwrap_or_else(PoisonError::into_inner);
    reserved.retain(|(_, when)| now.saturating_duration_since(*when) <= RESERVE_FOR);
    let held = reserved.iter().filter(|(held, _)| held == node).count();
    RESERVE_MIB * u64::try_from(held).unwrap_or(u64::MAX)
}

/// A node's memory as (total, used) in MiB: `n0`'s stats asked now, a joined node's from its last
/// heartbeat. None when the stats did not answer.
pub(crate) fn memory_of(app: &App, node: &str) -> Option<(u64, u64)> {
    if node == LOCAL {
        let Ok((200, stats)) = fleet::noded(app, LOCAL, "GET", "/v1/machine/stats", None) else {
            return None;
        };
        let used = stats["mem_used_mib"].as_u64()?;
        let total = stats["mem_total_mib"].as_u64()?;
        return Some((total, used));
    }
    // A joined node's heartbeat carries its live memory in use; its total is the machine's own memory.
    let machine = app.nodes.get(node)?.machine;
    let used = machine.stats?.mem_used_mib?;
    let total = machine.memory_mib?;
    Some((total, used))
}

/// The VMs running in a node's runs (its `GET /v1/runs`): every VM of a run that has not ended.
fn running_vms(runs: &Value) -> u64 {
    runs["runs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|run| run["ended_at"].is_null())
        .map(|run| u64::try_from(run["vms"].as_array().map_or(0, Vec::len)).unwrap_or(u64::MAX))
        .sum()
}

/// A node's template list (its `GET /v1/templates`); Null when it did not answer. A list fetched
/// less than [`TEMPLATES_FRESH`] ago is reused; only a successful answer is kept.
fn templates_of(app: &App, node: &str) -> Value {
    let fresh = TEMPLATES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(node)
        .filter(|(at, _)| at.elapsed() < TEMPLATES_FRESH)
        .map(|(_, value)| value.clone());
    if let Some(value) = fresh {
        return value;
    }
    match fleet::noded(app, node, "GET", "/v1/templates", None) {
        Ok((200, body)) => {
            TEMPLATES
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(node.to_string(), (Instant::now(), body.clone()));
            body
        }
        _ => Value::Null,
    }
}

/// Whether a template list (an array, or `{templates: [...]}`) holds one for `role`.
fn warm_for(templates: &Value, role: &str) -> bool {
    let list = templates
        .as_array()
        .or_else(|| templates["templates"].as_array());
    list.is_some_and(|items| items.iter().any(|item| item["role"].as_str() == Some(role)))
}

/// Why a move to `target` is refused (pure): the agent is already on it, the node is stopped
/// (`stopped`, `n0` included), or it is unknown, revoked or offline. `n0` is always there.
pub fn target_refusal(
    from: &str,
    target: &str,
    record: Option<&Node>,
    stopped: bool,
    now_ms: u64,
) -> Option<(u16, String)> {
    if target == from {
        return Some((409, format!("agent is already on {target}")));
    }
    if stopped {
        return Some((409, format!("node {target} is stopped")));
    }
    if target == LOCAL {
        return None;
    }
    let Some(node) = record else {
        return Some((404, format!("no node {target}")));
    };
    if node.revoked {
        return Some((409, format!("node {target} is revoked")));
    }
    if now_ms.saturating_sub(node.last_seen_ms) >= OFFLINE_AFTER_MS {
        return Some((409, format!("node {target} is offline")));
    }
    None
}

/// Why an agent in `status` cannot move now (pure): a working agent moves when it is idle, and an
/// offline one has no running VM to carry.
pub fn status_refusal(cid: u64, status: Status) -> Option<(u16, String)> {
    match status {
        Status::Working => Some((
            409,
            format!("agent {cid} is working; move it when it is idle"),
        )),
        Status::Offline => Some((
            409,
            format!("agent {cid} is offline; it has no running VM to move"),
        )),
        // A dozing agent moves like an idle one: the move's first step is `sleep::put_to_sleep`, which
        // noded takes from a paused VM too (`take_vm` accepts Running or Paused).
        Status::Idle | Status::Asleep | Status::Dozing => None,
    }
}

/// The agents whose move is in flight. A move holds its agent here, so a second move of it is refused.
static MOVING: Mutex<BTreeSet<u64>> = Mutex::new(BTreeSet::new());

/// Whether a move of agent `cid` is in flight.
pub fn is_moving(cid: u64) -> bool {
    MOVING.lock().is_ok_and(|set| set.contains(&cid))
}

/// Holds the agent's move slot; dropping it frees the slot whatever the move's outcome.
struct Claim(u64);

impl Claim {
    fn take(cid: u64) -> Option<Claim> {
        let mut set = MOVING.lock().unwrap_or_else(PoisonError::into_inner);
        // Lazily: an eager `then_some(Claim(cid))` would build a claim even when the slot is taken,
        // and dropping it here would lock MOVING again (and free the other move's slot).
        set.insert(cid).then(|| Claim(cid))
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        MOVING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.0);
    }
}

/// What a move did: where the agent came from and went to, the bytes carried, and how long it took.
#[derive(Debug)]
pub struct Moved {
    pub from: String,
    pub to: String,
    pub bytes: u64,
    pub ms: u64,
}

/// The record of what a move has taken so far, so an abort can undo exactly that.
#[derive(Default)]
struct Staged {
    /// The export on the source; deleted when the move ends, whatever happened.
    export: Option<String>,
    /// The import on the target, until it finishes.
    import: Option<String>,
    /// The sleeping run the finished import made on the target.
    target_run: Option<String>,
    /// The placement before this move, once this move recorded its own.
    overridden: Option<Option<String>>,
    /// The target's gateway was asked to serve the CID (it may be serving it).
    served: bool,
}

/// Moves agent `cid` to node `target` (see the module doc). The agent is asleep when the copy starts,
/// and awake on the target when this returns Ok. Errors are the response for the caller. Either way
/// the agent's queued work goes out afterwards, where the agent now is.
pub fn move_agent(app: &App, cid: u64, target: &str) -> Result<Moved, Response> {
    let moved = move_claimed(app, cid, target);
    app.work.dispatch_idle(app);
    moved
}

/// [`move_agent`]'s move, with its claim on the agent held until it returns.
fn move_claimed(app: &App, cid: u64, target: &str) -> Result<Moved, Response> {
    let started = Instant::now();
    if app.agents.get(cid).is_none() {
        return Err(Response::error(404, &format!("no agent {cid}")));
    }
    let from = fleet::node_of(app, cid);
    let record = app.nodes.get(target);
    if let Some((status, message)) = target_refusal(
        &from,
        target,
        record.as_ref(),
        app.nodes.is_stopped(target),
        apz_plaza::now_ms(),
    ) {
        return Err(Response::error(status, &message));
    }
    // The claim comes first: from here nothing is sent to the agent and no wake starts (the dispatch and
    // `sleep::wake_with` hold a moving agent), so the checks below stay true while its files are copied.
    let Some(claim) = Claim::take(cid) else {
        return Err(Response::error(
            409,
            &format!("agent {cid} is already moving"),
        ));
    };
    let Some(agent) = app.agents.get(cid) else {
        return Err(Response::error(404, &format!("no agent {cid}")));
    };
    if let Some((status, message)) = status_refusal(cid, agent.status) {
        return Err(Response::error(status, &message));
    }
    let (from_pool, target_pool) = (pool_of(app, &from), pool_of(app, target));
    if let Some((status, message)) = pool_refusal(target, from_pool.as_ref(), target_pool.as_ref())
    {
        return Err(Response::error(status, &message));
    }
    if app.work.current(cid).is_some() {
        return Err(Response::error(
            409,
            &format!("agent {cid} has work in flight; move it when it is idle"),
        ));
    }
    if sleep::in_transition(cid) {
        return Err(Response::error(
            409,
            &format!("agent {cid} is going to sleep or waking up; try again"),
        ));
    }
    let (run, vm) = crate::lifecycle::place(app, cid)?;
    // The target must hold every data volume the VM carries: its snapshot names their devices, and a
    // restore needs them (noded refuses an import that lacks one, so this only says so first).
    let needs = run_volumes(app, &from, &run)
        .map_err(|why| Response::error(502, &format!("cannot read the VM's volumes: {why}")))?;
    let held = volumes_on(app, target)
        .map_err(|why| Response::error(502, &format!("move to {target} failed: {why}")))?;
    let lacking = lacking_volumes(&held, &needs);
    if !lacking.is_empty() {
        return Err(Response::error(
            409,
            &format!("node {target} does not hold volume {}", lacking.join(", ")),
        ));
    }
    let was_asleep = agent.status == Status::Asleep;
    if !was_asleep {
        sleep::put_to_sleep(app, cid)?;
    }
    app.transcripts.record(
        &app.bus,
        cid,
        0,
        crate::transcript::Entry::MoveStarted {
            from: from.clone(),
            to: target.to_string(),
        },
    );

    let mut staged = Staged::default();
    let carried = carry(app, cid, &from, target, &run, vm, &mut staged);
    if let Some(export) = staged.export.take() {
        log_failure(
            "delete the export",
            call(app, &from, "DELETE", &format!("/v1/exports/{export}"), None),
        );
    }
    let (bytes, new_run) = match carried {
        Ok(carried) => carried,
        Err(message) => {
            abort(app, cid, target, &staged);
            // The placement is back on the source: a wake from here on restores the agent there.
            drop(claim);
            if !was_asleep && let Err(err) = sleep::ensure_awake(app, cid, WAKE_WAIT) {
                eprintln!("apid: move of agent {cid} aborted; it did not wake again: {err}");
            }
            return Err(Response::error(
                502,
                &format!("move to {target} failed: {message}"),
            ));
        }
    };

    // The agent's run and VM are the target's now, and so is its placement: the claim goes, so the
    // wake below (and any wake from here on) restores it there.
    app.agents.update(cid, |slot| {
        slot.run = Some(new_run.clone());
        slot.vm = Some(0);
    });
    drop(claim);
    let woke = sleep::ensure_awake(app, cid, WAKE_WAIT);

    // The source lets go of the agent: its gateway stops serving the CID, and its run goes.
    log_failure(
        "stop serving on the source",
        gateway_call_on(app, &from, "DELETE", &format!("/v1/cids/{cid}"), None),
    );
    log_failure(
        "delete the source run",
        call(app, &from, "DELETE", &format!("/v1/runs/{run}"), None),
    );
    if let Some(agent) = app.agents.get(cid) {
        app.bus.publish("agent", &agent);
    }
    let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    if let Err(err) = woke {
        return Err(Response::error(
            502,
            &format!("moved to {target} but the agent did not wake there: {err}"),
        ));
    }
    crate::metrics::ingest::record_event(
        &app.metrics,
        apz_metrics::Event {
            t_ms: apz_plaza::now_ms(),
            kind: "move".to_string(),
            entity_kind: apz_metrics::EntityKind::Agent,
            entity: cid.to_string(),
            duration_ms: Some(ms as f64),
            detail: format!("moved {from} → {target} in {ms} ms"),
            parts: Vec::new(),
        },
        target,
    );
    app.transcripts.record(
        &app.bus,
        cid,
        0,
        crate::transcript::Entry::Moved {
            from: from.clone(),
            to: target.to_string(),
            bytes,
            ms,
        },
    );
    Ok(Moved {
        from,
        to: target.to_string(),
        bytes,
        ms,
    })
}

/// The copy (plan 17.5): export the VM's files on `from`, have `from` offer them as blobs, have `target` fetch the chunks
/// it lacks straight from `from` under the hub's grant, and finish the import into a sleeping run, record the placement,
/// and have the target's gateway serve the CID. The hub relays no file bytes. Returns the bytes copied and the target's
/// run. What it takes goes into `staged`.
fn carry(
    app: &App,
    cid: u64,
    from: &str,
    target: &str,
    run: &str,
    vm: u32,
    staged: &mut Staged,
) -> Result<(u64, String), String> {
    let key = crate::transfer::key(app)?;
    let mut send = |node: &str, method: &str, path: &str, body: &Value| {
        answered(
            &format!("{node} {method} {path}"),
            fleet::noded(app, node, method, path, Some(body)),
        )
    };
    let exported = crate::transfer::exchange(
        &mut send,
        from,
        "POST",
        &format!("/v1/runs/{run}/vms/{vm}/export"),
        &json!({}),
    )?;
    let export = text(&exported, "export")?;
    staged.export = Some(export.clone());
    let sender = crate::transfer::sender_of(app, from)?;
    let broker = crate::transfer::broker_addr();
    let offered = crate::transfer::exchange(
        &mut send,
        from,
        "POST",
        &format!("/v1/exports/{export}/manifest"),
        &crate::transfer::hub_body(&key, from, json!({"files": exported["files"]})),
    )?;
    let id = text(&offered, "id")?;
    let expires = apz_transfer::now_ms() + apz_transfer::grant::TTL_MS;
    let grant = apz_transfer::Grant::issue(
        &key,
        &format!("move:{cid}:{export}"),
        &id,
        from,
        target,
        &sender.pin,
        expires,
    );
    let (routes, _) = crate::transfer::routes_for(&sender, broker.as_deref(), expires);
    let imported = crate::transfer::exchange(
        &mut send,
        target,
        "POST",
        "/v1/imports/from-transfer",
        &crate::transfer::hub_body(
            &key,
            target,
            json!({
                "routes": routes,
                "grant": grant.encode(),
                "manifest": offered["manifest"],
                "meta": exported["meta"],
                "files": exported["files"],
            }),
        ),
    )?;
    let bytes = imported["bytes"].as_u64().unwrap_or(0);
    let import = text(&imported, "import")?;
    staged.import = Some(import.clone());
    let finished = call(
        app,
        target,
        "POST",
        &format!("/v1/imports/{import}/finish"),
        Some(&json!({})),
    )?;
    staged.import = None;
    let new_run = text(&finished, "run")?;
    staged.target_run = Some(new_run.clone());
    if finished["cid"].as_u64() != Some(cid) {
        return Err(format!(
            "the import is for CID {}, not {cid}",
            finished["cid"]
        ));
    }
    // The placement is recorded and saved before anything routes by it.
    staged.overridden = Some(app.nodes.set_override(cid, Some(target)));
    save_placements(app)?;
    staged.served = true;
    gateway_call_on(
        app,
        target,
        "POST",
        &format!("/v1/cids/{cid}"),
        Some(&json!({})),
    )?;
    Ok((bytes, new_run))
}

/// Undoes what a failed move took: the placement goes back (and is saved so), and the target drops the
/// copy and the CID. The caller wakes an agent that was awake before the move, where it is.
fn abort(app: &App, cid: u64, target: &str, staged: &Staged) {
    if let Some(previous) = &staged.overridden {
        app.nodes.set_override(cid, previous.as_deref());
        log_failure("save the placement back", save_placements(app));
    }
    if let Some(import) = &staged.import {
        log_failure(
            "delete the import",
            call(
                app,
                target,
                "DELETE",
                &format!("/v1/imports/{import}"),
                None,
            ),
        );
    }
    if staged.served {
        log_failure(
            "stop serving on the target",
            gateway_call_on(app, target, "DELETE", &format!("/v1/cids/{cid}"), None),
        );
    }
    if let Some(run) = &staged.target_run {
        log_failure(
            "delete the target run",
            call(app, target, "DELETE", &format!("/v1/runs/{run}"), None),
        );
    }
}

/// Saves the node store, placements included, now (the saver's write, not its next pass). A failed write is logged
/// by the saver and retried on its next pass.
fn save_placements(app: &App) -> Result<(), String> {
    crate::store::saver::save_now(app, "nodes");
    Ok(())
}

/// A node's answer as its body when it is 2xx; otherwise the failure as text.
fn answered(what: &str, result: Result<(u16, Value), String>) -> Result<Value, String> {
    match result {
        Ok((code, body)) if (200..300).contains(&code) => Ok(body),
        Ok((code, body)) => Err(format!("{what} answered {code}: {body}")),
        Err(err) => Err(format!("{what}: {err}")),
    }
}

/// A noded call on `node`, answered as [`answered`].
fn call(
    app: &App,
    node: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<Value, String> {
    answered(
        &format!("{node} {method} {path}"),
        fleet::noded(app, node, method, path, body),
    )
}

/// A gateway (vsockd) call on `node`, answered as [`answered`].
fn gateway_call_on(
    app: &App,
    node: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<Value, String> {
    answered(
        &format!("{node} gateway {method} {path}"),
        fleet::hub_on(app, node, method, path, body),
    )
}

/// A field of a reply, as text.
fn text(body: &Value, key: &str) -> Result<String, String> {
    body[key]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("no {key} in the reply"))
}

/// Logs a clean-up step that failed; the move's outcome does not turn on it.
fn log_failure(what: &str, result: Result<impl Sized, String>) {
    if let Err(err) = result {
        eprintln!("apid: move: could not {what}: {err}");
    }
}

/// The text of an error response, for a log line.
pub(crate) fn body_text(resp: &Response) -> String {
    match &resp.body {
        Body::Bytes(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        _ => String::new(),
    }
}

/// Routes: `POST /v1/agents/{cid}/move`.
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .post("/v1/agents/{cid}/move", move_route)
        .doc(RouteDoc::new("Move an agent to another node"))
}

#[derive(Deserialize)]
struct MoveBody {
    node: String,
}

/// `POST /v1/agents/{cid}/move {node}`: the owner moves an agent to another node.
fn move_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let agent = match crate::agents::agent_at(app, p) {
        Ok(agent) => agent,
        Err(res) => return res,
    };
    let body: MoveBody = match req.json() {
        Ok(body) => body,
        Err(e) => return Response::error(400, &format!("bad body: {e}")),
    };
    match move_agent(app, agent.cid, &body.node) {
        Ok(moved) => Response::json(
            200,
            &json!({"ok": true, "from": moved.from, "to": moved.to, "bytes": moved.bytes, "ms": moved.ms}),
        ),
        Err(res) => res,
    }
}

/// A unit of Mac work placement starts: a build or a simulator. A Mac node reports a cap for each (`mac`).
// Mac build work places through these (engine 7.5); the tests place fake nodes through them until then.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacWork {
    Build,
    Simulator,
}

/// Why a Mac node cannot start `want` (pure): its cap for that kind of work is full. None when it has room, or
/// when it reports no caps (it is not a Mac with the node's sandbox).
#[allow(dead_code)]
pub fn mac_refusal(mac: Option<&MachineMac>, want: MacWork) -> Option<String> {
    let mac = mac?;
    let (running, cap, what) = match want {
        MacWork::Build => (mac.builds_running, mac.builds, "builds"),
        MacWork::Simulator => (mac.simulators_running, mac.simulators, "simulators"),
    };
    (running >= cap).then(|| format!("its cap of {cap} {what} is full ({running} running)"))
}

/// The Mac node a unit of `want` goes to (pure, over the nodes given with their caps): the one with the most room
/// for it, a tie going to the earlier. A node at its cap, and a node with no caps, is not chosen. None when every
/// Mac is full.
#[allow(dead_code)]
pub fn mac_pick(nodes: &[(String, Option<MachineMac>)], want: MacWork) -> Option<String> {
    let mut best: Option<(u32, &String)> = None;
    for (node, mac) in nodes {
        if mac_refusal(mac.as_ref(), want).is_some() {
            continue;
        }
        let Some(mac) = mac else { continue };
        let free = match want {
            MacWork::Build => mac.builds - mac.builds_running,
            MacWork::Simulator => mac.simulators - mac.simulators_running,
        };
        if best.is_none_or(|(most, _)| free > most) {
            best = Some((free, node));
        }
    }
    best.map(|(_, node)| node.clone())
}

/// The Mac node a unit of `want` goes to, among the nodes in service (see [`mac_pick`]).
#[allow(dead_code)]
pub fn choose_mac(app: &App, want: MacWork) -> Option<String> {
    let stopped = app.nodes.stopped_ids();
    let nodes: Vec<(String, Option<MachineMac>)> = app
        .nodes
        .list()
        .into_iter()
        .filter(|node| !stopped.contains(&node.id))
        .map(|node| (node.id, node.machine.mac))
        .collect();
    mac_pick(&nodes, want)
}

#[cfg(test)]
mod tests {
    use crate::fleet::node_for;

    use super::*;

    fn candidate(node: &str, free_mib: u64, running_vms: u64, warm: bool) -> Candidate {
        Candidate {
            node: node.to_string(),
            free_mib,
            running_vms,
            warm,
            pool: None,
            base: false,
            disk_free_gib: None,
            idle_cpus: None,
        }
    }

    /// A candidate in pool `pool`, a short name for an arm64 KVM pool or an x86_64 one.
    fn in_pool_of(node: &str, free_mib: u64, pool: Option<Pool>) -> Candidate {
        Candidate {
            pool,
            ..candidate(node, free_mib, 0, false)
        }
    }

    fn pool(arch: &str, hypervisor: &str, baseline: &str) -> Pool {
        Pool::of(Some(arch), Some(hypervisor), Some(baseline)).expect("a pool")
    }

    fn node_record(id: &str, last_seen_ms: u64, draining: bool, revoked: bool) -> Node {
        Node {
            id: id.to_string(),
            name: id.to_string(),
            public_key: String::new(),
            cid_base: 1000,
            cid_count: 100,
            joined_ms: 0,
            last_seen_ms,
            machine: crate::nodes::Machine::default(),
            draining,
            revoked,
            address: String::new(),
            label: String::new(),
            test_share: 0,
            path: crate::nodes::TUNNEL.to_string(),
            mesh_address: String::new(),
            mesh_token: String::new(),
        }
    }

    #[test]
    fn choose_weighs_memory_warmth_vms_and_ties() {
        for (case, nodes, want) in [
            (
                "the node with the most free memory takes a new agent",
                vec![
                    candidate("n0", 2048, 0, false),
                    candidate("n1", 8192, 0, false),
                ],
                "n1",
            ),
            (
                "a warm template outweighs a little more memory",
                vec![
                    candidate("n0", 3000, 0, true),
                    candidate("n1", 3500, 0, false),
                ],
                "n0",
            ),
            (
                "without the template the roomier node wins",
                vec![
                    candidate("n0", 3000, 0, false),
                    candidate("n1", 3500, 0, false),
                ],
                "n1",
            ),
            (
                "the VMs a node runs count against it",
                vec![
                    candidate("n0", 3200, 4, false),
                    candidate("n1", 3000, 0, false),
                ],
                "n1",
            ),
            (
                "a tie goes to n0 whatever the order",
                vec![
                    candidate("n1", 4096, 0, false),
                    candidate("n0", 4096, 0, false),
                ],
                "n0",
            ),
            (
                "a node without room for an agent is skipped",
                vec![
                    candidate("n0", 128, 0, false),
                    candidate("n1", 2048, 0, false),
                ],
                "n1",
            ),
            (
                "a node without room alone means n0",
                vec![candidate("n1", 128, 0, true)],
                "n0",
            ),
            ("no node at all means n0", vec![], "n0"),
        ] {
            assert_eq!(choose(&nodes), want, "{case}");
        }
    }

    #[test]
    fn choose_admits_a_node_with_admit_mib_free_and_no_more() {
        assert_eq!(choose(&[candidate("n1", 300, 0, false)]), "n1");
        assert_eq!(choose(&[candidate("n1", 200, 0, false)]), LOCAL);
        assert_eq!(
            choose(&[
                candidate("n1", 200, 0, false),
                candidate("n2", 300, 0, false),
            ]),
            "n2"
        );
    }

    #[test]
    fn a_reservation_counts_against_its_node_until_it_expires() {
        let now = Instant::now();
        let reserved = || RESERVED.lock().unwrap_or_else(PoisonError::into_inner);
        reserved().clear();
        assert_eq!(reserved_mib("n1", now), 0);
        for placed in 1..=2 {
            reserved().push(("n1".to_string(), now));
            assert_eq!(reserved_mib("n1", now), RESERVE_MIB * placed);
            assert_eq!(reserved_mib("n2", now), 0, "another node holds none");
        }
        if let Some(old) = now.checked_sub(RESERVE_FOR + Duration::from_secs(1)) {
            reserved().push(("n2".to_string(), old));
            assert_eq!(
                reserved_mib("n2", now),
                0,
                "an expired reservation is dropped"
            );
        }
        reserved().clear();
    }

    #[test]
    fn a_move_is_refused_to_the_same_node_an_unknown_or_unusable_node() {
        let now = 1_000_000_000;
        let fresh = node_record("n1", now, false, false);
        assert_eq!(
            target_refusal("n1", "n1", Some(&fresh), false, now).map(|r| r.0),
            Some(409)
        );
        assert_eq!(
            target_refusal("n0", "n2", None, false, now).map(|r| r.0),
            Some(404)
        );
        let revoked = node_record("n1", now, false, true);
        assert_eq!(
            target_refusal("n0", "n1", Some(&revoked), false, now).map(|r| r.0),
            Some(409)
        );
        let stopped = node_record("n1", now, true, false);
        assert_eq!(
            target_refusal("n0", "n1", Some(&stopped), true, now),
            Some((409, "node n1 is stopped".to_string()))
        );
        let silent = node_record("n1", now - OFFLINE_AFTER_MS, false, false);
        assert_eq!(
            target_refusal("n0", "n1", Some(&silent), false, now).map(|r| r.0),
            Some(409)
        );
        assert_eq!(
            target_refusal("n1", "n0", None, false, now),
            None,
            "n0 takes agents from a joined node"
        );
        assert_eq!(
            target_refusal("n1", "n0", None, true, now),
            Some((409, "node n0 is stopped".to_string())),
            "unless the owner stopped it"
        );
        assert_eq!(target_refusal("n0", "n1", Some(&fresh), false, now), None);
    }

    #[test]
    fn a_vm_moves_only_to_a_node_that_holds_every_volume_it_carries() {
        let held = vec!["data".to_string(), "models".to_string()];
        assert!(
            lacking_volumes(&held, &[]).is_empty(),
            "a VM with no volumes fits any node"
        );
        assert!(lacking_volumes(&held, &["models".to_string()]).is_empty());
        assert_eq!(
            lacking_volumes(&held, &["data".to_string(), "logs".to_string()]),
            ["logs"],
            "the volumes the node lacks, in the VM's order"
        );
        assert_eq!(
            lacking_volumes(&[], &["data".to_string()]),
            ["data"],
            "a node that holds none refuses every volume"
        );
    }

    #[test]
    fn pick_takes_the_best_eligible_candidate_and_none_without_one() {
        assert_eq!(pick(&[]), None);
        assert_eq!(pick(&[candidate("n1", ADMIT_MIB - 1, 0, false)]), None);
        assert_eq!(
            pick(&[
                candidate("n1", 2048, 0, false),
                candidate("n2", 4096, 0, false)
            ]),
            Some("n2".to_string())
        );
        assert_eq!(choose(&[]), "n0", "a new agent still has n0 to go to");
    }

    #[test]
    fn the_fleet_is_full_only_when_every_node_that_reports_is_full() {
        assert!(!full_of(&[]), "no node reports: noded decides, as before");
        let full = [
            candidate("n0", ADMIT_MIB - 1, 3, false),
            candidate("n1", 10, 0, false),
        ];
        assert!(full_of(&full));
        let room = [
            candidate("n0", ADMIT_MIB - 1, 3, false),
            candidate("n1", ADMIT_MIB, 0, false),
        ];
        assert!(!full_of(&room), "one node with ADMIT_MIB free has room");
    }

    #[test]
    fn a_working_or_offline_agent_does_not_move() {
        assert_eq!(status_refusal(7, Status::Working).map(|r| r.0), Some(409));
        assert_eq!(status_refusal(7, Status::Offline).map(|r| r.0), Some(409));
        assert_eq!(status_refusal(7, Status::Idle), None);
        assert_eq!(status_refusal(7, Status::Asleep), None);
        assert_eq!(
            status_refusal(7, Status::Dozing),
            None,
            "a dozing agent moves like an idle one"
        );
    }

    #[test]
    fn a_recorded_move_or_override_wins_over_the_range_owner() {
        // A recorded move wins over the cid ranges.
        {
            assert_eq!(node_for(Some("n0".into()), Some("n1".into())), "n0");
            assert_eq!(node_for(None, Some("n1".into())), "n1");
            assert_eq!(node_for(None, None), LOCAL);
        }
        // node_of takes the override over the range owner.
        {
            let store = crate::nodes::Store::holding(vec![crate::nodes::Node {
                id: "n1".into(),
                name: "a".into(),
                public_key: String::new(),
                cid_base: 1000,
                cid_count: 100,
                joined_ms: 0,
                last_seen_ms: 0,
                machine: crate::nodes::Machine::default(),
                draining: false,
                revoked: false,
                address: String::new(),
                label: String::new(),
                test_share: 0,
                path: crate::nodes::TUNNEL.to_string(),
                mesh_address: String::new(),
                mesh_token: String::new(),
            }]);
            let owner = |cid| store.owner_of_cid(cid).map(|n| n.id);
            assert_eq!(node_for(store.override_of(1005), owner(1005)), "n1");
            store.set_override(1005, Some("n0"));
            assert_eq!(node_for(store.override_of(1005), owner(1005)), "n0");
            assert_eq!(node_for(store.override_of(1006), owner(1006)), "n1");
        }
    }

    /// The node store's rows saved to their tables and loaded into a fresh store.
    fn saved(store: &crate::nodes::Store) -> crate::nodes::Store {
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::nodes::def().ddl()).unwrap();
        apz_store::table::replace(&conn, crate::nodes::def(), &store.rows().unwrap()).unwrap();
        let again = crate::nodes::Store::default();
        again.load(&conn).unwrap();
        again
    }

    #[test]
    fn the_override_map_survives_a_save_and_a_clear_is_saved_too() {
        let store = crate::nodes::Store::default();
        assert_eq!(store.set_override(1005, Some("n2")), None);
        assert_eq!(store.override_of(1005).as_deref(), Some("n2"));
        let restored = saved(&store);
        assert_eq!(restored.override_of(1005).as_deref(), Some("n2"));
        assert_eq!(restored.set_override(1005, None).as_deref(), Some("n2"));
        let after_clear = saved(&restored);
        assert_eq!(after_clear.override_of(1005), None, "the clear is saved");
    }

    #[test]
    fn an_empty_store_loads_with_no_overrides() {
        let store = crate::nodes::Store::default();
        assert_eq!(saved(&store).override_of(1005), None);
    }

    #[test]
    fn a_claim_holds_the_agent_until_it_is_dropped() {
        let first = Claim::take(4242).expect("the first claim");
        assert!(is_moving(4242));
        assert!(Claim::take(4242).is_none(), "a second move waits");
        drop(first);
        assert!(!is_moving(4242));
        assert!(Claim::take(4242).is_some());
    }

    fn mac(
        builds: u32,
        builds_running: u32,
        simulators: u32,
        simulators_running: u32,
    ) -> MachineMac {
        MachineMac {
            builds,
            simulators,
            builds_running,
            simulators_running,
        }
    }

    #[test]
    fn a_move_lands_only_in_the_pool_of_its_node() {
        let arm = pool("aarch64", "kvm", "arm64 midr=0x41:0xd0c:0x3:1");
        let x86 = pool("x86_64", "kvm", "x86_64 vendor=GenuineIntel sig=0x000806e9");
        let fleet = vec![
            in_pool_of("n1", 9000, Some(x86.clone())),
            in_pool_of("n2", 4000, Some(arm.clone())),
            in_pool_of("n3", 2000, Some(arm.clone())),
            in_pool_of(
                "n4",
                9000,
                Some(Pool {
                    hypervisor: "hvf".into(),
                    ..arm.clone()
                }),
            ),
            in_pool_of("n5", 9000, None),
        ];
        // An arm64 agent leaving n2 has room on n4 and n1 (other pools) and n5 (no pool): only n3 is in its pool.
        let movable = in_pool(fleet.clone(), "n2", Some(&arm));
        assert_eq!(pick(&movable).as_deref(), Some("n3"));
        // An x86_64 agent leaving n1 has no other machine in its pool.
        assert_eq!(pick(&in_pool(fleet.clone(), "n1", Some(&x86))), None);
        // A node with no pool takes no move out, and nothing moves to it.
        assert!(in_pool(fleet.clone(), "n5", None).is_empty());
        assert!(in_pool(fleet, "n2", None).is_empty());
    }

    #[test]
    fn a_new_agent_is_placed_on_the_best_node_of_any_pool() {
        let arm = pool("aarch64", "kvm", "arm64 midr=0x41:0xd0c:0x3:1");
        let x86 = pool("x86_64", "kvm", "x86_64 vendor=GenuineIntel sig=0x000806e9");
        let fleet = vec![
            in_pool_of("n2", 4000, Some(arm)),
            in_pool_of("n1", 9000, Some(x86)),
        ];
        assert_eq!(
            choose(&fleet),
            "n1",
            "a new agent is not pinned to a pool by placement"
        );
    }

    #[test]
    fn a_move_is_refused_across_pools_and_from_a_node_with_none() {
        let arm = pool("aarch64", "kvm", "arm64 midr=0x41:0xd0c:0x3:1");
        let other = pool("aarch64", "kvm", "arm64 midr=0x41:0xd0d:0x3:1");
        assert_eq!(pool_refusal("n3", Some(&arm), Some(&arm)), None);
        for (case, from, to) in [
            ("another baseline", Some(&arm), Some(&other)),
            ("the agent's node has no pool", None, Some(&arm)),
            ("the target has no pool", Some(&arm), None),
        ] {
            let (status, message) = pool_refusal("n3", from, to).expect(case);
            assert_eq!(status, 409, "{case}");
            assert!(
                message.contains("only within its pool"),
                "{case}: {message}"
            );
        }
    }

    #[test]
    fn a_mac_at_its_build_cap_gets_no_build_and_the_room_goes_to_another() {
        let nodes = vec![
            ("n1".to_string(), Some(mac(2, 2, 6, 0))),
            ("n2".to_string(), Some(mac(2, 1, 6, 0))),
            ("n3".to_string(), None),
        ];
        assert_eq!(mac_pick(&nodes, MacWork::Build).as_deref(), Some("n2"));
        let refused = mac_refusal(nodes[0].1.as_ref(), MacWork::Build).unwrap();
        assert!(refused.contains("cap of 2 builds"), "{refused}");
        assert_eq!(
            mac_refusal(nodes[0].1.as_ref(), MacWork::Simulator),
            None,
            "the caps are separate"
        );
        assert_eq!(
            mac_refusal(nodes[2].1.as_ref(), MacWork::Build),
            None,
            "a node with no caps is not refused as full"
        );
    }

    #[test]
    fn a_full_mac_gets_neither_a_build_nor_a_simulator() {
        let nodes = vec![("n1".to_string(), Some(mac(2, 2, 6, 6)))];
        assert_eq!(mac_pick(&nodes, MacWork::Build), None);
        assert_eq!(mac_pick(&nodes, MacWork::Simulator), None);
    }

    #[test]
    fn the_mac_with_most_simulator_room_takes_the_simulator() {
        let nodes = vec![
            ("n1".to_string(), Some(mac(2, 0, 6, 4))),
            ("n2".to_string(), Some(mac(2, 0, 6, 1))),
            ("n3".to_string(), None),
        ];
        assert_eq!(mac_pick(&nodes, MacWork::Simulator).as_deref(), Some("n2"));
        assert_eq!(mac_pick(&[], MacWork::Build), None);
    }

    #[test]
    fn a_heartbeat_carries_the_mac_caps_into_the_machine() {
        let machine: crate::nodes::Machine = serde_json::from_value(serde_json::json!({
            "mac": {"builds": 2, "simulators": 6, "builds_running": 1, "simulators_running": 0}
        }))
        .unwrap();
        assert_eq!(machine.mac, Some(mac(2, 1, 6, 0)));
    }

    #[test]
    fn a_heartbeat_names_the_pool_of_its_machine() {
        let machine: crate::nodes::Machine = serde_json::from_value(json!({
            "arch": "aarch64",
            "backend": "hvf",
            "cpu_baseline": "arm64 cpufamily=0x67f674b3"
        }))
        .unwrap();
        assert_eq!(
            machine.pool(),
            Some(pool("aarch64", "hvf", "arm64 cpufamily=0x67f674b3"))
        );
        let without: crate::nodes::Machine = serde_json::from_value(json!({
            "arch": "aarch64",
            "backend": "hvf"
        }))
        .unwrap();
        assert_eq!(without.pool(), None, "no baseline, no pool");
    }

    #[test]
    fn a_fork_starts_on_the_node_that_holds_its_base() {
        let mut holds = candidate("n2", 3000, 0, false);
        holds.base = true;
        assert_eq!(
            choose(&[candidate("n1", 9000, 0, true), holds]),
            "n2",
            "the base outweighs the memory and the warm template"
        );
        assert_eq!(
            choose(&[
                candidate("n1", 9000, 0, false),
                candidate("n2", 3000, 0, false)
            ]),
            "n1",
            "with no base on any node, the roomier node takes it"
        );
        let mut without_room = candidate("n2", 100, 0, false);
        without_room.base = true;
        assert_eq!(
            choose(&[without_room, candidate("n1", 2048, 0, false)]),
            "n1",
            "a node with no room for an agent takes no fork, base or not"
        );
        assert_eq!(base_key("pr1"), "pr1/build");
    }

    #[test]
    fn a_fork_starts_only_in_the_pools_its_project_declares() {
        let arm = project_stack::Pool {
            arch: project_stack::Arch::Aarch64,
            hypervisor: project_stack::Hypervisor::Kvm,
        };
        let fleet = vec![
            in_pool_of(
                "n1",
                9000,
                Some(pool(
                    "x86_64",
                    "kvm",
                    "x86_64 vendor=GenuineIntel sig=0x000806e9",
                )),
            ),
            in_pool_of(
                "n2",
                4000,
                Some(pool("aarch64", "kvm", "arm64 midr=0x41:0xd0c:0x3:1")),
            ),
            in_pool_of(
                "n3",
                9000,
                Some(pool("aarch64", "hvf", "arm64 cpufamily=0x67f674b3")),
            ),
            in_pool_of("n4", 9000, None),
        ];
        assert_eq!(
            pick(&in_pools(fleet.clone(), &[arm])).as_deref(),
            Some("n2"),
            "the roomier nodes are another arch, another hypervisor, or no pool"
        );
        assert_eq!(
            in_pools(fleet, &[]).len(),
            4,
            "a project that declares no pool takes any"
        );
    }

    #[test]
    fn a_project_that_declares_pools_gets_no_node_outside_them() {
        let arm = project_stack::Pool {
            arch: project_stack::Arch::Aarch64,
            hypervisor: project_stack::Hypervisor::Kvm,
        };
        let x86 = in_pool_of(
            "n1",
            9000,
            Some(pool(
                "x86_64",
                "kvm",
                "x86_64 vendor=GenuineIntel sig=0x000806e9",
            )),
        );
        assert_eq!(
            fork_node(vec![x86.clone()], std::slice::from_ref(&arm)),
            None,
            "the only node is in another pool: no fork starts"
        );
        assert_eq!(
            fork_node(vec![x86.clone()], &[]).as_deref(),
            Some("n1"),
            "with no declared pool the fork goes where the memory is"
        );
        let arm_node = in_pool_of(
            "n2",
            4000,
            Some(pool("aarch64", "kvm", "arm64 midr=0x41:0xd0c:0x3:1")),
        );
        assert_eq!(
            fork_node(vec![x86, arm_node], &[arm]).as_deref(),
            Some("n2")
        );
    }

    #[test]
    fn a_node_short_of_disk_takes_no_fork_and_one_that_does_not_say_is_not_kept_out() {
        let mut full = candidate("n1", 9000, 0, false);
        full.disk_free_gib = Some(FORK_DISK_GIB - 1);
        let mut roomy = candidate("n2", 3000, 0, false);
        roomy.disk_free_gib = Some(FORK_DISK_GIB);
        let unknown = candidate("n3", 2000, 0, false);
        assert_eq!(pick(&[full.clone(), roomy]).as_deref(), Some("n2"));
        assert_eq!(pick(&[full.clone(), unknown]).as_deref(), Some("n3"));
        assert_eq!(pick(std::slice::from_ref(&full)), None);
        assert_eq!(choose(&[full]), LOCAL, "no node takes it: n0 decides");
    }

    #[test]
    fn the_idle_cpus_of_a_node_count_for_it() {
        let mut idle = candidate("n1", 4096, 0, false);
        idle.idle_cpus = Some(8);
        let mut busy = candidate("n2", 4096, 0, false);
        busy.idle_cpus = Some(1);
        assert_eq!(choose(&[busy, idle]), "n1");
        assert_eq!(idle_cpus(Some(16), Some(25.0)), Some(12));
        assert_eq!(
            idle_cpus(Some(4), Some(150.0)),
            Some(0),
            "the load is clamped"
        );
        assert_eq!(idle_cpus(None, Some(10.0)), None);
        assert_eq!(idle_cpus(Some(4), None), None);
    }

    #[test]
    fn a_node_that_does_both_gives_agents_its_work_share_of_memory() {
        assert_eq!(work_share_of(8192, 0), 8192);
        assert_eq!(work_share_of(8192, 25), 6144);
        assert_eq!(work_share_of(8192, 100), 0);
        assert_eq!(work_share_of(8192, 250), 0);
    }
}
