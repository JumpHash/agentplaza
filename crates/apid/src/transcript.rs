//! Per-agent transcripts: everything an agent's loop did, in order.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use serde::Serialize;
use serde_json::Value;

use crate::events::Bus;

/// Entries kept per agent; older ones are dropped.
const KEEP: usize = 5_000;

/// Bytes kept per agent (each entry's kind plus its data as JSON); older ones are dropped past this.
/// The newest entry always stays, even when it alone is over the budget.
const MAX_BYTES_PER_AGENT: usize = 1 << 20;

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

/// One agent's window of recent entries, oldest first. Each entry's size is computed once on insert and
/// kept beside it, so eviction can bring `bytes` back under the budget.
#[derive(Default)]
struct Window {
    entries: VecDeque<Entry>,
    sizes: VecDeque<usize>,
    bytes: usize,
}

impl Window {
    fn push(&mut self, entry: Entry) {
        let size = entry.kind.len() + entry.data.to_string().len();
        // Kept sorted by seq so `after_seq` can binary search. A record takes its seq before it takes the
        // lock, so two records racing on one agent can arrive out of order; they are slotted in place here.
        let at = self.entries.partition_point(|e| e.seq < entry.seq);
        self.entries.insert(at, entry);
        self.sizes.insert(at, size);
        self.bytes += size;
        while self.entries.len() > 1
            && (self.entries.len() > KEEP || self.bytes > MAX_BYTES_PER_AGENT)
        {
            self.entries.pop_front();
            if let Some(size) = self.sizes.pop_front() {
                self.bytes -= size;
            }
        }
    }

    /// The entries with `seq > after`, oldest first.
    fn after_seq(&self, after: u64) -> impl DoubleEndedIterator<Item = &Entry> {
        let start = self.entries.partition_point(|e| e.seq <= after);
        self.entries.range(start..)
    }

