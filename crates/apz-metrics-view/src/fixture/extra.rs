//! The events, calls, traces, alerts and the other reports that are not series.
//!
//! Everything here is generated from fixed seeds and the fixture's `now_ms`, so two calls at the same moment agree.

use apz_metrics::alerts::{Cmp, Slo, State, slo_status, starter_rules, starter_slos};
use apz_metrics::trace::{Keep, Span, SpanKind, Trace};
use apz_metrics::{EntityKind, Event};
use serde_json::json;

use super::Fixture;
use crate::source::{AlertsState, Budget, CallRow, PriceRow, Prices, SelfCost};

const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
const DAY_MS: u64 = 24 * HOUR_MS;

/// The model switch of ada, 4.3 hours before now.
const MODEL_SWITCH_AGO_MS: u64 = 4 * HOUR_MS + 18 * MINUTE_MS;

/// Five-minute windows per service level: the month so far, in the fixture.
const SLO_WINDOWS: u64 = 1_000;

/// The runs of the plaza and the node each is on.
const RUNS: &[(&str, &str)] = &[("r12", "n1"), ("r15", "n1"), ("r19", "n2")];

/// The VMs of the plaza, as `run/index`.
const VMS: &[&str] = &[
    "r12/1", "r12/2", "r12/3", "r12/4", "r15/1", "r15/2", "r15/3", "r19/1", "r19/2", "r19/3",
    "r19/4", "r19/5",
];

/// An agent of the plaza.
struct Agent {
    cid: u64,
    name: &'static str,
    role: &'static str,
    run: &'static str,
    provider: &'static str,
    model: &'static str,
}

const AGENTS: &[Agent] = &[
    Agent {
        cid: 4417,
        name: "ada",
        role: "builder",
        run: "r12",
        provider: "anthropic",
        model: "opus-5.5",
    },
    Agent {
        cid: 4418,
        name: "kit",
        role: "builder",
        run: "r12",
        provider: "anthropic",
        model: "sonnet-5.5",
    },
    Agent {
        cid: 4421,
        name: "rhea",
        role: "reviewer",
        run: "r12",
        provider: "anthropic",
        model: "opus-5.5",
    },
    Agent {
        cid: 4430,
        name: "juno",
        role: "researcher",
        run: "r15",
        provider: "openai",
        model: "gpt-5",
    },
    Agent {
        cid: 4431,
        name: "otto",
        role: "ops",
        run: "r15",
        provider: "anthropic",
        model: "haiku-5.5",
    },
    Agent {
        cid: 4440,
        name: "mira",
        role: "builder",
        run: "r15",
        provider: "anthropic",
        model: "sonnet-5.5",
    },
];

/// The kinds of the forty events of the last day, in time order (one slot each, moved a little at random).
const DAY_KINDS: [&str; 40] = [
    "wake",
    "sleep",
    "wake",
    "compaction",
    "error",
    "wake",
    "scale",
    "snapshot",
    "wake",
    "sleep",
    "boot",
    "wake",
    "compaction",
    "error",
    "wake",
    "move",
    "sleep",
    "wake",
    "fork",
    "compaction",
    "wake",
    "error",
    "scale",
    "wake",
    "pause",
    "sleep",
    "wake",
    "compaction",
    "snapshot",
    "wake",
    "error",
    "wake",
    "sleep",
    "compaction",
    "wake",
    "scale",
    "wake",
    "law",
    "wake",
    "sleep",
];

/// The lifecycle events of the last week that are older than the last day.
const OLDER_KINDS: [&str; 5] = ["boot", "fork", "move", "snapshot", "scale"];

/// The fixed trace ids, oldest last. Trace `k` starts `8 + 9 k` minutes before now.
const TRACE_IDS: [&str; 12] = [
    "tr-5a10", "tr-5a2c", "tr-5a47", "tr-5a6e", "tr-5a93", "tr-5ab1", "tr-5ac8", "tr-5ae0",
    "tr-5af7", "tr-5b0d", "tr-5b26", "tr-5b3f",
];

/// The traces that failed, by index into [`TRACE_IDS`]: a failing test (2 and 6) and a rate-limited call (9).
const FAILED_TRACES: [usize; 3] = [2, 6, 9];

/// A deterministic xorshift generator, seeded from a fixed string.
struct Rng(u64);

impl Rng {
    fn new(seed: &str) -> Rng {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in seed.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        Rng(hash.max(1))
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// A number in `[lo, hi)`.
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        let unit = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        lo + (hi - lo) * unit
    }

    /// A whole number in `[lo, hi)`.
    fn int(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next_u64() % (hi - lo)
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[(self.next_u64() % items.len() as u64) as usize]
    }
}

