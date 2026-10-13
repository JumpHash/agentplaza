//! The audit log: `state_dir/vault-audit.jsonl`, one JSON object per line, a MAC chain.
//! `mac = hex(HMAC(K_audit, prev ‖ canonical_json(line without mac)))`; `prev` is the previous
//! line's mac (genesis for the first). A rekey restarts the MAC key: lines before the last rekey
//! can only be checked for their links, lines from it on with the current key.
//!
//! A record carries identifiers, names, bindings and counts. It has no field for a value, so a
//! value cannot reach the log.

use std::io::Write;
use std::path::Path;

use serde::Serialize;
use serde_json::{Map, Value};

use super::crypto;

/// The `prev` of the first line.
pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// The chain's head: the last line's sequence number and mac.
#[derive(Clone)]
pub struct Head {
    pub seq: u64,
    pub mac: String,
}

impl Head {
    pub fn genesis() -> Head {
        Head { seq: 0, mac: GENESIS.to_string() }
    }
}

/// What happened. Every field is optional except the event and the actor.
#[derive(Serialize, Clone, Default)]
pub struct Record {
    pub event: String,
    pub actor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Record {
    pub fn new(event: &str, actor: &str) -> Record {
        Record { event: event.to_string(), actor: actor.to_string(), ..Record::default() }
    }

    pub fn entry(mut self, id: &str, name: &str) -> Record {
        self.entry = Some(id.to_string());
        self.name = Some(name.to_string());
        self
    }

    pub fn binding(mut self, binding: &str) -> Record {
        self.binding = Some(binding.to_string());
        self
    }

    pub fn node(mut self, node: &str) -> Record {
        self.node = Some(node.to_string());
        self
    }

    pub fn cid(mut self, cid: u64) -> Record {
        self.cid = Some(cid);
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Record {
        self.detail = Some(detail.into());
        self
    }
}

/// The line's JSON object without its mac: the bytes the mac covers.
fn body(record: &Record, seq: u64, ts_ms: u64, prev: &str) -> Map<String, Value> {
    let mut map = match serde_json::to_value(record) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    };
    map.insert("seq".into(), Value::from(seq));
    map.insert("ts_ms".into(), Value::from(ts_ms));
    map.insert("prev".into(), Value::from(prev));
    map
}

/// Appends `record` to the log at `path`, chained onto `head`, and advances `head`.
pub fn append(path: &Path, head: &mut Head, key: &[u8; 32], record: &Record, ts_ms: u64) -> Result<(), String> {
    let seq = head.seq + 1;
    let mut map = body(record, seq, ts_ms, &head.mac);
    let mac = crypto::mac_hex(key, &[head.mac.as_bytes(), crypto::canonical(&Value::Object(map.clone())).as_bytes()]);
    map.insert("mac".into(), Value::from(mac.clone()));
    let mut line = serde_json::to_string(&Value::Object(map)).map_err(|e| e.to_string())?;
    line.push('\n');
    let mut file = apz_os::private::append(path).map_err(|e| e.to_string())?;
    file.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
    file.sync_data().map_err(|e| e.to_string())?;
    *head = Head { seq, mac };
    Ok(())
}

/// The result of checking the chain.
pub struct Verdict {
    pub ok: bool,
    /// The 1-based line that broke the chain, when it did.
    pub broken_at: Option<u64>,
    pub head: Head,
    /// The timestamp of the last rekey line, when there is one.
    pub last_rekey_ms: Option<u64>,
}

/// Checks every line: sequence numbers and links everywhere, MACs from the last rekey on.
pub fn verify(text: &str, key: &[u8; 32]) -> Verdict {
    let mut lines: Vec<Value> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        match serde_json::from_str::<Value>(line) {
            Ok(value) if value.is_object() => lines.push(value),
            _ => return broken(i as u64 + 1, Head::genesis(), None),
        }
    }
    let keyed_from = lines.iter().rposition(|l| l["event"] == "rekey").unwrap_or(0);
    let last_rekey_ms = lines.iter().rev().find(|l| l["event"] == "rekey").and_then(|l| l["ts_ms"].as_u64());
    let mut prev = GENESIS.to_string();
    let mut head = Head::genesis();
    for (i, line) in lines.iter().enumerate() {
        let number = i as u64 + 1;
        let (Some(seq), Some(line_prev), Some(mac)) = (line["seq"].as_u64(), line["prev"].as_str(), line["mac"].as_str()) else {
            return broken(number, head, last_rekey_ms);
        };
        if seq != number || line_prev != prev {
            return broken(number, head, last_rekey_ms);
        }
        if i >= keyed_from {
            let mut unsigned = line.as_object().cloned().unwrap_or_default();
            unsigned.remove("mac");
            let expected = crypto::mac_hex(key, &[prev.as_bytes(), crypto::canonical(&Value::Object(unsigned)).as_bytes()]);
            if expected != mac {
                return broken(number, head, last_rekey_ms);
            }
        }
        prev = mac.to_string();
        head = Head { seq, mac: mac.to_string() };
    }
    Verdict { ok: true, broken_at: None, head, last_rekey_ms }
}

