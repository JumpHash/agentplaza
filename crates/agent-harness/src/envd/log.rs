//! The daemon's own log: what ran in the sandbox (each process's start and end), newest last, the
//! last [`CAPACITY`] entries. `GET /logs` (a plaza extension) hands them to the hub, which serves
//! E2B's sandbox logs from them.

use std::collections::VecDeque;
use std::sync::Mutex;

use serde_json::{Value, json};

/// The most entries kept.
pub const CAPACITY: usize = 1000;

static LOG: Mutex<VecDeque<Value>> = Mutex::new(VecDeque::new());

/// Appends `entry` to `log`, dropping the oldest past [`CAPACITY`].
fn push(log: &mut VecDeque<Value>, entry: Value) {
    if log.len() == CAPACITY {
        log.pop_front();
    }
    log.push_back(entry);
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Records one entry: `level` is E2B's (`debug`, `info`, `warn`, `error`), `fields` its string map.
pub fn record(level: &str, message: impl Into<String>, fields: &[(&str, String)]) {
    let fields: serde_json::Map<String, Value> = fields
        .iter()
        .map(|(key, value)| ((*key).to_string(), Value::String(value.clone())))
        .collect();
    let entry = json!({
        "timestamp_ms": now_ms(),
        "level": level,
        "message": message.into(),
        "fields": fields,
    });
    let mut log = LOG
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if log.len() == CAPACITY {
        log.pop_front();
    }
    log.push_back(entry);
}

/// Every entry kept, oldest first.
pub fn entries() -> Value {
    let log = LOG
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Value::Array(log.iter().cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_kept_in_order_and_capped() {
        let mut log = VecDeque::new();
        for i in 0..CAPACITY + 5 {
            push(&mut log, json!({ "i": i }));
        }
        assert_eq!(log.len(), CAPACITY);
        assert_eq!(log[0]["i"], 5);
        assert_eq!(log[CAPACITY - 1]["i"], CAPACITY + 4);
    }

    #[test]
    fn a_record_has_its_fields() {
        record("warn", "a test entry", &[("pid", "7".to_string())]);
        let entries = entries();
        let found = entries.as_array().unwrap().iter().any(|entry| {
            entry["message"] == "a test entry"
                && entry["level"] == "warn"
                && entry["fields"]["pid"] == "7"
        });
        assert!(found);
    }
}