/// The events that a scope holds. Plaza-wide annotations belong to every scope; a node holds its runs, their VMs
/// and their agents, a run its VMs and agents, a role or a provider the agents that have it.
fn event_in_scope(event: &Event, scope: &str) -> bool {
    if scope == "global" || event.entity_kind == EntityKind::Global {
        return true;
    }
    let (prefix, id) = scope.split_once(':').unwrap_or((scope, ""));
    if event.entity_kind.prefix() == prefix && event.entity == id {
        return true;
    }
    let agent = match event.entity_kind {
        EntityKind::Agent => agent_by_entity(&event.entity),
        _ => None,
    };
    let run = match event.entity_kind {
        EntityKind::Vm => event.entity.split('/').next(),
        EntityKind::Run => Some(event.entity.as_str()),
        EntityKind::Agent => agent.map(|a| a.run),
        _ => None,
    };
    match prefix {
        "node" => run.map(node_of_run) == Some(id),
        "run" => run == Some(id),
        "role" => agent.is_some_and(|a| a.role == id),
        "provider" => agent.is_some_and(|a| a.provider == id),
        _ => false,
    }
}

/// Whether an agent is under a scope.
fn agent_in_scope(agent: &Agent, scope: &str) -> bool {
    let (prefix, id) = scope.split_once(':').unwrap_or((scope, ""));
    match prefix {
        "global" => true,
        "agent" => agent.cid.to_string() == id,
        "node" => node_of_run(agent.run) == id,
        "run" => agent.run == id,
        "role" => agent.role == id,
        "provider" => agent.provider == id,
        "model" => format!("{}/{}", agent.provider, agent.model) == id,
        _ => false,
    }
}

/// Whether a call is under a scope: a provider or model is matched on the call itself, anything else on its agent.
fn call_in_scope(call: &CallRow, scope: &str) -> bool {
    let (prefix, id) = scope.split_once(':').unwrap_or((scope, ""));
    match prefix {
        "provider" => call.provider == id,
        "model" => format!("{}/{}", call.provider, call.model) == id,
        _ => AGENTS
            .iter()
            .find(|agent| agent.cid == call.cid)
            .is_some_and(|agent| agent_in_scope(agent, scope)),
    }
}

/// Whether a trace is under a scope, judged by its agent.
fn trace_in_scope(trace: &Trace, scope: &str) -> bool {
    AGENTS
        .iter()
        .find(|agent| agent.cid == trace.cid)
        .is_some_and(|agent| agent_in_scope(agent, scope))
}

fn agent_by_entity(entity: &str) -> Option<&'static Agent> {
    AGENTS.iter().find(|agent| agent.cid.to_string() == entity)
}

fn node_of_run(run: &str) -> &'static str {
    RUNS.iter()
        .find(|entry| entry.0 == run)
        .map_or("", |entry| entry.1)
}

fn event(
    at_ms: u64,
    kind: &str,
    entity_kind: EntityKind,
    entity: &str,
    duration_ms: Option<f64>,
    detail: String,
    parts: Vec<(String, f64)>,
) -> Event {
    Event {
        t_ms: at_ms,
        kind: kind.to_string(),
        entity_kind,
        entity: entity.to_string(),
        duration_ms,
        detail,
        parts,
    }
}

