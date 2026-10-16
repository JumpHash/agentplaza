//! Each agent's private memory library: markdown notes that link to each other
//! with `[[Title]]`, one file per note under `<state-dir>/memory/<cid>/<slug>.md`.
//!
//! `write`, `search`, `journal` and `index` are the agents' tool surface; the
//! tools that call them are wired in a later change, so the binary does not
//! reach them yet.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use apz_http::{Response, RouteDoc, Router};
use serde::Serialize;
use serde_json::json;

use crate::state::App;

/// Notes per agent.
const NOTES_MAX: usize = 512;
/// Largest body per note.
const BODY_MAX: usize = 16 * 1024;
const TITLE_MAX: usize = 120;
const SLUG_MAX: usize = 60;
const KINDS: [&str; 3] = ["note", "journal", "person"];

/// Serializes writes, so a journal line is never lost to a concurrent write.
static LOCK: Mutex<()> = Mutex::new(());

/// A note without its body: what lists and link graphs are built from.
#[derive(Debug, Clone, Serialize)]
pub struct NoteMeta {
    pub slug: String,
    pub title: String,
    pub kind: String,
    pub links: Vec<String>,
    pub tags: Vec<String>,
    pub updated_ms: u64,
    /// Body size in bytes.
    pub size: usize,
}

/// The task and work item an agent wrote a note during, when there was one.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Provenance {
    pub task: Option<String>,
    pub work: Option<u64>,
}

/// A note with its body, the notes that link to it and its provenance.
#[derive(Debug, Clone, Serialize)]
pub struct NoteDoc {
    #[serde(flatten)]
    pub meta: NoteMeta,
    pub body: String,
    pub backlinks: Vec<String>,
    pub provenance: Provenance,
}

/// One search result.
#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub slug: String,
    pub title: String,
    pub snippet: String,
}

/// One note as read from disk.
struct Stored {
    meta: NoteMeta,
    body: String,
    provenance: Provenance,
}

/// The memory libraries of all agents, one directory per agent.
pub struct Memory {
    root: PathBuf,
    /// Told about every note written (apid publishes it as a `memory` event).
    listener: std::sync::OnceLock<Box<dyn Fn(u64, &NoteMeta) + Send + Sync>>,
}

