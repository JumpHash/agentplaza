//! The collection budget, measured on a simulated node: 50 VMs for ten simulated minutes, all in process. No clock
//! sleeps; the simulation steps a clock by hand, and the recording cost is timed with `Instant` around the recording
//! loops only. The budgets are the plan's (section 6.1).
//!
//! The workload is realistic. Each gauge is a slow random walk (a step of at most 0.3 % of its level, shown to one
//! decimal place, as the consoles show it). Gauges are sampled every 10 s and the node beats every 15 s, so a beat
//! carries one or two points per series. Most VMs are idle: their values do not change, so their points are skipped
//! on the wire. A heartbeat is measured as the batch's JSON, deflated and base64 encoded, the form the node sends.

use std::collections::BTreeMap;
use std::io::Write;
use std::time::{Duration, Instant};

use apz_metrics::{
    Closed, EntityKind, Gauge, Hist, Point, Registry, Rollup, SeriesKey, Tier, chunk, wire,
};
use base64::Engine as _;
use flate2::Compression;
use flate2::write::DeflateEncoder;

/// Gauges are sampled, and the rollup's windows are closed, on this grid.
const SAMPLE_MS: u64 = 10_000;
/// The node sends a heartbeat on this grid.
const BEAT_MS: u64 = 15_000;
/// The clock steps on the greatest common step of the sample and beat intervals, so both fall on a tick.
const TICK_MS: u64 = 5_000;
const RUN_MS: u64 = 600_000;
const VMS: usize = 50;
/// Of the VMs, the first few are busy and the rest idle. The node's own gauges are always busy.
const BUSY_VMS: usize = 3;
/// The heartbeat budget is stated for 20 VMs, so the batch carries that many.
const VMS_IN_BATCH: usize = 20;
const VM_GAUGES: usize = 15;
const HOST_GAUGES: usize = 25;
const HISTS: usize = 20;
/// Histogram values recorded per sample, spread over the histograms.
const RECORDS_PER_SAMPLE: usize = 50;

/// The catalog's gauges keyed by a VM. The catalog has nine; the rest of the fifteen are the same ids under a
/// `slot` label, so the series count matches the plan's budget.
const VM_IDS: [&str; 9] = [
    "vm.cpu_pct",
    "vm.rss_mib",
    "vm.guest_used_mib",
    "vm.guest_total_mib",
    "vm.blk_read_bps",
    "vm.blk_write_bps",
    "vm.vsock_rx_bps",
    "vm.vsock_tx_bps",
    "vm.workspace_used_mib",
];

/// The catalog's gauges keyed by a node. The catalog has eighteen; the rest of the twenty-five are the same ids
/// under a `slot` label.
const NODE_IDS: [&str; 18] = [
    "host.cpu_pct",
    "host.mem_used_mib",
    "host.mem_total_mib",
    "host.load1",
    "host.disk_read_bps",
    "host.disk_write_bps",
    "host.net_rx_bps",
    "host.net_tx_bps",
    "host.psi_cpu_pct",
    "host.psi_mem_pct",
    "host.psi_io_pct",
    "host.gpu_util_pct",
    "host.gpu_mem_pct",
    "vm.count",
    "life.snapshot_mib",
    "link.rtt_ms",
    "link.clock_offset_ms",
    "link.commands_queued",
];

/// A gauge's slow random walk. Each step moves the level by at most 0.3 % of itself.
struct Walk {
    level: f64,
    state: u64,
}

impl Walk {
    fn new(series: usize) -> Walk {
        Walk {
            level: 5.0 + (series % 97) as f64 * 3.7,
            state: 0x9e37_79b9_7f4a_7c15 ^ series as u64,
        }
    }

    /// A uniform number in [-1, 1).
    fn next_unit(&mut self) -> f64 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.state >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    }

    /// The reading the walk is at, shown to one decimal place.
    fn reading(&self) -> f64 {
        (self.level * 10.0).round() / 10.0
    }

    fn step(&mut self) -> f64 {
        self.level += self.level * 0.003 * self.next_unit();
        self.reading()
    }
}

/// A deterministic latency stream in milliseconds (a linear congruential generator).
struct Latency(u64);

