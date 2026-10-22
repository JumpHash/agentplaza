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

use std::collections::{BTreeMap, HashSet};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use apz_blobs::Hash;
use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, TableDef};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::agents::Agent;
use crate::state::App;

/// The host tools this module serves.
pub const TOOLS: &[&str] = &[
    "memory_search",
    "memory_get",
    "memory_propose",
    "memory_record_decision",
    "memory_anchored",
];

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
/// How many paths or symbols one `memory_anchored` query names.
const QUERY_MAX: usize = 100;

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

/// A landing of memory the hub queued and has not seen land yet: its change, and what it writes and deletes.
#[derive(Clone)]
struct Pending {
    change: String,
    write: Vec<Entry>,
    delete: Vec<String>,
}

/// The project memory: the entries read from each project's main, and the proposals agents made.
#[derive(Default)]
pub struct Store {
    entries: Mutex<Vec<Entry>>,
    /// The projects whose entries were read in this process (a project not in here is read at first use).
    read: Mutex<HashSet<String>>,
    proposals: Mutex<Vec<Proposal>>,
    /// The memory changes queued on each project's main (see [`land`]), so the next one builds on them.
    pending: Mutex<BTreeMap<String, Vec<Pending>>>,
    /// Serialises the landings: an entry's id is allocated and its change opened under it.
    landing: Mutex<()>,
}

impl Store {
    /// The proposals of `project`, in the order they were made.
    pub fn proposals_of(&self, project: &str) -> Vec<Proposal> {
        lock(&self.proposals)
            .iter()
            .filter(|p| p.project == project)
            .cloned()
            .collect()
    }

    /// Removes the proposals named `ids` (the curator answered them); the other proposals stay.
    pub fn take_proposals(&self, ids: &[String]) {
        lock(&self.proposals).retain(|p| !ids.contains(&p.id));
    }

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

/// A change landed on `repo`: the projects on it read their entries again, so the index follows main, and the entries
/// the change touches are flagged (9.6).
pub fn landed(app: &App, repo: &str, change: &str) {
    for project in app
        .projects
        .list()
        .into_iter()
        .filter(|p| p.repo.as_deref() == Some(repo))
    {
        if let Err(why) = refresh(app, &project.id) {
            eprintln!("apid: project memory of {}: {why}", project.id);
            continue;
        }
        if let Err(why) = flag_touched(app, &project.id, change) {
            eprintln!("apid: flagging the memory of {}: {why}", project.id);
        }
    }
}

/// Flags the active entries a landed change touches, as one change to main. The verifier answers them (see `verifier`).
fn flag_touched(app: &App, project: &str, change: &str) -> Result<(), String> {
    let (paths, symbols) = touched(app, change)?;
    let flagged = to_flag(&entries(app, project), &paths, &symbols, change);
    if flagged.is_empty() {
        return Ok(());
    }
    let title = format!("Flag {} entries that {change} touches", flagged.len());
    land(app, project, &title, flagged, &[]).map(|_| ())
}

/// What a landed change touched, as Weft's change tree names it: the files it changed, and the symbols it changed
/// (`path::qualified`, the ids an entry's anchors name).
fn touched(app: &App, change: &str) -> Result<(Vec<String>, Vec<String>), String> {
    let rev = landed_revision(app, change)?;
    let tree = app.weft.get(&format!("/revisions/{rev}/tree"))?;
    let mut paths = Vec::new();
    let mut symbols = Vec::new();
    for file in tree["files"].as_array().into_iter().flatten() {
        if let Some(path) = file["path"].as_str() {
            paths.push(path.to_string());
        }
        for symbol in file["symbols"].as_array().into_iter().flatten() {
            if let Some(id) = symbol["id"].as_str() {
                symbols.push(id.to_string());
            }
        }
    }
    Ok((paths, symbols))
}

/// The revision a change landed with: its last one (drafts, rounds, then the landed).
pub fn landed_revision(app: &App, change: &str) -> Result<String, String> {
    let detail = app.weft.get(&format!("/changes/{change}"))?;
    detail["revisions"]
        .as_array()
        .and_then(|revisions| revisions.last())
        .and_then(|revision| revision["id"].as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("{change} has no revision"))
}

/// The active entries anchored to what a change touched, as flagged copies (9.6): each one names the change that flagged
/// it as its provenance, and is flagged until the verifier answers it.
pub fn to_flag(
    entries: &[Entry],
    paths: &[String],
    symbols: &[String],
    change: &str,
) -> Vec<Entry> {
    anchored(entries, paths, symbols)
        .into_iter()
        .filter(|e| e.status == "active")
        .map(|mut e| {
            e.status = "flagged".into();
            e.provenance_change = change.into();
            e
        })
        .collect()
}

/// The live entries anchored to any of `paths` or `symbols` (9.5), by type then id. An anchor is a symbol id (`path::name`,
/// as Weft names one) or a path; it is about a path when it is that path or lies under it.
pub fn anchored(entries: &[Entry], paths: &[String], symbols: &[String]) -> Vec<Entry> {
    let about_path = |anchor: &str| {
        paths.iter().any(|path| {
            anchor == path
                || anchor
                    .strip_prefix(path.as_str())
                    .is_some_and(|rest| rest.starts_with("::"))
        })
    };
    let mut hits: Vec<Entry> = entries
        .iter()
        .filter(|e| LIVE.contains(&e.status.as_str()))
        .filter(|e| {
            e.anchors
                .iter()
                .any(|anchor| symbols.contains(anchor) || about_path(anchor))
        })
        .cloned()
        .collect();
    hits.sort_by(|a, b| (rank(&a.kind), &a.id).cmp(&(rank(&b.kind), &b.id)));
    hits
}

/// What a landing made: the change on main and the ids of the entries it writes, in the order they were given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landing {
    pub change: String,
    pub ids: Vec<String>,
}