impl Memory {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            listener: std::sync::OnceLock::new(),
        }
    }

    /// Sets the function told about every note written (once).
    pub fn set_listener(&self, listener: impl Fn(u64, &NoteMeta) + Send + Sync + 'static) {
        let _ = self.listener.set(Box::new(listener));
    }

    /// The notes of `cid`, newest first.
    pub fn list(&self, cid: u64) -> Vec<NoteMeta> {
        self.load_all(cid).into_iter().map(|s| s.meta).collect()
    }

    /// One note, with its backlinks and provenance.
    pub fn get(&self, cid: u64, slug: &str) -> Option<NoteDoc> {
        if !valid_slug(slug) {
            return None;
        }
        let stored = read_note(&self.path(cid, slug), slug)?;
        let backlinks: Vec<String> = self
            .load_all(cid)
            .into_iter()
            .filter(|s| s.meta.slug != slug && s.meta.links.iter().any(|l| l == slug))
            .map(|s| s.meta.slug)
            .collect();
        Some(NoteDoc {
            meta: stored.meta,
            body: stored.body,
            backlinks,
            provenance: stored.provenance,
        })
    }

    /// Creates or replaces the note whose slug is the slug of `title`.
    #[allow(clippy::too_many_arguments)]
    pub fn write(
        &self,
        cid: u64,
        title: &str,
        body: &str,
        kind: &str,
        tags: &[String],
        task: Option<String>,
        work: Option<u64>,
        now_ms: u64,
    ) -> Result<NoteMeta, String> {
        let _guard = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        self.store(cid, title, body, kind, tags, task, work, now_ms)
    }

    /// Appends `line` to today's journal note, `Journal YYYY-MM-DD` (UTC).
    pub fn journal(&self, cid: u64, line: &str, now_ms: u64) -> Result<NoteMeta, String> {
        let _guard = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let title = format!("Journal {}", utc_date(now_ms));
        let slug = slug(&title);
        let mut body = read_note(&self.path(cid, &slug), &slug)
            .map(|s| s.body)
            .unwrap_or_default();
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str(line.trim());
        body.push('\n');
        self.store(cid, &title, &body, "journal", &[], None, None, now_ms)
    }

    /// Notes scored by how often each term appears: a title hit counts three
    /// times a body hit. Case-insensitive; the snippet is about 160 characters
    /// around the first body hit.
    pub fn search(&self, cid: u64, query: &str, limit: usize) -> Vec<Hit> {
        let terms: Vec<Vec<char>> = query
            .split_whitespace()
            .map(|w| w.chars().map(fold).collect::<Vec<char>>())
            .collect();
        if terms.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(usize, Hit)> = Vec::new();
        for note in self.load_all(cid) {
            let title: Vec<char> = note.meta.title.chars().map(fold).collect();
            let body_chars: Vec<char> = note.body.chars().collect();
            let body: Vec<char> = body_chars.iter().map(|&c| fold(c)).collect();
            let mut score = 0;
            let mut first_hit: Option<usize> = None;
            for term in &terms {
                score += 3 * occurrences(&title, term).len();
                let hits = occurrences(&body, term);
                score += hits.len();
                if let Some(&at) = hits.first() {
                    first_hit = Some(first_hit.map_or(at, |f| f.min(at)));
                }
            }
            if score == 0 {
                continue;
            }
            let snippet = snippet(&body_chars, first_hit.unwrap_or(0));
            scored.push((
                score,
                Hit {
                    slug: note.meta.slug,
                    title: note.meta.title,
                    snippet,
                },
            ));
        }
        // A stable sort keeps the newest-first order among equal scores.
        scored.sort_by_key(|a| std::cmp::Reverse(a.0));
        scored.into_iter().take(limit).map(|(_, hit)| hit).collect()
    }

    /// The newest `limit` titles as `- Title (slug)` lines, for prompts.
    pub fn index(&self, cid: u64, limit: usize) -> String {
        self.load_all(cid)
            .iter()
            .take(limit)
            .map(|n| format!("- {} ({})\n", n.meta.title, n.meta.slug))
            .collect()
    }

    /// Writes a note file and tells the listener; the caller holds `LOCK`.
    #[allow(clippy::too_many_arguments)]
    fn store(
        &self,
        cid: u64,
        title: &str,
        body: &str,
        kind: &str,
        tags: &[String],
        task: Option<String>,
        work: Option<u64>,
        now_ms: u64,
    ) -> Result<NoteMeta, String> {
        let meta = self.store_file(cid, title, body, kind, tags, task, work, now_ms)?;
        if let Some(listener) = self.listener.get() {
            listener(cid, &meta);
        }
        Ok(meta)
    }

    #[allow(clippy::too_many_arguments)]
    fn store_file(
        &self,
        cid: u64,
        title: &str,
        body: &str,
        kind: &str,
        tags: &[String],
        task: Option<String>,
        work: Option<u64>,
        now_ms: u64,
    ) -> Result<NoteMeta, String> {
        let title = title.replace(['\r', '\n'], " ").trim().to_string();
        if title.chars().count() > TITLE_MAX {
            return Err(format!("title is longer than {TITLE_MAX} characters"));
        }
        if body.len() > BODY_MAX {
            return Err(format!("body is larger than {} KiB", BODY_MAX / 1024));
        }
        if !KINDS.contains(&kind) {
            return Err("kind must be note, journal or person".into());
        }
        let tags: Vec<String> = tags
            .iter()
            .map(|t| t.replace([',', '\r', '\n'], " ").trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        let task = task
            .map(|t| t.replace(['\r', '\n'], " ").trim().to_string())
            .filter(|t| !t.is_empty());
        let slug = slug(&title);
        let dir = self.dir(cid);
        let path = dir.join(format!("{slug}.md"));
        if !path.exists() && self.note_files(cid).len() >= NOTES_MAX {
            return Err(format!("memory is full: {NOTES_MAX} notes"));
        }

        let mut text = String::from("---\n");
        text.push_str(&format!("title: {title}\nkind: {kind}\n"));
        if !tags.is_empty() {
            text.push_str(&format!("tags: {}\n", tags.join(", ")));
        }
        if let Some(task) = &task {
            text.push_str(&format!("task: {task}\n"));
        }
        if let Some(work) = work {
            text.push_str(&format!("work: {work}\n"));
        }
        text.push_str(&format!("updated_ms: {now_ms}\n---\n"));
        text.push_str(body);

        std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        let tmp = dir.join(format!(".{slug}.tmp"));
        std::fs::write(&tmp, text).map_err(|e| format!("write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("rename {}: {e}", path.display()))?;
        Ok(NoteMeta {
            slug,
            title,
            kind: kind.to_string(),
            links: links(body),
            tags,
            updated_ms: now_ms,
            size: body.len(),
        })
    }

    /// Deletes every note of `cid`, its whole memory library.
    pub fn wipe(&self, cid: u64) {
        if let Err(err) = std::fs::remove_dir_all(self.dir(cid))
            && err.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("apid: memory wipe {cid}: {err}");
        }
    }

    fn dir(&self, cid: u64) -> PathBuf {
        self.root.join(cid.to_string())
    }

    fn path(&self, cid: u64, slug: &str) -> PathBuf {
        self.dir(cid).join(format!("{slug}.md"))
    }

    /// The note files of `cid`, in no particular order.
    fn note_files(&self, cid: u64) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(self.dir(cid)) else {
            return Vec::new();
        };
        entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
            .collect()
    }

    /// Every note of `cid`, newest first (ties by slug).
    fn load_all(&self, cid: u64) -> Vec<Stored> {
        let mut notes: Vec<Stored> = self
            .note_files(cid)
            .iter()
            .filter_map(|path| {
                let slug = path.file_stem()?.to_str()?;
                read_note(path, slug)
            })
            .collect();
        notes.sort_by(|a, b| {
            b.meta
                .updated_ms
                .cmp(&a.meta.updated_ms)
                .then_with(|| a.meta.slug.cmp(&b.meta.slug))
        });
        notes
    }
}