fn broken(line: u64, head: Head, last_rekey_ms: Option<u64>) -> Verdict {
    Verdict { ok: false, broken_at: Some(line), head, last_rekey_ms }
}

/// The lines after sequence number `after`, at most `limit` of them, as JSON.
pub fn page(text: &str, after: u64, limit: usize) -> Vec<Value> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|line| line["seq"].as_u64().unwrap_or(0) > after)
        .take(limit)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 32] = [3; 32];

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("apid-vault-audit-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn chain_verifies_and_an_edited_line_breaks_it_at_that_line() {
        let dir = scratch("chain");
        let path = dir.join("vault-audit.jsonl");
        let mut head = Head::genesis();
        for (i, event) in ["init", "unseal", "create", "use", "seal"].iter().enumerate() {
            let record = Record::new(event, "owner").entry("sec_1", "KEY").detail(format!("step {i}"));
            append(&path, &mut head, &KEY, &record, 1000 + i as u64).expect("appended");
        }
        let text = std::fs::read_to_string(&path).expect("read");
        let verdict = verify(&text, &KEY);
        assert!(verdict.ok, "a clean chain verifies");
        assert_eq!(verdict.head.seq, 5);
        assert!(!verify(&text, &[4; 32]).ok, "another key does not verify");

        // Edit the third line's detail: the MAC of line 3 no longer matches.
        let edited = text.replacen("step 2", "step X", 1);
        let verdict = verify(&edited, &KEY);
        assert!(!verdict.ok);
        assert_eq!(verdict.broken_at, Some(3));
    }

    #[test]
    fn a_rekey_restarts_the_mac_key_and_older_lines_keep_their_links() {
        let dir = scratch("rekey");
        let path = dir.join("vault-audit.jsonl");
        let mut head = Head::genesis();
        append(&path, &mut head, &KEY, &Record::new("init", "owner"), 1).expect("appended");
        let new_key = [9u8; 32];
        append(&path, &mut head, &new_key, &Record::new("rekey", "owner"), 2).expect("appended");
        append(&path, &mut head, &new_key, &Record::new("seal", "owner"), 3).expect("appended");
        let text = std::fs::read_to_string(&path).expect("read");
        let verdict = verify(&text, &new_key);
        assert!(verdict.ok);
        assert_eq!(verdict.last_rekey_ms, Some(2));
    }

    #[test]
    fn the_page_is_after_a_sequence_number_and_limited() {
        let dir = scratch("page");
        let path = dir.join("vault-audit.jsonl");
        let mut head = Head::genesis();
        for _ in 0..4 {
            append(&path, &mut head, &KEY, &Record::new("use", "hub"), 1).expect("appended");
        }
        let text = std::fs::read_to_string(&path).expect("read");
        let rows = page(&text, 1, 2);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["seq"], 2);
    }
}