/// An entry the curator, the planner or the owner writes, before it has an id: its type and text, and where it came
/// from. [`land`] gives it its id, path and verification; the caller sets the rest.
pub fn draft(project: &str, kind: &str, body: String) -> Entry {
    Entry {
        key: String::new(),
        project: project.to_string(),
        id: String::new(),
        kind: kind.to_string(),
        status: "active".into(),
        title: title_of(&body),
        body,
        path: String::new(),
        anchors: Vec::new(),
        supersedes: Vec::new(),
        links: Vec::new(),
        verified_commit: String::new(),
        verified_date: String::new(),
        provenance_task: String::new(),
        provenance_session: String::new(),
        provenance_change: String::new(),
    }
}

/// Lands entries and deletions on `project`'s main as one change, the hub's own: a lane the hub opens (stacked on the
/// memory change it queued last, if that has not landed), sealed and queued for the landing queue, where the land event
/// reads the entries back. An entry without an id gets the next one of its type; the index is regenerated with the
/// entries, and each entry is a file of its own. Landings are serialised, so ids are not reused.
pub fn land(
    app: &App,
    project: &str,
    title: &str,
    mut write: Vec<Entry>,
    delete: &[String],
) -> Result<Landing, String> {
    let _order = lock(&app.project_memory.landing);
    let repo = crate::weft_hub::ensure_repo(app, project)?;
    prune_landed(app, project)?;
    let stacked = pending_of(app, project).last().map(|p| p.change.clone());
    let detail = app.weft.get(&format!("/repos/{repo}"))?;
    let head = detail["refs"]["main"]
        .as_str()
        .ok_or_else(|| format!("{repo} has no main"))?
        .to_string();
    let now = now_ms();
    let mut taken: Vec<String> = effective(app, project).into_iter().map(|e| e.id).collect();
    for entry in write.iter_mut() {
        if entry.id.is_empty() {
            entry.id = next_id(&entry.kind, &taken);
            taken.push(entry.id.clone());
        }
        entry.project = project.to_string();
        entry.key = format!("{project}/{}", entry.id);
        if entry.path.is_empty() {
            entry.path = format!("{DIR}{}/{}-{}.md", entry.kind, entry.id, slug(&entry.title));
        }
        // An entry keeps the check it was given: a flagged one waits for the verifier, which clears it to confirm.
        if entry.verified_date.is_empty() {
            entry.verified_commit = head.clone();
            entry.verified_date = day_of(now);
        }
    }
    let mut expected = effective(app, project);
    expected.retain(|e| !delete.contains(&e.path) && !write.iter().any(|w| w.key == e.key));
    expected.extend(write.iter().cloned());
    let mut files: Vec<(String, Vec<u8>)> = write
        .iter()
        .map(|e| (e.path.clone(), render(e).into_bytes()))
        .collect();
    files.push((
        INDEX.to_string(),
        format!("{}\n", index_text(&expected)).into_bytes(),
    ));
    let hashes: Vec<Hash> = files.iter().map(|(_, bytes)| Hash::of(bytes)).collect();
    let bytes_of = |hash: &Hash| {
        files
            .iter()
            .zip(&hashes)
            .find(|(_, h)| *h == hash)
            .map(|((_, bytes), _)| bytes.clone())
            .ok_or_else(|| format!("no bytes for {}", hash.hex()))
    };
    app.weft.client("hub")?.upload(&hashes, &bytes_of)?;
    let mut lane = json!({ "repo": repo, "title": title, "author": { "kind": "hub" } });
    if let Some(change) = &stacked {
        lane["stacked_on"] = json!(change);
    }
    let opened = app.weft.post("/lanes", &lane)?;
    let change = opened["change"]
        .as_str()
        .ok_or("Weft opened no lane for the memory change")?
        .to_string();
    let mut deltas: Vec<Value> = files
        .iter()
        .zip(&hashes)
        .map(|((path, _), hash)| json!({ "path": path, "after": hash, "mode": 0o100644 }))
        .collect();
    deltas.extend(
        delete
            .iter()
            .map(|path| json!({ "path": path, "after": null, "mode": 0o100644 })),
    );
    let covers: Vec<String> = files
        .iter()
        .map(|(path, _)| path.clone())
        .chain(delete.iter().cloned())
        .collect();
    app.weft.hub(
        "revision.draft",
        json!({ "change": change, "n": 1, "deltas": deltas, "tool_calls": ["memory"] }),
    )?;
    app.weft.hub(
        "revision.seal",
        json!({ "change": change, "n": 1, "deltas": deltas }),
    )?;
    app.weft.hub(
        "intent.set",
        json!({ "change": change, "n": 1, "intents": [{ "title": title, "kind": "change", "covers": covers }] }),
    )?;
    app.weft.hub(
        "queue.add",
        json!({ "repo": repo, "ref": "main", "change": change, "n": 1 }),
    )?;
    let ids = write.iter().map(|e| e.id.clone()).collect();
    lock(&app.project_memory.pending)
        .entry(project.to_string())
        .or_default()
        .push(Pending {
            change: change.clone(),
            write,
            delete: delete.to_vec(),
        });
    Ok(Landing { change, ids })
}

