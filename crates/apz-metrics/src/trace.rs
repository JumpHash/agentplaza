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
        let keep = if !t.ok {
            Some(Keep::Error)
        } else if open {
            Some(Keep::Open)
        } else {
            // The turn is judged against the earlier turns of its role, so it is recorded only afterwards.
            let hist = self.roles.entry(t.role.clone()).or_default();
            let slow = hist.count >= 20 && hist.quantile(0.95).is_some_and(|p95| t.dur_ms > p95);
            if slow {
                Some(Keep::Slow)
            } else if self.keep_all {
                Some(Keep::Sampled)
            } else {
                self.roll()
            }
        };
        self.roles
            .entry(t.role.clone())
            .or_default()
            .record(t.dur_ms);
        keep
    }

    /// One xorshift step; keeps the turn at random with the sample rate.
    fn roll(&mut self) -> Option<Keep> {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        if x % 10_000 < (SAMPLE_RATE * 10_000.0) as u64 {
            Some(Keep::Sampled)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(role: &str, dur_ms: f64, ok: bool) -> Trace {
        Trace {
            id: format!("{role}-{dur_ms}"),
            cid: 1,
            role: role.to_string(),
            start_ms: 0,
            dur_ms,
            ok,
            spans: Vec::new(),
            kept: None,
        }
    }

    #[test]
    fn an_error_or_an_open_agent_turn_is_always_kept() {
        let mut sampler = TailSampler::new(7);
        assert_eq!(
            sampler.decide(&turn("worker", 10.0, false), false),
            Some(Keep::Error)
        );
        assert_eq!(
            sampler.decide(&turn("worker", 10.0, false), true),
            Some(Keep::Error)
        );
        assert_eq!(
            sampler.decide(&turn("worker", 10.0, true), true),
            Some(Keep::Open)
        );
        for _ in 0..100 {
            assert!(
                sampler
                    .decide(&turn("worker", 10.0, false), false)
                    .is_some()
            );
        }
    }

    #[test]
    fn a_slow_turn_after_warm_up_is_kept_and_about_one_percent_of_ordinary_turns_are_sampled() {
        let mut sampler = TailSampler::new(7);
        for i in 0..100 {
            let dur = 1000.0 + (i % 5) as f64 * 10.0;
            sampler.decide(&turn("worker", dur, true), false);
        }
        assert_eq!(
            sampler.decide(&turn("worker", 5000.0, true), false),
            Some(Keep::Slow)
        );

        let mut sampler = TailSampler::new(7);
        let sampled = (0..10_000)
            .filter(|_| sampler.decide(&turn("worker", 1000.0, true), false) == Some(Keep::Sampled))
            .count();
        assert!(
            (30..=300).contains(&sampled),
            "sampled {sampled} of 10000 ordinary turns, expected about 1 percent"
        );
    }
}
