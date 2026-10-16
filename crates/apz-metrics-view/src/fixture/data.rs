//! The fixture plaza's shape and its generated series.
//!
//! A value is a function of the metric, the leaf (an entity, or an entity split by a label), the minute and the
//! seed text, so a read repeated gives the same numbers and two windows agree wherever they overlap.

use std::f64::consts::PI;

use apz_metrics::catalog::{self, MetricDef, MetricKind, Unit};
use apz_metrics::query::AggFn;
use apz_metrics::{EntityKind, HistSnapshot};

use super::Fixture;
use crate::model::{ScopeNode, Status};
use crate::source::{AgentInfo, Entity, Group, GroupBy, Read};

const MINUTE_MS: u64 = 60_000;
const DAY_MS: u64 = 86_400_000;
const HOUR_MS: u64 = 3_600_000;
/// n2 was offline from 13.5 hours before now until 13.27 hours before now (about 14 minutes).
const OFFLINE_FROM_AGO_MS: u64 = 48_600_000;
const OFFLINE_TO_AGO_MS: u64 = 47_772_000;
/// Values drawn into each histogram step.
const SAMPLES_PER_STEP: u64 = 200;
/// Spread of the log-normal histogram samples: the p95 comes out near 2.3 times the median.
const HIST_SIGMA: f64 = 0.5;

/// One agent of the fixture: its place, its model, and its share of the plaza's model calls.
struct Roster {
    cid: u64,
    name: &'static str,
    role: &'static str,
    run: &'static str,
    provider: &'static str,
    model: &'static str,
    window: u64,
    fill: f64,
    status: &'static str,
    share: f64,
}

const AGENTS: &[Roster] = &[
    Roster {
        cid: 4417,
        name: "ada",
        role: "builder",
        run: "r12",
        provider: "anthropic",
        model: "opus-5.5",
        window: 200_000,
        fill: 93.0,
        status: "working",
        share: 0.30,
    },
    Roster {
        cid: 4418,
        name: "kit",
        role: "builder",
        run: "r12",
        provider: "anthropic",
        model: "sonnet-5.5",
        window: 200_000,
        fill: 61.0,
        status: "working",
        share: 0.20,
    },
    Roster {
        cid: 4421,
        name: "rhea",
        role: "reviewer",
        run: "r12",
        provider: "anthropic",
        model: "sonnet-5.5",
        window: 200_000,
        fill: 48.0,
        status: "idle",
        share: 0.12,
    },
    Roster {
        cid: 4430,
        name: "juno",
        role: "researcher",
        run: "r15",
        provider: "openai",
        model: "gpt-5",
        window: 400_000,
        fill: 77.0,
        status: "working",
        share: 0.18,
    },
    Roster {
        cid: 4431,
        name: "otto",
        role: "ops",
        run: "r15",
        provider: "anthropic",
        model: "haiku-5.5",
        window: 200_000,
        fill: 22.0,
        status: "idle",
        share: 0.05,
    },
    Roster {
        cid: 4440,
        name: "mira",
        role: "builder",
        run: "r15",
        provider: "local",
        model: "qwen3-32b",
        window: 32_000,
        fill: 35.0,
        status: "asleep",
        share: 0.15,
    },
];
const NODES: &[(&str, &str)] = &[
    ("n0", "n0 · hub box"),
    ("n1", "n1 · rack-a"),
    ("n2", "n2 · rack-b"),
];
const RUNS: &[(&str, &str)] = &[("r12", "n1"), ("r15", "n1"), ("r19", "n2")];
/// (run, index, state) of each VM.
const VMS: &[(&str, u64, &str)] = &[
    ("r12", 1, "running"),
    ("r12", 2, "running"),
    ("r12", 3, "asleep"),
    ("r12", 4, "running"),
    ("r15", 1, "running"),
    ("r15", 2, "running"),
    ("r15", 3, "running"),
    ("r19", 1, "running"),
    ("r19", 2, "running"),
    ("r19", 3, "running"),
    ("r19", 4, "paused"),
    ("r19", 5, "running"),
];
const ROLES: &[&str] = &["builder", "reviewer", "researcher", "ops"];
const PROVIDERS: &[&str] = &["anthropic", "openai", "local"];
/// (provider, model) of each model the fixture calls.
const MODELS: &[(&str, &str)] = &[
    ("anthropic", "opus-5.5"),
    ("anthropic", "sonnet-5.5"),
    ("anthropic", "haiku-5.5"),
    ("openai", "gpt-5"),
    ("local", "qwen3-32b"),
];
/// (id, label, node, used percent).
const VOLUMES: &[(&str, &str, &str, f64)] = &[
    ("n1:/var/lib/plaza", "n1 /var/lib/plaza", "n1", 54.0),
    ("n2:/var/lib/plaza", "n2 /var/lib/plaza", "n2", 91.0),
    ("n0:postgres", "n0 postgres", "n0", 38.0),
    ("n0:garage", "n0 garage", "n0", 46.0),
];
/// (id, label, node). The stores of the hub's node are services too; their ids are `store:<label>`.
const SERVICES: &[(&str, &str, &str)] = &[
    ("n0:apid", "apid (hub)", "n0"),
    ("n0:forgejo", "forgejo (hub)", "n0"),
    ("n1:noded", "noded (n1)", "n1"),
    ("n2:noded", "noded (n2)", "n2"),
    ("store:garage (objects)", "garage (objects)", "n0"),
    ("store:forgejo (git)", "forgejo (git)", "n0"),
    ("store:postgres · plaza", "postgres · plaza", "n0"),
    (
        "store:postgres · metrics chunks",
        "postgres · metrics chunks",
        "n0",
    ),
    ("store:postgres · metrics 1m", "postgres · metrics 1m", "n0"),
    ("store:postgres · metrics 1h", "postgres · metrics 1h", "n0"),
    (
        "store:postgres · model_calls",
        "postgres · model_calls",
        "n0",
    ),
    (
        "store:postgres · trace_spans",
        "postgres · trace_spans",
        "n0",
    ),
];
const GIB: f64 = 1_073_741_824.0;
const MB: f64 = 1_000_000.0;
/// (id, bytes held) of each store.
const STORES: &[(&str, f64)] = &[
    ("store:garage (objects)", 180.0 * GIB),
    ("store:forgejo (git)", 6.0 * GIB),
    ("store:postgres · plaza", 850.0 * MB),
    ("store:postgres · metrics chunks", 640.0 * MB),
    ("store:postgres · metrics 1m", 210.0 * MB),
    ("store:postgres · metrics 1h", 34.0 * MB),
    ("store:postgres · model_calls", 96.0 * MB),
    ("store:postgres · trace_spans", 380.0 * MB),
];
const TOOLS: &[(&str, f64)] = &[
    ("bash", 0.35),
    ("edit_file", 0.20),
    ("read_file", 0.25),
    ("web_fetch", 0.08),
    ("hub.git.push", 0.05),
    ("mcp:linear.search", 0.07),
];
const ROUTES: &[(&str, f64)] = &[
    ("POST /v1/llm/*", 0.50),
    ("POST /v1/events", 0.25),
    ("GET /v1/metrics/view", 0.15),
    ("POST /v1/work", 0.10),
];
const ENGINES: &[(&str, f64)] = &[("claude", 0.70), ("codex", 0.25), ("local", 0.05)];
const LAWS: &[(&str, f64)] = &[("secret-scan", 0.5), ("budget", 0.3), ("scope", 0.2)];
/// The states the hub books agent time to (crates/apid/src/metrics/collect.rs `agent_state`), with their shares.
const AGENT_STATES: &[(&str, f64)] = &[
    ("streaming", 0.16),
    ("first_token", 0.05),
    ("tools", 0.13),
    ("approval", 0.03),
    ("idle", 0.48),
    ("asleep", 0.15),
];
const PARTS: &[(&str, f64)] = &[
    ("system", 0.10),
    ("tools", 0.14),
    ("history", 0.46),
    ("tool_results", 0.18),
    ("attachments", 0.06),
    ("summary", 0.06),
];