/// One work event of the given kind at `at_ms`. `slot` decides the fixed compaction of ada in the day.
fn work_event(kind: &str, at_ms: u64, slot: usize, rng: &mut Rng) -> Event {
    match kind {
        "wake" => {
            let vm = *rng.pick(VMS);
            let restore = rng.range(120.0, 200.0).round();
            let ready = (restore + rng.range(230.0, 300.0)).round();
            event(
                at_ms,
                kind,
                EntityKind::Vm,
                vm,
                Some(ready),
                format!("ready in {ready:.0} ms (restore {restore:.0} ms)"),
                vec![
                    ("restore_ms".to_string(), restore),
                    ("ready_ms".to_string(), ready),
                ],
            )
        }
        "sleep" => {
            let vm = *rng.pick(VMS);
            let idle = rng.int(4, 26) as f64;
            event(
                at_ms,
                kind,
                EntityKind::Vm,
                vm,
                None,
                format!("asleep after {idle:.0} min idle"),
                vec![("idle_min".to_string(), idle)],
            )
        }
        "boot" => {
            let vm = *rng.pick(VMS);
            event(
                at_ms,
                kind,
                EntityKind::Vm,
                vm,
                Some(4_200.0),
                "booted from the base image in 4.2 s".to_string(),
                Vec::new(),
            )
        }
        "snapshot" => {
            let vm = *rng.pick(VMS);
            let gb = rng.range(1.2, 2.4);
            let secs = rng.range(4.0, 8.0);
            event(
                at_ms,
                kind,
                EntityKind::Vm,
                vm,
                Some(secs * 1_000.0),
                format!("snapshot {gb:.1} GB in {secs:.1} s"),
                vec![("size_gb".to_string(), gb)],
            )
        }
        "pause" => {
            let agent = rng.pick(AGENTS);
            event(
                at_ms,
                kind,
                EntityKind::Agent,
                &agent.cid.to_string(),
                None,
                format!("{} paused by the owner", agent.name),
                Vec::new(),
            )
        }
        "compaction" => {
            let agent = if slot == 3 {
                &AGENTS[0]
            } else {
                rng.pick(AGENTS)
            };
            let (before, after) = if slot == 3 {
                (188.0, 50.0)
            } else {
                (
                    rng.range(150.0, 200.0).round(),
                    rng.range(40.0, 60.0).round(),
                )
            };
            let freed = before - after;
            event(
                at_ms,
                kind,
                EntityKind::Agent,
                &agent.cid.to_string(),
                None,
                format!("freed {freed:.0}k tokens ({before:.0}k → {after:.0}k)"),
                vec![("before".to_string(), before), ("after".to_string(), after)],
            )
        }
        "error" => {
            let (agent, detail) = match rng.int(0, 3) {
                0 => (
                    &AGENTS[3],
                    "openai/gpt-5: 429 rate limited, retried in 2.0 s",
                ),
                1 => (
                    rng.pick(AGENTS),
                    "anthropic/sonnet-5.5: 529 overloaded, retried in 1.0 s",
                ),
                _ => (
                    rng.pick(AGENTS),
                    "bash: exit 101 in the test run, retried once",
                ),
            };
            event(
                at_ms,
                kind,
                EntityKind::Agent,
                &agent.cid.to_string(),
                None,
                detail.to_string(),
                Vec::new(),
            )
        }
        "scale" => {
            let run = *rng.pick(&["r12", "r15", "r19"]);
            let depth = rng.int(3, 12);
            event(
                at_ms,
                kind,
                EntityKind::Run,
                run,
                None,
                format!("autoscaler +1 VM (queue depth {depth})"),
                vec![("queue_depth".to_string(), depth as f64)],
            )
        }
        "move" => {
            let vm = *rng.pick(VMS);
            event(
                at_ms,
                kind,
                EntityKind::Vm,
                vm,
                Some(5_400.0),
                "moved to another host".to_string(),
                Vec::new(),
            )
        }
        "fork" => {
            let agent = rng.pick(AGENTS);
            let child = rng.int(4_441, 4_460);
            event(
                at_ms,
                kind,
                EntityKind::Agent,
                &agent.cid.to_string(),
                Some(820.0),
                format!("forked {child} from {} for a sub-task", agent.name),
                Vec::new(),
            )
        }
        "law" => event(
            at_ms,
            kind,
            EntityKind::Agent,
            "4417",
            None,
            "budget cap refused a model call".to_string(),
            Vec::new(),
        ),
        _ => event(
            at_ms,
            kind,
            EntityKind::Global,
            "global",
            None,
            kind.to_string(),
            Vec::new(),
        ),
    }
}

/// Every event of the plaza, oldest first.
fn plaza_events(now: u64) -> Vec<Event> {
    let mut rng = Rng::new("apz-metrics-view fixture events");
    let mut out = Vec::new();
    for (slot, kind) in DAY_KINDS.iter().enumerate() {
        let back = (2 * slot as u64 + 1) * DAY_MS / 80;
        let shift = rng.int(0, 20 * MINUTE_MS);
        let at = (now.saturating_sub(back) + shift)
            .saturating_sub(10 * MINUTE_MS)
            .min(now);
        out.push(work_event(kind, at, slot, &mut rng));
    }
    for (i, kind) in OLDER_KINDS.iter().enumerate() {
        let back = DAY_MS * 3 / 2 + i as u64 * DAY_MS * 13 / 10;
        out.push(work_event(
            kind,
            now.saturating_sub(back),
            100 + i,
            &mut rng,
        ));
    }
    let hours_ago = |hours: f64| now.saturating_sub((hours * HOUR_MS as f64) as u64);
    out.push(event(
        hours_ago(16.8),
        "release",
        EntityKind::Global,
        "global",
        None,
        "release 0.42.0 rolled out".to_string(),
        Vec::new(),
    ));
    out.push(event(
        hours_ago(10.0),
        "law_change",
        EntityKind::Global,
        "global",
        None,
        "law changed: builder budget cap $20/day".to_string(),
        Vec::new(),
    ));
    out.push(event(
        hours_ago(4.3),
        "model_switch",
        EntityKind::Global,
        "global",
        None,
        "ada switched model: opus-5.5 → sonnet-5.5".to_string(),
        Vec::new(),
    ));
    out.push(event(
        hours_ago(2.5),
        "alert",
        EntityKind::Model,
        "anthropic/opus-5.5",
        None,
        "fired: first token p95 over 2 s".to_string(),
        Vec::new(),
    ));
    out.sort_by_key(|e| e.t_ms);
    out
}

