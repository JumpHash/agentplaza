//! The scrubber (PLAN 22.5, W59): production data reaches the test lab only as a copy this module makes from a read-only
//! [`Source`]. The project declares its rules in `.plaza/scrub.toml` (per table and column, per document field by JSON
//! pointer, per log line pattern); each rule removes the value, replaces it with a fake of the same shape, or hashes it
//! with the run's salt. A fake is deterministic per value (with the salt), so the same value in two tables becomes the
//! same fake, and joins still match.
//!
//! Before the lab receives the copy, a scan looks for personal data in every column of every table: emails, phone
//! numbers, card numbers that pass the Luhn check, the national ids of [`crate::pii`] (US SSN, UK National Insurance,
//! German tax id) and the names in the list. The scan's report names the table, the column, the kind and the sample count,
//! never a value. A scrubbed cell (a fake or a hash the scrubber made) is known by its exact text and is not what the scan
//! judges; a BLOB the rules did not remove is refused, since the scan cannot read it.
//!
//! The copy is written to a staging file in the run's directory and moved to its final name only when the scan passes;
//! a failing scan removes the staging file and writes nothing where the lab reads. The [`Copy`] guard removes the copy
//! when it is dropped (the run's end). Nothing is written to a release or to blob storage.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use regex::Regex;
use rusqlite::types::Value;
use rusqlite::{Connection, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use sha2::{Digest, Sha256};

use crate::pii;
use crate::source::{self, Source, Table};

/// The file a project declares its scrubbing rules in, at the root of its repository.
pub const RULES_FILE: &str = ".plaza/scrub.toml";
/// The copy's file name in the run's directory, once its scan has passed.
pub const COPY_FILE: &str = "production.db";
/// Where a copy is written until its scan passes.
const STAGING_FILE: &str = "production.staging.db";
/// Names a scan looks for whatever the project lists: common first and family names, matched as whole words, case as
/// written. A project adds its own in `names`.
pub const COMMON_NAMES: &[&str] = &[
    "Alice",
    "Anna",
    "Ana",
    "Bob",
    "Carol",
    "Chloe",
    "David",
    "Emma",
    "Frank",
    "Grace",
    "Hannah",
    "Henry",
    "Irene",
    "Jack",
    "John",
    "Liam",
    "Lucas",
    "Maria",
    "Mia",
    "Mohammed",
    "Noah",
    "Oliver",
    "Olivia",
    "Peter",
    "Priya",
    "Sofia",
    "Thomas",
    "Wei",
    "Yuki",
    "Brown",
    "Davis",
    "Fischer",
    "Garcia",
    "Johnson",
    "Jones",
    "Miller",
    "Müller",
    "Schmidt",
    "Schneider",
    "Smith",
    "Weber",
    "Williams",
];

/// What a rule does to a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// The value becomes NULL (or, in a log line, the matched text is cut).
    Remove,
    /// The value becomes a fake of the same shape: digits for digits, letters for letters, an email with a `.invalid`
    /// domain, a card number that still passes Luhn.
    Fake,
    /// The value becomes its SHA-256 with the run's salt, in hex.
    Hash,
}

/// A rule on a column: every value of `table.column` takes the action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColumnRule {
    pub table: String,
    pub column: String,
    pub action: Action,
}

/// A rule on a document field: the value at `path` (a JSON pointer, `/user/email`) of each JSON document in
/// `table.column` takes the action. A document that is not JSON is cut whole (its column becomes NULL).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldRule {
    pub table: String,
    pub column: String,
    pub path: String,
    pub action: Action,
}

/// A rule on log lines: each match of `pattern` (a regular expression) takes the action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineRule {
    pub pattern: String,
    pub action: Action,
}

/// A project's scrubbing rules: what TOML declares. `names` comes first in the file, before the tables.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    /// Names the scan looks for, on top of [`COMMON_NAMES`].
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default, rename = "column")]
    pub columns: Vec<ColumnRule>,
    #[serde(default, rename = "field")]
    pub fields: Vec<FieldRule>,
    #[serde(default, rename = "line")]
    pub lines: Vec<LineRule>,
}

