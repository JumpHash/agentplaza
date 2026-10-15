//! Turn traces. A trace is one agent turn; its spans are the model calls (with time to first token), tool and
//! MCP calls, hub MCP calls and a VM wake when the turn needed one. Spans go into a per-agent ring buffer while
//! the turn runs; when it ends, the tail sampler decides whether the whole trace is kept. Discarded turns still
//! count in the histograms, so aggregates stay exact.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    Model,
    Tool,
    HubMcp,
    Wake,
    Restore,
    Ready,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u32>,
    pub kind: SpanKind,
    /// "model call · anthropic/opus", "tool · bash", "wake VM r12/2".
    pub name: String,
    pub start_ms: u64,
    pub dur_ms: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_token_ms: Option<f64>,
    pub ok: bool,
    /// Numbers and names only: model, tokens, cost, sizes, outcome. Never prompt text, arguments or output.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attrs: Vec<(String, String)>,
}

/// Why a trace was kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Keep {
    Error,
    Slow,
    Open,
    Sampled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Trace {
    pub id: String,
    pub cid: u64,
    pub role: String,
    pub start_ms: u64,
    pub dur_ms: f64,
    pub ok: bool,
    pub spans: Vec<Span>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<Keep>,
}

/// Spans per agent ring.
pub const RING: usize = 256;

/// The spans of one agent's running turn (the oldest dropped past [`RING`]).
#[derive(Clone, Debug, Default)]
pub struct Ring {
    pub spans: VecDeque<Span>,
}

impl Ring {
    pub fn push(&mut self, s: Span) {
        if self.spans.len() == RING {
            self.spans.pop_front();
        }
        self.spans.push_back(s);
    }

    /// Ends the turn: the spans so far, leaving the ring empty.
    pub fn take(&mut self) -> Vec<Span> {
        self.spans.drain(..).collect()
    }
}

/// Share of ordinary turns kept at random.
pub const SAMPLE_RATE: f64 = 0.01;

/// Decides which finished turns are kept: every error, every turn slower than the rolling p95 of its role (after
/// at least 20 turns of that role), every turn of an agent open in a console, and [`SAMPLE_RATE`] of the rest
/// (deterministic xorshift from `seed`). Every finished turn feeds the role's duration histogram.
#[derive(Clone, Debug)]
pub struct TailSampler {
    pub roles: HashMap<String, crate::hist::HistSnapshot>,
    pub rng: u64,
    /// In `Full`, every trace is kept.
    pub keep_all: bool,
}

impl TailSampler {
    pub fn new(seed: u64) -> TailSampler {
        TailSampler {
            roles: HashMap::new(),
            rng: seed.max(1),
            keep_all: false,
        }
    }

    /// Why to keep `t`, or `None` to drop it. `open` says whether the agent is open in a console right now.
    pub fn decide(&mut self, t: &Trace, open: bool) -> Option<Keep> {
        let _ = (t, open);
        todo!(
            "check error, open, slow (p95 of role before recording this one), sampled; keep_all -> Sampled for the rest; then record dur into the role histogram"
        )
    }
}
