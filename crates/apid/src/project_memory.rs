//! Project memory (PLAN 9.3 to 9.5): the entries a project keeps in its Weft repo under `.plaza/memory/`, the index the
//! hub renders from them, the search and reads the host tools answer from it, and the proposals agents make.
//!
//! An entry is one file: its front matter (typed here, see [`parse`]) and its text. The hub reads a project's entries
//! from its repo's main when the project is first read in this process, and again when a change lands on the repo
//! (`landed`). A lane's own entries are not read yet: the curator's landings bring them in (PLAN 9.4). A proposal is
//! stored in plaza.db and goes nowhere by itself.
//!
//! The index is part 3 of every session's prompt (agents.rs), so it depends on main alone: the sessions of a project
//! at the same main carry the same bytes.

use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, TableDef};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::agents::Agent;
use crate::state::App;

/// The host tools this module serves.
pub const TOOLS: &[&str] = &["memory_search", "memory_get", "memory_propose"];

/// Where a project's entries live in its repo, and the index the hub renders there (never edited by hand).
const DIR: &str = ".plaza/memory/";
const INDEX: &str = ".plaza/memory/index.md";

/// The entry types (9.3), in the order the index lists them.
pub const KINDS: &[&str] = &[
    "decision",
    "convention",
    "pitfall",
    "howto",
    "map",
    "glossary",
    "fact",
    "reference",
];
const STATUSES: &[&str] = &["proposed", "active", "flagged", "retired", "superseded"];
/// The statuses an entry is used with: the index and search read these; the others are kept for their history.
const LIVE: &[&str] = &["active", "flagged"];

/// How many hits a search answers with.
const HITS: usize = 20;
const TITLE_MAX: usize = 100;
const TEXT_MAX: usize = 2000;
const WHY_MAX: usize = 500;
const ANCHORS_MAX: usize = 20;
const ANCHOR_MAX: usize = 300;

/// One entry (9.3): its file's front matter and text, with the project it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "memory_entry")]
pub struct Entry {
    /// `<project>/<id>`: an id is unique in its project only.
    #[table(key)]
    pub key: String,
    pub project: String,
    /// `P-0017`.
    pub id: String,
    /// One of `KINDS`.
    pub kind: String,
    /// One of `STATUSES`.
    pub status: String,
    /// The first line of its text, without its heading marks.
    pub title: String,
    /// Its text, after the front matter.
    pub body: String,
    /// Its file in the repo.
    pub path: String,
    /// The Weft symbol ids (or paths and commits) it is about.
    #[serde(default)]
    #[table(list)]
    pub anchors: Vec<String>,
    #[serde(default)]
    #[table(list)]
    pub supersedes: Vec<String>,
    #[serde(default)]
    #[table(list)]
    pub links: Vec<String>,
    /// When it was last checked, and against which commit.
    #[serde(default)]
    pub verified_commit: String,
    #[serde(default)]
    pub verified_date: String,
    /// Where it came from: the task, the session and the change.
    #[serde(default)]
    pub provenance_task: String,
    #[serde(default)]
    pub provenance_session: String,
    #[serde(default)]
    pub provenance_change: String,
}

/// A proposal an agent made for its project's memory (`memory_propose`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "memory_proposal")]
pub struct Proposal {
    /// `mp<n>`.
    #[table(key)]
    pub id: String,
    pub project: String,
    pub kind: String,
    pub text: String,
    #[serde(default)]
    #[table(list)]
    pub anchors: Vec<String>,
    pub why: String,
    pub by_cid: u64,
    pub by: String,
    pub created_ms: u64,
}

/// The project memory: the entries read from each project's main, and the proposals agents made.
#[derive(Default)]
pub struct Store {
    entries: Mutex<Vec<Entry>>,
    /// The projects whose entries were read in this process (a project not in here is read at first use).
    read: Mutex<HashSet<String>>,
    proposals: Mutex<Vec<Proposal>>,
}

impl Store {
    /// The project's entries as the index last had them, all statuses.
    pub fn entries_of(&self, project: &str) -> Vec<Entry> {
        lock(&self.entries)
            .iter()
            .filter(|e| e.project == project)
            .cloned()
            .collect()
    }

    fn is_read(&self, project: &str) -> bool {
        lock(&self.read).contains(project)
    }

    /// Replaces the project's entries with `read` and marks the project read; the other projects keep theirs.
    fn replace(&self, project: &str, read: Vec<Entry>) {
        let mut entries = lock(&self.entries);
        entries.retain(|e| e.project != project);
        entries.extend(read);
        lock(&self.read).insert(project.to_string());
    }