impl Rules {
    /// Parses a rules file and checks it.
    pub fn parse(text: &str) -> Result<Rules, String> {
        let rules: Rules = toml::from_str(text).map_err(|why| format!("{RULES_FILE}: {why}"))?;
        rules.check()?;
        Ok(rules)
    }

    /// What TOML cannot say: names are not empty, a rule's pointer starts with `/`, no rule repeats, and each line
    /// pattern compiles.
    pub fn check(&self) -> Result<(), String> {
        for name in &self.names {
            if name.is_empty() || name.chars().any(char::is_whitespace) {
                return Err("a scrub name is one word".into());
            }
        }
        let mut seen: BTreeSet<(&str, &str)> = BTreeSet::new();
        for rule in &self.columns {
            ident(&rule.table, "table")?;
            ident(&rule.column, "column")?;
            if !seen.insert((&rule.table, &rule.column)) {
                return Err(format!(
                    "two rules name the column {}.{}",
                    rule.table, rule.column
                ));
            }
        }
        let mut paths: BTreeSet<(&str, &str, &str)> = BTreeSet::new();
        for rule in &self.fields {
            ident(&rule.table, "table")?;
            ident(&rule.column, "column")?;
            if !rule.path.starts_with('/') {
                return Err(format!(
                    "the field rule for {}.{} needs a JSON pointer that starts with /",
                    rule.table, rule.column
                ));
            }
            if !paths.insert((&rule.table, &rule.column, &rule.path)) {
                return Err(format!(
                    "two field rules name {}.{} {}",
                    rule.table, rule.column, rule.path
                ));
            }
        }
        for rule in &self.lines {
            if rule.pattern.is_empty() {
                return Err("a line rule has no pattern".into());
            }
            Regex::new(&rule.pattern)
                .map_err(|why| format!("a line pattern does not compile: {why}"))?;
        }
        Ok(())
    }
}

fn ident(text: &str, what: &str) -> Result<(), String> {
    if text.is_empty() || text.contains('\0') {
        return Err(format!("a rule names no {what}"));
    }
    Ok(())
}

/// One finding of the scan: a kind of personal data in a column, with how many cells held it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub table: String,
    pub column: String,
    pub kind: String,
    pub samples: u64,
}

/// What a copy (or a refused one) measured: its size and what the scan found. Never a value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    pub tables: u64,
    pub cells: u64,
    /// Document fields the rules cut because their documents were not JSON.
    pub cut_documents: u64,
    pub findings: Vec<Finding>,
}

impl Report {
    /// Whether the scan found nothing.
    pub fn clean(&self) -> bool {
        self.findings.is_empty()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.clean() {
            return write!(f, "clean: {} cells in {} tables", self.cells, self.tables);
        }
        write!(f, "personal data in")?;
        for (index, finding) in self.findings.iter().enumerate() {
            let sep = if index == 0 { " " } else { ", " };
            write!(
                f,
                "{sep}{}.{} ({}: {} samples)",
                finding.table, finding.column, finding.kind, finding.samples
            )?;
        }
        Ok(())
    }
}

/// Why a copy was not made: its scan found personal data (the report), or the source or the rules failed (the reason).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Scan(Report),
    Failed(String),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::Scan(report) => write!(f, "the copy was stopped: {report}"),
            Refusal::Failed(why) => write!(f, "the copy was not made: {why}"),
        }
    }
}

/// A finished copy. Dropping it removes the file: the copy lives as long as the run that holds it.
#[derive(Debug)]
pub struct Copy {
    path: PathBuf,
    pub report: Report,
}

