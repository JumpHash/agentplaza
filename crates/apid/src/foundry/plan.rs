//! The Foundry's GPU placement planner (`docs/design/foundry.md`, "Hardware" and "Coupling").
//!
//! Given the fleet's machines, their accelerators and the measured round trips between
//! machines, [`plan`] picks the devices a job runs on and groups them into islands: devices
//! that can run one tightly coupled collective. Pure: no I/O, no clock, no randomness. The
//! same inputs always give the same plan, whatever order the machines came in.

use std::cmp::{Ordering, Reverse};
use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Round trip below which two machines count as one LAN island.
pub const LAN_RTT_MS: f32 = 2.0;
/// Port of the torchrun rendezvous on an island's leader.
pub const RDZV_PORT: u16 = 29400;
/// Inner steps per DiLoCo round.
pub const INNER_STEPS: u32 = 500;
/// How many rounds a loose island may lag behind the hub.
pub const LOOSE_STALENESS: u32 = 2;

/// Backend families in planning order; `cpu` is the last resort.
const BACKEND_ORDER: [&str; 5] = ["cuda", "rocm", "xpu", "mps", "cpu"];

/// One accelerator as reported by noded in `/v1/machine`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Accelerator {
    pub index: u32,
    pub vendor: String,
    pub name: String,
    pub memory_mib: u64,
    /// `cuda` | `rocm` | `xpu` | `mps` | `cpu`.
    pub backend: String,
    /// `pcie` | `nvlink` | `xgmi` | `soc`.
    pub link: String,
    pub busy: bool,
    pub util_pct: u32,
    pub mem_used_mib: u64,
}

/// A machine in the fleet, with what the planner needs to place work on it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Machine {
    pub id: String,
    pub online: bool,
    pub accelerators: Vec<Accelerator>,
    pub trainer_ok: bool,
    /// Addresses other machines may reach this one on.
    pub fabric: Vec<String>,
}

/// Measured round-trip times between machines. Symmetric; a pair that was never measured,
/// or was measured unreachable, has no entry (`None`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "Vec<ReachPair>", into = "Vec<ReachPair>")]
pub struct Reach {
    pairs: BTreeMap<(String, String), Option<f32>>,
}

/// One measured pair, as it travels on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReachPair {
    pub a: String,
    pub b: String,
    pub rtt_ms: Option<f32>,
}

impl Reach {
    /// Records the round trip between two machines; `None` means unreachable.
    pub fn set(&mut self, a: &str, b: &str, rtt_ms: Option<f32>) {
        self.pairs.insert(pair_key(a, b), rtt_ms);
    }

    /// The round trip between two machines, `None` when unmeasured or unreachable.
    pub fn rtt_ms(&self, a: &str, b: &str) -> Option<f32> {
        self.pairs.get(&pair_key(a, b)).copied().flatten()
    }

    fn is_lan(&self, a: &str, b: &str) -> bool {
        self.rtt_ms(a, b).is_some_and(|ms| ms < LAN_RTT_MS)
    }
}

fn pair_key(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_owned(), b.to_owned())
    } else {
        (b.to_owned(), a.to_owned())
    }
}

impl From<Vec<ReachPair>> for Reach {
    fn from(pairs: Vec<ReachPair>) -> Self {
        let mut reach = Reach::default();
        for pair in pairs {
            reach.set(&pair.a, &pair.b, pair.rtt_ms);
        }
        reach
    }
}

impl From<Reach> for Vec<ReachPair> {
    fn from(reach: Reach) -> Self {
        reach
            .pairs
            .into_iter()
            .map(|((a, b), rtt_ms)| ReachPair { a, b, rtt_ms })
            .collect()
    }
}

/// How a job spreads over devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// One device.
    Single,
    /// One island: data parallel with torchrun.
    Tight,
    /// Several islands: DiLoCo-style low-communication training.
    Loose,
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Mode::Single => "single",
            Mode::Tight => "tight",
            Mode::Loose => "loose",
        })
    }
}

/// What a job asks for.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Request {
    pub min_devices: u32,
    pub max_devices: u32,
    /// Free memory a device needs (total minus what is in use).
    pub min_memory_mib: u64,
    /// Allowed backends; `None` allows every backend.
    pub backends: Option<Vec<String>>,
    /// Pins the mode; the plan fails rather than picking another one.
    pub mode: Option<Mode>,
    /// Pins the machines the job may use.
    pub machines: Option<Vec<String>>,
    /// CPU devices are used only when this is set and no GPU device is eligible.
    pub allow_cpu: bool,
    /// Carried with the job; placement does not read it.
    pub budget_gpu_hours: Option<f64>,
}

/// A placement: the mode, the islands in capacity order, and the training knobs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub mode: Mode,
    pub islands: Vec<Island>,
    /// Inner steps per round (the trainer's default; set for every mode).
    pub inner_steps: u32,
    /// Rounds a loose island may lag behind the hub; 0 for single and tight.
    pub staleness: u32,
    pub devices_total: u32,
}

/// Devices that run one collective: one backend family, mutually reachable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Island {
    /// `i1`, `i2`, ... in plan order.
    pub id: String,
    pub backend: String,
    /// Members in plan order: the leader first, then by device count, then id.
    pub machines: Vec<Placement>,
    pub leader: String,
    /// The leader's first fabric address and the rendezvous port; `None` on a single machine.
    pub rdzv: Option<String>,
}

