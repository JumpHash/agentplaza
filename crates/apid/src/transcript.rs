//! Per-agent transcripts: everything an agent's loop did, in order.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use serde::Serialize;
use serde_json::Value;

use crate::events::Bus;

/// Entries kept per agent; older ones are dropped.
const KEEP: usize = 5_000;

/// One transcript entry. `kind`: `work_start`, `model_request`,
/// `model_response`, `tool_request`, `tool_decision`, `tool_result`,
/// `work_end`, `error`.
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub seq: u64,
    pub at_ms: u64,
    pub cid: u64,
    pub work: u64,
    pub kind: String,
    pub data: Value,
}

/// Where each recorded entry is also written (Postgres): (cid, seq, work, kind, at_ms, data).
pub type Sink = Box<dyn Fn(u64, u64, u64, &str, u64, &Value) + Send + Sync>;

#[derive(Default)]
pub struct Store {
    seq: AtomicU64,
    agents: Mutex<HashMap<u64, VecDeque<Entry>>>,
    sink: Mutex<Option<Sink>>,
}

impl Store {
    /// Every entry recorded from now on is also handed to `sink`.
    pub fn set_sink(&self, sink: Sink) {
        *self.sink.lock().unwrap_or_else(PoisonError::into_inner) = Some(sink);
    }

    /// Loads stored entries back (oldest first) at start, and moves `seq` past the largest.
    pub fn preload(&self, cid: u64, entries: Vec<(u64, u64, String, u64, Value)>) {
        let largest = entries.iter().map(|e| e.0).max();
        {
            let mut agents = self.agents.lock().unwrap_or_else(PoisonError::into_inner);
            let log = agents.entry(cid).or_default();
            for (seq, work, kind, at_ms, data) in entries {
                if log.len() == KEEP {
                    log.pop_front();
                }
                log.push_back(Entry { seq, at_ms, cid, work, kind, data });
            }
        }
        if let Some(largest) = largest {
            self.seq.fetch_max(largest, Ordering::SeqCst);
        }
    }

    /// Appends an entry and publishes it as a `transcript` event.
    pub fn record(&self, bus: &Bus, cid: u64, work: u64, kind: &'static str, data: Value) -> u64 {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let entry = Entry { seq, at_ms: apz_plaza::now_ms(), cid, work, kind: kind.to_string(), data };
        bus.publish("transcript", &entry);
        if let Some(sink) = self.sink.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
            sink(cid, seq, work, kind, entry.at_ms, &entry.data);
        }
        let mut agents = self.agents.lock().unwrap_or_else(PoisonError::into_inner);
        let log = agents.entry(cid).or_default();
        if log.len() == KEEP {
            log.pop_front();
        }
        log.push_back(entry);
        seq
    }

    /// The agent's entries with `seq > after`, oldest first, at most `limit`.
    pub fn after(&self, cid: u64, after: u64, limit: usize) -> Vec<Entry> {
        let agents = self.agents.lock().unwrap_or_else(PoisonError::into_inner);
        agents
            .get(&cid)
            .map(|log| log.iter().filter(|e| e.seq > after).take(limit).cloned().collect())
            .unwrap_or_default()
    }

    /// The agent's entries whose data mentions `needle` (case-insensitive),
    /// newest first, at most `limit`.
    pub fn search(&self, cid: u64, needle: &str, limit: usize) -> Vec<Entry> {
        let needle = needle.to_lowercase();
        let agents = self.agents.lock().unwrap_or_else(PoisonError::into_inner);
        agents
            .get(&cid)
            .map(|log| {
                log.iter()
                    .rev()
                    .filter(|e| e.data.to_string().to_lowercase().contains(&needle))
                    .take(limit)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    #[test]
    fn preload_then_record_continues_seq_and_feeds_the_sink() {
        let (store, bus) = (Store::default(), Bus::default());
        store.preload(
            7,
            vec![
                (5, 1, "work_start".to_string(), 10, json!({})),
                (6, 1, "model_request".to_string(), 11, json!({"prompt": "hi"})),
            ],
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink_seen = Arc::clone(&seen);
        store.set_sink(Box::new(move |cid: u64, seq: u64, work: u64, kind: &str, _at: u64, _data: &Value| {
            sink_seen.lock().unwrap().push((cid, seq, work, kind.to_string()));
        }));
        let seq = store.record(&bus, 7, 1, "work_end", json!({"status": "done"}));
        assert!(seq > 6, "new seq {seq} must exceed the preloaded max");
        assert_eq!(store.after(7, 0, 10).len(), 3);
        assert_eq!(*seen.lock().unwrap(), vec![(7, seq, 1, "work_end".to_string())]);
    }

    #[test]
    fn entries_are_ordered_capped_and_searchable() {
        let (store, bus) = (Store::default(), Bus::default());
        let first = store.record(&bus, 7, 1, "work_start", json!({"prompt": "Write the README"}));
        store.record(&bus, 7, 1, "work_end", json!({"status": "done"}));
        store.record(&bus, 8, 2, "work_start", json!({}));
        assert_eq!(store.after(7, 0, 10).len(), 2);
        assert_eq!(store.after(7, first, 10).len(), 1);
        assert_eq!(store.search(7, "readme", 10).len(), 1);
        assert!(store.after(9, 0, 10).is_empty());
    }
}
