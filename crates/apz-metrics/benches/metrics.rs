//! Micro-benchmarks of the metrics hot path: a counter increment and a histogram record, each once with the series
//! key built at the call (the hub's pattern) and once on a handle the caller holds. The registry holds 1000 series
//! of each kind, so the keyed lookups find existing series as they do after warm-up.
//!
//! Run with `cargo bench -p apz-metrics`.

use apz_metrics::registry::{DEFAULT_CAP, Registry};
use apz_metrics::{EntityKind, SeriesKey};
use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;

const SERIES: usize = 1000;
const VALUES: usize = 1024;
const COUNTER: &str = "requests";
const HIST: &str = "llm.ttft_ms";

/// The entity ids of the pre-populated series: agents `0..SERIES`.
fn entity_ids() -> Vec<String> {
    (0..SERIES).map(|i| i.to_string()).collect()
}

/// `SERIES` counters and `SERIES` histograms, one of each per agent.
fn populated(ids: &[String]) -> Registry {
    let registry = Registry::new(DEFAULT_CAP);
    for id in ids {
        registry
            .counter(SeriesKey::new(COUNTER, EntityKind::Agent, id))
            .inc();
        registry
            .hist(SeriesKey::new(HIST, EntityKind::Agent, id))
            .record(1.0);
    }
    registry
}

/// Latencies in milliseconds, spread over four decades (0.1 ms to 1 s), from a fixed xorshift sequence so every
/// run records the same values.
fn sample_values() -> Vec<f64> {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    (0..VALUES)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let unit = (x >> 11) as f64 / (1u64 << 53) as f64;
            0.1 * 10f64.powf(unit * 4.0)
        })
        .collect()
}

fn counter_benches(c: &mut Criterion) {
    let ids = entity_ids();
    let registry = populated(&ids);

    c.bench_function("counter/inc_by_key", |b| {
        let mut i = 0usize;
        b.iter(|| {
            i = i.wrapping_add(1);
            let entity: &str = &ids[i % SERIES];
            let key = SeriesKey::new(black_box(COUNTER), EntityKind::Agent, black_box(entity));
            registry.counter(key).inc();
        });
    });

    c.bench_function("counter/inc_held_handle", |b| {
        let handle = registry.counter(SeriesKey::new(COUNTER, EntityKind::Agent, &ids[0]));
        b.iter(|| black_box(&handle).inc());
    });
}

fn hist_benches(c: &mut Criterion) {
    let ids = entity_ids();
    let registry = populated(&ids);
    let values = sample_values();

    c.bench_function("hist/record_by_key", |b| {
        let mut i = 0usize;
        b.iter(|| {
            i = i.wrapping_add(1);
            let entity: &str = &ids[i % SERIES];
            let key = SeriesKey::new(black_box(HIST), EntityKind::Agent, black_box(entity));
            registry.hist(key).record(black_box(values[i % VALUES]));
        });
    });

    c.bench_function("hist/record_held_handle", |b| {
        let handle = registry.hist(SeriesKey::new(HIST, EntityKind::Agent, &ids[0]));
        let mut i = 0usize;
        b.iter(|| {
            i = i.wrapping_add(1);
            black_box(&handle).record(black_box(values[i % VALUES]));
        });
    });
}

criterion_group!(benches, counter_benches, hist_benches);
criterion_main!(benches);