/// The scope tree, in order.
pub fn scopes() -> Vec<ScopeNode> {
    let mut out = vec![scope_node("global", "Global", "", 0, "", None)];
    for &(node, label) in NODES {
        let status = if node == "n2" {
            Status::Crit
        } else {
            Status::Ok
        };
        out.push(scope_node(
            &format!("node:{node}"),
            label,
            "Nodes",
            0,
            "node",
            Some(status),
        ));
        for &(run, _) in RUNS.iter().filter(|r| r.1 == node) {
            out.push(scope_node(
                &format!("run:{run}"),
                &format!("run {run}"),
                "Nodes",
                1,
                "run",
                None,
            ));
            for &(vm_run, idx, state) in VMS.iter().filter(|v| v.0 == run) {
                let status = match state {
                    "asleep" => Status::Mute,
                    "paused" => Status::Info,
                    _ => Status::Ok,
                };
                out.push(scope_node(
                    &format!("vm:{vm_run}/{idx}"),
                    &format!("VM {vm_run}/{idx}"),
                    "Nodes",
                    2,
                    "vm",
                    Some(status),
                ));
            }
        }
        if node == "n1" {
            out.push(scope_node(
                "ws:design",
                "design workstation",
                "Nodes",
                1,
                "qemu",
                None,
            ));
        }
        if node == "n2" {
            out.push(scope_node(
                "sb:7f3a",
                "sandbox 7f3a",
                "Nodes",
                1,
                "sandbox",
                None,
            ));
        }
    }
    for &role in ROLES {
        out.push(scope_node(
            &format!("role:{role}"),
            role,
            "Agents · by role",
            0,
            "group",
            None,
        ));
        for a in AGENTS.iter().filter(|a| a.role == role) {
            let status = if a.name == "ada" {
                Status::Warn
            } else {
                Status::Ok
            };
            out.push(scope_node(
                &format!("agent:{}", a.cid),
                &format!("{} ({})", a.name, a.cid),
                "Agents · by role",
                1,
                "",
                Some(status),
            ));
        }
    }
    for &provider in PROVIDERS {
        out.push(scope_node(
            &format!("provider:{provider}"),
            provider,
            "Providers",
            0,
            "group",
            None,
        ));
        for &(p, model) in MODELS.iter().filter(|m| m.0 == provider) {
            let full = format!("{p}/{model}");
            let status = if full == "anthropic/opus-5.5" {
                Some(Status::Warn)
            } else {
                None
            };
            out.push(scope_node(
                &format!("model:{full}"),
                model,
                "Providers",
                1,
                "",
                status,
            ));
        }
    }
    out
}

fn scope_node(
    id: &str,
    label: &str,
    group: &str,
    depth: u8,
    tag: &str,
    status: Option<Status>,
) -> ScopeNode {
    ScopeNode {
        id: id.into(),
        label: label.into(),
        kind: id.split(':').next().unwrap_or("").into(),
        group: group.into(),
        depth,
        tag: tag.into(),
        status,
    }
}

/// Entities of a kind under a scope.
pub fn entities(scope: &str, kind: &str) -> Vec<Entity> {
    members(scope, kind)
        .into_iter()
        .map(|m| Entity {
            id: m.id,
            label: m.label,
            state: m.state,
        })
        .collect()
}

/// An agent of the fixture.
pub fn agent(cid: u64) -> Option<AgentInfo> {
    AGENTS.iter().find(|a| a.cid == cid).map(|a| AgentInfo {
        cid,
        name: a.name.into(),
        role: a.role.into(),
        run: a.run.into(),
        node: run_node(a.run).into(),
        provider: a.provider.into(),
        model: a.model.into(),
        window: Some(a.window),
        status: a.status.into(),
    })
}

/// Grouped series of a read.
pub fn series(f: &Fixture, read: &Read) -> Vec<Group> {
    let Some(metric) = catalog::find(&read.metric) else {
        return Vec::new();
    };
    let all = leaves(metric, &read.scope);
    if all.is_empty() {
        return Vec::new();
    }
    let count = steps(read.from_ms, read.to_ms, read.step_ms);
    let bucket = counter_bucket_ms(read.to_ms.saturating_sub(read.from_ms));
    let groups: Vec<Group> = partition(&read.group, &read.scope, &all)
        .into_iter()
        .map(
            |Part {
                 key: _,
                 name,
                 scope,
                 leaves: members,
             }| Group {
                name,
                scope,
                values: (0..count)
                    .map(|i| {
                        let t = step_mid(read.from_ms, read.step_ms, i);
                        let from = read.from_ms + i * read.step_ms;
                        let value = if is_integrated(metric) {
                            counter_span(metric, &members, bucket, from, from + read.step_ms)
                        } else {
                            group_value(metric, &members, f.now_ms, t, read.step_ms)
                        };
                        value.map(|v| reduce(read.agg, metric, read.step_ms, v))
                    })
                    .collect(),
            },
        )
        .collect();
    match read.top {
        Some(top) => keep_top(groups, top),
        None => groups,
    }
}