/// The note's key: its lowercase title with each run of characters outside
/// `[a-z0-9]` as one `-`, trimmed, at most 60 characters, `untitled` if empty.
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in title.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if gap && !out.is_empty() {
                out.push('-');
            }
            out.push(c);
            gap = false;
        } else {
            gap = true;
        }
    }
    let mut out: String = out.chars().take(SLUG_MAX).collect();
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "untitled".into()
    } else {
        out
    }
}

/// A slug as it appears in a file name and a URL: never a path.
fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= SLUG_MAX
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The slugs of every `[[...]]` in `body`, in order, without repeats.
fn links(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else { break };
        let inner = &after[..end];
        if !inner.contains('\n') && !inner.trim().is_empty() {
            let target = slug(inner);
            if !out.contains(&target) {
                out.push(target);
            }
        }
        rest = &after[end + 2..];
    }
    out
}

/// Reads a note file: `---` front matter, then the body.
fn read_note(path: &Path, slug: &str) -> Option<Stored> {
    let text = std::fs::read_to_string(path).ok()?;
    let (header, body) = split(&text)?;
    let fields = Fields::parse(header);
    Some(Stored {
        meta: NoteMeta {
            slug: slug.to_string(),
            title: fields.title,
            kind: fields.kind,
            links: links(body),
            tags: fields.tags,
            updated_ms: fields.updated_ms,
            size: body.len(),
        },
        body: body.to_string(),
        provenance: Provenance {
            task: fields.task,
            work: fields.work,
        },
    })
}