/// The provider and model a call of `agent` at `at` used. Ada moved from opus to sonnet at the switch, and otto
/// runs every other call on the local model.
fn model_for(agent: &Agent, at: u64, now: u64, step: u64) -> (&'static str, &'static str) {
    if agent.cid == 4417 && at >= now.saturating_sub(MODEL_SWITCH_AGO_MS) {
        return ("anthropic", "sonnet-5.5");
    }
    if agent.cid == 4431 && step % 2 == 1 {
        return ("local", "qwen3-32b");
    }
    (agent.provider, agent.model)
}

/// Dollars per million input and output tokens of a model.
fn prices(model: &str) -> (f64, f64) {
    match model {
        "opus-5.5" => (15.0, 75.0),
        "sonnet-5.5" => (3.0, 15.0),
        "haiku-5.5" => (0.8, 4.0),
        "gpt-5" => (1.25, 10.0),
        _ => (0.0, 0.0),
    }
}

/// The context window of a provider's models, in tokens.
fn context_window(provider: &str) -> f64 {
    match provider {
        "openai" => 400_000.0,
        "local" => 32_000.0,
        _ => 200_000.0,
    }
}

/// The numbers of one model call.
struct Numbers {
    tokens_in: u64,
    cache_read: u64,
    tokens_out: u64,
    ttft_ms: f64,
    duration_ms: f64,
    speed: f64,
    cost_usd: f64,
    fill_pct: f64,
}

fn call_numbers(rng: &mut Rng, cid: u64, provider: &str, model: &str, slow: f64) -> Numbers {
    let tokens_in = if provider == "local" {
        rng.int(8_000, 30_000)
    } else if cid == 4417 {
        rng.int(160_000, 186_000)
    } else {
        rng.int(30_000, 90_000)
    };
    let cache_read = (tokens_in as f64 * rng.range(0.6, 0.9)) as u64;
    let tokens_out = rng.int(300, 1_800);
    let ttft_ms = rng.range(900.0, 2_600.0) * slow;
    let speed = rng.range(40.0, 70.0);
    let duration_ms = ttft_ms + tokens_out as f64 / speed * 1_000.0;
    let (rate_in, rate_out) = prices(model);
    let fresh = (tokens_in - cache_read) as f64;
    let cost_usd =
        (fresh * rate_in + cache_read as f64 * rate_in * 0.1 + tokens_out as f64 * rate_out)
            / 1_000_000.0;
    let fill_pct = tokens_in as f64 / context_window(provider) * 100.0;
    Numbers {
        tokens_in,
        cache_read,
        tokens_out,
        ttft_ms,
        duration_ms,
        speed,
        cost_usd,
        fill_pct,
    }
}

fn call_row(
    agent: &Agent,
    at_ms: u64,
    provider: &str,
    model: &str,
    numbers: &Numbers,
    status: u16,
    trace: Option<String>,
) -> CallRow {
    CallRow {
        at_ms,
        cid: agent.cid,
        agent: agent.name.to_string(),
        provider: provider.to_string(),
        model: model.to_string(),
        tokens_in: numbers.tokens_in,
        cache_read: numbers.cache_read,
        tokens_out: numbers.tokens_out,
        ttft_ms: Some(numbers.ttft_ms),
        duration_ms: Some(numbers.duration_ms),
        cost_usd: numbers.cost_usd,
        status,
        trace,
    }
}

fn plain_span(
    id: u32,
    parent: Option<u32>,
    kind: SpanKind,
    name: String,
    start_ms: u64,
    dur_ms: f64,
) -> Span {
    Span {
        id,
        parent,
        kind,
        name,
        start_ms,
        dur_ms,
        first_token_ms: None,
        ok: true,
        attrs: Vec::new(),
    }
}

/// The kept traces and the model calls of the plaza, built together so that each call names its trace.
struct Plan {
    traces: Vec<Trace>,
    calls: Vec<CallRow>,
}