impl Copy {
    /// The copy's path, which the data stage starts from.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Copy {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Applies rules to values and remembers the output it made, so the scan can tell the scrubber's own text from data.
pub struct Scrubber {
    rules: Rules,
    salt: [u8; 32],
    lines: Vec<Regex>,
    names: BTreeSet<String>,
    /// Every fake and hash the scrubber produced: exact texts the scan does not judge.
    synthetic: BTreeSet<String>,
    /// Document fields cut because their document was not JSON (or a non-text value sat in a document column).
    cut_documents: u64,
}

impl Scrubber {
    /// A scrubber for `rules` with the run's `salt`. Hashes and fakes are deterministic for one salt.
    pub fn new(rules: Rules, salt: [u8; 32]) -> Result<Scrubber, String> {
        rules.check()?;
        let lines = rules
            .lines
            .iter()
            .map(|rule| Regex::new(&rule.pattern).map_err(|why| why.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let names = COMMON_NAMES
            .iter()
            .map(|n| n.to_string())
            .chain(rules.names.iter().cloned())
            .collect();
        Ok(Scrubber {
            rules,
            salt,
            lines,
            names,
            synthetic: BTreeSet::new(),
            cut_documents: 0,
        })
    }

    fn column_action(&self, table: &str, column: &str) -> Option<Action> {
        self.rules
            .columns
            .iter()
            .find(|r| r.table == table && r.column == column)
            .map(|r| r.action)
    }

    /// The scrubbed value of one cell of `table.column`.
    pub fn value(&mut self, table: &str, column: &str, value: Value) -> Value {
        if let Some(action) = self.column_action(table, column) {
            return self.apply(action, value);
        }
        let fields: Vec<FieldRule> = self
            .rules
            .fields
            .iter()
            .filter(|r| r.table == table && r.column == column)
            .cloned()
            .collect();
        if fields.is_empty() {
            return value;
        }
        let text = match value {
            Value::Null => return Value::Null,
            Value::Text(text) => text,
            _ => {
                self.cut_documents += 1;
                return Value::Null;
            }
        };
        let Ok(mut doc) = serde_json::from_str::<Json>(&text) else {
            self.cut_documents += 1;
            return Value::Null;
        };
        for rule in &fields {
            if let Some(target) = doc.pointer_mut(&rule.path) {
                let replacement = match &*target {
                    Json::String(s) => Json::String(self.synth(rule.action, s)),
                    _ => Json::Null,
                };
                *target = replacement;
            }
        }
        Value::Text(doc.to_string())
    }

    fn apply(&mut self, action: Action, value: Value) -> Value {
        match (action, value) {
            (_, Value::Null) => Value::Null,
            (Action::Remove, _) => Value::Null,
            (action, Value::Text(text)) => Value::Text(self.synth(action, &text)),
            (Action::Fake, Value::Integer(n)) => Value::Integer(self.fake_integer(n)),
            (Action::Hash, Value::Integer(n)) => {
                Value::Text(self.synth(Action::Hash, &n.to_string()))
            }
            (action, Value::Real(x)) => Value::Text(self.synth(action, &x.to_string())),
            (_, Value::Blob(_)) => Value::Null,
        }
    }

    /// The fake or hash of a text, recorded as the scrubber's own output.
    fn synth(&mut self, action: Action, text: &str) -> String {
        let out = match action {
            Action::Hash => self.hash(text),
            Action::Fake | Action::Remove => self.fake(text),
        };
        self.synthetic.insert(out.clone());
        out
    }

    fn hash(&self, text: &str) -> String {
        let mut h = Sha256::new();
        h.update(self.salt);
        h.update(b"hash\0");
        h.update(text.as_bytes());
        hex(&h.finalize())
    }

    /// Deterministic bytes for a text: a stream of SHA-256 blocks keyed by the salt.
    fn stream(&self, text: &str, n: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(n + 32);
        let mut block: u32 = 0;
        while out.len() < n {
            let mut h = Sha256::new();
            h.update(self.salt);
            h.update(b"fake\0");
            h.update(block.to_be_bytes());
            h.update(text.as_bytes());
            out.extend_from_slice(&h.finalize());
            block += 1;
        }
        out
    }

    /// A fake of the same shape: each character replaced by one of its class; an email's domain ends in `.invalid`
    /// (a name that can never be a real address); a card number that passed Luhn still passes it.
    fn fake(&self, text: &str) -> String {
        let chars: Vec<char> = text.chars().collect();
        let bytes = self.stream(text, chars.len());
        let mut out: Vec<char> = chars
            .iter()
            .zip(&bytes)
            .map(|(c, b)| shape(*c, *b))
            .collect();
        let email = pii::kinds_in(text).contains(&"email");
        let digits = digits_of(text);
        if (13..=19).contains(&digits.len())
            && pii::luhn(&digits)
            && let Some(last) = out.iter().rposition(|c| c.is_ascii_digit())
        {
            let payload: String = out
                .iter()
                .enumerate()
                .filter(|(i, c)| *i != last && c.is_ascii_digit())
                .map(|(_, c)| *c)
                .collect();
            out[last] = char::from(b'0' + luhn_check_digit(&payload));
        }
        let mut s: String = out.into_iter().collect();
        if email
            && let Some(at) = s.find('@')
            && let Some(dot) = s[at..].rfind('.')
        {
            s.truncate(at + dot);
            s.push_str(".invalid");
        }
        s
    }

    /// An integer fake with as many digits as the original, its leading digit not zero.
    fn fake_integer(&self, n: i64) -> i64 {
        let digits = n.unsigned_abs().to_string().len() as u32;
        if digits > 18 {
            return 0;
        }
        let bytes = self.stream(&n.to_string(), 8);
        let raw = bytes
            .iter()
            .take(8)
            .fold(0u64, |acc, b| (acc << 8) | u64::from(*b));
        let v = if digits == 1 {
            raw % 10
        } else {
            let lead = 10u64.pow(digits - 1);
            lead + raw % (9 * lead)
        };
        let sign = if n < 0 { -1 } else { 1 };
        sign * v as i64
    }

    /// The text of one log line with the line rules applied: each match takes its rule's action.
    pub fn line(&mut self, text: &str) -> String {
        let mut out = text.to_string();
        for index in 0..self.lines.len() {
            let action = self.rules.lines[index].action;
            let spans: Vec<(usize, usize)> = self.lines[index]
                .find_iter(&out)
                .map(|m| (m.start(), m.end()))
                .collect();
            let mut next = String::with_capacity(out.len());
            let mut at = 0;
            for (start, end) in spans {
                next.push_str(&out[at..start]);
                if action != Action::Remove {
                    let matched = out[start..end].to_string();
                    next.push_str(&self.synth(action, &matched));
                }
                at = end;
            }
            next.push_str(&out[at..]);
            out = next;
        }
        out
    }

    /// The kinds of personal data a log line holds once its rules' matches are cut: the rules govern those matches, so
    /// the scan judges what is left (a line the copy keeps has the matches replaced, and what is left is the same).
    pub fn line_kinds(&self, text: &str) -> Vec<&'static str> {
        let mut out = text.to_string();
        for re in &self.lines {
            out = re.replace_all(&out, " ").into_owned();
        }
        self.kinds(&out)
    }

    /// The kinds of personal data one text holds, given the scrubber's own output is not what is judged.
    fn kinds(&self, text: &str) -> Vec<&'static str> {
        if self.synthetic.contains(text) {
            return Vec::new();
        }
        let mut kinds = pii::kinds_in(text);
        if self.names_in(text) {
            kinds.push("name");
        }
        kinds
    }

