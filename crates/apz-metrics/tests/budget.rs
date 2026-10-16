//! The collection budget, measured on a simulated node: 50 VMs for ten simulated minutes, all in process. No clock
//! sleeps; the simulation steps a counter, and the recording cost is timed with `Instant` around the recording
//! loops only. The budgets are the plan's (section 6.1).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use apz_metrics::{
    Closed, EntityKind, Gauge, Hist, Point, Registry, Rollup, SeriesKey, Tier, chunk, wire,
};

/// Gauges are set and histograms recorded every step.
const STEP_MS: u64 = 2_000;
/// Every scrape, rollup and heartbeat happens on this grid.
const SCRAPE_MS: u64 = 10_000;
const RUN_MS: u64 = 600_000;
const VMS: usize = 50;
/// The heartbeat budget is stated for 20 VMs, so the batch carries that many.
const VMS_IN_BATCH: usize = 20;
const VM_GAUGES: usize = 15;
const HOST_GAUGES: usize = 25;
const HISTS: usize = 20;
/// Histogram values recorded per step, spread over the histograms.
const RECORDS_PER_STEP: usize = 50;
/// Readings per 10 s window at the 2 s step.
const WINDOW_READINGS: usize = 5;

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

/// A slowly drifting reading, rounded to one decimal place as the consoles show it.
fn gauge_reading(series: usize, t_ms: u64) -> f64 {
    let base = 10.0 + (series % 60) as f64;
    let phase = series as f64 * 0.37;
    let wave = 4.0 * (t_ms as f64 / 90_000.0 + phase).sin();
    ((base + wave) * 10.0).round() / 10.0
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

    let mut rollup = Rollup::new();
    let mut readings: Vec<Vec<f64>> = vec![Vec::new(); gauges.len()];
    let mut closed: Vec<Closed> = Vec::new();
    let mut latency = Latency(7);
    let mut gauge_time = Duration::ZERO;
    let mut gauge_sets = 0u64;
    let mut hist_time = Duration::ZERO;
    let mut hist_records = 0u64;
    let mut batch_sizes: Vec<usize> = Vec::new();

    for step in 1..=RUN_MS / STEP_MS {
        let now = step * STEP_MS;
        // The values are computed before the clock starts, so the timing covers only the recording.
        let values: Vec<f64> = (0..gauges.len()).map(|i| gauge_reading(i, now)).collect();
        let latencies: Vec<f64> = (0..RECORDS_PER_STEP).map(|_| latency.next_ms()).collect();

        let started = Instant::now();
        for (gauge, value) in gauges.iter().zip(&values) {
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

        for (history, value) in readings.iter_mut().zip(&values) {
            history.push(*value);
        }

        if now % SCRAPE_MS == 0 {
            for sample in registry.scrape(now, false) {
                closed.extend(rollup.ingest(&sample));
            }
            let columns = (0..HOST_GAUGES + VMS_IN_BATCH * VM_GAUGES)
                .map(|i| {
                    let history = &readings[i];
                    let window = &history[history.len() - WINDOW_READINGS..];
                    wire::encode_gauge(
                        keys[i].clone(),
                        now - (WINDOW_READINGS as u64 - 1) * STEP_MS,
                        STEP_MS,
                        window,
                    )
                })
                .collect();
            let batch = wire::Batch {
                node: "r1".to_string(),
                sent_ms: now,
                series: columns,
                ..Default::default()
            };
            batch_sizes.push(
                serde_json::to_vec(&batch)
                    .expect("a batch serializes")
                    .len(),
            );
        }
    }
    closed.extend(rollup.flush_all());

    let mut points_by_series: BTreeMap<SeriesKey, Vec<Point>> = BTreeMap::new();
    for window in closed.iter().filter(|window| window.tier == Tier::Raw10s) {
        points_by_series
            .entry(window.key.clone())
            .or_default()
            .push((window.start_ms, window.agg.mean().unwrap_or(0.0)));
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
    let batch_mean = batch_sizes.iter().sum::<usize>() as f64 / batch_sizes.len() as f64;
    let batch_max = batch_sizes.iter().max().copied().unwrap_or(0);
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
        "heartbeat batch, {VMS_IN_BATCH} VMs: mean {batch_mean:.0} B of JSON over {} batches, max {batch_max} B",
        batch_sizes.len()
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
        batch_sizes.len(),
        (RUN_MS / SCRAPE_MS) as usize,
        "one batch per scrape"
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
        batch_mean < 4_096.0,
        "a heartbeat for {VMS_IN_BATCH} VMs averages {batch_mean:.0} B, over the 4 KB budget"
    );
    assert!(
        bytes_per_point < 2.5,
        "chunks cost {bytes_per_point:.3} B per point, over the 2.5 B budget"
    );
}