fn plan(now: u64) -> Plan {
    let mut rng = Rng::new("apz-metrics-view fixture traces");
    let mut traces = Vec::new();
    let mut calls = Vec::new();
    for (k, id) in TRACE_IDS.iter().enumerate() {
        let agent = &AGENTS[k % AGENTS.len()];
        let start = now.saturating_sub((8 + 9 * k as u64) * MINUTE_MS);
        let failing = FAILED_TRACES.contains(&k);
        let slow = if k == 4 || k == 10 { 2.5 } else { 1.0 };
        let mut spans: Vec<Span> = Vec::new();
        let mut at = start;
        let mut next_id = 0u32;

        if k % 4 == 1 {
            let restore = rng.range(140.0, 180.0).round();
            let ready = restore + rng.range(230.0, 260.0).round();
            let vm = VMS[k % VMS.len()];
            spans.push(plain_span(
                0,
                None,
                SpanKind::Wake,
                format!("wake VM {vm}"),
                at,
                ready,
            ));
            spans.push(plain_span(
                1,
                Some(0),
                SpanKind::Restore,
                "restore".to_string(),
                at,
                restore,
            ));
            spans.push(plain_span(
                2,
                Some(0),
                SpanKind::Ready,
                "ready".to_string(),
                at + restore as u64,
                ready - restore,
            ));
            next_id = 3;
            at += ready as u64;
        }

        let steps = 3 + k % 3;
        for step in 0..steps {
            let (provider, model) = model_for(agent, at, now, step as u64);
            let numbers = call_numbers(&mut rng, agent.cid, provider, model, slow);
            let status: u16 = if k == 9 && step == 0 { 429 } else { 200 };
            let mut model_span = plain_span(
                next_id,
                None,
                SpanKind::Model,
                format!("model call · {provider}/{model}"),
                at,
                numbers.duration_ms,
            );
            model_span.first_token_ms = Some(numbers.ttft_ms);
            model_span.ok = status == 200;
            model_span.attrs = vec![
                ("model".to_string(), model.to_string()),
                ("input_tokens".to_string(), numbers.tokens_in.to_string()),
                ("cache_read".to_string(), numbers.cache_read.to_string()),
                ("output_tokens".to_string(), numbers.tokens_out.to_string()),
                (
                    "first_token".to_string(),
                    format!("{:.0} ms", numbers.ttft_ms),
                ),
                (
                    "output_speed".to_string(),
                    format!("{:.0} tok/s", numbers.speed),
                ),
                ("cost".to_string(), format!("${:.3}", numbers.cost_usd)),
                (
                    "context_fill".to_string(),
                    format!("{:.0} %", numbers.fill_pct),
                ),
            ];
            if status != 200 {
                model_span.attrs.push((
                    "status".to_string(),
                    "429 rate limited, retried in 2.0 s".to_string(),
                ));
            }
            calls.push(call_row(
                agent,
                at,
                provider,
                model,
                &numbers,
                status,
                Some(id.to_string()),
            ));
            spans.push(model_span);
            next_id += 1;
            at += numbers.duration_ms.round() as u64;

            if step + 1 < steps {
                let tool = if step % 2 == 0 { "bash" } else { "read_file" };
                let fails = failing && k != 9 && step + 2 == steps;
                let tool_ms = rng.range(300.0, 2_400.0).round();
                let mut tool_span = plain_span(
                    next_id,
                    None,
                    SpanKind::Tool,
                    format!("tool · {tool}"),
                    at,
                    tool_ms,
                );
                tool_span.ok = !fails;
                tool_span.attrs = vec![
                    ("tool".to_string(), tool.to_string()),
                    (
                        "arguments".to_string(),
                        "1.2k bytes (content not stored)".to_string(),
                    ),
                    ("result tokens".to_string(), rng.int(120, 900).to_string()),
                    (
                        "outcome".to_string(),
                        if fails {
                            "exit 101: test failed".to_string()
                        } else {
                            "ok".to_string()
                        },
                    ),
                ];
                spans.push(tool_span);
                next_id += 1;
                at += tool_ms as u64;
            }
        }

        if k % 3 == 2 {
            let mut hub = plain_span(
                next_id,
                None,
                SpanKind::HubMcp,
                "hub · git.push".to_string(),
                at,
                380.0,
            );
            hub.attrs = vec![
                ("tool".to_string(), "hub.git.push".to_string()),
                ("outcome".to_string(), "ok".to_string()),
            ];
            spans.push(hub);
            at += 380;
        }

        let ok = spans.iter().all(|span| span.ok);
        let kept = if !ok {
            Keep::Error
        } else if k == 0 {
            Keep::Open
        } else if slow > 1.0 {
            Keep::Slow
        } else {
            Keep::Sampled
        };
        traces.push(Trace {
            id: id.to_string(),
            cid: agent.cid,
            role: agent.role.to_string(),
            start_ms: start,
            dur_ms: (at - start) as f64,
            ok,
            spans,
            kept: Some(kept),
        });
    }

    // Ten calls outside any kept turn, back over the last day.
    for i in 0..10u64 {
        let agent = &AGENTS[(i as usize) % AGENTS.len()];
        let at = now.saturating_sub((42 + 132 * i) * MINUTE_MS);
        let (provider, model) = model_for(agent, at, now, i);
        let numbers = call_numbers(&mut rng, agent.cid, provider, model, 1.0);
        calls.push(call_row(agent, at, provider, model, &numbers, 200, None));
    }

    Plan { traces, calls }
}