/// The memory changes queued on `project`'s main, which have not landed yet.
fn pending_of(app: &App, project: &str) -> Vec<Pending> {
    lock(&app.project_memory.pending)
        .get(project)
        .cloned()
        .unwrap_or_default()
}

/// Drops the queued changes that have landed or were abandoned. A landed one is read back first, so the entries it
/// wrote stay in the index.
fn prune_landed(app: &App, project: &str) -> Result<(), String> {
    let queued = pending_of(app, project);
    let state = |change: &str| {
        app.weft
            .get(&format!("/changes/{change}"))
            .map(|c| c["state"].as_str().unwrap_or("").to_string())
            .unwrap_or_default()
    };
    let done: Vec<bool> = queued
        .iter()
        .map(|p| matches!(state(&p.change).as_str(), "landed" | "abandoned"))
        .collect();
    if done.iter().any(|d| *d) {
        refresh(app, project)?;
    }
    let open: Vec<Pending> = queued
        .into_iter()
        .zip(done)
        .filter(|(_, done)| !done)
        .map(|(p, _)| p)
        .collect();
    lock(&app.project_memory.pending).insert(project.to_string(), open);
    Ok(())
}

/// The entries `project` will have once the memory changes queued on its main land: the entries read from main, with
/// each queued change applied in order.
fn effective(app: &App, project: &str) -> Vec<Entry> {
    let mut all = entries(app, project);
    for change in pending_of(app, project) {
        all.retain(|e| {
            !change.delete.contains(&e.path) && !change.write.iter().any(|w| w.key == e.key)
        });
        all.extend(change.write);
    }
    all
}

