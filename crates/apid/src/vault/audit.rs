//! The audit log: the `vault_audit` table of `plaza.db`, one row per line, a MAC chain.
//! `mac = hex(HMAC(K_audit, prev ‖ canonical_json(line without mac)))`; `prev` is the previous
//! line's mac (genesis for the first). A rekey restarts the MAC key: lines before the last rekey
//! can only be checked for their links, lines from it on with the current key.
//!
//! A record carries identifiers, names, bindings and counts. It has no field for a value, so a
//! value cannot reach the log.

use apz_store::rusqlite::{Connection, params_from_iter};
use apz_store::table::quote;
use apz_store::{Column, Db, RowSet, StoreError, Table, TableDef};
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
        Head {
            seq: 0,
            mac: GENESIS.to_string(),
        }
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
        Record {
            event: event.to_string(),
            actor: actor.to_string(),
            ..Record::default()
        }
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

/// One line of the chain, as a row of `vault_audit`: the record's fields and its chain fields. Its key is the
/// sequence number, and the table's position column holds the same number, so the rows read back in chain order.
#[derive(Table)]
#[table(name = "vault_audit")]
struct AuditRow {
    #[table(key)]
    seq: u64,
    ts_ms: u64,
    prev: String,
    mac: String,
    event: String,
    actor: String,
    entry: Option<String>,
    name: Option<String>,
    binding: Option<String>,
    node: Option<String>,
    cid: Option<u64>,
    detail: Option<String>,
}

/// The audit table's definition.
pub(crate) fn def() -> &'static TableDef {
    apz_store::root_def!(AuditRow)
}

impl AuditRow {
    /// The line's JSON object without its mac: the bytes the mac covers.
    fn unsigned(&self) -> Map<String, Value> {
        let mut map = Map::new();
        map.insert("event".into(), Value::from(self.event.as_str()));
        map.insert("actor".into(), Value::from(self.actor.as_str()));
        let text = [
            ("entry", &self.entry),
            ("name", &self.name),
            ("binding", &self.binding),
            ("node", &self.node),
            ("detail", &self.detail),
        ];
        for (key, value) in text {
            if let Some(value) = value {
                map.insert(key.into(), Value::from(value.as_str()));
            }
        }
        if let Some(cid) = self.cid {
            map.insert("cid".into(), Value::from(cid));
        }
        map.insert("seq".into(), Value::from(self.seq));
        map.insert("ts_ms".into(), Value::from(self.ts_ms));
        map.insert("prev".into(), Value::from(self.prev.as_str()));
        map
    }

    /// The line as the log shows it: its fields, its `prev` and its `mac`.
    fn line(&self) -> Value {
        let mut map = self.unsigned();
        map.insert("mac".into(), Value::from(self.mac.as_str()));
        Value::Object(map)
    }
}

/// The row that chains `record` onto `head`, and the head that row leaves. Its mac covers the previous line's mac and
/// the line's canonical JSON without the mac.
fn chain(head: &Head, key: &[u8; 32], record: &Record, ts_ms: u64) -> (AuditRow, Head) {
    let seq = head.seq + 1;
    let mut row = AuditRow {
        seq,
        ts_ms,
        prev: head.mac.clone(),
        mac: String::new(),
        event: record.event.clone(),
        actor: record.actor.clone(),
        entry: record.entry.clone(),
        name: record.name.clone(),
        binding: record.binding.clone(),
        node: record.node.clone(),
        cid: record.cid,
        detail: record.detail.clone(),
    };
    let body = crypto::canonical(&Value::Object(row.unsigned()));
    let mac = crypto::mac_hex(key, &[head.mac.as_bytes(), body.as_bytes()]);
    row.mac = mac.clone();
    (row, Head { seq, mac })
}

/// Inserts `row` as a new line. Its sequence number is its key: a number already there is refused, never replaced.
fn insert(conn: &Connection, row: &AuditRow) -> Result<(), StoreError> {
    let table = def();
    // The position column is the sequence number too, so the lines read back in chain order.
    let mut out = RowSet::new();
    row.put(table, vec![u64::to_value(&row.seq)?], &mut out)?;
    let values = out
        .tables
        .remove(&table.name)
        .and_then(|rows| rows.into_iter().next())
        .ok_or_else(|| StoreError::invalid("an audit line did not lay out"))?;
    let columns: Vec<String> = table
        .key
        .iter()
        .chain(&table.columns)
        .map(|c| quote(&c.name))
        .collect();
    let marks: Vec<String> = (1..=columns.len()).map(|i| format!("?{i}")).collect();
    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        quote(&table.name),
        columns.join(", "),
        marks.join(", ")
    );
    conn.prepare_cached(&sql)?
        .execute(params_from_iter(values.iter()))?;
    Ok(())
}