/// One number for a metric over a window.
pub fn scalar(
    f: &Fixture,
    metric: &str,
    scope: &str,
    from_ms: u64,
    to_ms: u64,
    agg: AggFn,
) -> Option<f64> {
    let metric = catalog::find(metric)?;
    let all = leaves(metric, scope);
    let refs: Vec<&Leaf> = all.iter().collect();
    let step = scalar_step(from_ms, to_ms);
    let values: Vec<f64> = if is_integrated(metric) {
        let bucket = counter_bucket_ms(to_ms.saturating_sub(from_ms));
        counter_span(metric, &refs, bucket, from_ms, to_ms)
            .into_iter()
            .collect()
    } else {
        (0..steps(from_ms, to_ms, step))
            .filter_map(|i| group_value(metric, &refs, f.now_ms, step_mid(from_ms, step, i), step))
            .collect()
    };
    if values.is_empty() {
        return None;
    }
    // How many values a distribution holds: the fixture's events of it, a few per leaf an hour.
    if matches!(agg, AggFn::Count) && matches!(metric.kind, MetricKind::Histogram) {
        let hours = (to_ms - from_ms) as f64 / 3_600_000.0;
        return Some((refs.len() as f64 * hours * EVENTS_PER_LEAF_HOUR).round());
    }
    if is_counter(metric) {
        let total: f64 = values.iter().sum();
        let seconds = (to_ms - from_ms) as f64 / 1000.0;
        return Some(if matches!(agg, AggFn::Rate) {
            total / seconds
        } else {
            total
        });
    }
    let picked = match agg {
        AggFn::Min => values.iter().copied().fold(f64::INFINITY, f64::min),
        AggFn::Max => values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        _ => values.iter().sum::<f64>() / values.len() as f64,
    };
    Some(reduce(agg, metric, step, picked))
}

/// How many values one leaf of a distribution records an hour, for a count of them.
const EVENTS_PER_LEAF_HOUR: f64 = 4.0;

/// A distribution over a window.
pub fn hist(f: &Fixture, metric: &str, scope: &str, from_ms: u64, to_ms: u64) -> HistSnapshot {
    let step = (to_ms.saturating_sub(from_ms) / 48).max(MINUTE_MS);
    let mut out = HistSnapshot::default();
    for part in hist_steps(f, metric, scope, from_ms, to_ms, step) {
        out.merge(&part);
    }
    out
}

/// One distribution per step.
pub fn hist_steps(
    f: &Fixture,
    metric: &str,
    scope: &str,
    from_ms: u64,
    to_ms: u64,
    step_ms: u64,
) -> Vec<HistSnapshot> {
    let Some(metric) = catalog::find(metric) else {
        return Vec::new();
    };
    let all = leaves(metric, scope);
    let refs: Vec<&Leaf> = all.iter().collect();
    (0..steps(from_ms, to_ms, step_ms))
        .map(|i| {
            let t = step_mid(from_ms, step_ms, i);
            match group_value(metric, &refs, f.now_ms, t, step_ms) {
                Some(median) => sample_histogram(metric.id, scope, t, median),
                None => HistSnapshot::default(),
            }
        })
        .collect()
}

/// A deterministic value in [0, 1) for a seed and a bucket: FNV-1a over both, then one xorshift round.
fn noise(seed: &str, bucket: u64) -> f64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in seed.bytes().chain(bucket.to_le_bytes()) {
        h ^= u64::from(byte);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let mut x = h | 1;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    (x >> 11) as f64 / (1u64 << 53) as f64
}

/// The base level of a metric: per minute for a counter, the level itself for a gauge, the median for a histogram.
/// Metrics not listed have a base of 1, and their leaves carry the absolute level as their weight.
fn base_level(id: &str) -> f64 {
    match id {
        "llm.calls" => 6.0,
        "llm.errors" => 0.05,
        "llm.retries" => 0.03,
        "llm.ttft_ms" => 900.0,
        "llm.duration_ms" => 4_200.0,
        "llm.out_tps" => 60.0,
        "llm.tokens_in" => 30_000.0,
        "llm.tokens_cache_read" => 90_000.0,
        "llm.tokens_cache_write" => 8_000.0,
        "llm.tokens_out" => 9_000.0,
        "llm.spend_usd" | "llm.engine_spend_usd" => 1.6 / 60.0,
        "llm.ratelimit_requests_left_pct" => 70.0,
        "llm.ratelimit_tokens_left_pct" => 60.0,
        "ctx.compactions" => 0.3,
        "ctx.freed_tokens" => 900.0,
        "ctx.carried_tokens" => 1_200.0,
        "turn.duration_ms" => 9_000.0,
        "tool.duration_ms" => 300.0,
        "tool.calls" => 4.0,
        "tool.errors" | "tool.errors_by_agent" => 0.1,
        "agent.state_ms" => 60_000.0,
        "work.queued" => 2.0,
        "work.wait_ms" => 180.0,
        "work.done" => 3.0,
        "work.failed" => 0.05,
        "work.turns" => 6.0,
        "work.cost_usd" => 0.04,
        "approval.wait_ms" => 4_000.0,
        "approval.pending" => 0.6,
        "law.infractions" => 0.002,
        "host.mem_total_mib" => 64_000.0,
        "host.disk_read_bps" => 12e6,
        "host.disk_write_bps" => 8e6,
        "host.net_rx_bps" => 4e6,
        "host.net_tx_bps" => 2.5e6,
        "host.psi_cpu_pct" => 4.0,
        "host.psi_mem_pct" => 1.5,
        "host.psi_io_pct" => 2.0,
        "host.gpu_util_pct" => 35.0,
        "host.gpu_mem_pct" => 40.0,
        "volume.size_bytes" | "volume.used_bytes" => 1e11,
        "svc.cpu_pct" => 6.0,
        "svc.rss_mib" => 900.0,
        "vm.cpu_pct" => 22.0,
        "vm.rss_mib" => 2_400.0,
        "vm.guest_used_mib" => 1_800.0,
        "vm.guest_total_mib" => 4_096.0,
        "vm.blk_read_bps" => 2e6,
        "vm.blk_write_bps" => 1e6,
        "vm.blk_ops" => 300.0,
        "vm.vsock_rx_bps" | "vm.vsock_tx_bps" => 4e4,
        "vm.workspace_used_mib" => 6_000.0,
        "life.boot_ms" => 9_000.0,
        "life.resume_ms" => 1_800.0,
        "life.wake_ms" => 400.0,
        "life.wake_restore_ms" => 250.0,
        "life.wake_ready_ms" => 120.0,
        "life.pause_ms" => 300.0,
        "life.snapshot_ms" => 2_500.0,
        "life.snapshot_mib" => 1_500.0,
        "life.fork_ms" => 3_000.0,
        "life.sandbox_create_ms" => 2_200.0,
        "life.move_ms" => 6_000.0,
        "life.wake_cold" => 0.002,
        "hub.wake_queue" => 1.0,
        "vsock.sessions" => 3.4,
        "vsock.session_wakeups" => 7.5,
        "hub.http_ms" => 16.5,
        "hub.http_requests" => 240.0,
        "hub.http_errors" => 0.6,
        "hub.pg_ms" => 3.0,
        "hub.pg_in_use" => 6.0,
        "hub.bus_lag_ms" => 2.0,
        "hub.ws_clients" => 4.0,
        "hub.events" => 900.0,
        "link.beat_lag_ms" => 40.0,
        "link.rtt_ms" => 12.0,
        "link.clock_offset_ms" => 3.0,
        "link.commands_queued" => 1.0,
        "link.payload_bytes" => 2_000.0,
        "ext.latency_ms" => 180.0,
        "ext.errors" => 0.1,
        "self.cpu_pct" => 1.8,
        "self.mem_mb" => 240.0,
        "self.points_in" => 3_000.0,
        "self.series" => 14_000.0,
        "self.cache_hit_pct" => 92.0,
        "self.pg_bytes_day" => 2e9,
        "self.alert_eval_ms" => 3.0,
        "self.traces_bytes" => 5e7,
        _ => 1.0,
    }
}