/// The prefix an entry's id takes for its type (`D-0042`, `P-0017`).
fn prefix(kind: &str) -> char {
    match kind {
        "decision" => 'D',
        "convention" => 'C',
        "pitfall" => 'P',
        "howto" => 'H',
        "map" => 'M',
        "glossary" => 'G',
        "fact" => 'F',
        _ => 'R',
    }
}

/// The next id of `kind`, after every id in `taken`: four digits, as in 9.3.
fn next_id(kind: &str, taken: &[String]) -> String {
    let head = format!("{}-", prefix(kind));
    let last = taken
        .iter()
        .filter_map(|id| id.strip_prefix(&head)?.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    format!("{head}{:04}", last + 1)
}

/// The first words of a title, lowercase and joined by dashes, for an entry's file name.
fn slug(title: &str) -> String {
    let words: Vec<String> = title
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .take(6)
        .map(str::to_string)
        .collect();
    if words.is_empty() {
        "entry".into()
    } else {
        words.join("-")
    }
}

/// An entry's file: its front matter (9.3), then its text.
pub fn render(entry: &Entry) -> String {
    let mut lines = vec![
        "---".to_string(),
        format!("id: {}", entry.id),
        format!("type: {}", entry.kind),
        format!("status: {}", entry.status),
    ];
    if !entry.anchors.is_empty() {
        lines.push("anchors:".into());
        lines.extend(entry.anchors.iter().map(|anchor| format!("  - {anchor}")));
    }
    lines.push(format!("supersedes: [{}]", entry.supersedes.join(", ")));
    lines.push(format!("links: [{}]", entry.links.join(", ")));
    lines.push(format!(
        "provenance: {{ task: {}, session: {}, change: {} }}",
        entry.provenance_task, entry.provenance_session, entry.provenance_change
    ));
    lines.push(format!(
        "verified: {{ commit: {}, date: {} }}",
        entry.verified_commit, entry.verified_date
    ));
    lines.push("---".into());
    format!("{}\n{}\n", lines.join("\n"), entry.body)
}

/// The UTC date (`YYYY-MM-DD`) of `ms` since the epoch (Howard Hinnant's civil-from-days).
fn day_of(ms: u64) -> String {
    let z = (ms / 86_400_000) as i64 + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The entry as the memory view shows it.
fn entry_json(entry: &Entry) -> Value {
    json!({
        "id": entry.id,
        "type": entry.kind,
        "status": entry.status,
        "flagged": entry.status == "flagged",
        "title": entry.title,
        "text": entry.body,
        "path": entry.path,
        "anchors": entry.anchors,
        "links": entry.links,
        "supersedes": entry.supersedes,
        "verified": { "commit": entry.verified_commit, "date": entry.verified_date },
        "provenance": {
            "task": entry.provenance_task,
            "session": entry.provenance_session,
            "change": entry.provenance_change,
        },
    })
}

/// `GET /v1/projects/{id}/memory`: the index and every entry (retired and superseded ones too), by type then id.
fn memory_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    let project = p.get("id");
    if app.projects.get(project).is_none() {
        return Response::error(404, "no such project");
    }
    let mut all = entries(app, project);
    all.sort_by(|a, b| (rank(&a.kind), &a.id).cmp(&(rank(&b.kind), &b.id)));
    let list: Vec<Value> = all.iter().map(entry_json).collect();
    Response::json(200, &json!({ "index": index_text(&all), "entries": list }))
}

/// The entry `id` of `project`, when it has one.
fn entry_of(app: &App, project: &str, id: &str) -> Option<Entry> {
    entries(app, project).into_iter().find(|e| e.id == id)
}

/// `PATCH /v1/projects/{id}/memory/{entry}`: the owner's edit, as a change to main (202 with the change). A decision's
/// new text is a new decision that supersedes it (9.6); anything else changes in place.
fn patch_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let project = p.get("id");
    if app.projects.get(project).is_none() {
        return Response::error(404, "no such project");
    }
    let Some(old) = entry_of(app, project, p.get("entry")) else {
        return Response::error(404, "no such entry");
    };
    let patch: Value = match req.json() {
        Ok(patch) => patch,
        Err(e) => return Response::error(400, &format!("bad body: {e}")),
    };
    let edit = match owner_edit(&old, &patch) {
        Ok(edit) => edit,
        Err(why) => return Response::error(400, &why),
    };
    let (write, title) = edit;
    match land(app, project, &title, write, &[]) {
        Ok(landing) => Response::json(202, &json!({ "change": landing.change })),
        Err(why) => Response::error(503, &why),
    }
}

