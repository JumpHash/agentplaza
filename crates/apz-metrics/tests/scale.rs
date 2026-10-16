//! The cost of metrics for a plaza of 1000 agents on 10 nodes of 100 VMs, measured on the real registry, rollup,
//! chunk encoding and wire format: the hub's CPU per 10 s tick and its memory, the storage a day writes, and the
//! heartbeat a node of 100 VMs sends. It simulates one hour on a hand-stepped clock and prints the numbers.
//!
//! Run it with `cargo test --release -p apz-metrics --test scale -- --ignored --nocapture`.
//!
//! The workload: a quarter of the agents work at any time and make one model call per 10 s each (one turn per
//! minute, one tool call per 20 s); the others are idle or asleep and only their state time moves. Each agent has the
//! hub's per-agent series (calls, tokens, spend and cache by model; first token, call, turn and tool latencies; the
//! six context parts and the fill; time per state; compactions; infractions). The nodes' VM gauges arrive as samples
//! for the VMs that changed (a busy VM every 10 s, an idle one every 60 s, as the levels sample them).

use std::collections::HashMap;
use std::io::Write;
use std::time::{Duration, Instant};

use apz_metrics::wire;
use apz_metrics::{
    Agg, Closed, EntityKind, Registry, Rollup, Sample, SeriesKey, Tier, Value, chunk,
};
use flate2::Compression;
use flate2::write::DeflateEncoder;

const AGENTS: usize = 1000;
const NODES: usize = 10;
const VMS_PER_NODE: usize = AGENTS / NODES;
/// One agent in four works at any time.
const BUSY_EVERY: usize = 4;
const TICK_MS: u64 = 10_000;
const HOUR_MS: u64 = 3_600_000;
const VM_GAUGES: [&str; 10] = [
    "vm.cpu_pct",
    "vm.rss_mib",
    "vm.guest_used_mib",
    "vm.guest_total_mib",
    "vm.blk_read_bps",
    "vm.blk_write_bps",
    "vm.vsock_rx_bps",
    "vm.vsock_tx_bps",
    "vm.workspace_used_mib",
    "vm.blk_ops",
];
const MODELS: [&str; 5] = [
    "anthropic/opus-5.5",
    "anthropic/sonnet-5.5",
    "anthropic/haiku-5.5",
    "openai/gpt-5",
    "local/qwen3-32b",
];
const PARTS: [&str; 6] = [
    "system",
    "tools",
    "history",
    "tool_results",
    "attachments",
    "summary",
];
const STATES: [&str; 5] = ["working", "idle", "waiting", "asleep", "blocked"];

/// The resident memory of this process, in bytes (Linux; 0 elsewhere).
fn rss_bytes() -> u64 {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
        .map_or(0, |pages| pages * 4096)
}

fn deflated_len(bytes: &[u8]) -> usize {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::new(6));
    encoder.write_all(bytes).expect("deflate to memory");
    encoder.finish().expect("deflate to memory").len()
}

/// A small deterministic generator.
struct Lcg(u64);