/// How much of the daily swing and noise a metric carries: 0 for the steady ones (counts and capacities).
fn amplitude(id: &str) -> f64 {
    match id {
        "vm.count" | "vm.guest_total_mib" | "host.mem_total_mib" | "ctx.window"
        | "volume.size_bytes" | "store.bytes" => 0.0,
        "volume.used_pct" | "volume.used_bytes" => 0.2,
        _ => 1.0,
    }
}

/// The level of one leaf at a moment: the base times the leaf's weight, with the daily swing, the noise and the
/// occasional spike (a noise draw above 0.97 is 1.8 times the level). Percentages stay within 0 to 100.
fn level(metric: &MetricDef, leaf: &Leaf, t_ms: u64) -> f64 {
    if metric.id == "ctx.fill_pct" {
        return fill_level(leaf, t_ms);
    }
    let amp = amplitude(metric.id);
    let factor = if amp == 0.0 {
        1.0
    } else {
        let n = noise(&format!("{}|{}", metric.id, leaf.key), t_ms / MINUTE_MS);
        let phase = 2.0 * PI * (t_ms as f64 / DAY_MS as f64);
        let mut factor = 1.0 + amp * (0.25 * phase.sin() + 0.3 * (n - 0.5));
        if n > 0.97 {
            factor *= 1.8;
        }
        factor
    };
    let value = base_level(metric.id) * leaf.weight * factor * growth(metric.id, t_ms);
    if matches!(metric.unit, Unit::Percent) {
        value.clamp(0.0, 100.0)
    } else {
        value
    }
}

/// A context fill: the agent's fill, with a swing of up to 4 points over the day and a little noise. It stays
/// between 0 and 99, so a window never reads as full.
fn fill_level(leaf: &Leaf, t_ms: u64) -> f64 {
    let n = noise(&format!("ctx.fill_pct|{}", leaf.key), t_ms / MINUTE_MS);
    let phase = 2.0 * PI * (t_ms as f64 / DAY_MS as f64);
    (leaf.weight + 4.0 * (0.8 * phase.sin() + 0.2 * (2.0 * n - 1.0))).clamp(0.0, 99.0)
}

/// The instant a store's growth is counted from. Fixed, so a read repeated gives the same numbers.
const STORE_GROWTH_ORIGIN_MS: u64 = 1_700_000_000_000;
/// How much a store grows each day, as a fraction of its size.
const STORE_GROWTH_PER_DAY: f64 = 0.0002;

/// The growth of a level by the moment: a store holds a little more each day. Other metrics do not grow.
fn growth(id: &str, t_ms: u64) -> f64 {
    match id {
        "store.bytes" => {
            let days = t_ms.saturating_sub(STORE_GROWTH_ORIGIN_MS) as f64 / DAY_MS as f64;
            1.0 + STORE_GROWTH_PER_DAY * days
        }
        _ => 1.0,
    }
}

/// What one leaf contributes to a step: the level, or for a counter the amount over the step.
fn leaf_value(metric: &MetricDef, leaf: &Leaf, t_ms: u64, step_ms: u64) -> f64 {
    let rate = level(metric, leaf, t_ms);
    if is_counter(metric) {
        let amount = rate * step_ms as f64 / MINUTE_MS as f64;
        // A compaction or a cold wake is a whole event: a step counts whole ones, never a fraction of one.
        if metric.id == "life.wake_cold" {
            // Rare: a step holds one with the chance of its expected amount.
            let n = noise(&format!("cold|{}", leaf.key), t_ms / step_ms.max(1));
            if n < amount { 1.0 } else { 0.0 }
        } else if metric.id == "ctx.compactions" {
            amount.round()
        } else {
            amount
        }
    } else if matches!(metric.id, "hub.wake_queue" | "vsock.sessions") {
        // Things waiting and sessions open are whole.
        rate.round()
    } else {
        rate
    }
}

/// Whether a counter's amount is integrated over time buckets. The whole-event counters (a cold wake, a compaction)
/// keep their per-step count, since a step holds whole events and not a fraction of one.
fn is_integrated(metric: &MetricDef) -> bool {
    is_counter(metric) && !matches!(metric.id, "life.wake_cold" | "ctx.compactions")
}

/// The width of the buckets a counter is integrated over. The bucket grows with the window, so a long window stays
/// cheap, and the width depends only on the window: the series and the scalar of one window use the same buckets.
fn counter_bucket_ms(window_ms: u64) -> u64 {
    if window_ms <= DAY_MS {
        MINUTE_MS
    } else if window_ms <= 7 * DAY_MS {
        5 * MINUTE_MS
    } else {
        HOUR_MS
    }
}

