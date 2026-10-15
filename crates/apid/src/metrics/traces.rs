//! Turn traces on the hub: a span ring per agent while its turn runs, the tail sampler when it ends, and which
//! agents are open in a console (their turns are always kept).

use std::collections::HashMap;
use std::sync::Mutex;

use apz_metrics::trace::{Ring, Span, TailSampler, Trace};

use super::store::Store;

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

    /// Adds a span to the agent's running turn (starting the turn at the span's start when none runs).
    pub fn span(&self, cid: u64, span: Span) {
        let _ = (cid, span, &self.rings);
        todo!()
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
        let _ = (store, cid, role, ok, now_ms, &self.sampler, &self.open);
        todo!()
    }

    /// Marks an agent open in a console until `until_ms` (the console renews it while the agent's view is open).
    pub fn mark_open(&self, cid: u64, until_ms: u64) {
        let _ = (cid, until_ms);
        todo!()
    }

    /// Keep every trace (the Full level) or sample as usual.
    pub fn keep_all(&self, all: bool) {
        super::lock(&self.sampler).keep_all = all;
    }
}
