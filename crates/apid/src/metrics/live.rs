//! Scopes watched live. A console holds a lease (`POST /v1/metrics/live`, renewed every 10 s while visible); while
//! any lease is active the hub scrapes every 2 s, asks the nodes under the leased scopes for 2 s batches, and every
//! 2 s recomposes each leased tab's live panels into a `metrics` event ([`apz_metrics_view::LiveFrame`]).

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use apz_metrics_view::compose::compose;
use apz_metrics_view::{LiveFrame, LiveLease, Panel, ViewQuery};
use serde_json::json;

use super::Metrics;
use super::scope;
use super::source::LiveSource;
use crate::state::App;

/// A lease lapses this long after its last renewal.
pub const LEASE_MS: u64 = 15_000;

/// A node is asked for live batches at most this often.
const PING_EVERY_MS: u64 = 10_000;

/// Each ask keeps the node streaming for this long.
const PING_FOR_MS: u64 = 15_000;

/// The hub's own node, which it reads every tick without asking.
const HUB_NODE: &str = "n0";

/// When each node was last asked for live batches (`None` until the first ask).
static PINGED: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

#[derive(Default)]
pub struct Leases {
    inner: Mutex<HashMap<LiveLease, u64>>,
}

impl Leases {
    /// Takes or renews a lease.
    pub fn lease(&self, l: LiveLease, now_ms: u64) {
        super::lock(&self.inner).insert(l, now_ms + LEASE_MS);
    }

    /// The leases still active (lapsed ones are dropped).
    pub fn active(&self, now_ms: u64) -> Vec<LiveLease> {
        let mut map = super::lock(&self.inner);
        map.retain(|_, until| *until > now_ms);
        map.keys().cloned().collect()
    }
}

/// Publishes one frame per active lease, and keeps the nodes under leased scopes streaming.
pub fn frames(m: &Metrics, app: &App, now_ms: u64) {
    let source = LiveSource { app, m, now_ms };
    let mut nodes: Vec<String> = Vec::new();
    for lease in distinct(m.live.active(now_ms)) {
        let view = compose(&query_for(&lease), &source);
        let panels: Vec<Panel> = view.panels.into_iter().filter(|p| p.live).collect();
        if !panels.is_empty() {
            let frame = LiveFrame {
                scope: lease.scope.clone(),
                tab: lease.tab.clone(),
                group: lease.group.clone(),
                range: lease.range.clone(),
                t_ms: now_ms,
                panels,
            };
            app.bus.publish("metrics", &frame);
        }
        for entity in scope::entities(app, &lease.scope, "node") {
            if let Some(id) = entity.id.strip_prefix("node:") {
                nodes.push(id.to_string());
            }
        }
    }
    ping(app, &nodes, now_ms);
}

/// The leases with duplicates removed, in the order they were first seen.
fn distinct(leases: Vec<LiveLease>) -> Vec<LiveLease> {
    let mut seen = HashSet::new();
    leases
        .into_iter()
        .filter(|lease| seen.insert(lease.clone()))
        .collect()
}

/// The query a lease composes: its tab, scope and range, grouped as the lease asks.
fn query_for(lease: &LiveLease) -> ViewQuery {
    let mut query = ViewQuery::new(&lease.tab, &lease.scope, &lease.range);
    query.group = lease.group.clone();
    query
}

/// The nodes to ask now, from `nodes` (ids without the `node:` prefix): each once, never the hub's own node, and
/// only those not asked within [`PING_EVERY_MS`].
fn nodes_to_ping(nodes: &[String], last_ping: &HashMap<String, u64>, now_ms: u64) -> Vec<String> {
    let mut due: Vec<String> = Vec::new();
    for id in nodes {
        if id == HUB_NODE || due.contains(id) {
            continue;
        }
        let asked_recently = match last_ping.get(id) {
            Some(last) => now_ms.saturating_sub(*last) <= PING_EVERY_MS,
            None => false,
        };
        if !asked_recently {
            due.push(id.clone());
        }
    }
    due
}

/// Queues the live-batch command on each node that is due, and records when it was asked.
fn ping(app: &App, nodes: &[String], now_ms: u64) {
    let mut pinged = super::lock(&PINGED);
    let last_ping = pinged.get_or_insert_with(HashMap::new);
    for node in nodes_to_ping(nodes, last_ping, now_ms) {
        let body = json!({"kind": "metrics.live", "until_ms": now_ms + PING_FOR_MS});
        app.nodes
            .enqueue(&node, "noded", "POST", "/v1/metrics/live", body);
        last_ping.insert(node, now_ms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_is_asked_for_live_batches_once_per_ten_seconds_and_never_the_hub_node() {
        let nodes: Vec<String> = ["n0", "n1", "n2", "n3", "n2"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let last_ping = HashMap::from([("n2".to_string(), 95_000u64)]);
        // n1 and n3 were never asked, n2 five seconds ago, n0 is the hub's own node and n2 is listed twice.
        assert_eq!(nodes_to_ping(&nodes, &last_ping, 100_000), vec!["n1", "n3"]);
        // Exactly ten seconds after the last ask is still too soon; a moment later n2 is due again.
        assert_eq!(nodes_to_ping(&nodes, &last_ping, 105_000), vec!["n1", "n3"]);
        assert_eq!(
            nodes_to_ping(&nodes, &last_ping, 105_001),
            vec!["n1", "n2", "n3"]
        );
    }
}