/// What the leaves of a counter add up to over `[from_ms, to_ms)`. Each bucket of the grid that the span overlaps
/// adds its leaves' rates, taken at the bucket's middle, times the part of the bucket inside the span, in minutes.
/// The buckets sit on a fixed grid, so the steps of a window add up to its total.
fn counter_span(
    metric: &MetricDef,
    leaves: &[&Leaf],
    bucket_ms: u64,
    from_ms: u64,
    to_ms: u64,
) -> Option<f64> {
    if leaves.is_empty() {
        return None;
    }
    let mut total = 0.0;
    let mut bucket = from_ms - from_ms % bucket_ms;
    while bucket < to_ms {
        let lo = bucket.max(from_ms);
        let hi = (bucket + bucket_ms).min(to_ms);
        if hi > lo {
            let mid = bucket + bucket_ms / 2;
            let rate: f64 = leaves.iter().map(|l| level(metric, l, mid)).sum();
            total += rate * (hi - lo) as f64 / MINUTE_MS as f64;
        }
        bucket += bucket_ms;
    }
    Some(total)
}

/// The value of a group at a moment: the sum of its live leaves for an additive metric, their mean otherwise.
/// A leaf offline at that moment is left out; the group is a gap when every leaf is offline.
fn group_value(
    metric: &MetricDef,
    leaves: &[&Leaf],
    now: u64,
    t_ms: u64,
    step_ms: u64,
) -> Option<f64> {
    let values: Vec<f64> = leaves
        .iter()
        .filter(|l| !offline(metric, l, now, t_ms))
        .map(|l| leaf_value(metric, l, t_ms, step_ms))
        .collect();
    if values.is_empty() {
        return None;
    }
    let total: f64 = values.iter().sum();
    Some(if is_additive(metric) {
        total
    } else {
        total / values.len() as f64
    })
}

/// Turns a group's raw value into the aggregate asked for. Percentiles of a histogram scale its median; a counter
/// rate is an increase per second.
fn reduce(agg: AggFn, metric: &MetricDef, step_ms: u64, value: f64) -> f64 {
    if is_counter(metric) {
        return if matches!(agg, AggFn::Rate) {
            value / (step_ms as f64 / 1000.0)
        } else {
            value
        };
    }
    if !matches!(metric.kind, MetricKind::Histogram) {
        return value;
    }
    match agg {
        AggFn::P10 => value * 0.45,
        AggFn::P90 => value * 1.9,
        AggFn::P95 => value * 2.3,
        AggFn::P99 => value * 3.0,
        AggFn::Min => value * 0.3,
        AggFn::Max => value * 3.2,
        _ => value,
    }
}

/// One histogram step: about 200 log-normal samples around the median.
fn sample_histogram(metric_id: &str, scope: &str, t_ms: u64, median: f64) -> HistSnapshot {
    let seed = format!("{metric_id}|{scope}|hist");
    let base_bucket = (t_ms / MINUTE_MS) * 512;
    let mut out = HistSnapshot::default();
    for k in 0..SAMPLES_PER_STEP {
        let u1 = noise(&seed, base_bucket + 2 * k).max(1e-9);
        let u2 = noise(&seed, base_bucket + 2 * k + 1);
        let z = (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos();
        out.record(median * (HIST_SIGMA * z).exp());
    }
    out
}

/// Whether a leaf is offline at a moment: n2's host and VM series have a gap in the window from 13.5 to 13.27
/// hours before now.
fn offline(metric: &MetricDef, leaf: &Leaf, now: u64, t_ms: u64) -> bool {
    let host_or_vm = metric.id.starts_with("host.") || metric.id.starts_with("vm.");
    host_or_vm
        && leaf_attr(leaf, "node") == Some("n2")
        && t_ms >= now.saturating_sub(OFFLINE_FROM_AGO_MS)
        && t_ms < now.saturating_sub(OFFLINE_TO_AGO_MS)
}

fn is_counter(metric: &MetricDef) -> bool {
    matches!(metric.kind, MetricKind::Counter)
}

/// Whether the group values add up (a total) rather than average (a level).
fn is_additive(metric: &MetricDef) -> bool {
    is_counter(metric)
        || matches!(metric.unit, Unit::Bytes | Unit::Mib | Unit::Tokens)
        || matches!(metric.id, "vm.count" | "work.queued" | "approval.pending")
}

fn steps(from_ms: u64, to_ms: u64, step_ms: u64) -> u64 {
    if step_ms == 0 || to_ms <= from_ms {
        0
    } else {
        (to_ms - from_ms) / step_ms
    }
}

fn step_mid(from_ms: u64, step_ms: u64, index: u64) -> u64 {
    from_ms + index * step_ms + step_ms / 2
}

fn scalar_step(from_ms: u64, to_ms: u64) -> u64 {
    (to_ms.saturating_sub(from_ms) / 120).max(1_000)
}

/// Keeps the `top` largest groups in their order and folds the rest into one "other" group.
fn keep_top(groups: Vec<Group>, top: usize) -> Vec<Group> {
    if groups.len() <= top {
        return groups;
    }
    let mut order: Vec<usize> = (0..groups.len()).collect();
    order.sort_by(|&a, &b| groups[b].total().total_cmp(&groups[a].total()));
    let kept_index: Vec<usize> = order[..top].to_vec();
    let mut kept = Vec::new();
    let mut rest = Vec::new();
    for (i, group) in groups.into_iter().enumerate() {
        if kept_index.contains(&i) {
            kept.push(group);
        } else {
            rest.push(group);
        }
    }
    let len = rest.first().map_or(0, |g| g.values.len());
    let values = (0..len)
        .map(|i| {
            let live: Vec<f64> = rest.iter().filter_map(|g| g.values[i]).collect();
            (!live.is_empty()).then(|| live.iter().sum::<f64>())
        })
        .collect();
    kept.push(Group {
        name: "other".into(),
        scope: None,
        values,
    });
    kept
}

/// One entity or label value of an entity, as a unit the generator draws: its weight and the attributes a read
/// groups it by.
struct Leaf {
    member: String,
    label: String,
    scope: Option<String>,
    attrs: Vec<(&'static str, String)>,
    key: String,
    weight: f64,
}

fn leaf_attr<'a>(leaf: &'a Leaf, key: &str) -> Option<&'a str> {
    leaf.attrs.iter().find(|a| a.0 == key).map(|a| a.1.as_str())
}

