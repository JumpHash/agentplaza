//! What comes in: node batches (with the node's clock corrected), events, and the hub's own samples.

use apz_metrics::registry::{Sample, Value};
use apz_metrics::wire::{self, Batch};
use apz_metrics::{EntityKind, Event, SeriesKey};

use super::{Metrics, lock};

/// The histogram each lifecycle event kind feeds, with its parts: (event kind, metric, [(part name, metric)]).
pub const EVENT_HISTS: &[(&str, &str, &[(&str, &str)])] = &[
    ("boot", "life.boot_ms", &[]),
    ("resume", "life.resume_ms", &[]),
    (
        "wake",
        "life.wake_ms",
        &[
            ("restore_ms", "life.wake_restore_ms"),
            ("ready_ms", "life.wake_ready_ms"),
        ],
    ),
    (
        "snapshot",
        "life.snapshot_ms",
        &[("paused_ms", "life.pause_ms")],
    ),
    ("fork", "life.fork_ms", &[]),
    ("sandbox_create", "life.sandbox_create_ms", &[]),
    ("move", "life.move_ms", &[]),
];

/// Stores an event and records its duration (and parts) into the lifecycle histograms, keyed by `node` (the node
/// the entity is on; empty when unknown, then the entity itself when it is a node, else `hub`), with a `run` label
/// when the entity is a run or a VM (the part before `/`). Compactions count into `ctx.compactions` and
/// `ctx.freed_tokens` of the agent (parts `before`/`after`).
pub fn record_event(m: &Metrics, e: Event, node: &str) {
    let node_key = if !node.is_empty() {
        node.to_string()
    } else if e.entity_kind == EntityKind::Node {
        e.entity.clone()
    } else {
        "hub".to_string()
    };
    let run = match e.entity_kind {
        EntityKind::Run => Some(e.entity.as_str()),
        EntityKind::Vm => e.entity.split('/').next(),
        _ => None,
    };
    let keyed = |metric: &str| {
        let key = SeriesKey::new(metric, EntityKind::Node, &node_key);
        match run {
            Some(run) => key.label("run", run),
            None => key,
        }
    };

    if let Some(&(_, metric, parts)) = EVENT_HISTS.iter().find(|entry| entry.0 == e.kind.as_str()) {
        if let Some(ms) = e.duration_ms {
            m.registry.hist(keyed(metric)).record(ms);
        }
        for &(part, part_metric) in parts {
            if let Some(v) = e.part(part) {
                m.registry.hist(keyed(part_metric)).record(v);
            }
        }
    }

    if e.kind == "compaction" && e.entity_kind == EntityKind::Agent {
        m.registry
            .counter(SeriesKey::new(
                "ctx.compactions",
                EntityKind::Agent,
                &e.entity,
            ))
            .inc();
        if let (Some(before), Some(after)) = (e.part("before"), e.part("after")) {
            m.registry
                .counter(SeriesKey::new(
                    "ctx.freed_tokens",
                    EntityKind::Agent,
                    &e.entity,
                ))
                .add(before - after);
        }
    }

    m.store.put_events(std::slice::from_ref(&e));
}

/// Feeds samples (the hub's own scrape, or a decoded batch) into the rollup, remembers the latest point of each
/// series for live frames, stores the windows that closed, and hands them to the alert checks.
pub fn samples(m: &Metrics, samples: &[Sample], now_ms: u64) {
    let closed = {
        let mut rollup = lock(&m.rollup);
        let mut closed = Vec::new();
        for s in samples {
            closed.extend(rollup.ingest(s));
        }
        closed.extend(rollup.flush_due(now_ms));
        closed
    };
    {
        let mut latest = lock(&m.latest);
        for s in samples {
            match s.value {
                Value::Gauge(v) | Value::Counter(v) => {
                    latest.insert(s.key.clone(), (s.t_ms, v));
                }
                Value::Hist(_) => {}
            }
        }
    }
    m.registry
        .counter(SeriesKey::new("self.points_in", EntityKind::Hub, "hub"))
        .add(samples.len() as f64);
    if !closed.is_empty() {
        m.store.put(&closed);
    }
}