/// Events in a scope and window, newest first. `kinds` empty means every kind.
pub fn events(
    f: &Fixture,
    scope: &str,
    from_ms: u64,
    to_ms: u64,
    kinds: &[&str],
    limit: usize,
) -> Vec<Event> {
    let mut found: Vec<Event> = plaza_events(f.now_ms)
        .into_iter()
        .filter(|e| e.t_ms >= from_ms && e.t_ms <= to_ms)
        .filter(|e| kinds.is_empty() || kinds.contains(&e.kind.as_str()))
        .filter(|e| event_in_scope(e, scope))
        .collect();
    found.sort_by(|a, b| b.t_ms.cmp(&a.t_ms));
    found.truncate(limit);
    found
}

/// Model calls in a scope, newest first, before `before_ms`.
pub fn calls(f: &Fixture, scope: &str, before_ms: u64, limit: usize) -> Vec<CallRow> {
    let mut rows: Vec<CallRow> = plan(f.now_ms)
        .calls
        .into_iter()
        .filter(|c| c.at_ms < before_ms)
        .filter(|c| call_in_scope(c, scope))
        .collect();
    rows.sort_by(|a, b| b.at_ms.cmp(&a.at_ms));
    rows.truncate(limit);
    rows
}

/// Kept traces in a scope and window, newest first.
pub fn traces(f: &Fixture, scope: &str, from_ms: u64, to_ms: u64, limit: usize) -> Vec<Trace> {
    let mut found: Vec<Trace> = plan(f.now_ms)
        .traces
        .into_iter()
        .filter(|t| t.start_ms >= from_ms && t.start_ms <= to_ms)
        .filter(|t| trace_in_scope(t, scope))
        .collect();
    found.sort_by(|a, b| b.start_ms.cmp(&a.start_ms));
    found.truncate(limit);
    found
}

pub fn trace(f: &Fixture, id: &str) -> Option<Trace> {
    plan(f.now_ms).traces.into_iter().find(|t| t.id == id)
}

/// One thousand windows of a service level's metric. `breaking` of them break the objective, spread by a
/// multiplier coprime with the window count so the count is exact.
fn slo_windows(slo: &Slo, breaking: u64) -> Vec<f64> {
    let mut rng = Rng::new(&slo.id);
    (0..SLO_WINDOWS)
        .map(|i| {
            let breaks = (i * 337) % SLO_WINDOWS < breaking;
            // Below the threshold meets a less-than objective, above it a greater-than one.
            let low = matches!(slo.cmp, Cmp::Lt) != breaks;
            let factor = if low {
                rng.range(0.5, 0.85)
            } else {
                rng.range(1.15, 1.4)
            };
            slo.threshold * factor
        })
        .collect()
}