/// Every leaf of a metric under a scope. Metrics split by a label (a context part, a state, a law, a GPU, a run)
/// expand each entity into one leaf per value.
fn leaves(metric: &MetricDef, scope: &str) -> Vec<Leaf> {
    let list = members(scope, entity_name(metric.entity));
    let mut out = Vec::new();
    for m in &list {
        match metric.id {
            "ctx.tokens" => {
                let Some(a) = agent_by_id(&m.id) else {
                    continue;
                };
                for &(part, share) in PARTS {
                    let tokens = a.window as f64 * a.fill / 100.0 * share;
                    out.push(leaf(m, &[("part", part)], tokens));
                }
            }
            "agent.state_ms" => {
                for &(state, share) in AGENT_STATES {
                    out.push(leaf(m, &[("state", state)], share));
                }
            }
            "law.infractions" => {
                for &(law, share) in LAWS {
                    out.push(leaf(m, &[("law", law)], leaf_weight(metric, m) * share));
                }
            }
            "vm.count" => {
                let node = m.id.trim_start_matches("node:");
                for state in ["running", "asleep", "paused"] {
                    let count = VMS
                        .iter()
                        .filter(|v| run_node(v.0) == node && v.2 == state)
                        .count();
                    out.push(leaf(m, &[("state", state)], count as f64));
                }
            }
            "host.gpu_util_pct" | "host.gpu_mem_pct" => {
                if m.id == "node:n1" {
                    out.push(leaf(m, &[("gpu", "gpu0")], 1.0));
                }
            }
            _ if metric.labels.contains(&"run") => {
                for run in runs_of(&m.id) {
                    out.push(leaf(m, &[("run", run)], 1.0));
                }
            }
            _ => out.push(leaf(m, &[], leaf_weight(metric, m))),
        }
    }
    out
}

fn leaf(m: &Member, extra: &[(&'static str, &'static str)], weight: f64) -> Leaf {
    let mut attrs = m.attrs.clone();
    let mut key = m.id.clone();
    for &(name, value) in extra {
        attrs.push((name, value.to_string()));
        key.push('|');
        key.push_str(value);
    }
    Leaf {
        member: m.id.clone(),
        label: m.label.clone(),
        scope: m.scope.clone(),
        attrs,
        key,
        weight,
    }
}

/// The weight of an entity: its share of a counter or gauge, or its absolute level for the gauges that are levels.
/// Histograms weigh every entity the same, so their mean is the level.
fn leaf_weight(metric: &MetricDef, m: &Member) -> f64 {
    if matches!(metric.kind, MetricKind::Histogram) {
        return 1.0;
    }
    match metric.id {
        "ctx.fill_pct" => agent_by_id(&m.id).map_or(1.0, |a| a.fill),
        "ctx.window" => agent_by_id(&m.id).map_or(1.0, |a| a.window as f64),
        "host.cpu_pct" => node_weight(&m.id, [25.0, 39.0, 53.0]),
        "host.mem_used_mib" => node_weight(&m.id, [18_000.0, 27_000.0, 36_000.0]),
        "host.load1" => node_weight(&m.id, [0.6, 1.2, 1.6]),
        "volume.used_pct" => volume_pct(&m.id),
        "volume.used_bytes" => volume_pct(&m.id) / 100.0,
        "store.bytes" => store_bytes(&m.id),
        _ => match m.kind {
            "agent" => agent_by_id(&m.id).map_or(1.0, |a| a.share),
            "vm" if m.state == "running" => 0.6 + 0.1 * vm_index(&m.id),
            "vm" => 0.0,
            "service" => service_weight(&m.id),
            "tool" => share_of(TOOLS, entity_of(&m.id, "tool")),
            "route" => share_of(ROUTES, entity_of(&m.id, "route")),
            "engine" => share_of(ENGINES, &m.id),
            _ => 1.0,
        },
    }
}

fn node_weight(id: &str, weights: [f64; 3]) -> f64 {
    match id {
        "node:n0" => weights[0],
        "node:n1" => weights[1],
        "node:n2" => weights[2],
        _ => 1.0,
    }
}

fn volume_pct(id: &str) -> f64 {
    let entity = entity_of(id, "volume");
    VOLUMES.iter().find(|v| v.0 == entity).map_or(40.0, |v| v.3)
}

fn store_bytes(id: &str) -> f64 {
    let entity = entity_of(id, "service");
    STORES.iter().find(|s| s.0 == entity).map_or(0.0, |s| s.1)
}

fn service_weight(id: &str) -> f64 {
    match entity_of(id, "service") {
        "n0:apid" => 1.0,
        "n0:forgejo" => 0.6,
        _ => 0.8,
    }
}

fn vm_index(id: &str) -> f64 {
    id.rsplit('/')
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(1.0)
}

fn share_of(table: &[(&str, f64)], id: &str) -> f64 {
    table.iter().find(|t| t.0 == id).map_or(0.0, |t| t.1)
}

/// The groups of a read: one for All, one per entity for Entity, one per attribute value for Label and AgentProp.
struct Part<'a> {
    key: String,
    name: String,
    scope: Option<String>,
    leaves: Vec<&'a Leaf>,
}

fn partition<'a>(group: &GroupBy, scope: &str, leaves: &'a [Leaf]) -> Vec<Part<'a>> {
    let mut parts: Vec<Part<'a>> = Vec::new();
    match group {
        GroupBy::All => {
            let name = scopes()
                .iter()
                .find(|s| s.id == scope)
                .map_or_else(|| scope.to_string(), |s| s.label.clone());
            parts.push(Part {
                key: scope.to_string(),
                name,
                scope: Some(scope.to_string()),
                leaves: leaves.iter().collect(),
            });
        }
        GroupBy::Entity => {
            for leaf in leaves {
                place(
                    &mut parts,
                    leaf.member.clone(),
                    leaf.label.clone(),
                    leaf.scope.clone(),
                    leaf,
                );
            }
        }
        GroupBy::Label(attr) | GroupBy::AgentProp(attr) => {
            for leaf in leaves {
                if let Some(value) = leaf_attr(leaf, attr) {
                    let (name, group_scope) = attr_group(attr, value);
                    place(&mut parts, value.to_string(), name, group_scope, leaf);
                }
            }
        }
    }
    parts
}

fn place<'a>(
    parts: &mut Vec<Part<'a>>,
    key: String,
    name: String,
    scope: Option<String>,
    leaf: &'a Leaf,
) {
    match parts.iter().position(|p| p.key == key) {
        Some(i) => parts[i].leaves.push(leaf),
        None => parts.push(Part {
            key,
            name,
            scope,
            leaves: vec![leaf],
        }),
    }
}

/// The name and scope a label value shows as.
fn attr_group(key: &str, value: &str) -> (String, Option<String>) {
    match key {
        "provider" => (value.into(), Some(format!("provider:{value}"))),
        "model" => (
            value.rsplit('/').next().unwrap_or(value).into(),
            Some(format!("model:{value}")),
        ),
        "role" => (value.into(), Some(format!("role:{value}"))),
        "run" => (format!("run {value}"), Some(format!("run:{value}"))),
        "node" => (node_label(value).into(), Some(format!("node:{value}"))),
        _ => (value.into(), None),
    }
}