/// The devices one machine contributes to an island.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub id: String,
    pub devices: Vec<u32>,
}

/// Why no plan exists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum PlanError {
    /// The request is malformed.
    BadRequest { reason: String },
    /// No machine has a device that qualifies.
    NoDevices { reason: String },
    /// Fewer eligible devices than the job's minimum.
    NotEnough { available: u32, min_devices: u32 },
    /// The pinned mode cannot be met.
    Infeasible { mode: Mode, reason: String },
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::BadRequest { reason } => write!(f, "bad request: {reason}"),
            PlanError::NoDevices { reason } => write!(f, "no eligible devices: {reason}"),
            PlanError::NotEnough {
                available,
                min_devices,
            } => write!(
                f,
                "{available} eligible devices, the job needs at least {min_devices}"
            ),
            PlanError::Infeasible { mode, reason } => {
                write!(f, "{mode} placement is infeasible: {reason}")
            }
        }
    }
}

impl std::error::Error for PlanError {}

/// A device that passed the request's filters.
#[derive(Debug, Clone)]
struct Device {
    index: u32,
    backend: String,
    memory_mib: u64,
    util_pct: u32,
}

/// A machine with its eligible devices, sorted by index.
#[derive(Debug, Clone)]
struct Node {
    id: String,
    fabric: Vec<String>,
    devices: Vec<Device>,
}

/// An island in the making: one backend family, members sorted by id.
#[derive(Debug, Clone)]
struct Group {
    backend: String,
    members: Vec<Node>,
}

impl Group {
    fn count(&self) -> u32 {
        self.members.iter().map(|m| m.devices.len() as u32).sum()
    }

    fn memory(&self) -> u64 {
        self.members
            .iter()
            .flat_map(|m| &m.devices)
            .map(|d| d.memory_mib)
            .sum()
    }

    fn describe(&self) -> String {
        let ids: Vec<&str> = self.members.iter().map(|m| m.id.as_str()).collect();
        format!("{} on {}", self.backend, ids.join(", "))
    }
}

/// Places a job on the fleet. Errors name the reason when no plan exists.
pub fn plan(machines: &[Machine], reach: &Reach, req: &Request) -> Result<Plan, PlanError> {
    if req.max_devices == 0 {
        return Err(PlanError::BadRequest {
            reason: "max_devices must be at least 1".into(),
        });
    }
    let min = req.min_devices.max(1);
    let max = req.max_devices;
    if min > max {
        return Err(PlanError::BadRequest {
            reason: format!("min_devices {min} exceeds max_devices {max}"),
        });
    }

    let mut nodes = nodes_where(machines, req, |backend| backend != "cpu");
    if nodes.is_empty() {
        nodes = nodes_where(machines, req, |backend| backend == "cpu");
    }
    if nodes.is_empty() {
        return Err(PlanError::NoDevices {
            reason: no_devices_reason(req),
        });
    }

    let groups = build_groups(&nodes, reach);
    let total: u32 = groups.iter().map(Group::count).sum();
    if total < min {
        return Err(PlanError::NotEnough {
            available: total,
            min_devices: min,
        });
    }

    let fitting = || groups.iter().find(|g| g.count() >= min);
    match req.mode {
        None if max == 1 || total == 1 => single(&groups),
        None => match fitting() {
            Some(group) => Ok(finish(Mode::Tight, vec![take(group, max)])),
            None => loose(&groups, max),
        },
        Some(Mode::Single) if min > 1 => Err(PlanError::Infeasible {
            mode: Mode::Single,
            reason: format!("one device, but the job needs at least {min}"),
        }),
        Some(Mode::Single) => single(&groups),
        Some(Mode::Tight) => match fitting() {
            Some(group) => Ok(finish(Mode::Tight, vec![take(group, max)])),
            None => {
                let largest = groups
                    .iter()
                    .max_by_key(|g| g.count())
                    .expect("total >= min >= 1, so there is a group");
                Err(PlanError::Infeasible {
                    mode: Mode::Tight,
                    reason: format!(
                        "the job needs {min} devices on one island, but the largest island has {} ({})",
                        largest.count(),
                        largest.describe()
                    ),
                })
            }
        },
        Some(Mode::Loose) => loose(&groups, max),
    }
}

/// Eligible machines and their usable devices. `want` filters backends.
fn nodes_where(machines: &[Machine], req: &Request, want: impl Fn(&str) -> bool) -> Vec<Node> {
    let mut sorted: Vec<&Machine> = machines
        .iter()
        .filter(|m| m.online && m.trainer_ok)
        .filter(|m| req.machines.as_ref().is_none_or(|ids| ids.contains(&m.id)))
        .collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    sorted.dedup_by(|a, b| a.id == b.id);

    sorted
        .into_iter()
        .filter_map(|m| {
            let mut devices: Vec<Device> = m
                .accelerators
                .iter()
                .filter(|a| want(&a.backend) && usable(a, req))
                .map(|a| Device {
                    index: a.index,
                    backend: a.backend.clone(),
                    memory_mib: a.memory_mib,
                    util_pct: a.util_pct,
                })
                .collect();
            devices.sort_by_key(|d| d.index);
            (!devices.is_empty()).then(|| Node {
                id: m.id.clone(),
                fabric: m.fabric.clone(),
                devices,
            })
        })
        .collect()
}