pub fn alerts(f: &Fixture) -> AlertsState {
    let now = f.now_ms;
    let ago = |hours: f64| now.saturating_sub((hours * HOUR_MS as f64) as u64);
    let row =
        |hours: f64, rule: &str, subject: &str, what: &str, value: f64, lasted: Option<u64>| {
            (
                ago(hours),
                rule.to_string(),
                subject.to_string(),
                what.to_string(),
                value,
                lasted,
            )
        };
    let slos = starter_slos()
        .into_iter()
        .map(|slo| {
            let breaking = match slo.id.as_str() {
                "slo-wake" => 8,
                "slo-ttft" => 29,
                "slo-beat" => 2,
                _ => 0,
            };
            let windows = slo_windows(&slo, breaking);
            let status = slo_status(&slo, &windows);
            (slo, status)
        })
        .collect();
    AlertsState {
        rules: starter_rules(),
        subjects: vec![
            (
                "starter-ttft".to_string(),
                "model:anthropic/opus-5.5".to_string(),
                State::Firing { since_ms: ago(2.5) },
                2380.0,
            ),
            (
                "starter-disk".to_string(),
                "volume:n2:/var/lib/plaza".to_string(),
                State::Firing { since_ms: ago(7.0) },
                91.0,
            ),
            (
                "starter-context".to_string(),
                "agent:4417".to_string(),
                State::Pending {
                    since_ms: ago(0.07),
                },
                93.0,
            ),
            (
                "starter-wake".to_string(),
                "global".to_string(),
                State::Ok,
                640.0,
            ),
            (
                "starter-spend".to_string(),
                "global".to_string(),
                State::Ok,
                38.4,
            ),
            (
                "starter-node".to_string(),
                "global".to_string(),
                State::Ok,
                840.0,
            ),
        ],
        muted: vec![("starter-spend".to_string(), now + 2 * HOUR_MS)],
        acked: vec![(
            "starter-ttft".to_string(),
            "model:anthropic/opus-5.5".to_string(),
        )],
        history: vec![
            row(
                1.2,
                "starter-ttft",
                "model:anthropic/opus-5.5",
                "acknowledged",
                2380.0,
                None,
            ),
            row(
                2.5,
                "starter-ttft",
                "model:anthropic/opus-5.5",
                "fired",
                2410.0,
                None,
            ),
            row(
                7.0,
                "starter-disk",
                "volume:n2:/var/lib/plaza",
                "fired",
                91.0,
                None,
            ),
            row(9.0, "starter-spend", "global", "muted", 58.2, None),
            row(
                11.0,
                "starter-wake",
                "global",
                "resolved",
                880.0,
                Some(30 * MINUTE_MS),
            ),
            row(11.5, "starter-wake", "global", "fired", 1310.0, None),
            row(
                25.0,
                "starter-context",
                "agent:4417",
                "resolved",
                88.0,
                Some(HOUR_MS),
            ),
            row(26.0, "starter-context", "agent:4417", "fired", 94.0, None),
        ],
        slos,
    }
}

fn cost_row(label: &str, value: f64, budget: f64, unit: &str) -> (String, f64, f64, String) {
    (label.to_string(), value, budget, unit.to_string())
}

pub fn self_cost() -> SelfCost {
    SelfCost {
        rows: vec![
            cost_row("noded sampler CPU (n1, 7 VMs)", 0.21, 0.5, "% of a core"),
            cost_row("hub ingest + rollups CPU", 0.62, 1.0, "% of a core"),
            cost_row("hub metrics memory", 38.0, 64.0, "MB"),
            cost_row("heartbeat payload", 2.3, 4.0, "KB avg"),
            cost_row("Postgres growth", 41.0, 100.0, "MB / day"),
            cost_row("console while live", 1.1, 2.0, "% CPU"),
            cost_row("alert rule evaluation", 0.04, 0.2, "% of a core"),
            cost_row("kept traces", 120.0, 2048.0, "MB of 2 GB cap"),
        ],
    }
}

pub fn budgets() -> Vec<Budget> {
    let budget = |label: &str, scope: &str, cap_usd: f64, today_usd: f64| Budget {
        label: label.to_string(),
        scope: scope.to_string(),
        cap_usd,
        today_usd,
    };
    vec![
        budget("builder (role)", "role:builder", 20.0, 17.2),
        budget("reviewer (role)", "role:reviewer", 12.0, 6.1),
        budget("researcher (role)", "role:researcher", 10.0, 9.8),
        budget("ops (role)", "role:ops", 5.0, 1.2),
        budget("ada (agent)", "agent:4417", 10.0, 9.4),
    ]
}

/// The report of a node, by its id (`n1`) or its scope id (`node:n1`).
pub fn report(node: &str) -> serde_json::Value {
    match node.strip_prefix("node:").unwrap_or(node) {
        "n1" => json!({
            "top_processes": [
                ["rustc", "r12/1", 92, 1400],
                ["cargo", "r12/1", 34, 860],
                ["qemu-system-x86_64", "r15/2", 27, 2100],
                ["postgres", "hub", 3, 310]
            ],
            "gpus": [
                {"name": "GPU 0", "util_pct": 64, "mem_used_mb": 9120, "mem_total_mb": 24576}
            ]
        }),
        "n0" => json!({
            "top_processes": [
                ["hub", "hub", 18, 640],
                ["postgres", "hub", 7, 1480]
            ],
            "gpus": []
        }),
        "n2" => json!({
            "top_processes": [
                ["qemu-system-x86_64", "r19/3", 22, 2300],
                ["noded", "n2", 2, 48]
            ],
            "gpus": []
        }),
        _ => serde_json::Value::Null,
    }
}