    fn iter(&self) -> impl DoubleEndedIterator<Item = &Entry> {
        self.entries.iter()
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

#[derive(Default)]
pub struct Store {
    seq: AtomicU64,
    agents: Mutex<HashMap<u64, Window>>,
    sink: Mutex<Option<Sink>>,
}

impl Store {
    /// Drops the in-memory log of `cid` (the stored rows are deleted separately).
    pub fn clear(&self, cid: u64) {
        self.agents
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&cid);
    }

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
                log.push(Entry {
                    seq,
                    at_ms,
                    cid,
                    work,
                    kind,
                    data,
                });
            }
        }
        if let Some(largest) = largest {
            self.seq.fetch_max(largest, Ordering::SeqCst);
        }
    }

    /// Appends an entry and publishes it as a `transcript` event.
    pub fn record(&self, bus: &Bus, cid: u64, work: u64, kind: &'static str, data: Value) -> u64 {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let entry = Entry {
            seq,
            at_ms: apz_plaza::now_ms(),
            cid,
            work,
            kind: kind.to_string(),
            data,
        };
        bus.publish("transcript", &entry);
        if let Some(sink) = self
            .sink
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            sink(cid, seq, work, kind, entry.at_ms, &entry.data);
        }
        let mut agents = self.agents.lock().unwrap_or_else(PoisonError::into_inner);
        agents.entry(cid).or_default().push(entry);
        seq
    }

    /// The agent's entries with `seq > after`, oldest first, at most `limit`.
    pub fn after(&self, cid: u64, after: u64, limit: usize) -> Vec<Entry> {
        let agents = self.agents.lock().unwrap_or_else(PoisonError::into_inner);
        agents
            .get(&cid)
            .map(|log| log.after_seq(after).take(limit).cloned().collect())
            .unwrap_or_default()
    }

    /// The agent's entries on `thread` with `seq > after`, oldest first, at most `limit`: those of the work items
    /// whose `work_start` names the thread (a conversation's or an editor session's turns, without the others').
    pub fn on_thread(&self, cid: u64, thread: &str, after: u64, limit: usize) -> Vec<Entry> {
        let agents = self.agents.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(log) = agents.get(&cid) else {
            return Vec::new();
        };
        let works: std::collections::HashSet<u64> = log
            .iter()
            .filter(|e| e.kind == "work_start" && e.data["thread"].as_str() == Some(thread))
            .map(|e| e.work)
            .collect();
        log.iter()
            .filter(|e| e.seq > after && works.contains(&e.work))
            .take(limit)
            .cloned()
            .collect()
    }

    /// The agent's newest `n` entries, oldest first (fewer when the log is shorter).
    pub fn last(&self, cid: u64, n: usize) -> Vec<Entry> {
        let agents = self.agents.lock().unwrap_or_else(PoisonError::into_inner);
        agents
            .get(&cid)
            .map(|log| {
                log.iter()
                    .skip(log.len().saturating_sub(n))
                    .cloned()
                    .collect()
            })
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
                (
                    6,
                    1,
                    "model_request".to_string(),
                    11,
                    json!({"prompt": "hi"}),
                ),
            ],
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink_seen = Arc::clone(&seen);
        store.set_sink(Box::new(
            move |cid: u64, seq: u64, work: u64, kind: &str, _at: u64, _data: &Value| {
                sink_seen
                    .lock()
                    .unwrap()
                    .push((cid, seq, work, kind.to_string()));
            },
        ));
        let seq = store.record(&bus, 7, 1, "work_end", json!({"status": "done"}));
        assert!(seq > 6, "new seq {seq} must exceed the preloaded max");
        assert_eq!(store.after(7, 0, 10).len(), 3);
        assert_eq!(
            *seen.lock().unwrap(),
            vec![(7, seq, 1, "work_end".to_string())]
        );
    }

    #[test]
    fn entries_are_ordered_capped_and_searchable() {
        let (store, bus) = (Store::default(), Bus::default());
        let first = store.record(
            &bus,
            7,
            1,
            "work_start",
            json!({"prompt": "Write the README"}),
        );
        store.record(&bus, 7, 1, "work_end", json!({"status": "done"}));
        store.record(&bus, 8, 2, "work_start", json!({}));
        assert_eq!(store.after(7, 0, 10).len(), 2);
        assert_eq!(store.after(7, first, 10).len(), 1);
        assert_eq!(store.search(7, "readme", 10).len(), 1);
        assert!(store.after(9, 0, 10).is_empty());
    }

    #[test]
    fn a_thread_keeps_its_own_work_items_turns() {
        let (store, bus) = (Store::default(), Bus::default());
        store.record(&bus, 9, 1, "work_start", json!({"thread": "conv-1"}));
        store.record(&bus, 9, 2, "work_start", json!({"thread": "conv-2"}));
        store.record(&bus, 9, 1, "model_request", json!({}));
        store.record(&bus, 9, 2, "model_request", json!({}));
        let last = store.record(&bus, 9, 1, "work_end", json!({"status": "done"}));
        let first: Vec<(u64, String)> = store
            .on_thread(9, "conv-1", 0, 10)
            .into_iter()
            .map(|e| (e.work, e.kind))
            .collect();
        assert_eq!(
            first,
            [
                (1, "work_start".to_string()),
                (1, "model_request".to_string()),
                (1, "work_end".to_string())
            ]
        );
        assert_eq!(store.on_thread(9, "conv-2", 0, 10).len(), 2);
        assert!(store.on_thread(9, "conv-1", last, 10).is_empty());
        assert!(store.on_thread(9, "conv-3", 0, 10).is_empty());
        assert!(store.on_thread(4, "conv-1", 0, 10).is_empty());
    }

    #[test]
    fn last_is_the_newest_entries_in_order() {
        let (store, bus) = (Store::default(), Bus::default());
        for i in 0..5 {
            store.record(&bus, 7, 1, "model_request", json!({ "i": i }));
        }
        store.record(&bus, 8, 2, "work_start", json!({}));
        let newest = store.last(7, 2);
        assert_eq!(
            newest
                .iter()
                .map(|e| e.data["i"].as_i64())
                .collect::<Vec<_>>(),
            vec![Some(3), Some(4)]
        );
        assert!(newest[0].seq < newest[1].seq);
        assert_eq!(
            store.last(7, 99).len(),
            5,
            "a window longer than the log returns all of it"
        );
        assert_eq!(store.last(7, 0).len(), 0);
        assert!(store.last(9, 3).is_empty());
    }

    #[test]
    fn the_entry_cap_still_holds() {
        let (store, bus) = (Store::default(), Bus::default());
        let mut newest = 0;
        for i in 0..KEEP + 10 {
            newest = store.record(&bus, 7, 1, "model_request", json!({ "i": i }));
        }
        assert_eq!(store.last(7, usize::MAX).len(), KEEP);
        assert_eq!(store.last(7, 1)[0].seq, newest);
    }

    #[test]
    fn a_window_stays_under_its_byte_budget() {
        let (store, bus) = (Store::default(), Bus::default());
        let big = "x".repeat(64 << 10);
        for i in 0..200 {
            store.record(
                &bus,
                7,
                1,
                "tool_result",
                json!({ "i": i, "text": big.as_str() }),
            );
        }
        let rows: Vec<(u64, u64, String, u64, Value)> = (1..=200u64)
            .map(|seq| {
                (
                    seq,
                    1,
                    "tool_result".to_string(),
                    0,
                    json!({ "text": big.as_str() }),
                )
            })
            .collect();
        store.preload(8, rows);
        for cid in [7u64, 8] {
            let (len, bytes) = {
                let agents = store.agents.lock().unwrap();
                (agents[&cid].len(), agents[&cid].bytes)
            };
            assert!(
                bytes <= MAX_BYTES_PER_AGENT,
                "cid {cid} holds {bytes} bytes"
            );
            assert!(
                (15..200).contains(&len),
                "cid {cid} keeps what fits: {len} entries"
            );
            let summed: usize = store
                .last(cid, usize::MAX)
                .iter()
                .map(|e| e.kind.len() + e.data.to_string().len())
                .sum();
            assert_eq!(summed, bytes, "the tracked size matches the entries kept");
        }
    }

    #[test]
    fn the_newest_entry_is_kept_even_over_the_budget() {
        let (store, bus) = (Store::default(), Bus::default());
        for i in 0..10 {
            store.record(&bus, 7, 1, "model_request", json!({ "i": i }));
        }
        let huge = "y".repeat(2 * MAX_BYTES_PER_AGENT);
        let seq = store.record(&bus, 7, 1, "tool_result", json!({ "text": huge.as_str() }));
        let kept = store.last(7, usize::MAX);
        assert_eq!(
            kept.len(),
            1,
            "older entries go once the newest is over budget"
        );
        assert_eq!(kept[0].seq, seq);

        let next = store.record(&bus, 7, 1, "work_end", json!({ "status": "done" }));
        let kept = store.last(7, usize::MAX);
        assert_eq!(
            kept.len(),
            1,
            "the oversized entry goes once a newer one arrives"
        );
        assert_eq!(kept[0].seq, next);
    }

    #[test]
    fn after_pages_from_the_cursor_in_seq_order() {
        let mut window = Window::default();
        // 4 is pushed before 3, as a record racing another on the same agent can be.
        for seq in [1u64, 2, 4, 3, 5] {
            window.push(Entry {
                seq,
                at_ms: 0,
                cid: 7,
                work: 1,
                kind: "model_request".to_string(),
                data: json!({}),
            });
        }
        let seqs = |after: u64| -> Vec<u64> { window.after_seq(after).map(|e| e.seq).collect() };
        for after in 0..=6 {
            let filtered: Vec<u64> = window
                .iter()
                .filter(|e| e.seq > after)
                .map(|e| e.seq)
                .collect();
            assert_eq!(seqs(after), filtered, "after {after}");
        }
        assert_eq!(seqs(2), vec![3, 4, 5]);
        assert!(seqs(5).is_empty());
    }
}