/// `DELETE /v1/projects/{id}/memory/{entry}`: the owner deletes the entry's file, as a change to main (202).
fn delete_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    let project = p.get("id");
    if app.projects.get(project).is_none() {
        return Response::error(404, "no such project");
    }
    let Some(old) = entry_of(app, project, p.get("entry")) else {
        return Response::error(404, "no such entry");
    };
    match land(
        app,
        project,
        &format!("The owner deletes {}", old.id),
        Vec::new(),
        std::slice::from_ref(&old.path),
    ) {
        Ok(landing) => Response::json(202, &json!({ "change": landing.change })),
        Err(why) => Response::error(503, &why),
    }
}

/// The entries an owner's PATCH writes and the change's title: a decision whose text changes is superseded by a new
/// decision (the old one is kept, as superseded); any other entry is edited in place (its text, anchors, or status
/// `active` or `retired`).
fn owner_edit(old: &Entry, patch: &Value) -> Result<(Vec<Entry>, String), String> {
    let text = match patch.get("text") {
        Some(_) => Some(text_of(&arg(patch, "text"))?),
        None => None,
    };
    let anchors = match patch.get("anchors") {
        Some(_) => Some(anchors_from(patch)?),
        None => None,
    };
    let status = match patch.get("status").and_then(Value::as_str) {
        None => None,
        Some(s @ ("active" | "retired")) => Some(s.to_string()),
        Some(_) => return Err("status is active or retired".into()),
    };
    if text.is_none() && anchors.is_none() && status.is_none() {
        return Err("nothing to change: give text, anchors or status".into());
    }
    let title = format!("The owner edits {}", old.id);
    if old.kind == "decision" && text.is_some() {
        let mut new = draft(&old.project, "decision", text.unwrap_or_default());
        new.anchors = anchors.unwrap_or_else(|| old.anchors.clone());
        new.links = old.links.clone();
        new.supersedes = vec![old.id.clone()];
        let mut superseded = old.clone();
        superseded.status = "superseded".into();
        return Ok((vec![new, superseded], title));
    }
    let mut edited = old.clone();
    if let Some(text) = text {
        edited.title = title_of(&text);
        edited.body = text;
    }
    if let Some(anchors) = anchors {
        edited.anchors = anchors;
    }
    if let Some(status) = status {
        edited.status = status;
    }
    Ok((vec![edited], title))
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
        "memory_record_decision" => record_decision(app, agent, &project, input),
        "memory_anchored" => {
            let paths = strings(input, "paths");
            let symbols = strings(input, "symbols");
            if paths.is_empty() && symbols.is_empty() {
                return Err("paths or symbols is required".into());
            }
            let hits: Vec<Value> = anchored(&entries(app, &project), &paths, &symbols)
                .iter()
                .take(HITS)
                .map(|e| {
                    json!({ "id": e.id, "kind": e.kind, "status": e.status, "title": e.title, "text": e.body })
                })
                .collect();
            Ok(json!({ "entries": hits }).to_string())
        }
        other => Err(format!("{other} is not a memory tool")),
    }
}