/// Appends `record` to the log, chained onto `head`, and advances `head` once the line is committed.
pub fn append(
    db: &Db,
    head: &mut Head,
    key: &[u8; 32],
    record: &Record,
    ts_ms: u64,
) -> Result<(), String> {
    let (row, next) = chain(head, key, record, ts_ms);
    db.write(move |conn| insert(conn, &row))
        .map(|_| ())
        .map_err(|e| e.to_string())?;
    *head = next;
    Ok(())
}

/// The head a new line chains onto: the stored line with the highest sequence number. After a broken chain the new
/// lines still follow the stored ones, so no event is lost; the verdict says where the chain broke.
pub fn tail(lines: &[Value]) -> Head {
    lines
        .iter()
        .filter_map(|line| Some((line["seq"].as_u64()?, line["mac"].as_str()?)))
        .max_by_key(|(seq, _)| *seq)
        .map_or_else(Head::genesis, |(seq, mac)| Head {
            seq,
            mac: mac.to_string(),
        })
}

/// Every line, in chain order, as the chain is checked.
pub fn lines(db: &Db) -> Result<Vec<Value>, StoreError> {
    Ok(rows(db)?.iter().map(AuditRow::line).collect())
}

/// The lines after sequence number `after`, at most `limit` of them, as JSON: one indexed range of the table.
pub fn page(db: &Db, after: u64, limit: usize) -> Result<Vec<Value>, StoreError> {
    db.read(|conn| {
        let mut stmt = conn.prepare_cached(crate::store::sql::VAULT_AUDIT_PAGE)?;
        let rows = stmt.query_map(
            apz_store::rusqlite::params![
                i64::try_from(after).unwrap_or(i64::MAX),
                i64::try_from(limit).unwrap_or(i64::MAX)
            ],
            |r| {
                let n = |i: usize| r.get::<_, i64>(i).map(|v| u64::try_from(v).unwrap_or(0));
                Ok(AuditRow {
                    seq: n(0)?,
                    ts_ms: n(1)?,
                    prev: r.get(2)?,
                    mac: r.get(3)?,
                    event: r.get(4)?,
                    actor: r.get(5)?,
                    entry: r.get(6)?,
                    name: r.get(7)?,
                    binding: r.get(8)?,
                    node: r.get(9)?,
                    cid: r
                        .get::<_, Option<i64>>(10)?
                        .map(|v| u64::try_from(v).unwrap_or(0)),
                    detail: r.get(11)?,
                })
            },
        )?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.line());
        }
        Ok(out)
    })
}

fn rows(db: &Db) -> Result<Vec<AuditRow>, StoreError> {
    db.read(|conn| apz_store::table::load_list(conn, def()))
}

/// The first line that breaks the chain: its 0-based position, its seq when it has one, and the expected
/// and found hashes cut to their first 12 characters (both empty when the line is unreadable).
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Broken {
    pub index: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    pub expected: String,
    pub found: String,
}

/// The result of checking the chain.
pub struct Verdict {
    pub ok: bool,
    /// The 1-based line that broke the chain, when it did.
    pub broken_at: Option<u64>,
    /// The timestamp of the last rekey line, when there is one.
    pub last_rekey_ms: Option<u64>,
    /// The entries that verified before the first broken one (all of them when the chain is intact).
    pub verified: u64,
    /// The first broken entry, when the chain is broken.
    pub first_broken: Option<Broken>,
}

/// Checks every line, in order: sequence numbers and links everywhere, MACs from the last rekey on.
pub fn verify(lines: &[Value], key: &[u8; 32]) -> Verdict {
    let keyed_from = lines
        .iter()
        .rposition(|l| l["event"] == "rekey")
        .unwrap_or(0);
    let last_rekey_ms = lines
        .iter()
        .rev()
        .find(|l| l["event"] == "rekey")
        .and_then(|l| l["ts_ms"].as_u64());
    let mut prev = GENESIS.to_string();
    for (i, line) in lines.iter().enumerate() {
        let number = i as u64 + 1;
        let (Some(seq), Some(line_prev), Some(mac)) = (
            line["seq"].as_u64(),
            line["prev"].as_str(),
            line["mac"].as_str(),
        ) else {
            return broken(
                i,
                line["seq"].as_u64(),
                &prev,
                line["prev"].as_str().unwrap_or(""),
                last_rekey_ms,
            );
        };
        if seq != number || line_prev != prev {
            return broken(i, Some(seq), &prev, line_prev, last_rekey_ms);
        }
        if i >= keyed_from {
            let mut unsigned = line.as_object().cloned().unwrap_or_default();
            unsigned.remove("mac");
            let expected = crypto::mac_hex(
                key,
                &[
                    prev.as_bytes(),
                    crypto::canonical(&Value::Object(unsigned)).as_bytes(),
                ],
            );
            if expected != mac {
                return broken(i, Some(seq), &expected, mac, last_rekey_ms);
            }
        }
        prev = mac.to_string();
    }
    Verdict {
        ok: true,
        broken_at: None,
        last_rekey_ms,
        verified: lines.len() as u64,
        first_broken: None,
    }
}