pub fn rate_limits() -> Vec<(String, Option<f64>, Option<f64>)> {
    vec![
        ("anthropic".to_string(), Some(72.0), Some(41.0)),
        ("openai".to_string(), Some(12.0), Some(28.0)),
        ("local".to_string(), None, None),
    ]
}

pub fn outside() -> Vec<(String, bool, Option<f64>, String)> {
    vec![
        (
            "anthropic API".to_string(),
            true,
            Some(2380.0),
            "—".to_string(),
        ),
        (
            "openai API".to_string(),
            false,
            Some(1900.0),
            "429 rate limited · 11:48".to_string(),
        ),
        (
            "local inference (n1)".to_string(),
            true,
            Some(240.0),
            "—".to_string(),
        ),
        (
            "MCP: linear".to_string(),
            true,
            Some(610.0),
            "—".to_string(),
        ),
        ("garage S3".to_string(), true, Some(9.0), "—".to_string()),
    ]
}

pub fn links() -> Vec<(String, String, u64, f64, f64, u32, f64, f64)> {
    vec![
        (
            "n0".to_string(),
            "hub box".to_string(),
            2_000,
            4.0,
            0.2,
            0,
            0.0,
            900.0,
        ),
        (
            "n1".to_string(),
            "rack-a".to_string(),
            6_000,
            120.0,
            1.1,
            0,
            -18.0,
            2_600.0,
        ),
        (
            "n2".to_string(),
            "rack-b".to_string(),
            11_000,
            840.0,
            2.3,
            2,
            640.0,
            3_100.0,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;

    #[test]
    fn events_filter_by_kind_and_scope_and_come_newest_first() {
        let f = Fixture::new(NOW);
        let wakes = events(&f, "global", NOW - DAY_MS, NOW, &["wake"], 100);
        assert!(!wakes.is_empty());
        assert!(wakes.iter().all(|e| e.kind == "wake"));
        assert!(wakes.windows(2).all(|w| w[0].t_ms >= w[1].t_ms));

        let limited = events(&f, "global", NOW - DAY_MS, NOW, &[], 5);
        assert_eq!(limited.len(), 5);

        let n1 = events(&f, "node:n1", NOW - DAY_MS, NOW, &[], 100);
        assert!(!n1.is_empty());
        assert!(
            n1.iter()
                .all(|e| !e.entity.starts_with("r19/") && e.entity != "n2")
        );
        assert!(n1.windows(2).all(|w| w[0].t_ms >= w[1].t_ms));
    }

    #[test]
    fn the_alerts_state_has_two_firing_subjects_and_an_slo_over_its_budget() {
        let state = alerts(&Fixture::new(NOW));
        let firing = state
            .subjects
            .iter()
            .filter(|s| matches!(s.2, State::Firing { .. }))
            .count();
        assert_eq!(firing, 2);
        assert!(
            state
                .slos
                .iter()
                .any(|(_, status)| status.budget_left < 0.0)
        );
    }
}

/// The fixture's price list: every model it calls is priced except the local one, fetched an hour before now.
pub fn price_list(f: &Fixture, models: &[String]) -> Prices {
    // (model, source, input, output, cache read, cache write, used by)
    const RATES: &[(&str, &str, f64, f64, Option<f64>, Option<f64>, &str)] = &[
        (
            "anthropic/opus-5.5",
            "OpenRouter",
            15.0,
            75.0,
            Some(1.5),
            Some(18.75),
            "planner, reviewer",
        ),
        (
            "anthropic/sonnet-5.5",
            "OpenRouter",
            3.0,
            15.0,
            Some(0.3),
            Some(3.75),
            "builder",
        ),
        (
            "anthropic/haiku-5.5",
            "LiteLLM",
            0.8,
            4.0,
            Some(0.08),
            Some(1.0),
            "scout",
        ),
        ("openai/gpt-5", "LiteLLM", 1.25, 10.0, Some(0.125), None, ""),
    ];
    let mut names: Vec<String> = models.to_vec();
    for (model, ..) in RATES {
        if !names.iter().any(|n| n == model) {
            names.push((*model).to_string());
        }
    }
    let rows = names
        .into_iter()
        .map(|model| match RATES.iter().find(|r| r.0 == model) {
            Some(&(_, source, input, output, cache_read, cache_write, used_by)) => PriceRow {
                model,
                source: source.into(),
                input: Some(input),
                output: Some(output),
                cache_read,
                cache_write,
                used_by: used_by.into(),
            },
            None => PriceRow {
                model,
                ..PriceRow::default()
            },
        })
        .collect();
    Prices {
        fetched_at_ms: f.now_ms.saturating_sub(3_600_000),
        sources: vec!["OpenRouter".into(), "LiteLLM".into()],
        rows,
    }
}