/// `memory_record_decision` (9.4): the planner records a decision and its reason. It lands at once as a decision entry,
/// and the project's chat is told; a decision the owner made in the chat also opens a memory card, for the owner to keep
/// it or retire it.
fn record_decision(
    app: &App,
    agent: &Agent,
    project: &str,
    input: &Value,
) -> Result<String, String> {
    if agent.role != crate::work_loop::PLANNER_ROLE {
        return Err("only the project's planner records decisions".into());
    }
    let text = text_of(&arg(input, "text"))?;
    let reason = arg(input, "reason");
    if reason.is_empty() {
        return Err("reason is required".into());
    }
    let mut decision = draft(project, "decision", format!("{text}\n\nReason: {reason}"));
    decision.anchors = anchors_from(input)?;
    decision.provenance_session = app
        .sessions
        .by_cid(agent.cid)
        .map(|s| s.id)
        .unwrap_or_default();
    let title = decision.title.clone();
    let landing = land(
        app,
        project,
        &format!("Decision: {title}"),
        vec![decision],
        &[],
    )?;
    let id = landing.ids.first().cloned().unwrap_or_default();
    let now = now_ms();
    let chat = crate::work_loop::project_chat(app, project, now);
    crate::work_loop::notice(app, &chat, &format!("Decision {id} recorded: {title}"), now);
    if input.get("owner").and_then(Value::as_bool) == Some(true) {
        crate::curator::ask_owner(app, project, &id, &title, now);
    }
    Ok(format!(
        "Recorded decision {id}; it joins the project's memory when the change lands. End your turn now."
    ))
}

/// The non-empty strings of `input`'s array `key`, up to `QUERY_MAX` of them.
fn strings(input: &Value, key: &str) -> Vec<String> {
    input
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .take(QUERY_MAX)
        .map(str::to_string)
        .collect()
}

/// The text of an entry: not empty, within `TEXT_MAX`, and with no line that would end its front matter.
pub fn text_of(text: &str) -> Result<String, String> {
    if text.is_empty() {
        return Err("text is required".into());
    }
    if text.chars().count() > TEXT_MAX {
        return Err(format!("text is over {TEXT_MAX} characters"));
    }
    if text.lines().any(|line| line.trim_end() == "---") {
        return Err("text may not have a line of ---".into());
    }
    Ok(text.to_string())
}

/// The anchors `input` names (`anchors`): up to `ANCHORS_MAX`, each one non-empty and within `ANCHOR_MAX`, without
/// repeats.
pub fn anchors_from(input: &Value) -> Result<Vec<String>, String> {
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
    Ok(anchors)
}