    /// The kinds a cell holds: a JSON document's string leaves are judged one by one (a leaf the scrubber made is not),
    /// any other text whole.
    pub fn cell_kinds(&self, text: &str) -> Vec<&'static str> {
        if self.synthetic.contains(text) {
            return Vec::new();
        }
        if let Ok(doc @ (Json::Object(_) | Json::Array(_))) = serde_json::from_str::<Json>(text) {
            let mut kinds = Vec::new();
            walk(&doc, &mut |leaf: &str| {
                for kind in self.kinds(leaf) {
                    if !kinds.contains(&kind) {
                        kinds.push(kind);
                    }
                }
            });
            return kinds;
        }
        self.kinds(text)
    }

    fn names_in(&self, text: &str) -> bool {
        text.split(|c: char| !c.is_alphanumeric())
            .any(|word| self.names.contains(word))
    }
}

/// Calls `leaf` with each string leaf of a JSON document (and each number's text).
fn walk(doc: &Json, leaf: &mut dyn FnMut(&str)) {
    match doc {
        Json::String(s) => leaf(s),
        Json::Number(n) => leaf(&n.to_string()),
        Json::Array(items) => items.iter().for_each(|item| walk(item, leaf)),
        Json::Object(map) => map.values().for_each(|item| walk(item, leaf)),
        Json::Null | Json::Bool(_) => {}
    }
}

