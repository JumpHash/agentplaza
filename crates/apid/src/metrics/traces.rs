//! Turn traces on the hub: a span ring per agent while its turn runs, the tail sampler when it ends, and which
//! agents are open in a console (their turns are always kept).

use std::collections::HashMap;
use std::sync::Mutex;

use apz_metrics::trace::{Ring, Span, TailSampler, Trace};

use super::store::Store;
use crate::inference::fnv1a64;

/// How long a turn may run before its spans are dropped unfinished: no turn runs this long, so a turn still open
/// after it has lost its agent.
const STALE_TURN_MS: u64 = 6 * 3_600_000;

pub struct Traces {
    /// Per agent: the turn's start and its spans so far.
    rings: Mutex<HashMap<u64, (u64, Ring)>>,
    sampler: Mutex<TailSampler>,
    /// Agents open in a console, until when.
    open: Mutex<HashMap<u64, u64>>,
    next_span: std::sync::atomic::AtomicU32,
}

impl Traces {
    pub fn new(seed: u64) -> Traces {
        Traces {
            rings: Mutex::new(HashMap::new()),
            sampler: Mutex::new(TailSampler::new(seed)),
            open: Mutex::new(HashMap::new()),
            next_span: std::sync::atomic::AtomicU32::new(1),
        }
    }

    /// A fresh span id.
    pub fn span_id(&self) -> u32 {
        self.next_span
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Adds a span to the agent's running turn (starting the turn at the span's start when none runs). A turn
    /// that never ended (its agent went away mid-turn) is dropped once it is older than [`STALE_TURN_MS`].
    pub fn span(&self, cid: u64, span: Span) {
        let mut rings = super::lock(&self.rings);
        rings.retain(|_, (start_ms, _)| start_ms.saturating_add(STALE_TURN_MS) >= span.start_ms);
        let entry = rings
            .entry(cid)
            .or_insert_with(|| (span.start_ms, Ring::default()));
        entry.0 = entry.0.min(span.start_ms);
        entry.1.push(span);
    }

    /// Ends the agent's turn: builds the trace, asks the sampler, stores it when kept. Returns it when kept.
    pub fn end(
        &self,
        store: &dyn Store,
        cid: u64,
        role: &str,
        ok: bool,
        now_ms: u64,
    ) -> Option<Trace> {
        let (start_ms, mut ring) = super::lock(&self.rings).remove(&cid)?;
        let spans = ring.take();
        let mut key = cid.to_le_bytes().to_vec();
        key.extend_from_slice(&start_ms.to_le_bytes());
        let mut trace = Trace {
            id: format!("tr-{:08x}", fnv1a64(&key) as u32),
            cid,
            role: role.to_string(),
            start_ms,
            dur_ms: now_ms.saturating_sub(start_ms) as f64,
            ok: ok && spans.iter().all(|span| span.ok),
            spans,
            kept: None,
        };
        let open = {
            let mut open = super::lock(&self.open);
            open.retain(|_, until| *until > now_ms);
            open.contains_key(&cid)
        };
        let keep = super::lock(&self.sampler).decide(&trace, open)?;
        trace.kept = Some(keep);
        store.put_trace(&trace);
        Some(trace)
    }

    /// Marks an agent open in a console until `until_ms` (the console renews it while the agent's view is open).
    pub fn mark_open(&self, cid: u64, until_ms: u64) {
        super::lock(&self.open).insert(cid, until_ms);
    }

    /// Keep every trace (the Full level) or sample as usual.
    pub fn keep_all(&self, all: bool) {
        super::lock(&self.sampler).keep_all = all;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::metrics::sqlite::{SqliteStore, scratch_store};
    use apz_metrics::trace::{Keep, SpanKind};

    fn span(id: u32, start_ms: u64, ok: bool) -> Span {
        Span {
            id,
            parent: None,
            kind: SpanKind::Model,
            name: "model call · anthropic/opus".to_string(),
            start_ms,
            dur_ms: 40.0,
            first_token_ms: None,
            ok,
            attrs: Vec::new(),
        }
    }

    #[test]
    fn a_turn_left_open_for_hours_is_dropped_when_another_turn_runs() {
        let traces = Traces::new(1);
        traces.span(1, span(1, 0, true));
        traces.span(2, span(2, STALE_TURN_MS + 1, true));
        let rings = super::super::lock(&traces.rings);
        assert_eq!(rings.len(), 1);
        assert!(rings.contains_key(&2));
    }

    #[test]
    fn an_error_turn_is_kept_and_stored() {
        let files = scratch_store("traces-error");
        let store = SqliteStore::new(Arc::clone(files.db(crate::store::METRICS)));
        let traces = Traces::new(7);
        traces.span(1, span(1, 100, true));
        let kept = traces
            .end(&store, 1, "worker", false, 1000)
            .expect("an error turn is kept");
        assert_eq!(kept.kept, Some(Keep::Error));
        assert!(!kept.ok);
        assert_eq!(store.trace(&kept.id), Some(kept.clone()));
    }

    #[test]
    fn an_ordinary_turn_of_an_open_agent_is_kept_as_open() {
        let files = scratch_store("traces-open");
        let store = SqliteStore::new(Arc::clone(files.db(crate::store::METRICS)));
        let traces = Traces::new(7);
        traces.mark_open(2, 5000);
        traces.span(2, span(1, 100, true));
        let kept = traces
            .end(&store, 2, "worker", true, 1000)
            .expect("the turn of an open agent is kept");
        assert_eq!(kept.kept, Some(Keep::Open));
        assert!(kept.ok);
    }

    #[test]
    fn spans_added_before_the_end_appear_in_the_stored_trace_in_order() {
        let files = scratch_store("traces-order");
        let store = SqliteStore::new(Arc::clone(files.db(crate::store::METRICS)));
        let traces = Traces::new(7);
        traces.mark_open(3, u64::MAX);
        traces.span(3, span(1, 100, true));
        traces.span(3, span(2, 150, true));
        let kept = traces
            .end(&store, 3, "worker", true, 1000)
            .expect("the turn is kept");
        assert_eq!(kept.start_ms, 100);
        assert_eq!(kept.dur_ms, 900.0);
        let stored = store.trace(&kept.id).expect("the kept trace is stored");
        let ids: Vec<u32> = stored.spans.iter().map(|span| span.id).collect();
        assert_eq!(ids, vec![1, 2]);
    }
}