impl Lcg {
    fn unit(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

struct AgentSeries {
    calls: apz_metrics::Counter,
    tokens_in: apz_metrics::Counter,
    tokens_out: apz_metrics::Counter,
    cache_read: apz_metrics::Counter,
    cache_write: apz_metrics::Counter,
    spend: apz_metrics::Counter,
    ttft: apz_metrics::Hist,
    duration: apz_metrics::Hist,
    turn: apz_metrics::Hist,
    tool: apz_metrics::Hist,
    parts: Vec<apz_metrics::Gauge>,
    fill: apz_metrics::Gauge,
    states: Vec<apz_metrics::Counter>,
    compactions: apz_metrics::Counter,
    infractions: apz_metrics::Counter,
}

fn agent_series(registry: &Registry, cid: usize) -> AgentSeries {
    let entity = cid.to_string();
    let model = MODELS[cid % MODELS.len()];
    let provider = model.split('/').next().unwrap_or_default();
    let by_model = |id: &str| {
        SeriesKey::new(id, EntityKind::Agent, &entity)
            .label("model", model)
            .label("provider", provider)
    };
    let plain = |id: &str| SeriesKey::new(id, EntityKind::Agent, &entity);
    AgentSeries {
        calls: registry.counter(by_model("llm.calls")),
        tokens_in: registry.counter(by_model("llm.tokens_in")),
        tokens_out: registry.counter(by_model("llm.tokens_out")),
        cache_read: registry.counter(by_model("llm.tokens_cache_read")),
        cache_write: registry.counter(by_model("llm.tokens_cache_write")),
        spend: registry.counter(by_model("llm.spend_usd")),
        ttft: registry.hist(by_model("llm.ttft_ms")),
        duration: registry.hist(by_model("llm.duration_ms")),
        turn: registry.hist(plain("turn.duration_ms")),
        tool: registry.hist(plain("tool.duration_ms")),
        parts: PARTS
            .iter()
            .map(|part| registry.gauge(plain("ctx.tokens").label("part", part)))
            .collect(),
        fill: registry.gauge(plain("ctx.fill_pct")),
        states: STATES
            .iter()
            .map(|state| registry.counter(plain("agent.state_s").label("state", state)))
            .collect(),
        compactions: registry.counter(plain("ctx.compactions")),
        infractions: registry.counter(plain("law.infractions").label("law", "egress")),
    }
}

#[test]
#[ignore = "a measurement, not a check: run it with --release --ignored --nocapture"]
fn the_cost_of_metrics_for_a_thousand_agents() {
    let rss_start = rss_bytes();
    let registry = Registry::new(1_000_000);
    let agents: Vec<AgentSeries> = (0..AGENTS)
        .map(|cid| agent_series(&registry, cid))
        .collect();
    let agent_series_count = registry.len();
    let vm_keys: Vec<Vec<SeriesKey>> = (0..AGENTS)
        .map(|vm| {
            let entity = format!(
                "n{}:r{}/{}",
                vm / VMS_PER_NODE,
                vm / VMS_PER_NODE,
                vm % VMS_PER_NODE
            );
            VM_GAUGES
                .iter()
                .map(|id| SeriesKey::new(id, EntityKind::Vm, &entity))
                .collect()
        })
        .collect();
    let mut levels: Vec<Vec<f64>> = (0..AGENTS)
        .map(|vm| {
            VM_GAUGES
                .iter()
                .enumerate()
                .map(|(i, _)| 10.0 + ((vm * 7 + i) % 90) as f64)
                .collect()
        })
        .collect();

    let mut rollup = Rollup::new();
    let mut store = StoreModel::default();
    let mut rng = Lcg(42);
    let mut tick_time = Duration::ZERO;
    let mut record_time = Duration::ZERO;
    let mut store_time = Duration::ZERO;
    let mut samples_in = 0usize;

    for tick in 1..=HOUR_MS / TICK_MS {
        let now = tick * TICK_MS;
        // What the hub records as it serves agents (the hot path).
        let started = Instant::now();
        for (cid, a) in agents.iter().enumerate() {
            let busy = (cid + tick as usize / 30) % BUSY_EVERY == 0;
            if busy {
                let tokens = 20_000.0 + 30_000.0 * rng.unit();
                a.calls.inc();
                a.tokens_in.add(tokens * 0.3);
                a.cache_read.add(tokens * 0.7);
                a.cache_write.add(500.0);
                a.tokens_out.add(400.0 + 600.0 * rng.unit());
                a.spend.add(0.01 + 0.03 * rng.unit());
                a.ttft.record(800.0 + 2_000.0 * rng.unit());
                a.duration.record(3_000.0 + 9_000.0 * rng.unit());
                if tick % 2 == 0 {
                    a.tool.record(50.0 + 900.0 * rng.unit());
                }
                if tick % 6 == 0 {
                    a.turn.record(20_000.0 + 60_000.0 * rng.unit());
                    for (i, part) in a.parts.iter().enumerate() {
                        part.set((tokens * (i + 1) as f64 / 21.0).round());
                    }
                    a.fill.set((tokens / 2_000.0).round());
                }
                a.states[0].add(10.0);
            } else {
                a.states[if cid % 3 == 0 { 3 } else { 1 }].add(10.0);
            }
            if tick % 360 == cid as u64 % 360 && cid % 20 == 0 {
                a.compactions.inc();
                a.infractions.inc();
            }
        }
        record_time += started.elapsed();

        // The node batches' samples: busy VMs every tick, idle VMs every minute.
        let mut vm_samples: Vec<Sample> = Vec::new();
        for (vm, keys) in vm_keys.iter().enumerate() {
            let busy = (vm + tick as usize / 30) % BUSY_EVERY == 0;
            if busy || tick % 6 == 0 {
                for (i, key) in keys.iter().enumerate() {
                    if busy {
                        levels[vm][i] *= 1.0 + 0.01 * (rng.unit() - 0.5);
                    }
                    vm_samples.push(Sample {
                        key: key.clone(),
                        t_ms: now,
                        value: Value::Gauge((levels[vm][i] * 10.0).round() / 10.0),
                    });
                }
            }
        }

        // The hub's tick: scrape its own series, ingest them and the nodes' samples, close due windows.
        let started = Instant::now();
        let mut samples = registry.scrape(now, false);
        samples.extend(vm_samples);
        samples_in += samples.len();
        let mut closed: Vec<Closed> = Vec::new();
        for s in &samples {
            closed.extend(rollup.ingest(s));
        }
        closed.extend(rollup.flush_due(now));
        tick_time += started.elapsed();

        // The store: buffer the closed windows and write what is due, as the Postgres store does every 10 s.
        let started = Instant::now();
        store.put(&closed);
        store.flush(now, false);
        store_time += started.elapsed();
    }
    // The hour's end, just before its minute windows are written: the most the hub holds.
    let rss_peak = rss_bytes();
    // The hour's last windows close, then the hour's rows are written whole.
    store.put(&rollup.flush_all());
    store.flush(HOUR_MS + 1, true);

    // The heartbeat of one node of 100 VMs: 15 s of samples, deflated and base64, as noded sends it.
    let node_columns: Vec<wire::Column> = vm_keys[..VMS_PER_NODE]
        .iter()
        .enumerate()
        .flat_map(|(vm, keys)| {
            let busy = vm % BUSY_EVERY == 0;
            keys.iter()
                .enumerate()
                .map(move |(i, key)| {
                    let base = 10.0 + ((vm * 7 + i) % 90) as f64;
                    let values: Vec<f64> = if busy {
                        vec![base, base * 1.004]
                    } else {
                        vec![base, base]
                    };
                    wire::encode_gauge(key.clone(), 0, TICK_MS, &values)
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let batch = wire::Batch {
        node: "n0".into(),
        sent_ms: 15_000,
        series: node_columns,
        ..Default::default()
    };
    let batch_json = serde_json::to_vec(&batch).expect("a batch serializes");
    let beat_bytes = deflated_len(&batch_json) * 4 / 3;

    let ticks = (HOUR_MS / TICK_MS) as f64;
    let per_tick = |d: Duration| d.as_secs_f64() * 1000.0 / ticks;
    let (tick_ms, record_ms, store_ms) = (
        per_tick(tick_time),
        per_tick(record_time),
        per_tick(store_time),
    );
    let mib = |b: f64| b / (1024.0 * 1024.0);
    // A row costs its tuple header, its index entry and its key columns besides the data: about 60 B.
    const ROW_OVERHEAD: f64 = 60.0;
    let per_day =
        |bytes: usize, rows: usize| mib((bytes as f64 + rows as f64 * ROW_OVERHEAD) * 24.0);
    let chunks_day = per_day(store.chunk_bytes, store.chunk_rows);
    let minutes_day = per_day(store.folded_bytes, store.minute_rows / 6);
    let hours_day = per_day(store.hour_bytes, store.hour_rows) / 24.0;
    println!(
        "build: {}",
        if cfg!(debug_assertions) {
            "debug (use --release)"
        } else {
            "release"
        }
    );
    println!(
        "series: {} recorded by the hub ({} per agent), {} VM series from {NODES} nodes",
        registry.len(),
        agent_series_count / AGENTS,
        AGENTS * VM_GAUGES.len()
    );
    println!(
        "samples into the rollup: {:.0} per 10 s tick",
        samples_in as f64 / ticks
    );
    println!(
        "hub CPU per 10 s tick: scrape + rollup {tick_ms:.1} ms, store buffering and encoding {store_ms:.1} ms = {:.2} % of one core; recording on the hot path {record_ms:.2} ms ({:.3} %)",
        (tick_ms + store_ms) / 100.0,
        record_ms / 100.0
    );
    println!(
        "hub memory at the hour's end (registry, open windows, the store's unwritten windows): {:.0} MiB over the start",
        mib(rss_peak.saturating_sub(rss_start) as f64)
    );
    println!(
        "disk per day: 10 s chunks {chunks_day:.0} MiB ({:.2} B per point), 1 min rows {minutes_day:.0} MiB, 1 h rows {hours_day:.1} MiB",
        store.chunk_bytes as f64 / store.chunk_points as f64,
    );
    println!(
        "kept at the default retention (48 h of chunks, 14 d of minutes, 400 d of hours): {:.0} + {:.0} + {:.0} MiB",
        chunks_day * 2.0,
        minutes_day * 14.0,
        hours_day * 400.0
    );
    println!(
        "Postgres writes per day: {:.0} MiB (each 10-minute block once, each hour folded once, the day's hour rows hourly)",
        mib(store.write_bytes as f64 * 24.0)
    );
    println!(
        "node heartbeat, {VMS_PER_NODE} VMs: {beat_bytes} B per 15 s beat ({} B of JSON) = {:.2} KiB/s per node, {:.1} KiB/s into the hub from {NODES} nodes",
        batch_json.len(),
        beat_bytes as f64 / 15.0 / 1024.0,
        beat_bytes as f64 * NODES as f64 / 15.0 / 1024.0
    );
}

/// The Postgres store's buffering and write cadence (crates/apid/src/metrics/pg.rs), counting bytes instead of
/// writing: the open 10-minute chunk of each series, the open hour's 1-minute windows and the open day's 1-hour
/// windows per series; a minute row (ten windows of a series) written once when its windows have all arrived.
#[derive(Default)]
struct StoreModel {
    chunks: HashMap<SeriesKey, (u64, Vec<(u64, f64)>)>,
    minutes: HashMap<(SeriesKey, u64), Vec<(u64, Agg)>>,
    hours: HashMap<SeriesKey, Vec<(u64, Agg)>>,
    /// One series in ten keeps its hour of windows, to size the rows the hourly fold writes (scaled by ten).
    sampled: HashMap<SeriesKey, Vec<(u64, Agg)>>,
    folded_bytes: usize,
    chunk_bytes: usize,
    chunk_rows: usize,
    chunk_points: usize,
    minute_final_bytes: usize,
    minute_rows: usize,
    hour_bytes: usize,
    hour_rows: usize,
    write_bytes: usize,
}

impl StoreModel {
    fn put(&mut self, closed: &[Closed]) {
        for w in closed {
            match w.tier {
                Tier::Raw10s => {
                    if w.agg.hist.is_some() {
                        continue;
                    }
                    let start = w.start_ms - w.start_ms % 600_000;
                    let point = (w.start_ms, chunk::quantize(w.agg.last));
                    let entry = self
                        .chunks
                        .entry(w.key.clone())
                        .or_insert((start, Vec::new()));
                    if entry.0 != start {
                        let points = std::mem::take(&mut entry.1);
                        entry.0 = start;
                        self.write_chunk(&points);
                    }
                    self.chunks
                        .get_mut(&w.key)
                        .expect("the entry is there")
                        .1
                        .push(point);
                }
                Tier::Min1 => {
                    if w.key.id().len() % 10 == 0 {
                        self.sampled
                            .entry(w.key.clone())
                            .or_default()
                            .push((w.start_ms, w.agg.clone()));
                    }
                    let block = w.start_ms - w.start_ms % 600_000;
                    self.minutes
                        .entry((w.key.clone(), block))
                        .or_default()
                        .push((w.start_ms, w.agg.clone()));
                }
                Tier::Hour1 => {
                    self.hours
                        .entry(w.key.clone())
                        .or_default()
                        .push((w.start_ms, w.agg.clone()));
                }
            }
        }
    }

    fn write_chunk(&mut self, points: &[(u64, f64)]) {
        if points.is_empty() {
            return;
        }
        let bytes = chunk::encode(points).len();
        self.chunk_bytes += bytes;
        self.write_bytes += bytes;
        self.chunk_rows += 1;
        self.chunk_points += points.len();
    }

    /// One flush at `now`: minute blocks whose windows have all arrived are written once and leave memory;
    /// `last` writes everything, as at the hour's end.
    fn flush(&mut self, now: u64, last: bool) {
        let settled: Vec<(SeriesKey, u64)> = self
            .minutes
            .keys()
            .filter(|(_, block)| last || block + 600_000 + 200_000 <= now)
            .cloned()
            .collect();
        for id in settled {
            if let Some(windows) = self.minutes.remove(&id) {
                let row = pack(&windows);
                self.write_bytes += row;
                self.minute_final_bytes += row;
                self.minute_rows += 1;
            }
        }
        if last {
            // The fold: each series' six block rows become one row of the hour (sampled series, scaled).
            let sampled: usize = self.sampled.values().map(|windows| pack(windows)).sum();
            self.folded_bytes = sampled * self.minute_rows / self.sampled.len().max(1) / 6;
            self.write_bytes += self.folded_bytes;
            let open: Vec<Vec<(u64, f64)>> =
                self.chunks.drain().map(|(_, (_, points))| points).collect();
            for points in open {
                self.write_chunk(&points);
            }
            // A day's hour row holds 24 windows: the simulated hour's window, at each hour of the day.
            for windows in self.hours.values() {
                if let Some((start, agg)) = windows.first() {
                    let day: Vec<(u64, Agg)> = (0..24)
                        .map(|h| (start + h * 3_600_000, agg.clone()))
                        .collect();
                    let row = pack(&day);
                    self.hour_bytes += row;
                    self.write_bytes += row / 24;
                    self.hour_rows += 1;
                }
            }
        }
    }
}

/// A row of windows as the store writes it: JSON, deflated.
fn pack(windows: &[(u64, Agg)]) -> usize {
    deflated_len(&serde_json::to_vec(windows).expect("windows serialize"))
}