/// An entity under a scope, with the attributes a read can group it by.
struct Member {
    kind: &'static str,
    id: String,
    label: String,
    state: String,
    scope: Option<String>,
    attrs: Vec<(&'static str, String)>,
}

impl Member {
    fn new(kind: &'static str, id: &str, label: &str) -> Member {
        Member {
            kind,
            id: id.into(),
            label: label.into(),
            state: String::new(),
            scope: None,
            attrs: Vec::new(),
        }
    }
    fn scoped(mut self) -> Member {
        self.scope = Some(self.id.clone());
        self
    }
    fn with(mut self, key: &'static str, value: &str) -> Member {
        self.attrs.push((key, value.into()));
        self
    }
    fn in_state(mut self, state: &str) -> Member {
        self.state = state.into();
        self
    }
}

/// The entities of a kind under a scope.
fn members(scope: &str, kind: &str) -> Vec<Member> {
    match kind {
        "global" => vec![Member::new("global", "global", "Global")],
        "hub" => vec![Member::new("hub", "hub", "hub")],
        "node" => nodes_of(scope)
            .into_iter()
            .map(|n| {
                Member::new("node", &format!("node:{n}"), node_label(n))
                    .scoped()
                    .with("node", n)
            })
            .collect(),
        "run" => runs_of(scope)
            .into_iter()
            .map(|r| {
                Member::new("run", &format!("run:{r}"), &format!("run {r}"))
                    .scoped()
                    .with("run", r)
                    .with("node", run_node(r))
            })
            .collect(),
        "vm" => vms_of(scope)
            .into_iter()
            .map(|(run, idx, state)| {
                Member::new("vm", &format!("vm:{run}/{idx}"), &format!("VM {run}/{idx}"))
                    .scoped()
                    .in_state(state)
                    .with("run", run)
                    .with("node", run_node(run))
            })
            .collect(),
        "agent" => agents_of(scope)
            .into_iter()
            .map(|a| {
                Member::new(
                    "agent",
                    &format!("agent:{}", a.cid),
                    &format!("{} ({})", a.name, a.cid),
                )
                .scoped()
                .in_state(a.status)
                .with("provider", a.provider)
                .with("model", &format!("{}/{}", a.provider, a.model))
                .with("role", a.role)
                .with("run", a.run)
                .with("node", run_node(a.run))
                .with("engine", engine_of(a.provider))
            })
            .collect(),
        "role" => roles_of(scope)
            .into_iter()
            .map(|r| {
                Member::new("role", &format!("role:{r}"), r)
                    .scoped()
                    .with("role", r)
            })
            .collect(),
        "provider" => providers_of(scope)
            .into_iter()
            .map(|p| {
                Member::new("provider", &format!("provider:{p}"), p)
                    .scoped()
                    .with("provider", p)
            })
            .collect(),
        "model" => models_of(scope)
            .into_iter()
            .map(|(p, m)| {
                let full = format!("{p}/{m}");
                Member::new("model", &format!("model:{full}"), m)
                    .scoped()
                    .with("provider", p)
                    .with("model", &full)
            })
            .collect(),
        "volume" => {
            let nodes = nodes_of(scope);
            let only = scoped_entity(scope, "volume");
            VOLUMES
                .iter()
                .filter(|v| match only {
                    Some(entity) => v.0 == entity,
                    None => nodes.contains(&v.2),
                })
                .map(|v| Member::new("volume", &format!("volume:{}", v.0), v.1).with("node", v.2))
                .collect()
        }
        "service" => {
            let nodes = nodes_of(scope);
            let only = scoped_entity(scope, "service");
            SERVICES
                .iter()
                .filter(|s| match only {
                    Some(entity) => s.0 == entity,
                    None => nodes.contains(&s.2),
                })
                .map(|s| Member::new("service", &format!("service:{}", s.0), s.1).with("node", s.2))
                .collect()
        }
        "tool" => {
            let only = scoped_entity(scope, "tool");
            TOOLS
                .iter()
                .filter(|t| only.is_none_or(|entity| t.0 == entity))
                .map(|t| Member::new("tool", &format!("tool:{}", t.0), t.0))
                .collect()
        }
        "route" => {
            let only = scoped_entity(scope, "route");
            ROUTES
                .iter()
                .filter(|r| only.is_none_or(|entity| r.0 == entity))
                .map(|r| Member::new("route", &format!("route:{}", r.0), r.0))
                .collect()
        }
        "engine" => ENGINES
            .iter()
            .map(|e| Member::new("engine", e.0, e.0).with("engine", e.0))
            .collect(),
        _ => Vec::new(),
    }
}

fn entity_name(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Global => "global",
        EntityKind::Node => "node",
        EntityKind::Run => "run",
        EntityKind::Vm => "vm",
        EntityKind::Workstation => "workstation",
        EntityKind::Sandbox => "sandbox",
        EntityKind::Agent => "agent",
        EntityKind::Role => "role",
        EntityKind::Provider => "provider",
        EntityKind::Model => "model",
        EntityKind::Tool => "tool",
        EntityKind::Route => "route",
        EntityKind::Service => "service",
        EntityKind::Volume => "volume",
        EntityKind::Engine => "engine",
        EntityKind::Hub => "hub",
    }
}

/// Splits a scope id into its kind and its rest: `node:n1` is `("node", "n1")`, `global` is `("global", "")`.
fn split(scope: &str) -> (&str, &str) {
    scope.split_once(':').unwrap_or((scope, ""))
}

/// The entity a scope id names when the scope is of `kind`: `volume:n1:/var/lib/plaza` is `n1:/var/lib/plaza` for
/// `volume`, and `global` is nothing.
fn scoped_entity<'a>(scope: &'a str, kind: &str) -> Option<&'a str> {
    scope.strip_prefix(kind)?.strip_prefix(':')
}

/// The entity of an id of `kind`, with its prefix taken off if it has one: a bare entity is its own.
fn entity_of<'a>(id: &'a str, kind: &str) -> &'a str {
    scoped_entity(id, kind).unwrap_or(id)
}

fn unique<T: PartialEq>(items: Vec<T>) -> Vec<T> {
    let mut out: Vec<T> = Vec::new();
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

fn run_node(run: &str) -> &'static str {
    RUNS.iter().find(|r| r.0 == run).map_or("", |r| r.1)
}

fn node_label(node: &str) -> &'static str {
    NODES.iter().find(|n| n.0 == node).map_or("", |n| n.1)
}

/// The run of a VM id's rest: `r12/1` is `r12`.
fn run_of_vm(vm: &str) -> &str {
    vm.split('/').next().unwrap_or("")
}