/// The first 12 characters of a hash: enough to compare by eye, never the whole chain.
fn prefix(hash: &str) -> String {
    hash.chars().take(12).collect()
}

/// The verdict for a chain that breaks at the line with 0-based position `index`: the entries before it verified.
fn broken(
    index: usize,
    seq: Option<u64>,
    expected: &str,
    found: &str,
    last_rekey_ms: Option<u64>,
) -> Verdict {
    let index = index as u64;
    Verdict {
        ok: false,
        broken_at: Some(index + 1),
        last_rekey_ms,
        verified: index,
        first_broken: Some(Broken {
            index,
            seq,
            expected: prefix(expected),
            found: prefix(found),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 32] = [3; 32];

    /// The lines that a sequence of (record, key, time) makes, chained from genesis, as the log holds them.
    fn chained(steps: &[(Record, [u8; 32], u64)]) -> Vec<Value> {
        let mut head = Head::genesis();
        let mut lines = Vec::new();
        for (record, key, ts) in steps {
            let (row, next) = chain(&head, key, record, *ts);
            lines.push(row.line());
            head = next;
        }
        lines
    }

    fn five_steps() -> Vec<(Record, [u8; 32], u64)> {
        ["init", "unseal", "create", "use", "seal"]
            .iter()
            .enumerate()
            .map(|(i, event)| {
                let record = Record::new(event, "owner")
                    .entry("sec_1", "KEY")
                    .detail(format!("step {i}"));
                (record, KEY, 1000 + i as u64)
            })
            .collect()
    }

    #[test]
    fn chain_verifies_and_an_edited_line_breaks_it_at_that_line() {
        let mut lines = chained(&five_steps());
        let verdict = verify(&lines, &KEY);
        assert!(verdict.ok, "a clean chain verifies");
        assert_eq!(tail(&lines).seq, 5, "the tail is the last line");
        assert!(!verify(&lines, &[4; 32]).ok, "another key does not verify");

        // Edit the third line's detail: the MAC of line 3 no longer matches.
        lines[2]["detail"] = Value::from("step X");
        let verdict = verify(&lines, &KEY);
        assert!(!verdict.ok);
        assert_eq!(verdict.broken_at, Some(3));
    }

    #[test]
    fn the_first_broken_entry_is_named_by_position_with_short_hashes() {
        let mut lines = chained(&five_steps());
        let clean = verify(&lines, &KEY);
        assert!(clean.ok);
        assert_eq!(clean.verified, 5);
        assert!(clean.first_broken.is_none());

        // The middle entry (index 2, seq 3) is edited: the two entries before it verify.
        let stored_mac = lines[2]["mac"].as_str().expect("mac").to_string();
        lines[2]["detail"] = Value::from("step X");
        let verdict = verify(&lines, &KEY);
        assert!(!verdict.ok);
        assert_eq!(verdict.verified, 2);
        let broken = verdict.first_broken.expect("the edit is named");
        assert_eq!(broken.index, 2);
        assert_eq!(broken.seq, Some(3));
        assert_eq!(broken.expected.len(), 12);
        assert_eq!(broken.found.len(), 12);
        assert_ne!(broken.expected, broken.found);
        assert_eq!(
            broken.found,
            &stored_mac[..12],
            "the found hash is the stored mac's prefix"
        );
    }

    #[test]
    fn a_rekey_restarts_the_mac_key_and_older_lines_keep_their_links() {
        let new_key = [9u8; 32];
        let lines = chained(&[
            (Record::new("init", "owner"), KEY, 1),
            (Record::new("rekey", "owner"), new_key, 2),
            (Record::new("seal", "owner"), new_key, 3),
        ]);
        let verdict = verify(&lines, &new_key);
        assert!(verdict.ok);
        assert_eq!(verdict.last_rekey_ms, Some(2));
    }

    #[test]
    fn lines_written_to_the_table_read_back_verify_and_page() {
        let dir =
            std::env::temp_dir().join(format!("apid-vault-audit-table-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let files = super::super::test_files(&dir);
        let db = files.db(crate::store::PLAZA);
        let mut head = Head::genesis();
        for _ in 0..4 {
            append(db, &mut head, &KEY, &Record::new("use", "hub"), 1).expect("appended");
        }
        let stored = lines(db).expect("read");
        assert_eq!(stored.len(), 4);
        assert!(verify(&stored, &KEY).ok, "the stored chain verifies");
        let rows = page(db, 1, 2).expect("page");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["seq"], 2);

        // A line whose sequence number is already stored is refused, and the stored line stays.
        let mut stale = Head::genesis();
        assert!(append(db, &mut stale, &KEY, &Record::new("use", "hub"), 1).is_err());
        assert_eq!(lines(db).expect("read").len(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