/// The header lines and the body of a note file.
fn split(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    let after = &rest[end + 4..];
    let body = after.strip_prefix('\n').unwrap_or("");
    Some((&rest[..end], body))
}

struct Fields {
    title: String,
    kind: String,
    tags: Vec<String>,
    task: Option<String>,
    work: Option<u64>,
    updated_ms: u64,
}

impl Fields {
    fn parse(header: &str) -> Self {
        let mut f = Fields {
            title: String::new(),
            kind: "note".into(),
            tags: Vec::new(),
            task: None,
            work: None,
            updated_ms: 0,
        };
        for line in header.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "title" => f.title = value.to_string(),
                "kind" => f.kind = value.to_string(),
                "tags" => {
                    f.tags = value
                        .split(',')
                        .map(str::trim)
                        .filter(|t| !t.is_empty())
                        .map(String::from)
                        .collect()
                }
                "task" => f.task = (!value.is_empty()).then(|| value.to_string()),
                "work" => f.work = value.parse().ok(),
                "updated_ms" => f.updated_ms = value.parse().unwrap_or(0),
                _ => {}
            }
        }
        f
    }
}

/// The lowercase of `c` when that is one character, else `c`, so character
/// positions in the body stay aligned with the original text.
fn fold(c: char) -> char {
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(l), None) => l,
        _ => c,
    }
}

/// Start positions of `needle` in `hay`, overlaps included.
fn occurrences(hay: &[char], needle: &[char]) -> Vec<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return Vec::new();
    }
    (0..=hay.len() - needle.len())
        .filter(|&i| hay[i..].starts_with(needle))
        .collect()
}

/// About 160 characters of `body` around `at`, with `…` where cut.
fn snippet(body: &[char], at: usize) -> String {
    let start = at.saturating_sub(60);
    let end = (start + 160).min(body.len());
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(body[start..end].iter());
    if end < body.len() {
        out.push('…');
    }
    out.replace('\n', " ")
}