fn engine_of(provider: &str) -> &'static str {
    match provider {
        "anthropic" => "claude",
        "openai" => "codex",
        _ => "local",
    }
}

fn agent_by_id(id: &str) -> Option<&'static Roster> {
    let cid: u64 = id.strip_prefix("agent:")?.parse().ok()?;
    AGENTS.iter().find(|a| a.cid == cid)
}

fn agents_of(scope: &str) -> Vec<&'static Roster> {
    let (kind, id) = split(scope);
    AGENTS
        .iter()
        .filter(|a| match kind {
            "global" => true,
            "node" => run_node(a.run) == id,
            "run" => a.run == id,
            "vm" => a.run == run_of_vm(id),
            "agent" => a.cid.to_string() == id,
            "role" => a.role == id,
            "provider" => a.provider == id,
            "model" => format!("{}/{}", a.provider, a.model) == id,
            _ => false,
        })
        .collect()
}

fn runs_of(scope: &str) -> Vec<&'static str> {
    let (kind, id) = split(scope);
    match kind {
        "global" => RUNS.iter().map(|r| r.0).collect(),
        "node" => RUNS.iter().filter(|r| r.1 == id).map(|r| r.0).collect(),
        "run" => RUNS.iter().filter(|r| r.0 == id).map(|r| r.0).collect(),
        "vm" => RUNS
            .iter()
            .filter(|r| r.0 == run_of_vm(id))
            .map(|r| r.0)
            .collect(),
        "agent" | "role" | "provider" | "model" => {
            unique(agents_of(scope).iter().map(|a| a.run).collect())
        }
        _ => Vec::new(),
    }
}

fn vms_of(scope: &str) -> Vec<(&'static str, u64, &'static str)> {
    let (kind, id) = split(scope);
    let runs = runs_of(scope);
    VMS.iter()
        .copied()
        .filter(|&(run, idx, _)| {
            if kind == "vm" {
                format!("{run}/{idx}") == id
            } else {
                runs.contains(&run)
            }
        })
        .collect()
}

fn nodes_of(scope: &str) -> Vec<&'static str> {
    let (kind, id) = split(scope);
    match kind {
        "global" => NODES.iter().map(|n| n.0).collect(),
        "node" => NODES.iter().filter(|n| n.0 == id).map(|n| n.0).collect(),
        "ws" => vec!["n1"],
        "sb" => vec!["n2"],
        _ => {
            let mut nodes: Vec<&'static str> = runs_of(scope).into_iter().map(run_node).collect();
            nodes.extend(agents_of(scope).into_iter().map(|a| run_node(a.run)));
            unique(nodes)
        }
    }
}

fn roles_of(scope: &str) -> Vec<&'static str> {
    if scope == "global" {
        ROLES.to_vec()
    } else {
        unique(agents_of(scope).iter().map(|a| a.role).collect())
    }
}

fn providers_of(scope: &str) -> Vec<&'static str> {
    if scope == "global" {
        PROVIDERS.to_vec()
    } else {
        unique(agents_of(scope).iter().map(|a| a.provider).collect())
    }
}

fn models_of(scope: &str) -> Vec<(&'static str, &'static str)> {
    if scope == "global" {
        MODELS.to_vec()
    } else {
        unique(
            agents_of(scope)
                .iter()
                .map(|a| (a.provider, a.model))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;
    const HOUR: u64 = 3_600_000;

    fn spend(from_ms: u64) -> Read {
        Read {
            metric: "llm.spend_usd".into(),
            scope: "global".into(),
            from_ms,
            to_ms: NOW,
            step_ms: HOUR,
            agg: AggFn::Sum,
            group: GroupBy::All,
            top: None,
        }
    }

    #[test]
    fn the_scope_tree_holds_every_scope_and_its_children() {
        let ids: Vec<String> = scopes().into_iter().map(|s| s.id).collect();
        for id in [
            "global",
            "node:n0",
            "node:n1",
            "node:n2",
            "run:r12",
            "vm:r12/1",
            "vm:r12/3",
            "ws:design",
            "sb:7f3a",
            "role:builder",
            "agent:4417",
            "provider:anthropic",
            "model:anthropic/opus-5.5",
        ] {
            assert!(ids.iter().any(|i| i.as_str() == id), "missing scope {id}");
        }
        let runs: Vec<String> = entities("node:n1", "run")
            .into_iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(runs, ["run:r12", "run:r15"]);
        assert_eq!(entities("run:r12", "vm").len(), 4);
        assert_eq!(entities("global", "agent").len(), 6);
        assert_eq!(agent(4417).map(|a| a.node), Some("n1".to_string()));
    }

    #[test]
    fn a_read_repeated_gives_identical_values_and_overlapping_windows_agree() {
        let f = Fixture::new(NOW);
        let first = series(&f, &spend(NOW - 10 * HOUR));
        assert_eq!(first, series(&f, &spend(NOW - 10 * HOUR)));
        let later = series(&f, &spend(NOW - 5 * HOUR));
        assert_eq!(first[0].values.len(), 10);
        assert_eq!(&first[0].values[5..], &later[0].values[..]);
    }

    #[test]
    fn n2_cpu_has_a_gap_in_the_last_day_and_the_other_nodes_do_not() {
        let f = Fixture::new(NOW);
        let cpu = |scope: &str| Read {
            metric: "host.cpu_pct".into(),
            scope: scope.into(),
            from_ms: NOW - DAY_MS,
            to_ms: NOW,
            step_ms: 5 * MINUTE_MS,
            agg: AggFn::Avg,
            group: GroupBy::All,
            top: None,
        };
        let gaps = |scope: &str| {
            series(&f, &cpu(scope))[0]
                .values
                .iter()
                .filter(|v| v.is_none())
                .count()
        };
        assert!(gaps("node:n2") > 0);
        assert_eq!(gaps("node:n1"), 0);
        assert_eq!(gaps("node:n0"), 0);
    }

    #[test]
    fn a_volume_scope_reads_its_own_volume_and_the_volumes_have_scope_ids() {
        let f = Fixture::new(NOW);
        let volumes = entities("global", "volume");
        assert!(!volumes.is_empty());
        assert!(volumes.iter().all(|v| v.id.starts_with("volume:")));
        let own = entities("volume:n2:/var/lib/plaza", "volume");
        assert_eq!(own.len(), 1);
        let used = scalar(
            &f,
            "volume.used_pct",
            "volume:n2:/var/lib/plaza",
            NOW - HOUR,
            NOW,
            AggFn::Last,
        )
        .expect("the volume has a level");
        assert!((used - 91.0).abs() < 6.0, "the volume is at {used}");
    }
}