    /// Stores a proposal under the next `mp<n>` id; returns it with the id.
    pub fn add_proposal(&self, mut proposal: Proposal) -> Proposal {
        let mut items = lock(&self.proposals);
        let next = items
            .iter()
            .filter_map(|p| p.id.strip_prefix("mp")?.parse::<u64>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        proposal.id = format!("mp{next}");
        items.push(proposal.clone());
        proposal
    }

    /// Every entry's rows, as they are now (the saver writes what changed).
    pub fn entry_rows(&self) -> Result<RowSet, StoreError> {
        RowSet::list(entry_def(), lock(&self.entries).iter())
    }

    /// Replaces the entries with what plaza.db holds (at start). The projects are read again at first use.
    pub fn load_entries(&self, conn: &Connection) -> Result<(), StoreError> {
        let stored: Vec<Entry> = apz_store::table::load_list(conn, entry_def())?;
        *lock(&self.entries) = stored;
        Ok(())
    }

    /// Every proposal's rows, as they are now.
    pub fn proposal_rows(&self) -> Result<RowSet, StoreError> {
        RowSet::list(proposal_def(), lock(&self.proposals).iter())
    }

    /// Replaces the proposals with what plaza.db holds (at start), in the order they were kept.
    pub fn load_proposals(&self, conn: &Connection) -> Result<(), StoreError> {
        let stored: Vec<Proposal> = apz_store::table::load_list(conn, proposal_def())?;
        *lock(&self.proposals) = stored;
        Ok(())
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// The entries' table: `memory_entry`, with their anchors, supersedes and links as child rows.
pub fn entry_def() -> &'static TableDef {
    apz_store::root_def!(Entry)
}

/// The proposals' table: `memory_proposal`, with their anchors as child rows.
pub fn proposal_def() -> &'static TableDef {
    apz_store::root_def!(Proposal)
}

/// The project's entries as its index has them, read from its repo's main first when this process has not read them.
/// A project with no repo yet has none; with Weft down, the entries plaza.db kept (none at first). The next call tries
/// again.
pub fn entries(app: &App, project: &str) -> Vec<Entry> {
    let has_repo = app.projects.get(project).is_some_and(|p| p.repo.is_some());
    if has_repo
        && !app.project_memory.is_read(project)
        && let Err(why) = refresh(app, project)
    {
        eprintln!("apid: project memory of {project}: {why}");
    }
    app.project_memory.entries_of(project)
}

/// Reads the project's entries from its repo's main and replaces the index with them; the count read.
pub fn refresh(app: &App, project: &str) -> Result<usize, String> {
    let repo = app
        .projects
        .get(project)
        .and_then(|p| p.repo)
        .ok_or_else(|| format!("{project} has no repo yet"))?;
    let detail = app.weft.get(&format!("/repos/{repo}"))?;
    let rev = detail["refs"]["main"]
        .as_str()
        .ok_or_else(|| format!("{repo} has no main"))?;
    let listing = app.weft.get(&format!("/revisions/{rev}/files"))?;
    let client = app.weft.client("hub")?;
    let mut read = Vec::new();
    for file in listing["files"].as_array().into_iter().flatten() {
        let Some(path) = file["path"].as_str().filter(|p| is_entry(p)) else {
            continue;
        };
        let Ok(bytes) = client.get_bytes(&format!("/revisions/{rev}/file?path={path}")) else {
            continue;
        };
        if let Some(entry) = parse(project, path, &String::from_utf8_lossy(&bytes)) {
            read.push(entry);
        }
    }
    let count = read.len();
    app.project_memory.replace(project, read);
    Ok(count)
}

/// A change landed on `repo`: the projects on it read their entries again, so the index follows main.
pub fn landed(app: &App, repo: &str) {
    for project in app
        .projects
        .list()
        .into_iter()
        .filter(|p| p.repo.as_deref() == Some(repo))
    {
        if let Err(why) = refresh(app, &project.id) {
            eprintln!("apid: project memory of {}: {why}", project.id);
        }
    }
}

/// Whether a file of the repo is an entry: a `.md` under the memory folder, other than the index. Paths with other
/// characters are left out, so the read needs no escaping.
fn is_entry(path: &str) -> bool {
    path.starts_with(DIR)
        && path.ends_with(".md")
        && path != INDEX
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c))
}