fn usable(a: &Accelerator, req: &Request) -> bool {
    BACKEND_ORDER.contains(&a.backend.as_str())
        && (a.backend != "cpu" || req.allow_cpu)
        && req
            .backends
            .as_ref()
            .is_none_or(|allowed| allowed.contains(&a.backend))
        && !a.busy
        && a.memory_mib.saturating_sub(a.mem_used_mib) >= req.min_memory_mib
}

fn no_devices_reason(req: &Request) -> String {
    let scope = match &req.machines {
        Some(ids) => format!("none of the pinned machines ({}) has", ids.join(", ")),
        None => "no online machine with a working trainer has".to_string(),
    };
    format!(
        "{scope} a free device of the allowed backends with {} MiB free",
        req.min_memory_mib
    )
}

/// Islands per backend family, in capacity order. Only NVIDIA, AMD and Intel collectives
/// span machines; `mps` and `cpu` islands are always one machine each.
fn build_groups(nodes: &[Node], reach: &Reach) -> Vec<Group> {
    let mut groups = Vec::new();
    for family in BACKEND_ORDER {
        let members: Vec<Node> = nodes
            .iter()
            .filter_map(|n| {
                let devices: Vec<Device> = n
                    .devices
                    .iter()
                    .filter(|d| d.backend == family)
                    .cloned()
                    .collect();
                (!devices.is_empty()).then(|| Node {
                    id: n.id.clone(),
                    fabric: n.fabric.clone(),
                    devices,
                })
            })
            .collect();

        let joinable = matches!(family, "cuda" | "rocm" | "xpu");
        let lan = |i: usize, j: usize| {
            joinable
                && !members[i].fabric.is_empty()
                && !members[j].fabric.is_empty()
                && reach.is_lan(&members[i].id, &members[j].id)
        };
        for part in components(members.len(), lan) {
            groups.push(Group {
                backend: family.to_string(),
                members: part.into_iter().map(|i| members[i].clone()).collect(),
            });
        }
    }
    groups.sort_by(capacity);
    groups
}

/// Connected components over `n` nodes, each sorted by index, in order of first index.
fn components(n: usize, linked: impl Fn(usize, usize) -> bool) -> Vec<Vec<usize>> {
    let mut seen = vec![false; n];
    let mut out = Vec::new();
    for start in 0..n {
        if seen[start] {
            continue;
        }
        seen[start] = true;
        let mut stack = vec![start];
        let mut part = Vec::new();
        while let Some(i) = stack.pop() {
            part.push(i);
            for (j, seen_j) in seen.iter_mut().enumerate() {
                if !*seen_j && linked(i, j) {
                    *seen_j = true;
                    stack.push(j);
                }
            }
        }
        part.sort_unstable();
        out.push(part);
    }
    out
}

/// Capacity order: most memory, then most devices, then backend order, then first member id.
fn capacity(a: &Group, b: &Group) -> Ordering {
    b.memory()
        .cmp(&a.memory())
        .then(b.count().cmp(&a.count()))
        .then(rank(&a.backend).cmp(&rank(&b.backend)))
        .then_with(|| {
            a.members
                .first()
                .map(|m| &m.id)
                .cmp(&b.members.first().map(|m| &m.id))
        })
}

fn rank(backend: &str) -> usize {
    BACKEND_ORDER
        .iter()
        .position(|b| *b == backend)
        .unwrap_or(usize::MAX)
}