/// Takes a node's batch: measures the node's clock offset from `sent_ms` against `hub_now_ms` (smoothed, with
/// the beat's round trip when known), shifts every time by it, decodes columns into samples, records histograms
/// and events, keeps the reports, and records `link.*` for the node.
pub fn batch(m: &Metrics, node: &str, b: Batch, hub_now_ms: u64) {
    let payload_bytes = serde_json::to_vec(&b).map_or(0, |bytes| bytes.len());
    let Batch {
        sent_ms,
        series,
        hists,
        events,
        reports,
        ..
    } = b;

    let offset = {
        let mut links = lock(&m.links);
        let link = links.entry(node.to_string()).or_default();
        // The beat's round trip, when the hub knows it, is half spent on the way in: the node sent at `sent_ms`,
        // which is `rtt / 2` before the hub's clock read `hub_now_ms`.
        let half_rtt = if link.rtt_ms > 0.0 {
            link.rtt_ms / 2.0
        } else {
            0.0
        };
        let measured = sent_ms as f64 - (hub_now_ms as f64 - half_rtt);
        link.offset_ms = if link.last_beat_ms == 0 {
            measured
        } else {
            0.8 * link.offset_ms + 0.2 * measured
        };
        link.last_beat_ms = hub_now_ms;
        if !reports.is_null() {
            link.reports = merged_reports(std::mem::take(&mut link.reports), reports);
        }
        link.offset_ms
    };
    let shift = offset.round() as i64;

    let mut out = Vec::new();
    for c in &series {
        for (t_ms, v) in wire::decode(c) {
            let value = if c.counter {
                Value::Counter(v)
            } else {
                Value::Gauge(v)
            };
            out.push(Sample {
                key: owned_by(&c.key, node),
                t_ms: corrected(t_ms, shift),
                value,
            });
        }
    }
    for h in hists {
        out.push(Sample {
            key: owned_by(&h.key, node),
            t_ms: corrected(h.t_ms, shift),
            value: Value::Hist(h.hist),
        });
    }
    samples(m, &out, hub_now_ms);

    for mut e in events {
        e.t_ms = corrected(e.t_ms, shift);
        record_event(m, e, node);
    }

    m.registry
        .gauge(SeriesKey::new(
            "link.clock_offset_ms",
            EntityKind::Node,
            node,
        ))
        .set(offset);
    m.registry
        .hist(SeriesKey::new("link.payload_bytes", EntityKind::Node, node))
        .record(payload_bytes as f64);
}

/// A node's series keyed by what the hub knows it as: a node keys its own series `node` (it does not know its id),
/// and its volumes and services by their names; the hub keys them by the node's id (`n1`, `n1:/var/lib/plaza`,
/// `n1:noded`).
fn owned_by(key: &SeriesKey, node: &str) -> SeriesKey {
    let mut key = key.clone();
    match key.kind {
        EntityKind::Node if key.entity.is_empty() || key.entity == "node" => {
            key.entity = node.to_string()
        }
        // Always prefixed, even a name with a colon of its own (`C:\`): a node never knows its id.
        EntityKind::Volume | EntityKind::Service
            if !key.entity.starts_with(&format!("{node}:")) =>
        {
            key.entity = format!("{node}:{}", key.entity);
        }
        _ => {}
    }
    key
}

/// A node time moved to the hub's clock: the node's clock is `shift` ms ahead of the hub's.
fn corrected(t_ms: u64, shift: i64) -> u64 {
    (t_ms as i64 - shift).max(0) as u64
}