/// The entry a file holds (9.3): its front matter, then its text. `None` when the file has no front matter, or its
/// type or status is not one of ours.
pub fn parse(project: &str, path: &str, text: &str) -> Option<Entry> {
    let rest = text.strip_prefix("---\n")?;
    let (block, body) = rest.split_once("\n---")?;
    if !body.is_empty() && !body.starts_with('\n') {
        return None;
    }
    let fields = front(block)?;
    let id = field(&fields, "id")?;
    let kind = field(&fields, "type")?;
    let status = field(&fields, "status").unwrap_or_else(|| "active".into());
    if !KINDS.contains(&kind.as_str()) || !STATUSES.contains(&status.as_str()) {
        return None;
    }
    let verified = map(&fields, "verified");
    let provenance = map(&fields, "provenance");
    let body = body.trim().to_string();
    Some(Entry {
        key: format!("{project}/{id}"),
        project: project.to_string(),
        title: title_of(&body),
        id,
        kind,
        status,
        body,
        path: path.to_string(),
        anchors: list(&fields, "anchors"),
        supersedes: list(&fields, "supersedes"),
        links: list(&fields, "links"),
        verified_commit: field(&verified, "commit").unwrap_or_default(),
        verified_date: field(&verified, "date").unwrap_or_default(),
        provenance_task: field(&provenance, "task").unwrap_or_default(),
        provenance_session: field(&provenance, "session").unwrap_or_default(),
        provenance_change: field(&provenance, "change").unwrap_or_default(),
    })
}

/// The front matter's YAML subset: `key: value` lines, where a value is a scalar, a flow list `[a, b]`, a flow map
/// `{ k: v }`, or, when empty, the `- item` lines indented under its key. A `#` after a space starts a comment.
fn front(block: &str) -> Option<Map<String, Value>> {
    let mut fields = Map::new();
    let mut list_key: Option<String> = None;
    for raw in block.lines() {
        let line = strip_comment(raw);
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            let item = line.trim().strip_prefix("- ")?;
            match fields.get_mut(list_key.as_ref()?)? {
                Value::Array(items) => items.push(scalar(item)),
                _ => return None,
            }
        } else {
            let (key, value) = line.split_once(':')?;
            let (key, value) = (key.trim(), value.trim());
            list_key = value.is_empty().then(|| key.to_string());
            let value = if value.is_empty() {
                Value::Array(Vec::new())
            } else {
                flow(value)
            };
            fields.insert(key.to_string(), value);
        }
    }
    Some(fields)
}

fn strip_comment(line: &str) -> &str {
    match line.find(" #") {
        Some(at) => &line[..at],
        None if line.starts_with('#') => "",
        None => line,
    }
}

fn flow(value: &str) -> Value {
    if let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
        Value::Array(
            inner
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(scalar)
                .collect(),
        )
    } else if let Some(inner) = value.strip_prefix('{').and_then(|v| v.strip_suffix('}')) {
        Value::Object(
            inner
                .split(',')
                .filter_map(|pair| {
                    let (key, value) = pair.split_once(':')?;
                    Some((key.trim().to_string(), scalar(value.trim())))
                })
                .collect(),
        )
    } else {
        scalar(value)
    }
}

fn scalar(value: &str) -> Value {
    Value::String(value.trim().trim_matches(['"', '\'']).to_string())
}

fn field(fields: &Map<String, Value>, key: &str) -> Option<String> {
    fields.get(key)?.as_str().map(str::to_string)
}