/// The check digit that makes a payload of digits pass Luhn once appended.
pub fn luhn_check_digit(payload: &str) -> u8 {
    let mut sum = 0u32;
    for (i, c) in payload.chars().rev().enumerate() {
        let mut n = c.to_digit(10).unwrap_or(0);
        if i % 2 == 0 {
            n *= 2;
            if n > 9 {
                n -= 9;
            }
        }
        sum += n;
    }
    ((10 - sum % 10) % 10) as u8
}

fn digits_of(text: &str) -> String {
    text.chars().filter(char::is_ascii_digit).collect()
}

fn shape(c: char, b: u8) -> char {
    match c {
        '0'..='9' => char::from(b'0' + b % 10),
        'a'..='z' => char::from(b'a' + b % 26),
        'A'..='Z' => char::from(b'A' + b % 26),
        c if c.is_alphabetic() => char::from(b'a' + b % 26),
        c => c,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Makes the copy of `source` into `dir` under `rules`' scrubber: the staging file is written and scanned, and only a
/// clean scan moves it to [`COPY_FILE`]. Nothing is written beside a refused copy.
pub fn copy(source: &mut dyn Source, scrubber: &mut Scrubber, dir: &Path) -> Result<Copy, Refusal> {
    let failed = |why: String| Refusal::Failed(why);
    std::fs::create_dir_all(dir)
        .map_err(|why| failed(format!("the copy's directory is not usable: {why}")))?;
    let staging = dir.join(STAGING_FILE);
    let path = dir.join(COPY_FILE);
    for stale in [&staging, &path] {
        let _ = std::fs::remove_file(stale);
    }
    let tables = source.tables().map_err(failed)?;
    check_rules(&scrubber.rules, &tables)?;
    let conn = Connection::open(&staging)
        .map_err(|why| failed(format!("the staging copy did not open: {why}")))?;
    let mut report = Report {
        tables: tables.len() as u64,
        ..Report::default()
    };
    for table in &tables {
        write_table(&conn, source, scrubber, table, &mut report).map_err(failed)?;
    }
    report.cut_documents = scrubber.cut_documents;
    scan(&conn, scrubber, &tables, &mut report).map_err(failed)?;
    drop(conn);
    if !report.clean() {
        let _ = std::fs::remove_file(&staging);
        return Err(Refusal::Scan(report));
    }
    std::fs::rename(&staging, &path)
        .map_err(|why| failed(format!("the copy was not moved into place: {why}")))?;
    Ok(Copy { path, report })
}

/// Every rule names a table and a column the source has: a rule on nothing is a configuration error, not a pass.
fn check_rules(rules: &Rules, tables: &[Table]) -> Result<(), Refusal> {
    let has = |table: &str, column: &str| {
        tables
            .iter()
            .any(|t| t.name == table && t.columns.iter().any(|c| c.name == column))
    };
    for rule in &rules.columns {
        if !has(&rule.table, &rule.column) {
            return Err(Refusal::Failed(format!(
                "a rule names {}.{}, which the source does not have",
                rule.table, rule.column
            )));
        }
    }
    for rule in &rules.fields {
        if !has(&rule.table, &rule.column) {
            return Err(Refusal::Failed(format!(
                "a field rule names {}.{}, which the source does not have",
                rule.table, rule.column
            )));
        }
    }
    Ok(())
}

fn write_table(
    conn: &Connection,
    source: &mut dyn Source,
    scrubber: &mut Scrubber,
    table: &Table,
    report: &mut Report,
) -> Result<(), String> {
    let mut decls = vec!["rowid INTEGER PRIMARY KEY".to_string()];
    let mut names = Vec::new();
    for column in &table.columns {
        decls.push(format!("{} {}", source::quote(&column.name), column.decl));
        names.push(source::quote(&column.name));
    }
    conn.execute(
        &format!(
            "CREATE TABLE {} ({})",
            source::quote(&table.name),
            decls.join(", ")
        ),
        [],
    )
    .map_err(|why| why.to_string())?;
    let marks = vec!["?"; table.columns.len() + 1].join(", ");
    let sql = format!(
        "INSERT INTO {} (rowid, {}) VALUES ({marks})",
        source::quote(&table.name),
        names.join(", ")
    );
    let tx = conn
        .unchecked_transaction()
        .map_err(|why| why.to_string())?;
    {
        let mut stmt = tx.prepare(&sql).map_err(|why| why.to_string())?;
        let mut rowid: i64 = 0;
        source.each_row(table, &mut |row| {
            rowid += 1;
            report.cells += row.len() as u64;
            let mut params = vec![Value::Integer(rowid)];
            for (column, value) in table.columns.iter().zip(row) {
                params.push(scrubber.value(&table.name, &column.name, value));
            }
            stmt.execute(params_from_iter(params))
                .map(|_| ())
                .map_err(|why| why.to_string())
        })?;
    }
    tx.commit().map_err(|why| why.to_string())
}

/// The scan of the staging copy: every text cell of every column, and every BLOB (which the scan cannot read, so it is
/// refused as its own kind).
fn scan(
    conn: &Connection,
    scrubber: &Scrubber,
    tables: &[Table],
    report: &mut Report,
) -> Result<(), String> {
    let mut found: BTreeMap<(String, String, &'static str), u64> = BTreeMap::new();
    for table in tables {
        for column in &table.columns {
            let sql = format!(
                "SELECT {} FROM {}",
                source::quote(&column.name),
                source::quote(&table.name)
            );
            let mut stmt = conn.prepare(&sql).map_err(|why| why.to_string())?;
            let mut rows = stmt.query([]).map_err(|why| why.to_string())?;
            while let Some(row) = rows.next().map_err(|why| why.to_string())? {
                let value: Value = row.get(0).map_err(|why| why.to_string())?;
                let kinds: Vec<&'static str> = match &value {
                    Value::Null => continue,
                    Value::Text(text) => scrubber.cell_kinds(text),
                    Value::Integer(n) => scrubber.cell_kinds(&n.to_string()),
                    Value::Real(x) => scrubber.cell_kinds(&x.to_string()),
                    Value::Blob(_) => vec!["unscannable-blob"],
                };
                for kind in kinds {
                    *found
                        .entry((table.name.clone(), column.name.clone(), kind))
                        .or_insert(0) += 1;
                }
            }
        }
    }
    report.findings = found
        .into_iter()
        .map(|((table, column, kind), samples)| Finding {
            table,
            column,
            kind: kind.to_string(),
            samples,
        })
        .collect();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SALT: [u8; 32] = [7; 32];

    fn scrubber(rules: &str) -> Scrubber {
        Scrubber::new(Rules::parse(rules).expect("the rules parse"), SALT).expect("a scrubber")
    }

    fn text(value: Value) -> String {
        match value {
            Value::Text(t) => t,
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn the_luhn_check_digit_completes_a_card_number() {
        assert_eq!(luhn_check_digit("453957876362148"), 6);
        assert!(pii::luhn("4539578763621486"));
        assert_eq!(luhn_check_digit("000000000000000"), 0);
    }

    #[test]
    fn a_fake_email_keeps_its_shape_and_a_reserved_domain() {
        let s = scrubber("");
        let fake = s.fake("ana.keller@example.com");
        let (local, domain) = fake.split_once('@').expect("an email shape");
        assert_eq!(local.len(), "ana.keller".len());
        assert!(domain.ends_with(".invalid"), "{fake}");
        assert!(pii::kinds_in(&fake).contains(&"email"));
        assert_ne!(
            fake,
            s.fake("ana.keller@example.org"),
            "different values, different fakes"
        );
        assert_eq!(
            fake,
            s.fake("ana.keller@example.com"),
            "deterministic per value"
        );
    }

    #[test]
    fn a_fake_phone_number_keeps_its_separators_and_digit_count() {
        let s = scrubber("");
        let fake = s.fake("555-010-7788");
        assert_eq!(fake.len(), "555-010-7788".len());
        assert_eq!(fake.matches('-').count(), 2);
        assert!(fake.chars().filter(char::is_ascii_digit).count() == 10);
    }

    #[test]
    fn a_fake_card_number_still_passes_luhn() {
        let s = scrubber("");
        let fake = s.fake("4539 5787 6362 1486");
        assert_eq!(fake.len(), "4539 5787 6362 1486".len());
        let digits: String = fake.chars().filter(char::is_ascii_digit).collect();
        assert!(pii::luhn(&digits), "{fake}");
        assert_ne!(digits, "4539578763621486");
    }

    #[test]
    fn a_fake_integer_keeps_its_digit_count_and_its_lead() {
        let s = scrubber("");
        for n in [7i64, 42, 100_000, -9_876_543] {
            let fake = s.fake_integer(n);
            assert_eq!(
                fake.unsigned_abs().to_string().len(),
                n.unsigned_abs().to_string().len()
            );
            assert_eq!(fake < 0, n < 0);
        }
    }

    #[test]
    fn a_hash_is_hex_deterministic_for_one_salt_and_differs_across_salts() {
        let a = scrubber("").hash("ana@example.com");
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(a, scrubber("").hash("ana@example.com"));
        let other = Scrubber::new(Rules::default(), [9; 32]).expect("a scrubber");
        assert_ne!(a, other.hash("ana@example.com"));
    }

    #[test]
    fn the_same_value_in_two_columns_becomes_the_same_fake_so_joins_match() {
        let mut s = scrubber(
            "[[column]]\ntable = \"users\"\ncolumn = \"email\"\naction = \"fake\"\n[[column]]\ntable = \"contacts\"\ncolumn = \"address\"\naction = \"hash\"\n",
        );
        let a = text(s.value("users", "email", Value::Text("ana@example.com".into())));
        let b = text(s.value("contacts", "address", Value::Text("ana@example.com".into())));
        // Different actions: the join is kept within one action (the fake of one column and the hash of another differ),
        // and the same action on the same value is the same output everywhere.
        assert_ne!(a, b);
        let c = text(s.value("users", "email", Value::Text("ana@example.com".into())));
        assert_eq!(a, c);
    }

    #[test]
    fn a_removed_value_is_null_and_a_blob_in_a_ruled_column_is_null() {
        let mut s = scrubber("[[column]]\ntable = \"t\"\ncolumn = \"c\"\naction = \"remove\"\n");
        assert_eq!(s.value("t", "c", Value::Text("x".into())), Value::Null);
        assert_eq!(s.value("t", "c", Value::Blob(vec![1, 2])), Value::Null);
        assert_eq!(
            s.value("t", "other", Value::Text("kept".into())),
            Value::Text("kept".into())
        );
    }

    #[test]
    fn a_document_field_is_replaced_and_a_document_that_is_not_json_is_cut() {
        let mut s = scrubber(
            "[[field]]\ntable = \"events\"\ncolumn = \"payload\"\npath = \"/user/email\"\naction = \"hash\"\n",
        );
        let out = text(s.value(
            "events",
            "payload",
            Value::Text(r#"{"user":{"email":"ana@example.com","id":4},"kind":"login"}"#.into()),
        ));
        assert!(!out.contains("ana@example.com"), "{out}");
        assert!(out.contains("\"kind\":\"login\""));
        assert!(out.contains("\"id\":4"));
        assert_eq!(
            s.value(
                "events",
                "payload",
                Value::Text("not json ana@example.com".into())
            ),
            Value::Null
        );
        assert_eq!(s.cut_documents, 1);
    }

    #[test]
    fn a_log_line_rule_cuts_its_matches_and_the_scan_judges_the_rest() {
        let mut s = scrubber("[[line]]\npattern = \"token=\\\\S+\"\naction = \"remove\"\n");
        assert_eq!(s.line("auth token=abc123 ok"), "auth  ok");
        let mut fake = scrubber("[[line]]\npattern = \"token=\\\\S+\"\naction = \"fake\"\n");
        let out = fake.line("auth token=abc123 ok");
        assert!(!out.contains("abc123"), "{out}");
        assert!(
            s.line_kinds("user ana@example.com token=abc123")
                .contains(&"email")
        );
        assert!(
            s.line_kinds("auth token=ana@example.com").is_empty(),
            "the matched text is the rule's, not the scan's"
        );
    }

    #[test]
    fn names_are_found_as_whole_words_and_a_project_adds_its_own() {
        let s = Scrubber::new(
            Rules::parse("names = [\"Zoltan\"]\n").expect("parses"),
            SALT,
        )
        .expect("a scrubber");
        assert!(s.cell_kinds("Ana's order").contains(&"name"));
        assert!(s.cell_kinds("Zoltan").contains(&"name"));
        assert!(
            s.cell_kinds("Anabel").is_empty(),
            "a name inside a word is not a name"
        );
        assert!(
            s.cell_kinds("ana lowercase").is_empty(),
            "names are case as written"
        );
    }

    #[test]
    fn a_cell_that_is_the_scrubbers_own_output_is_not_judged() {
        let mut s =
            scrubber("[[column]]\ntable = \"users\"\ncolumn = \"email\"\naction = \"fake\"\n");
        let fake = text(s.value("users", "email", Value::Text("ana@example.com".into())));
        assert!(
            s.cell_kinds(&fake).is_empty(),
            "the fake is the scrubber's, not data"
        );
        assert_eq!(s.cell_kinds("ana@example.com"), ["email"]);
    }

    #[test]
    fn the_rules_file_is_checked() {
        assert!(Rules::parse("[[field]]\ntable = \"t\"\ncolumn = \"c\"\npath = \"user/email\"\naction = \"hash\"\n").is_err());
        assert!(Rules::parse("[[line]]\npattern = \"(\"\naction = \"remove\"\n").is_err());
        assert!(
            Rules::parse("[[column]]\ntable = \"t\"\ncolumn = \"c\"\naction = \"keep\"\n").is_err()
        );
        let twice = "[[column]]\ntable = \"t\"\ncolumn = \"c\"\naction = \"hash\"\n[[column]]\ntable = \"t\"\ncolumn = \"c\"\naction = \"fake\"\n";
        assert!(Rules::parse(twice).is_err());
        assert!(Rules::parse("unknown = 1\n").is_err());
    }

    #[test]
    fn the_report_names_places_and_counts_and_never_a_value() {
        let report = Report {
            tables: 1,
            cells: 2,
            cut_documents: 0,
            findings: vec![Finding {
                table: "users".into(),
                column: "email".into(),
                kind: "email".into(),
                samples: 2,
            }],
        };
        let shown = report.to_string();
        assert_eq!(shown, "personal data in users.email (email: 2 samples)");
        assert!(!shown.contains("@"));
    }
}