/// A node's new reports in place of its old ones, except the reports only the guests send (`top_processes`, from
/// each VM's stats), which stay when the node's batch does not carry them.
fn merged_reports(old: serde_json::Value, mut new: serde_json::Value) -> serde_json::Value {
    if let (serde_json::Value::Object(old), serde_json::Value::Object(fresh)) = (old, &mut new) {
        {
            let key = "top_processes";
            if !fresh.contains_key(key)
                && let Some(rows) = old.get(key)
            {
                fresh.insert(key.to_string(), rows.clone());
            }
        }
    }
    new
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nodes_volumes_are_prefixed_with_its_id_even_with_a_colon_in_the_name() {
        let key = |e: &str| SeriesKey::new("volume.used_pct", EntityKind::Volume, e);
        assert_eq!(owned_by(&key("/"), "n1").entity, "n1:/");
        assert_eq!(owned_by(&key("C:\\"), "n1").entity, "n1:C:\\");
        assert_eq!(owned_by(&key("n1:/data"), "n1").entity, "n1:/data");
    }

    #[test]
    fn a_node_batch_keeps_the_process_rows_the_guests_reported() {
        use serde_json::json;
        let old = json!({"top_processes": [["cargo", "r1/0", 80.0, 900.0]], "volumes": []});
        let merged = merged_reports(old.clone(), json!({"volumes": [1]}));
        assert_eq!(
            merged,
            json!({"volumes": [1], "top_processes": [["cargo", "r1/0", 80.0, 900.0]]})
        );
        let replaced = merged_reports(old, json!({"top_processes": []}));
        assert_eq!(replaced, json!({"top_processes": []}));
    }
    use apz_metrics::Tier;
    use apz_metrics::wire::encode_gauge;

    /// Metrics over a fresh metrics file; the store is returned too, so it stays open while the test runs.
    fn open(name: &str) -> (apz_store::Store, Metrics) {
        let files = crate::metrics::sqlite::scratch_store(name);
        let m = Metrics::open(
            std::sync::Arc::clone(files.db(crate::store::METRICS)),
            1_000_000,
        );
        (files, m)
    }

    fn hist_count(samples: &[Sample], metric: &str, run: &str) -> Option<u64> {
        samples.iter().find_map(|s| match &s.value {
            Value::Hist(h) if s.key.metric == metric && s.key.get("run") == Some(run) => {
                Some(h.count)
            }
            _ => None,
        })
    }

    #[test]
    fn a_batch_gauge_column_lands_in_latest_with_times_shifted_by_the_clock_offset() {
        let (_files, m) = open("offset");
        let key = SeriesKey::new("cpu.used", EntityKind::Node, "n1");
        let column = encode_gauge(key.clone(), 1_010_000, 10_000, &[0.5, 0.75]);
        let b = Batch {
            node: "n1".to_string(),
            sent_ms: 1_005_000,
            series: vec![column],
            ..Default::default()
        };

        batch(&m, "n1", b, 1_000_000);

        // The node's clock is 5 s ahead, so its last point (node 1_020_000) is hub 1_015_000.
        let latest = m.latest.lock().unwrap();
        assert_eq!(latest.get(&key), Some(&(1_015_000, 0.75)));
    }

    #[test]
    fn a_wake_event_records_its_duration_and_parts_and_is_stored() {
        let (_files, m) = open("wake");
        let event = Event {
            t_ms: 5_000,
            kind: "wake".to_string(),
            entity_kind: EntityKind::Vm,
            entity: "r12/3".to_string(),
            duration_ms: Some(412.0),
            detail: String::new(),
            parts: vec![
                ("restore_ms".to_string(), 160.0),
                ("ready_ms".to_string(), 252.0),
            ],
        };

        record_event(&m, event, "n1");

        let scraped = m.registry.scrape(10_000, true);
        assert_eq!(hist_count(&scraped, "life.wake_ms", "r12"), Some(1));
        assert_eq!(hist_count(&scraped, "life.wake_restore_ms", "r12"), Some(1));
        assert_eq!(hist_count(&scraped, "life.wake_ready_ms", "r12"), Some(1));
        let stored = m.store.events(0, 10_000, &|_| true, 10);
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].kind, "wake");
    }

    #[test]
    fn samples_that_close_a_window_reach_the_store() {
        let (_files, m) = open("windows");
        let key = SeriesKey::new("cpu.used", EntityKind::Vm, "vm-1");
        let at = |t_ms: u64| Sample {
            key: key.clone(),
            t_ms,
            value: Value::Gauge(0.5),
        };

        samples(&m, &[at(10_000), at(20_000)], 1_000_000);

        let rows = m
            .store
            .rows(Tier::Raw10s, "cpu.used", 0, 100_000, &|_| true);
        assert!(
            rows.iter()
                .any(|(k, windows)| k == &key && !windows.is_empty())
        );
    }
}