fn list(fields: &Map<String, Value>, key: &str) -> Vec<String> {
    fields
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn map(fields: &Map<String, Value>, key: &str) -> Map<String, Value> {
    fields
        .get(key)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// The first line of text, without its heading marks, cut to the title's length.
fn title_of(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .trim_start_matches('#')
        .trim()
        .chars()
        .take(TITLE_MAX)
        .collect()
}

/// The position of an entry's type in `KINDS`: the order the index lists them in.
fn rank(kind: &str) -> usize {
    KINDS
        .iter()
        .position(|known| *known == kind)
        .unwrap_or(KINDS.len())
}

/// The index (`index.md`, and part 3 of every session's prompt): one line per live entry, `- id type: title`, by type
/// then id. Empty when the project has no live entry.
pub fn index_text(entries: &[Entry]) -> String {
    let mut live: Vec<&Entry> = entries
        .iter()
        .filter(|e| LIVE.contains(&e.status.as_str()))
        .collect();
    live.sort_by(|a, b| (rank(&a.kind), &a.id).cmp(&(rank(&b.kind), &b.id)));
    live.iter()
        .map(|e| format!("- {} {}: {}", e.id, e.kind, e.title))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The live entries that match: every word of `text` in the title or the text, the type (when given) and the anchor
/// (when given, an exact match), by type then id.
pub fn search(entries: &[Entry], text: &str, kind: &str, anchor: &str) -> Vec<Entry> {
    let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
    let mut hits: Vec<Entry> = entries
        .iter()
        .filter(|e| LIVE.contains(&e.status.as_str()))
        .filter(|e| kind.is_empty() || e.kind == kind)
        .filter(|e| anchor.is_empty() || e.anchors.iter().any(|a| a == anchor))
        .filter(|e| {
            let (title, body) = (e.title.to_lowercase(), e.body.to_lowercase());
            words.iter().all(|w| title.contains(w) || body.contains(w))
        })
        .cloned()
        .collect();
    hits.sort_by(|a, b| (rank(&a.kind), &a.id).cmp(&(rank(&b.kind), &b.id)));
    hits
}

/// An entry as `memory_get` answers it: its header lines, then its text.
fn show(entry: &Entry) -> String {
    let mut lines = vec![format!("{} {} ({})", entry.id, entry.kind, entry.status)];
    for (label, items) in [
        ("anchors", &entry.anchors),
        ("links", &entry.links),
        ("supersedes", &entry.supersedes),
    ] {
        if !items.is_empty() {
            lines.push(format!("{label}: {}", items.join(", ")));
        }
    }
    if !entry.verified_date.is_empty() {
        lines.push(format!(
            "verified: {} {}",
            entry.verified_date, entry.verified_commit
        ));
    }
    lines.push(String::new());
    lines.push(entry.body.clone());
    lines.join("\n")
}

/// The spec of `memory_search`, `memory_get` and `memory_propose` lives in builtin_specs.json; this is what they answer.
pub fn run(app: &App, agent: &Agent, name: &str, input: &Value) -> Result<String, String> {
    let project = crate::projects::project_of(app, agent.cid)
        .ok_or("project memory belongs to a project: this session is not on one")?;
    match name {
        "memory_search" => {
            let hits = search(
                &entries(app, &project),
                &arg(input, "text"),
                &arg(input, "type"),
                &arg(input, "anchor"),
            );
            if hits.is_empty() {
                return Ok("No entries match.".into());
            }
            let mut lines: Vec<String> = hits
                .iter()
                .take(HITS)
                .map(|e| format!("{} {}: {}", e.id, e.kind, e.title))
                .collect();
            if hits.len() > HITS {
                lines.push(format!("{} more: narrow the search", hits.len() - HITS));
            }
            Ok(lines.join("\n"))
        }
        "memory_get" => {
            let id = arg(input, "id");
            if id.is_empty() {
                return Err("id is required".into());
            }
            entries(app, &project)
                .into_iter()
                .find(|e| e.id == id)
                .map(|e| show(&e))
                .ok_or_else(|| format!("no entry {id} in this project"))
        }
        "memory_propose" => {
            let proposal = app.project_memory.add_proposal(proposal_from(
                &project,
                agent.cid,
                &agent.label(),
                input,
                now_ms(),
            )?);
            Ok(format!(
                "Proposal {} is stored for the project's curator. Nothing lands in its memory from it by itself.",
                proposal.id
            ))
        }
        other => Err(format!("{other} is not a memory tool")),
    }
}

/// A proposal from `memory_propose`'s input, checked: a type, the text and why (within their limits), and up to
/// `ANCHORS_MAX` anchors (each one non-empty, without repeats).
fn proposal_from(
    project: &str,
    by_cid: u64,
    by: &str,
    input: &Value,
    created_ms: u64,
) -> Result<Proposal, String> {
    let kind = arg(input, "type");
    if !KINDS.contains(&kind.as_str()) {
        return Err(format!("type must be one of {}", KINDS.join(", ")));
    }
    let text = arg(input, "text");
    let why = arg(input, "why");
    for (name, value, max) in [("text", &text, TEXT_MAX), ("why", &why, WHY_MAX)] {
        if value.is_empty() {
            return Err(format!("{name} is required"));
        }
        if value.chars().count() > max {
            return Err(format!("{name} is over {max} characters"));
        }
    }
    let raw = input
        .get("anchors")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if raw.len() > ANCHORS_MAX {
        return Err(format!("anchors takes at most {ANCHORS_MAX} entries"));
    }
    let mut anchors: Vec<String> = Vec::new();
    for anchor in raw.iter().filter_map(Value::as_str).map(str::trim) {
        if anchor.is_empty() || anchor.chars().count() > ANCHOR_MAX {
            return Err(format!(
                "an anchor is empty or over {ANCHOR_MAX} characters"
            ));
        }
        if !anchors.iter().any(|a| a == anchor) {
            anchors.push(anchor.to_string());
        }
    }
    Ok(Proposal {
        id: String::new(),
        project: project.to_string(),
        kind,
        text,
        anchors,
        why,
        by_cid,
        by: by.to_string(),
        created_ms,
    })
}

fn arg(input: &Value, key: &str) -> String {
    input
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// The example of PLAN 9.3, as its file holds it.
    const PITFALL: &str = "---
id: P-0017
type: pitfall            # decision | convention | pitfall | howto | map | glossary | fact | reference
anchors:                 # Weft symbol ids where they exist, paths and commits otherwise
  - sym:apid::placement::move_claimed
  - sym:apid::sleep::wake_with
provenance: { task: T-311, session: s-9042, change: lane/node-stop@4d96 }
verified: { commit: 976118ce, date: 2026-10-20 }   # when, and against what, it was last checked
status: active           # proposed | active | flagged | retired | superseded
supersedes: []
links: [D-0042]
---
A move must release its claim before waking the agent on the target: `wake_with` skips a moving agent.
";

    fn entry(id: &str, kind: &str, status: &str, text: &str) -> Entry {
        entry_in("pr-1", id, kind, status, text)
    }

    fn entry_in(project: &str, id: &str, kind: &str, status: &str, text: &str) -> Entry {
        parse(
            project,
            &format!(".plaza/memory/{kind}/{id}.md"),
            &format!("---\nid: {id}\ntype: {kind}\nstatus: {status}\n---\n{text}\n"),
        )
        .unwrap_or_else(|| panic!("{id} parses"))
    }

    #[test]
    fn an_entry_file_reads_its_front_matter_and_its_title() {
        let e = parse("pr-1", ".plaza/memory/pitfalls/P-0017-move.md", PITFALL).unwrap();
        assert_eq!(e.key, "pr-1/P-0017");
        assert_eq!(
            (e.id.as_str(), e.kind.as_str(), e.status.as_str()),
            ("P-0017", "pitfall", "active")
        );
        assert_eq!(
            e.anchors,
            [
                "sym:apid::placement::move_claimed",
                "sym:apid::sleep::wake_with"
            ]
        );
        assert_eq!(e.links, ["D-0042"]);
        assert!(e.supersedes.is_empty());
        assert_eq!(
            (e.verified_commit.as_str(), e.verified_date.as_str()),
            ("976118ce", "2026-10-20")
        );
        assert_eq!(
            (
                e.provenance_task.as_str(),
                e.provenance_session.as_str(),
                e.provenance_change.as_str()
            ),
            ("T-311", "s-9042", "lane/node-stop@4d96")
        );
        assert!(
            e.title.starts_with("A move must release its claim"),
            "{}",
            e.title
        );
        assert!(!e.body.contains("---"), "{}", e.body);
    }

    #[test]
    fn a_file_without_front_matter_or_with_an_unknown_type_is_not_an_entry() {
        assert!(parse("pr-1", "a.md", "# Just text\n").is_none());
        assert!(parse("pr-1", "a.md", "---\nid: X-1\ntype: rumour\n---\nText\n").is_none());
        assert!(
            parse(
                "pr-1",
                "a.md",
                "---\nid: X-1\ntype: fact\nstatus: maybe\n---\nText\n"
            )
            .is_none()
        );
        assert!(parse("pr-1", "a.md", "---\nid: X-1\ntype: fact\n---x\nText\n").is_none());
    }

    #[test]
    fn only_entries_under_the_memory_folder_are_read() {
        assert!(is_entry(".plaza/memory/decisions/D-0042-sessions.md"));
        assert!(is_entry(".plaza/memory/glossary.md"));
        assert!(!is_entry(".plaza/memory/index.md"));
        assert!(!is_entry("src/main.rs"));
        assert!(!is_entry(".plaza/project.toml"));
        assert!(!is_entry(".plaza/memory/notes with spaces.md"));
    }

    #[test]
    fn the_index_lists_the_live_entries_by_type_then_id() {
        let entries = [
            entry(
                "P-0017",
                "pitfall",
                "active",
                "Move releases its claim first.",
            ),
            entry(
                "D-0042",
                "decision",
                "flagged",
                "Sessions, not named agents.",
            ),
            entry("D-0007", "decision", "retired", "Old thing."),
            entry("C-0007", "convention", "active", "Merge, never rebase."),
        ];
        assert_eq!(
            index_text(&entries),
            "- D-0042 decision: Sessions, not named agents.\n\
             - C-0007 convention: Merge, never rebase.\n\
             - P-0017 pitfall: Move releases its claim first."
        );
        assert_eq!(index_text(&[]), "");
    }

    #[test]
    fn search_matches_every_word_and_the_type_and_the_anchor_when_given() {
        let mut move_entry = entry(
            "P-0017",
            "pitfall",
            "active",
            "Move releases its claim first.",
        );
        move_entry.anchors = vec!["sym:apid::placement::move_claimed".into()];
        let entries = [
            move_entry,
            entry("P-0018", "pitfall", "active", "The wake never starts."),
            entry("D-0042", "decision", "active", "Move is a decision too."),
            entry(
                "P-0019",
                "pitfall",
                "superseded",
                "Move was once different.",
            ),
        ];
        let ids = |hits: Vec<Entry>| hits.into_iter().map(|e| e.id).collect::<Vec<_>>();
        assert_eq!(ids(search(&entries, "move", "", "")), ["D-0042", "P-0017"]);
        assert_eq!(ids(search(&entries, "move claim", "", "")), ["P-0017"]);
        assert_eq!(ids(search(&entries, "move", "decision", "")), ["D-0042"]);
        assert_eq!(
            ids(search(
                &entries,
                "",
                "",
                "sym:apid::placement::move_claimed"
            )),
            ["P-0017"]
        );
        assert!(search(&entries, "absent", "", "").is_empty());
    }

    #[test]
    fn a_proposal_is_checked_and_stored_under_its_own_id() {
        let good = json!({
            "type": "pitfall",
            "text": "The wake never starts while the agent moves.",
            "anchors": ["sym:apid::sleep::wake_with", "sym:apid::sleep::wake_with"],
            "why": "Found while debugging the move.",
        });
        let proposal = proposal_from("pr-1", 7, "builder #7", &good, 5).unwrap();
        assert_eq!(proposal.anchors, ["sym:apid::sleep::wake_with"]);
        assert_eq!(
            (
                proposal.project.as_str(),
                proposal.by_cid,
                proposal.created_ms
            ),
            ("pr-1", 7, 5)
        );

        let store = Store::default();
        let first = store.add_proposal(proposal.clone());
        let second = store.add_proposal(proposal);
        assert_eq!((first.id.as_str(), second.id.as_str()), ("mp1", "mp2"));
        assert_eq!(lock(&store.proposals).len(), 2);

        let refused = |input: Value| proposal_from("pr-1", 7, "b", &input, 0).unwrap_err();
        assert!(
            refused(json!({ "type": "rumour", "text": "x", "why": "y" }))
                .starts_with("type must be")
        );
        assert_eq!(
            refused(json!({ "type": "fact", "text": "", "why": "y" })),
            "text is required"
        );
        assert_eq!(
            refused(json!({ "type": "fact", "text": "x" })),
            "why is required"
        );
        assert!(
            refused(json!({ "type": "fact", "text": "x", "why": "y", "anchors": [" "] }))
                .starts_with("an anchor is empty")
        );
        let many: Vec<String> = (0..=ANCHORS_MAX).map(|i| format!("sym:x::{i}")).collect();
        assert!(
            refused(json!({ "type": "fact", "text": "x", "why": "y", "anchors": many }))
                .starts_with("anchors takes at most")
        );
    }

    #[test]
    fn replacing_one_project_keeps_the_others_entries() {
        let store = Store::default();
        store.replace(
            "pr-1",
            vec![entry_in("pr-1", "P-0001", "fact", "active", "One.")],
        );
        store.replace(
            "pr-2",
            vec![entry_in("pr-2", "P-0001", "fact", "active", "Two.")],
        );
        store.replace("pr-1", Vec::new());
        assert!(store.entries_of("pr-1").is_empty());
        assert_eq!(store.entries_of("pr-2")[0].title, "Two.");
        assert!(store.is_read("pr-1") && store.is_read("pr-2"));
    }
}