/// The first `n` devices of a group, taken from its largest member first. The leader (most
/// devices, then id) ends up first.
fn take(group: &Group, n: u32) -> Group {
    let mut members = group.members.clone();
    members.sort_by(|a, b| {
        b.devices
            .len()
            .cmp(&a.devices.len())
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut left = n as usize;
    let mut kept = Vec::new();
    for mut node in members {
        if left == 0 {
            break;
        }
        node.devices.truncate(left);
        left -= node.devices.len();
        kept.push(node);
    }
    Group {
        backend: group.backend.clone(),
        members: kept,
    }
}

/// One device: the one with the most memory, then the least busy, then the lowest id.
fn single(groups: &[Group]) -> Result<Plan, PlanError> {
    let mut best: Option<(&Node, &Device)> = None;
    for node in groups.iter().flat_map(|g| &g.members) {
        for dev in &node.devices {
            let better = match best {
                None => true,
                Some((bn, bd)) => {
                    (Reverse(dev.memory_mib), dev.util_pct, &node.id, dev.index)
                        < (Reverse(bd.memory_mib), bd.util_pct, &bn.id, bd.index)
                }
            };
            if better {
                best = Some((node, dev));
            }
        }
    }
    let Some((node, dev)) = best else {
        return Err(PlanError::NoDevices {
            reason: "no device left after filtering".into(),
        });
    };
    let group = Group {
        backend: dev.backend.clone(),
        members: vec![Node {
            id: node.id.clone(),
            fabric: node.fabric.clone(),
            devices: vec![dev.clone()],
        }],
    };
    Ok(finish(Mode::Single, vec![group]))
}

/// Islands in capacity order, each taken whole, until `max` devices are placed. The last one
/// is trimmed to fit. Needs at least two islands.
fn loose(groups: &[Group], max: u32) -> Result<Plan, PlanError> {
    let mut picked: Vec<Group> = Vec::new();
    let mut taken = 0;
    for group in groups {
        if taken == max {
            break;
        }
        let part = take(group, max - taken);
        taken += part.count();
        picked.push(part);
    }
    if picked.len() < 2 {
        let only = picked
            .first()
            .map_or_else(String::new, |g| format!(" ({})", g.describe()));
        return Err(PlanError::Infeasible {
            mode: Mode::Loose,
            reason: format!("loose training needs two islands; only one is eligible{only}"),
        });
    }
    Ok(finish(Mode::Loose, picked))
}

fn finish(mode: Mode, groups: Vec<Group>) -> Plan {
    let devices_total: u32 = groups.iter().map(Group::count).sum();
    Plan {
        mode,
        islands: groups
            .into_iter()
            .enumerate()
            .map(|(i, group)| island(i, group))
            .collect(),
        inner_steps: INNER_STEPS,
        staleness: if mode == Mode::Loose {
            LOOSE_STALENESS
        } else {
            0
        },
        devices_total,
    }
}

fn island(index: usize, group: Group) -> Island {
    let leader = &group.members[0];
    let rdzv = match (group.members.len() > 1, leader.fabric.first()) {
        (true, Some(addr)) => Some(rendezvous(addr)),
        _ => None,
    };
    Island {
        id: format!("i{}", index + 1),
        backend: group.backend.clone(),
        machines: group
            .members
            .iter()
            .map(|m| Placement {
                id: m.id.clone(),
                devices: m.devices.iter().map(|d| d.index).collect(),
            })
            .collect(),
        leader: leader.id.clone(),
        rdzv,
    }
}

/// `host:port`, with IPv6 hosts in brackets.
fn rendezvous(addr: &str) -> String {
    if addr.contains(':') {
        format!("[{addr}]:{RDZV_PORT}")
    } else {
        format!("{addr}:{RDZV_PORT}")
    }
}

#[cfg(test)]
mod tests {
    use super::plan as place;
    use super::*;
    use serde::Deserialize;

    fn gpu(index: u32, backend: &str, memory_mib: u64) -> Accelerator {
        Accelerator {
            index,
            backend: backend.into(),
            memory_mib,
            ..Accelerator::default()
        }
    }

    fn cuda(count: u32) -> Vec<Accelerator> {
        (0..count).map(|i| gpu(i, "cuda", 24_576)).collect()
    }

    fn machine(id: &str, accelerators: Vec<Accelerator>, fabric: &[&str]) -> Machine {
        Machine {
            id: id.into(),
            online: true,
            trainer_ok: true,
            accelerators,
            fabric: fabric.iter().map(|addr| addr.to_string()).collect(),
        }
    }

    fn reach(pairs: &[(&str, &str, f32)]) -> Reach {
        let mut reach = Reach::default();
        for &(a, b, ms) in pairs {
            reach.set(a, b, Some(ms));
        }
        reach
    }

    fn req(min: u32, max: u32) -> Request {
        Request {
            min_devices: min,
            max_devices: max,
            ..Request::default()
        }
    }

    fn expect_plan(result: Result<Plan, PlanError>) -> Plan {
        result.unwrap_or_else(|err| panic!("expected a plan, got: {err}"))
    }

    fn expect_err(result: Result<Plan, PlanError>) -> PlanError {
        match result {
            Ok(plan) => panic!("expected an error, got {plan:?}"),
            Err(err) => err,
        }
    }

    /// (machine id, device indexes) of an island, in plan order.
    fn shape(island: &Island) -> Vec<(&str, Vec<u32>)> {
        island
            .machines
            .iter()
            .map(|m| (m.id.as_str(), m.devices.clone()))
            .collect()
    }

    /// Three machines on one LAN: 2 + 2 + 1 CUDA devices.
    fn lan_trio() -> Vec<Machine> {
        vec![
            machine("a", cuda(2), &["192.168.1.10"]),
            machine("b", cuda(2), &["192.168.1.11"]),
            machine("c", cuda(1), &["192.168.1.12"]),
        ]
    }

    fn lan_trio_reach() -> Reach {
        reach(&[("a", "b", 0.4), ("a", "c", 0.5), ("b", "c", 0.6)])
    }

    /// An NVIDIA LAN pair (2 + 1), a Mac (1 mps) and an AMD box (2 rocm), on separate networks.
    fn nvidia_mac_amd() -> (Vec<Machine>, Reach) {
        let machines = vec![
            machine("a", cuda(2), &["10.0.0.1"]),
            machine("b", cuda(1), &["10.0.0.2"]),
            machine("m", vec![gpu(0, "mps", 36_864)], &["203.0.113.5"]),
            machine(
                "r",
                vec![gpu(0, "rocm", 24_576), gpu(1, "rocm", 24_576)],
                &["198.51.100.7"],
            ),
        ];
        let reach = reach(&[
            ("a", "b", 0.3),
            ("a", "m", 80.0),
            ("a", "r", 120.0),
            ("b", "m", 80.0),
            ("b", "r", 120.0),
            ("m", "r", 150.0),
        ]);
        (machines, reach)
    }

    #[test]
    fn single_gpu_is_single() {
        let machines = vec![machine("a", cuda(1), &["10.0.0.1"])];
        let plan = expect_plan(place(&machines, &Reach::default(), &req(1, 8)));
        assert_eq!(plan.mode, Mode::Single);
        assert_eq!(plan.islands.len(), 1);
        assert_eq!(shape(&plan.islands[0]), vec![("a", vec![0])]);
        assert_eq!(plan.islands[0].rdzv, None);
        assert_eq!(plan.devices_total, 1);
        assert_eq!(plan.staleness, 0);
    }

    #[test]
    fn four_gpus_in_one_box_are_one_tight_island() {
        let machines = vec![machine("a", cuda(4), &["10.0.0.1"])];
        let plan = expect_plan(place(&machines, &Reach::default(), &req(1, 8)));
        assert_eq!(plan.mode, Mode::Tight);
        assert_eq!(plan.islands.len(), 1);
        assert_eq!(shape(&plan.islands[0]), vec![("a", vec![0, 1, 2, 3])]);
        assert_eq!(plan.islands[0].leader, "a");
        assert_eq!(plan.islands[0].rdzv, None);
        assert_eq!(plan.inner_steps, INNER_STEPS);
    }

    #[test]
    fn lan_trio_is_tight_across_machines_led_by_a_two_gpu_box() {
        let plan = expect_plan(place(&lan_trio(), &lan_trio_reach(), &req(5, 8)));
        assert_eq!(plan.mode, Mode::Tight);
        assert_eq!(plan.devices_total, 5);
        let island = &plan.islands[0];
        assert_eq!(island.backend, "cuda");
        assert_eq!(
            shape(island),
            vec![("a", vec![0, 1]), ("b", vec![0, 1]), ("c", vec![0])]
        );
        assert_eq!(island.leader, "a");
        assert_eq!(island.rdzv.as_deref(), Some("192.168.1.10:29400"));
    }

    #[test]
    fn max_devices_caps_a_tight_island_from_the_largest_member() {
        let plan = expect_plan(place(&lan_trio(), &lan_trio_reach(), &req(3, 3)));
        assert_eq!(plan.mode, Mode::Tight);
        assert_eq!(plan.devices_total, 3);
        assert_eq!(
            shape(&plan.islands[0]),
            vec![("a", vec![0, 1]), ("b", vec![0])]
        );
        assert_eq!(plan.islands[0].leader, "a");
    }

    #[test]
    fn plan_does_not_depend_on_machine_order() {
        let forward = expect_plan(place(&lan_trio(), &lan_trio_reach(), &req(5, 8)));
        let mut reversed_machines = lan_trio();
        reversed_machines.reverse();
        let reversed = expect_plan(place(&reversed_machines, &lan_trio_reach(), &req(5, 8)));
        assert_eq!(forward, reversed);
    }

    #[test]
    fn nvidia_lan_pair_mac_and_amd_over_wan_is_loose_with_three_islands() {
        let (machines, reach) = nvidia_mac_amd();
        let plan = expect_plan(place(&machines, &reach, &req(6, 8)));
        assert_eq!(plan.mode, Mode::Loose);
        assert_eq!(plan.devices_total, 6);
        assert_eq!(plan.staleness, LOOSE_STALENESS);
        let backends: Vec<&str> = plan.islands.iter().map(|i| i.backend.as_str()).collect();
        assert_eq!(backends, vec!["cuda", "rocm", "mps"]);

        let cuda_island = &plan.islands[0];
        assert_eq!(cuda_island.id, "i1");
        assert_eq!(shape(cuda_island), vec![("a", vec![0, 1]), ("b", vec![0])]);
        assert_eq!(cuda_island.rdzv.as_deref(), Some("10.0.0.1:29400"));
        assert_eq!(plan.islands[1].rdzv, None);
        assert_eq!(plan.islands[2].rdzv, None);
        assert_eq!(shape(&plan.islands[2]), vec![("m", vec![0])]);
    }

    #[test]
    fn same_fleet_with_a_low_minimum_is_tight_on_the_biggest_island() {
        let (machines, reach) = nvidia_mac_amd();
        let plan = expect_plan(place(&machines, &reach, &req(2, 8)));
        assert_eq!(plan.mode, Mode::Tight);
        assert_eq!(plan.islands.len(), 1);
        assert_eq!(plan.devices_total, 3);
    }

    #[test]
    fn pinned_loose_wins_over_a_tight_fit() {
        let mut machines = lan_trio();
        machines.push(machine("m", vec![gpu(0, "mps", 36_864)], &["203.0.113.5"]));
        let mut reach = lan_trio_reach();
        reach.set("a", "m", Some(80.0));
        reach.set("b", "m", Some(80.0));
        reach.set("c", "m", Some(80.0));
        let request = Request {
            mode: Some(Mode::Loose),
            ..req(2, 8)
        };
        let plan = expect_plan(place(&machines, &reach, &request));
        assert_eq!(plan.mode, Mode::Loose);
        let backends: Vec<&str> = plan.islands.iter().map(|i| i.backend.as_str()).collect();
        assert_eq!(backends, vec!["cuda", "mps"]);
    }

    #[test]
    fn two_lan_clusters_over_wan_are_two_tight_islands() {
        let machines = vec![
            machine("a", cuda(2), &["10.1.0.1"]),
            machine("b", cuda(2), &["10.1.0.2"]),
            machine("c", cuda(2), &["10.2.0.1"]),
            machine("d", cuda(2), &["10.2.0.2"]),
        ];
        let reach = reach(&[
            ("a", "b", 0.3),
            ("c", "d", 0.3),
            ("a", "c", 40.0),
            ("a", "d", 40.0),
            ("b", "c", 40.0),
            ("b", "d", 40.0),
        ]);
        let plan = expect_plan(place(&machines, &reach, &req(5, 8)));
        assert_eq!(plan.mode, Mode::Loose);
        assert_eq!(plan.islands.len(), 2);
        assert_eq!(plan.devices_total, 8);
        assert_eq!(
            shape(&plan.islands[0]),
            vec![("a", vec![0, 1]), ("b", vec![0, 1])]
        );
        assert_eq!(plan.islands[0].rdzv.as_deref(), Some("10.1.0.1:29400"));
        assert_eq!(
            shape(&plan.islands[1]),
            vec![("c", vec![0, 1]), ("d", vec![0, 1])]
        );
        assert_eq!(plan.islands[1].rdzv.as_deref(), Some("10.2.0.1:29400"));
    }

    #[test]
    fn busy_and_low_memory_devices_are_skipped() {
        let mut busy = gpu(0, "cuda", 24_576);
        busy.busy = true;
        let small = gpu(1, "cuda", 8_192);
        let mut full = gpu(2, "cuda", 24_576);
        full.mem_used_mib = 23_000;
        let machines = vec![machine(
            "a",
            vec![
                busy,
                small,
                full,
                gpu(3, "cuda", 24_576),
                gpu(4, "cuda", 24_576),
            ],
            &["10.0.0.1"],
        )];
        let request = Request {
            min_memory_mib: 16_384,
            ..req(1, 8)
        };
        let plan = expect_plan(place(&machines, &Reach::default(), &request));
        assert_eq!(plan.mode, Mode::Tight);
        assert_eq!(shape(&plan.islands[0]), vec![("a", vec![3, 4])]);
    }

    #[test]
    fn backend_filter_keeps_only_allowed_families() {
        let machines = vec![machine(
            "a",
            vec![
                gpu(0, "cuda", 24_576),
                gpu(1, "cuda", 24_576),
                gpu(2, "rocm", 24_576),
                gpu(3, "rocm", 24_576),
            ],
            &["10.0.0.1"],
        )];
        let request = Request {
            backends: Some(vec!["rocm".into()]),
            ..req(1, 8)
        };
        let plan = expect_plan(place(&machines, &Reach::default(), &request));
        assert_eq!(plan.islands[0].backend, "rocm");
        assert_eq!(shape(&plan.islands[0]), vec![("a", vec![2, 3])]);
    }

    #[test]
    fn pinned_machines_restrict_the_plan() {
        let request = Request {
            machines: Some(vec!["b".into(), "c".into()]),
            ..req(3, 8)
        };
        let plan = expect_plan(place(&lan_trio(), &lan_trio_reach(), &request));
        assert_eq!(plan.mode, Mode::Tight);
        assert_eq!(
            shape(&plan.islands[0]),
            vec![("b", vec![0, 1]), ("c", vec![0])]
        );
        assert_eq!(plan.islands[0].leader, "b");
    }

    #[test]
    fn pinned_tight_that_cannot_fit_names_the_largest_island() {
        let machines = vec![
            machine("a", cuda(2), &["10.0.0.1"]),
            machine("b", cuda(1), &["10.0.0.2"]),
        ];
        let reach = reach(&[("a", "b", 50.0)]);
        let request = Request {
            mode: Some(Mode::Tight),
            ..req(3, 8)
        };
        let err = expect_err(place(&machines, &reach, &request));
        match &err {
            PlanError::Infeasible { mode, reason } => {
                assert_eq!(*mode, Mode::Tight);
                assert!(reason.contains("needs 3 devices"), "{reason}");
                assert!(
                    reason.contains("largest island has 2 (cuda on a)"),
                    "{reason}"
                );
            }
            other => panic!("expected Infeasible, got {other:?}"),
        }
        assert!(err.to_string().starts_with("tight placement is infeasible"));
        // Unpinned, the same fleet runs loose on the two islands.
        let auto = expect_plan(place(&machines, &reach, &req(3, 8)));
        assert_eq!(auto.mode, Mode::Loose);
    }

    #[test]
    fn pinned_loose_on_one_island_is_infeasible() {
        let machines = vec![machine("a", cuda(4), &["10.0.0.1"])];
        let request = Request {
            mode: Some(Mode::Loose),
            ..req(1, 8)
        };
        let err = expect_err(place(&machines, &Reach::default(), &request));
        assert!(matches!(
            err,
            PlanError::Infeasible {
                mode: Mode::Loose,
                ..
            }
        ));
    }

    #[test]
    fn pinned_single_takes_the_device_with_the_most_memory() {
        let machines = vec![
            machine("a", vec![gpu(0, "cuda", 24_576)], &["10.0.0.1"]),
            machine("b", vec![gpu(0, "cuda", 49_152)], &["10.0.0.2"]),
        ];
        let request = Request {
            mode: Some(Mode::Single),
            ..req(1, 4)
        };
        let plan = expect_plan(place(&machines, &Reach::default(), &request));
        assert_eq!(plan.mode, Mode::Single);
        assert_eq!(shape(&plan.islands[0]), vec![("b", vec![0])]);
        // max_devices 1 picks single on its own.
        let one = expect_plan(place(&machines, &Reach::default(), &req(1, 1)));
        assert_eq!(one.mode, Mode::Single);
        assert_eq!(shape(&one.islands[0]), vec![("b", vec![0])]);
    }

    #[test]
    fn pinned_single_with_a_larger_minimum_is_infeasible() {
        let machines = vec![machine("a", cuda(2), &["10.0.0.1"])];
        let request = Request {
            mode: Some(Mode::Single),
            ..req(2, 4)
        };
        let err = expect_err(place(&machines, &Reach::default(), &request));
        assert!(matches!(
            err,
            PlanError::Infeasible {
                mode: Mode::Single,
                ..
            }
        ));
    }

    #[test]
    fn cpu_devices_are_used_only_when_allowed_and_nothing_else_is_eligible() {
        let machines = vec![machine(
            "a",
            vec![gpu(0, "cpu", 32_768), gpu(1, "cpu", 32_768)],
            &["10.0.0.1"],
        )];
        let refused = expect_err(place(&machines, &Reach::default(), &req(1, 4)));
        assert!(matches!(refused, PlanError::NoDevices { .. }));

        let allowed = Request {
            allow_cpu: true,
            ..req(1, 4)
        };
        let plan = expect_plan(place(&machines, &Reach::default(), &allowed));
        assert_eq!(plan.islands[0].backend, "cpu");
        assert_eq!(shape(&plan.islands[0]), vec![("a", vec![0, 1])]);
    }

    #[test]
    fn cpu_is_never_mixed_with_gpus() {
        let mut cpu = (0..4)
            .map(|i| gpu(i + 1, "cpu", 32_768))
            .collect::<Vec<_>>();
        let mut accelerators = vec![gpu(0, "cuda", 24_576)];
        accelerators.append(&mut cpu);
        let machines = vec![machine("a", accelerators, &["10.0.0.1"])];
        let request = Request {
            allow_cpu: true,
            ..req(1, 8)
        };
        let plan = expect_plan(place(&machines, &Reach::default(), &request));
        assert_eq!(plan.mode, Mode::Single);
        assert_eq!(plan.islands[0].backend, "cuda");
        assert_eq!(plan.devices_total, 1);
    }

    #[test]
    fn mps_and_cpu_islands_never_span_machines() {
        let machines = vec![
            machine("m1", vec![gpu(0, "mps", 16_384)], &["10.0.0.1"]),
            machine("m2", vec![gpu(0, "mps", 16_384)], &["10.0.0.2"]),
        ];
        let lan = reach(&[("m1", "m2", 0.5)]);
        let plan = expect_plan(place(&machines, &lan, &req(2, 4)));
        assert_eq!(plan.mode, Mode::Loose);
        assert_eq!(plan.islands.len(), 2);
        assert!(
            plan.islands
                .iter()
                .all(|i| i.machines.len() == 1 && i.rdzv.is_none())
        );

        let cpus = vec![
            machine("c1", vec![gpu(0, "cpu", 8_192)], &["10.0.0.3"]),
            machine("c2", vec![gpu(0, "cpu", 8_192)], &["10.0.0.4"]),
        ];
        let cpu_lan = reach(&[("c1", "c2", 0.5)]);
        let allowed = Request {
            allow_cpu: true,
            ..req(2, 4)
        };
        let plan = expect_plan(place(&cpus, &cpu_lan, &allowed));
        assert_eq!(plan.mode, Mode::Loose);
        assert_eq!(plan.islands.len(), 2);
    }

    #[test]
    fn max_devices_caps_the_plan() {
        let machines = vec![machine("a", cuda(8), &["10.0.0.1"])];
        let plan = expect_plan(place(&machines, &Reach::default(), &req(1, 3)));
        assert_eq!(plan.devices_total, 3);
        assert_eq!(shape(&plan.islands[0]), vec![("a", vec![0, 1, 2])]);
    }

    #[test]
    fn no_devices_is_an_error() {
        let none = expect_err(place(&[], &Reach::default(), &req(1, 4)));
        assert!(matches!(none, PlanError::NoDevices { .. }));

        let mut offline = machine("a", cuda(2), &["10.0.0.1"]);
        offline.online = false;
        let mut no_trainer = machine("b", cuda(2), &["10.0.0.2"]);
        no_trainer.trainer_ok = false;
        let err = expect_err(place(&[offline, no_trainer], &Reach::default(), &req(1, 4)));
        assert!(matches!(err, PlanError::NoDevices { .. }), "{err}");
    }

    #[test]
    fn too_few_devices_is_not_enough() {
        let machines = vec![machine("a", cuda(2), &["10.0.0.1"])];
        let err = expect_err(place(&machines, &Reach::default(), &req(3, 8)));
        assert_eq!(
            err,
            PlanError::NotEnough {
                available: 2,
                min_devices: 3
            }
        );
    }

    #[test]
    fn malformed_requests_are_rejected() {
        let machines = vec![machine("a", cuda(2), &["10.0.0.1"])];
        let zero_max = expect_err(place(&machines, &Reach::default(), &req(1, 0)));
        assert!(matches!(zero_max, PlanError::BadRequest { .. }));
        let inverted = expect_err(place(&machines, &Reach::default(), &req(4, 2)));
        assert!(matches!(inverted, PlanError::BadRequest { .. }));
    }

    #[test]
    fn machines_without_fabric_or_unreachable_pairs_do_not_join() {
        let no_fabric = vec![machine("a", cuda(2), &[]), machine("b", cuda(2), &[])];
        let close = reach(&[("a", "b", 0.3)]);
        let plan_a = expect_plan(place(&no_fabric, &close, &req(3, 8)));
        assert_eq!(plan_a.mode, Mode::Loose);
        assert_eq!(plan_a.islands.len(), 2);

        let fabric = vec![
            machine("a", cuda(2), &["10.0.0.1"]),
            machine("b", cuda(2), &["10.0.0.2"]),
        ];
        let mut unreachable = Reach::default();
        unreachable.set("a", "b", None);
        let plan_b = expect_plan(place(&fabric, &unreachable, &req(3, 8)));
        assert_eq!(plan_b.mode, Mode::Loose);
        assert_eq!(plan_b.islands.len(), 2);
    }

    #[test]
    fn ipv6_rendezvous_is_bracketed() {
        let machines = vec![
            machine("a", cuda(2), &["fd00::1"]),
            machine("b", cuda(2), &["fd00::2"]),
        ];
        let lan = reach(&[("a", "b", 0.2)]);
        let plan = expect_plan(place(&machines, &lan, &req(3, 8)));
        assert_eq!(plan.mode, Mode::Tight);
        assert_eq!(plan.islands[0].rdzv.as_deref(), Some("[fd00::1]:29400"));
    }

    /// The wire shape: accelerators with missing optional fields, the reach list, and a request.
    const FIXTURE: &str = r#"{
        "machines": [
            {"id": "n0", "online": true, "trainer_ok": true, "fabric": ["192.168.1.20"],
             "accelerators": [
                {"index": 0, "vendor": "nvidia", "name": "RTX 3090", "memory_mib": 24576,
                 "backend": "cuda", "link": "pcie"},
                {"index": 1, "vendor": "nvidia", "memory_mib": 24576, "backend": "cuda", "busy": true}
             ]},
            {"id": "n3", "online": true, "trainer_ok": true, "fabric": ["192.168.1.21"],
             "accelerators": [{"index": 0, "memory_mib": 24576, "backend": "cuda"}]},
            {"id": "n5", "online": true, "trainer_ok": true, "fabric": [],
             "accelerators": [{"index": 0, "memory_mib": 36864, "backend": "mps"}]}
        ],
        "reach": [
            {"a": "n0", "b": "n3", "rtt_ms": 0.4},
            {"a": "n0", "b": "n5", "rtt_ms": 80.0},
            {"a": "n3", "b": "n5", "rtt_ms": null}
        ],
        "request": {"min_devices": 3, "max_devices": 8}
    }"#;

    #[derive(Deserialize)]
    struct Fixture {
        machines: Vec<Machine>,
        reach: Reach,
        request: Request,
    }

    #[test]
    fn wire_fixture_plans_and_serialises() {
        let fixture: Fixture = serde_json::from_str(FIXTURE).expect("fixture parses");
        let plan = expect_plan(place(&fixture.machines, &fixture.reach, &fixture.request));
        assert_eq!(plan.mode, Mode::Loose);
        assert_eq!(plan.devices_total, 3);
        assert_eq!(
            shape(&plan.islands[0]),
            vec![("n0", vec![0]), ("n3", vec![0])]
        );
        assert_eq!(plan.islands[0].rdzv.as_deref(), Some("192.168.1.20:29400"));
        assert_eq!(shape(&plan.islands[1]), vec![("n5", vec![0])]);
        assert_eq!(plan.islands[1].rdzv, None);

        let json = serde_json::to_value(&plan).expect("plan serialises");
        assert_eq!(json["mode"], "loose");
        assert_eq!(json["islands"][0]["id"], "i1");
        assert_eq!(json["islands"][0]["leader"], "n0");
        assert_eq!(json["staleness"], 2);
        let back: Plan = serde_json::from_value(json).expect("plan round-trips");
        assert_eq!(back, plan);
    }

    #[test]
    fn reach_round_trips_through_its_wire_form() {
        let original = reach(&[("b", "a", 1.5)]);
        let wire = serde_json::to_value(&original).expect("reach serialises");
        let back: Reach = serde_json::from_value(wire).expect("reach parses");
        assert_eq!(back.rtt_ms("a", "b"), Some(1.5));
        assert_eq!(back.rtt_ms("b", "a"), Some(1.5));
        assert_eq!(back.rtt_ms("a", "c"), None);
    }
}