impl Latency {
    fn next_ms(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let unit = (self.0 >> 11) as f64 / (1u64 << 53) as f64;
        120.0 + 900.0 * unit * unit
    }
}

/// `count` keys over `ids`, the second and later rounds carrying a `slot` label.
fn gauge_keys(kind: EntityKind, entity: &str, ids: &[&str], count: usize) -> Vec<SeriesKey> {
    (0..count)
        .map(|i| {
            let round = i / ids.len();
            let key = SeriesKey::new(ids[i % ids.len()], kind, entity);
            if round == 0 {
                key
            } else {
                key.label("slot", &round.to_string())
            }
        })
        .collect()
}

/// The heartbeat's size as the node sends it: the batch's JSON, deflated and base64 encoded. Returns the JSON size
/// and the sent size.
fn heartbeat_sizes(batch: &wire::Batch) -> (usize, usize) {
    let json = serde_json::to_vec(batch).expect("a batch serializes");
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::new(6));
    encoder.write_all(&json).expect("deflate to memory");
    let deflated = encoder.finish().expect("deflate to memory");
    let sent = base64::engine::general_purpose::STANDARD.encode(deflated);
    (json.len(), sent.len())
}

#[test]
fn a_simulated_node_of_fifty_vms_stays_inside_the_collection_budget() {
    let registry = Registry::default();

    // Node gauges first, then the VMs in order, so the heartbeat batch is a prefix of the list.
    let mut keys = gauge_keys(EntityKind::Node, "r1", &NODE_IDS, HOST_GAUGES);
    for vm in 0..VMS {
        keys.extend(gauge_keys(
            EntityKind::Vm,
            &format!("r1/{vm}"),
            &VM_IDS,
            VM_GAUGES,
        ));
    }
    let gauges: Vec<Gauge> = keys.iter().map(|key| registry.gauge(key.clone())).collect();
    let hists: Vec<Hist> = (0..HISTS)
        .map(|i| {
            registry.hist(SeriesKey::new(
                "llm.ttft_ms",
                EntityKind::Agent,
                &format!("agent-{i}"),
            ))
        })
        .collect();
    let series = gauges.len() + hists.len();

    // Whether each gauge belongs to a busy source: the node's own, or one of the first busy VMs.
    let busy: Vec<bool> = (0..gauges.len())
        .map(|i| i < HOST_GAUGES || (i - HOST_GAUGES) / VM_GAUGES < BUSY_VMS)
        .collect();
    let mut walks: Vec<Walk> = (0..gauges.len()).map(Walk::new).collect();
    let mut current: Vec<f64> = walks.iter().map(Walk::reading).collect();
    // Every sample of every gauge, so a beat can send the samples since the last one.
    let mut histories: Vec<Vec<f64>> = vec![Vec::new(); gauges.len()];
    let mut sent_samples = 0usize;

    let mut rollup = Rollup::new();
    let mut closed: Vec<Closed> = Vec::new();
    let mut latency = Latency(7);
    let mut gauge_time = Duration::ZERO;
    let mut gauge_sets = 0u64;
    let mut hist_time = Duration::ZERO;
    let mut hist_records = 0u64;
    let mut beat_sizes: Vec<(usize, usize)> = Vec::new();

    for tick in 1..=RUN_MS / TICK_MS {
        let now = tick * TICK_MS;
        if now % SAMPLE_MS == 0 {
            for ((walk, value), &is_busy) in walks.iter_mut().zip(current.iter_mut()).zip(&busy) {
                if is_busy {
                    *value = walk.step();
                }
            }
            // The latencies are computed before the clock starts, so the timing covers only the recording.
            let latencies: Vec<f64> = (0..RECORDS_PER_SAMPLE).map(|_| latency.next_ms()).collect();

            let started = Instant::now();
            for (gauge, value) in gauges.iter().zip(&current) {
                gauge.set(*value);
            }
            gauge_time += started.elapsed();
            gauge_sets += gauges.len() as u64;

            let started = Instant::now();
            for (j, value) in latencies.iter().enumerate() {
                hists[j % HISTS].record(*value);
            }
            hist_time += started.elapsed();
            hist_records += latencies.len() as u64;

            for (history, value) in histories.iter_mut().zip(&current) {
                history.push(*value);
            }

            for sample in registry.scrape(now, false) {
                closed.extend(rollup.ingest(&sample));
            }
        }

        if now % BEAT_MS == 0 {
            let columns = (0..HOST_GAUGES + VMS_IN_BATCH * VM_GAUGES)
                .map(|i| {
                    // The samples since the last beat; sample k (counting from 1) was taken at k · SAMPLE_MS.
                    wire::encode_gauge(
                        keys[i].clone(),
                        (sent_samples as u64 + 1) * SAMPLE_MS,
                        SAMPLE_MS,
                        &histories[i][sent_samples..],
                    )
                })
                .collect();
            sent_samples = histories[0].len();
            let batch = wire::Batch {
                node: "r1".to_string(),
                sent_ms: now,
                series: columns,
                ..Default::default()
            };
            beat_sizes.push(heartbeat_sizes(&batch));
        }
    }
    closed.extend(rollup.flush_all());

    let mut points_by_series: BTreeMap<SeriesKey, Vec<Point>> = BTreeMap::new();
    for window in closed.iter().filter(|window| window.tier == Tier::Raw10s) {
        points_by_series
            .entry(window.key.clone())
            .or_default()
            .push((
                window.start_ms,
                chunk::quantize(window.agg.mean().unwrap_or(0.0)),
            ));
    }
    let mut chunk_bytes = 0usize;
    let mut points = 0usize;
    for points_of in points_by_series.values() {
        let bytes = chunk::encode(points_of);
        assert_eq!(
            chunk::decode(&bytes).expect("a chunk decodes"),
            *points_of,
            "a chunk must decode to the points it encoded"
        );
        chunk_bytes += bytes.len();
        points += points_of.len();
    }

    let gauge_ns = gauge_time.as_nanos() as f64 / gauge_sets as f64;
    let hist_ns = hist_time.as_nanos() as f64 / hist_records as f64;
    let limit_ns = if cfg!(debug_assertions) {
        5_000.0
    } else {
        1_000.0
    };
    let json_mean = beat_sizes.iter().map(|s| s.0).sum::<usize>() as f64 / beat_sizes.len() as f64;
    let sent_mean = beat_sizes.iter().map(|s| s.1).sum::<usize>() as f64 / beat_sizes.len() as f64;
    let sent_max = beat_sizes.iter().map(|s| s.1).max().unwrap_or(0);
    let bytes_per_point = chunk_bytes as f64 / points as f64;

    println!(
        "build: {}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    println!(
        "series: {} (registry {}), folded {}",
        series,
        registry.len(),
        registry.folded()
    );
    println!("gauge set: {gauge_ns:.1} ns (mean over {gauge_sets} sets; limit {limit_ns} ns)");
    println!(
        "histogram record: {hist_ns:.1} ns (mean over {hist_records} records; limit {limit_ns} ns)"
    );
    println!(
        "heartbeat, {VMS_IN_BATCH} VMs: mean {sent_mean:.0} B deflated and base64 ({json_mean:.0} B of JSON) over {} beats, max {sent_max} B",
        beat_sizes.len()
    );
    println!(
        "chunks: {chunk_bytes} B for {points} points over {} series = {bytes_per_point:.3} B per point",
        points_by_series.len()
    );

    assert_eq!(
        registry.len(),
        VMS * VM_GAUGES + HOST_GAUGES + HISTS,
        "the series count"
    );
    assert_eq!(registry.folded(), 0, "no series may fold into overflow");
    assert_eq!(
        beat_sizes.len(),
        (RUN_MS / BEAT_MS) as usize,
        "one beat per heartbeat interval"
    );
    assert!(
        gauge_ns < limit_ns,
        "a gauge set costs {gauge_ns:.1} ns, over the {limit_ns} ns budget"
    );
    assert!(
        hist_ns < limit_ns,
        "a histogram record costs {hist_ns:.1} ns, over the {limit_ns} ns budget"
    );
    assert!(
        sent_mean < 4_096.0,
        "a heartbeat for {VMS_IN_BATCH} VMs averages {sent_mean:.0} B on the wire, over the 4 KB budget"
    );
    assert!(
        bytes_per_point < 2.5,
        "chunks cost {bytes_per_point:.3} B per point, over the 2.5 B budget"
    );
}