/// The routes of the owner's memory view (the IDE's).
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/projects/{id}/memory", memory_route)
        .doc(RouteDoc::new("Read a project's memory: its index and every entry, flagged and retired ones included").errors(&[404]))
        .route("PATCH", "/v1/projects/{id}/memory/{entry}", patch_route)
        .doc(RouteDoc::new("Edit an entry of a project's memory as the owner: a decision's new text supersedes it, other entries change in place; the change lands on main").errors(&[400, 404, 503]))
        .route("DELETE", "/v1/projects/{id}/memory/{entry}", delete_route)
        .doc(RouteDoc::new("Delete an entry of a project's memory as the owner: a change to main").errors(&[404, 503]))
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
    let anchors = anchors_from(input)?;
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

    #[test]
    fn an_entry_renders_to_a_file_that_reads_back_as_the_same_entry() {
        let mut e = draft(
            "pr-1",
            "pitfall",
            "A move must release its claim.\nThe wake waits for it.".into(),
        );
        e.id = "P-0017".into();
        e.key = "pr-1/P-0017".into();
        e.path = ".plaza/memory/pitfalls/P-0017-a-move-must-release.md".into();
        e.anchors = vec![
            "sym:apid::placement::move_claimed".into(),
            "sym:apid::sleep::wake_with".into(),
        ];
        e.links = vec!["D-0042".into()];
        e.provenance_task = "T-311".into();
        e.provenance_session = "s-9042".into();
        e.verified_commit = "976118ce".into();
        e.verified_date = "2026-10-20".into();
        let back = parse("pr-1", &e.path, &render(&e)).expect("the file parses");
        assert_eq!(back, e);
        assert!(!render(&e).contains("supersedes: [D"), "{}", render(&e));
    }

    #[test]
    fn ids_count_on_from_the_highest_of_their_type_and_files_take_the_title_words() {
        let taken = ["D-0042", "C-0007", "P-0017", "D-0007"].map(String::from);
        assert_eq!(next_id("decision", &taken), "D-0043");
        assert_eq!(next_id("fact", &taken), "F-0001");
        assert_eq!(next_id("pitfall", &taken[..2]), "P-0001");
        assert_eq!(
            slug("Move releases its claim first, then wakes."),
            "move-releases-its-claim-first-then"
        );
        assert_eq!(slug("  --  "), "entry");
    }

    #[test]
    fn a_date_is_the_utc_day_of_its_milliseconds() {
        assert_eq!(day_of(0), "1970-01-01");
        assert_eq!(day_of(86_400_000 * 20_384 + 5_000), "2025-10-23");
        assert_eq!(day_of(86_400_000 * 20_384 - 1), "2025-10-22");
    }

    #[test]
    fn an_owner_edit_supersedes_a_decision_and_changes_other_entries_in_place() {
        let mut old = entry_in(
            "pr-1",
            "D-0042",
            "decision",
            "active",
            "Sessions, not agents.",
        );
        old.path = ".plaza/memory/decisions/D-0042-sessions-not-agents.md".into();
        let (write, title) =
            owner_edit(&old, &json!({ "text": "Sessions, not named agents." })).unwrap();
        assert_eq!(title, "The owner edits D-0042");
        assert_eq!(
            write.len(),
            2,
            "the new decision and the old one, superseded"
        );
        assert!(write[0].id.is_empty() && write[0].kind == "decision");
        assert_eq!(write[0].supersedes, ["D-0042"]);
        assert_eq!(write[0].body, "Sessions, not named agents.");
        assert_eq!(
            (write[1].id.as_str(), write[1].status.as_str()),
            ("D-0042", "superseded")
        );

        let mut pitfall = entry_in("pr-1", "P-0002", "pitfall", "active", "Old.");
        pitfall.path = ".plaza/memory/pitfalls/P-0002-old.md".into();
        let (write, _) = owner_edit(&pitfall, &json!({ "status": "retired" })).unwrap();
        assert_eq!(write.len(), 1, "a pitfall changes in place");
        assert_eq!(
            (
                write[0].id.as_str(),
                write[0].status.as_str(),
                write[0].path.as_str()
            ),
            ("P-0002", "retired", pitfall.path.as_str())
        );
        assert!(
            owner_edit(&pitfall, &json!({}))
                .unwrap_err()
                .starts_with("nothing to change")
        );
        assert_eq!(
            owner_edit(&pitfall, &json!({ "status": "flagged" })).unwrap_err(),
            "status is active or retired"
        );
        assert_eq!(
            owner_edit(&pitfall, &json!({ "text": "---" })).unwrap_err(),
            "text may not have a line of ---"
        );
    }

    #[test]
    fn a_landing_takes_the_proposals_it_answers_and_leaves_the_others() {
        let store = Store::default();
        for text in ["one", "two"] {
            let input = json!({ "type": "fact", "text": text, "why": "seen" });
            store.add_proposal(proposal_from("pr-1", 7, "b", &input, 0).unwrap());
        }
        store.add_proposal(
            proposal_from(
                "pr-2",
                7,
                "b",
                &json!({ "type": "fact", "text": "three", "why": "seen" }),
                0,
            )
            .unwrap(),
        );
        assert_eq!(store.proposals_of("pr-1").len(), 2);
        store.take_proposals(&["mp1".to_string()]);
        let left: Vec<String> = store
            .proposals_of("pr-1")
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(left, ["mp2"]);
        assert_eq!(store.proposals_of("pr-2").len(), 1);
    }

    #[test]
    fn an_anchor_is_about_a_path_or_a_symbol_under_it_and_only_live_entries_answer() {
        let mut on_path = entry("D-0001", "decision", "active", "A.");
        on_path.anchors = vec!["crates/apid/src/a.rs::run".into()];
        let mut on_file = entry("P-0002", "pitfall", "flagged", "B.");
        on_file.anchors = vec!["crates/apid/src/a.rs".into()];
        let mut retired = entry("P-0003", "pitfall", "retired", "C.");
        retired.anchors = vec!["crates/apid/src/a.rs".into()];
        let mut elsewhere = entry("P-0004", "pitfall", "active", "D.");
        elsewhere.anchors = vec!["crates/apid/src/ab.rs".into()];
        let mut by_symbol = entry("M-0005", "map", "active", "E.");
        by_symbol.anchors = vec!["crates/x/src/q.rs::f".into()];
        let all = [on_path, on_file, retired, elsewhere, by_symbol];
        let ids = |hits: Vec<Entry>| hits.into_iter().map(|e| e.id).collect::<Vec<_>>();
        assert_eq!(
            ids(anchored(&all, &["crates/apid/src/a.rs".into()], &[])),
            ["D-0001", "P-0002"]
        );
        assert_eq!(
            ids(anchored(&all, &[], &["crates/x/src/q.rs::f".into()])),
            ["M-0005"]
        );
        assert!(anchored(&all, &["crates/apid/src/zzz.rs".into()], &[]).is_empty());
    }

    #[test]
    fn a_landing_flags_only_the_active_entries_its_paths_and_symbols_reach() {
        let mut by_path = entry("P-0001", "pitfall", "active", "A.");
        by_path.anchors = vec!["crates/apid/src/a.rs::run".into()];
        let mut already = entry("P-0002", "pitfall", "flagged", "B.");
        already.anchors = vec!["crates/apid/src/a.rs".into()];
        let mut other = entry("P-0003", "pitfall", "active", "C.");
        other.anchors = vec!["crates/apid/src/b.rs".into()];
        let entries = [by_path, already, other];
        let flagged = to_flag(&entries, &["crates/apid/src/a.rs".into()], &[], "c-7");
        assert_eq!(flagged.len(), 1);
        assert_eq!(
            (
                flagged[0].id.as_str(),
                flagged[0].status.as_str(),
                flagged[0].provenance_change.as_str()
            ),
            ("P-0001", "flagged", "c-7")
        );
        let by_symbol = to_flag(&entries, &[], &["crates/apid/src/a.rs::run".into()], "c-8");
        assert_eq!(by_symbol.len(), 1);
        assert_eq!(by_symbol[0].provenance_change, "c-8");
    }
}