/// `YYYY-MM-DD` of a Unix time in milliseconds, in UTC.
fn utc_date(now_ms: u64) -> String {
    // Howard Hinnant's civil-from-days, shifted to the 0000-03-01 epoch.
    let z = (now_ms / 86_400_000) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `GET /v1/agents/{cid}/memory` (`{notes}`, newest first) and
/// `GET /v1/agents/{cid}/memory/{slug}` (one note with its body).
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/agents/{cid}/memory", |app, _, p| {
            match p.get("cid").parse::<u64>() {
                Ok(cid) => Response::json(200, &json!({"notes": app.memory.list(cid)})),
                Err(_) => Response::error(400, "bad cid"),
            }
        })
        .doc(RouteDoc::new("List an agent's memory notes"))
        .get("/v1/agents/{cid}/memory/{slug}", |app, _, p| {
            match p
                .get("cid")
                .parse::<u64>()
                .ok()
                .and_then(|cid| app.memory.get(cid, p.get("slug")))
            {
                Some(doc) => Response::json(200, &doc),
                None => Response::error(404, "not found"),
            }
        })
        .doc(RouteDoc::new("Get one of an agent's memory notes").errors(&[404]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apid-memory-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn write_and_get_round_trip_with_links_and_backlinks() {
        let root = scratch("roundtrip");
        let memory = Memory::new(root.clone());
        let body = "See [[Beta Note]], then [[beta note]] and [[Gamma]].\n";
        let tags = vec!["x".to_string(), "y".to_string()];
        let alpha = memory
            .write(
                7,
                "Alpha Note",
                body,
                "note",
                &tags,
                Some("t12".into()),
                Some(34),
                1000,
            )
            .unwrap();
        assert_eq!(alpha.slug, "alpha-note");
        assert_eq!(alpha.links, vec!["beta-note", "gamma"]);
        memory
            .write(
                7,
                "Beta Note",
                "Back to [[Alpha Note]].",
                "note",
                &[],
                None,
                None,
                2000,
            )
            .unwrap();

        let doc = memory.get(7, "alpha-note").unwrap();
        assert_eq!(doc.body, body);
        assert_eq!(doc.meta.tags, vec!["x", "y"]);
        assert_eq!(doc.backlinks, vec!["beta-note"]);
        assert_eq!(doc.provenance.task.as_deref(), Some("t12"));
        assert_eq!(doc.provenance.work, Some(34));
        let beta = memory.get(7, "beta-note").unwrap();
        assert_eq!(beta.backlinks, vec!["alpha-note"]);
        assert_eq!(beta.provenance.task, None);

        let listed = memory.list(7);
        let slugs: Vec<&str> = listed.iter().map(|n| n.slug.as_str()).collect::<Vec<_>>();
        assert_eq!(slugs, vec!["beta-note", "alpha-note"], "newest first");
        assert!(memory.get(7, "missing").is_none());
        assert!(memory.get(7, "../escape").is_none());
        assert!(
            memory.get(8, "alpha-note").is_none(),
            "each agent has its own library"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn slug_rules() {
        assert_eq!(slug("  Hello, World!! "), "hello-world");
        assert_eq!(slug("a--b  c"), "a-b-c");
        assert_eq!(slug("!!!"), "untitled");
        assert_eq!(slug(""), "untitled");
        assert_eq!(slug(&"A".repeat(100)).len(), 60);
        assert_eq!(
            slug(&format!("{} b", "a".repeat(59))),
            "a".repeat(59),
            "a dash left by the cut is trimmed"
        );
    }

    #[test]
    fn search_ranks_a_title_hit_first() {
        let root = scratch("search");
        let memory = Memory::new(root.clone());
        memory
            .write(1, "Notes", "rust rust and more", "note", &[], None, None, 1)
            .unwrap();
        memory
            .write(
                1,
                "Rust",
                "nothing relevant here",
                "note",
                &[],
                None,
                None,
                2,
            )
            .unwrap();
        let hits = memory.search(1, "RUST", 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].slug, "rust");
        assert!(hits[1].snippet.contains("rust"));
        assert!(memory.search(1, "absent", 10).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn journal_appends_to_one_note_per_day() {
        let root = scratch("journal");
        let memory = Memory::new(root.clone());
        let day = 1_760_000_000_000; // 2025-10-09 UTC
        assert_eq!(utc_date(day), "2025-10-09");
        assert_eq!(utc_date(0), "1970-01-01");
        let first = memory.journal(1, "started the import", day).unwrap();
        assert_eq!(first.slug, "journal-2025-10-09");
        memory.journal(1, "finished it", day + 3_600_000).unwrap();
        let doc = memory.get(1, "journal-2025-10-09").unwrap();
        assert_eq!(doc.body, "started the import\nfinished it\n");
        assert_eq!(doc.meta.kind, "journal");
        assert_eq!(doc.meta.title, "Journal 2025-10-09");
        memory.journal(1, "next day", day + 86_400_000).unwrap();
        assert_eq!(memory.list(1).len(), 2);
        assert_eq!(
            memory.index(1, 1),
            "- Journal 2025-10-10 (journal-2025-10-10)\n"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn caps_refuse_oversize_bodies_and_titles() {
        let root = scratch("caps");
        let memory = Memory::new(root.clone());
        assert!(
            memory
                .write(1, "Big", &"x".repeat(BODY_MAX), "note", &[], None, None, 1)
                .is_ok()
        );
        assert!(
            memory
                .write(
                    1,
                    "Bigger",
                    &"x".repeat(BODY_MAX + 1),
                    "note",
                    &[],
                    None,
                    None,
                    1
                )
                .is_err()
        );
        assert!(
            memory
                .write(
                    1,
                    &"t".repeat(TITLE_MAX + 1),
                    "",
                    "note",
                    &[],
                    None,
                    None,
                    1
                )
                .is_err()
        );
        assert!(
            memory
                .write(1, "Odd", "", "gossip", &[], None, None, 1)
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
