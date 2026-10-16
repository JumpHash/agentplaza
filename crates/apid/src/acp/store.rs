//! The editors' ACP sessions: which agent each one drives, its work items, and its title. Kept in
//! memory and saved with the plaza state (`db::snapshot_of` component `acp_sessions`), so an editor
//! can list, load and resume its sessions after the hub restarts.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

/// The most sessions kept; the oldest closed ones go first, then the oldest.
pub const KEEP: usize = 500;

/// The longest title, in characters.
const TITLE_CHARS: usize = 80;

/// The random bytes behind a session id: 8 bytes, 16 hex characters.
const ID_BYTES: usize = 8;

/// Tells the fallback ids apart when the OS refuses random bytes.
static FALLBACK: AtomicU64 = AtomicU64::new(0);

/// One editor session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// `acp-<16 hex>`; also the agent's thread for this session.
    pub id: String,
    pub cid: u64,
    /// The editor's project folder, as the editor named it (shown and listed, never opened).
    pub cwd: String,
    /// The editor that opened it (`clientInfo.name` and version), for the console.
    pub client: String,
    /// The first prompt's first line, at most 80 characters.
    pub title: String,
    /// The work items this session queued, oldest first.
    pub works: Vec<u64>,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub closed: bool,
    /// The plaza repository the editor's project is mirrored to, when the editor linked one; the
    /// agent works on a branch of it. Snapshots from before the link existed have none.
    #[serde(default)]
    pub repo: Option<RepoLink>,
    /// Who opened it: `owner`, or `token:<id>` for the editor of a hub MCP token. Only the opener and
    /// the owner may use the session. A snapshot from before this field existed has `""`, which is
    /// the owner's.
    #[serde(default)]
    pub opener: String,
}

/// The plaza repository a session's project is mirrored to (`_meta.agentplaza.repo`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoLink {
    pub name: String,
    /// The editor's branch on the plaza: `editor/<branch>`.
    pub branch: String,
    /// The editor's commit that was pushed; empty when the editor did not say.
    pub commit: String,
    /// The editor had uncommitted changes, which the repository does not have.
    pub dirty: bool,
}

/// All sessions.
#[derive(Default)]
pub struct Sessions {
    inner: Mutex<BTreeMap<String, Session>>,
}

impl Sessions {
    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, Session>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Opens a new session on agent `cid` for `opener` (see [`Session::opener`]).
    pub fn open(&self, cid: u64, cwd: &str, client: &str, opener: &str, now_ms: u64) -> Session {
        let mut inner = self.lock();
        let id = new_id(&inner, now_ms);
        let session = Session {
            id: id.clone(),
            cid,
            cwd: cwd.to_string(),
            client: client.to_string(),
            title: String::new(),
            works: Vec::new(),
            created_ms: now_ms,
            updated_ms: now_ms,
            closed: false,
            repo: None,
            opener: opener.to_string(),
        };
        inner.insert(id.clone(), session.clone());
        trim(&mut inner, &id);
        session
    }

    pub fn get(&self, id: &str) -> Option<Session> {
        self.lock().get(id).cloned()
    }

    /// The sessions, newest first; only those with `cwd` when it is given.
    pub fn list(&self, cwd: Option<&str>) -> Vec<Session> {
        let inner = self.lock();
        let mut out: Vec<Session> = inner
            .values()
            .filter(|s| cwd.is_none_or(|c| s.cwd == c))
            .cloned()
            .collect();
        out.sort_by(|a, b| {
            b.updated_ms
                .cmp(&a.updated_ms)
                .then_with(|| b.id.cmp(&a.id))
        });
        out
    }

    /// Records work item `work` on session `id`; the first prompt's first line becomes the title.
    pub fn add_work(&self, id: &str, work: u64, prompt: &str, now_ms: u64) {
        let mut inner = self.lock();
        let Some(session) = inner.get_mut(id) else {
            return;
        };
        session.works.push(work);
        session.updated_ms = now_ms;
        if session.title.is_empty() {
            session.title = title_of(prompt);
        }
    }

    /// Links session `id` to a repository; the latest link wins. Nothing when there is no such session.
    pub fn set_repo(&self, id: &str, repo: RepoLink) {
        if let Some(session) = self.lock().get_mut(id) {
            session.repo = Some(repo);
        }
    }

    /// Marks session `id` closed; false when there is no such session.
    pub fn close(&self, id: &str, now_ms: u64) -> bool {
        let mut inner = self.lock();
        match inner.get_mut(id) {
            Some(session) => {
                session.closed = true;
                session.updated_ms = now_ms;
                true
            }
            None => false,
        }
    }

    /// The whole state as JSON, for Postgres; "" when there is nothing to save.
    pub fn snapshot(&self) -> String {
        let inner = self.lock();
        if inner.is_empty() {
            return String::new();
        }
        serde_json::to_string(&*inner).unwrap_or_default()
    }

    /// Replaces the state with a [`Sessions::snapshot`]; a refusal changes nothing. An empty
    /// snapshot is no sessions, which is what [`Sessions::snapshot`] writes for them.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let mut map: BTreeMap<String, Session> = if json.is_empty() {
            BTreeMap::new()
        } else {
            serde_json::from_str(json).map_err(|e| format!("bad acp sessions snapshot: {e}"))?
        };
        trim(&mut map, "");
        *self.lock() = map;
        Ok(())
    }
}

/// A fresh `acp-<16 hex>` id that no session has. The OS random bytes are the usual source; the
/// fallback (a counter mixed with the time) only runs when those fail, and it never repeats.
fn new_id(map: &BTreeMap<String, Session>, now_ms: u64) -> String {
    loop {
        let hex = match crate::hub_mcp::auth::random_hex(ID_BYTES) {
            Ok(hex) => hex,
            Err(_) => format!(
                "{:016x}",
                now_ms.rotate_left(17)
                    ^ FALLBACK
                        .fetch_add(1, Ordering::Relaxed)
                        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ),
        };
        let id = format!("acp-{hex}");
        if !map.contains_key(&id) {
            return id;
        }
    }
}

/// Drops the oldest sessions past [`KEEP`]: the closed ones first, then the rest, oldest first.
/// `keep` is never dropped (the session just opened).
fn trim(map: &mut BTreeMap<String, Session>, keep: &str) {
    while map.len() > KEEP {
        let victim = map
            .values()
            .filter(|s| s.id != keep)
            .min_by_key(|s| (!s.closed, s.updated_ms))
            .map(|s| s.id.clone());
        match victim {
            Some(id) => {
                map.remove(&id);
            }
            None => break,
        }
    }
}

/// The first non-empty line of `prompt`, trimmed, at most [`TITLE_CHARS`] characters.
fn title_of(prompt: &str) -> String {
    prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .chars()
        .take(TITLE_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_id(id: &str) -> bool {
        id.strip_prefix("acp-")
            .is_some_and(|hex| hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
    }

    #[test]
    fn open_gives_a_fresh_id_and_no_title() {
        let sessions = Sessions::default();
        let first = sessions.open(7, "/work/a", "zed 0.1", "owner", 1_000);
        let second = sessions.open(7, "/work/a", "zed 0.1", "owner", 1_000);
        assert!(is_id(&first.id), "{}", first.id);
        assert_ne!(first.id, second.id);
        assert_eq!(
            (first.cid, first.created_ms, first.updated_ms),
            (7, 1_000, 1_000)
        );
        assert!(first.title.is_empty() && first.works.is_empty() && !first.closed);
        assert_eq!(sessions.get(&first.id), Some(first));
    }

    #[test]
    fn list_is_newest_first_and_filters_by_cwd() {
        let sessions = Sessions::default();
        let a = sessions.open(1, "/a", "zed", "owner", 10);
        let b = sessions.open(1, "/b", "zed", "owner", 20);
        let c = sessions.open(1, "/a", "zed", "owner", 30);
        sessions.add_work(&a.id, 5, "hi", 40);
        let ids: Vec<String> = sessions.list(None).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, [a.id.clone(), c.id.clone(), b.id.clone()]);
        let in_a: Vec<String> = sessions
            .list(Some("/a"))
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(in_a, [a.id, c.id]);
        assert!(sessions.list(Some("/nope")).is_empty());
    }

    #[test]
    fn the_first_prompt_line_becomes_the_title() {
        let sessions = Sessions::default();
        let s = sessions.open(1, "/a", "zed", "owner", 10);
        sessions.add_work(&s.id, 1, "\n   \n  Fix the parser  \nmore detail", 20);
        sessions.add_work(&s.id, 2, "a second prompt", 30);
        let got = sessions.get(&s.id).unwrap();
        assert_eq!(got.title, "Fix the parser");
        assert_eq!(got.works, [1, 2]);
        assert_eq!(got.updated_ms, 30);
    }

    #[test]
    fn close_marks_a_session_and_refuses_an_unknown_one() {
        let sessions = Sessions::default();
        let s = sessions.open(1, "/a", "zed", "owner", 10);
        assert!(sessions.close(&s.id, 20));
        assert!(sessions.get(&s.id).unwrap().closed);
        assert!(!sessions.close("acp-nope", 20));
    }

    #[test]
    fn past_keep_the_oldest_closed_session_goes_first() {
        let sessions = Sessions::default();
        let old = sessions.open(1, "/a", "zed", "owner", 1);
        sessions.close(&old.id, 2);
        let mut kept = Vec::new();
        for t in 0..KEEP as u64 {
            kept.push(sessions.open(1, "/a", "zed", "owner", 100 + t));
        }
        assert_eq!(sessions.list(None).len(), KEEP);
        assert!(sessions.get(&old.id).is_none());
        assert!(kept.iter().all(|s| sessions.get(&s.id).is_some()));
    }

    #[test]
    fn past_keep_with_none_closed_the_oldest_goes() {
        let sessions = Sessions::default();
        let first = sessions.open(1, "/a", "zed", "owner", 1);
        let mut last = None;
        for t in 0..KEEP as u64 {
            last = Some(sessions.open(1, "/a", "zed", "owner", 10 + t));
        }
        assert_eq!(sessions.list(None).len(), KEEP);
        assert!(sessions.get(&first.id).is_none());
        assert!(sessions.get(&last.unwrap().id).is_some());
    }

    #[test]
    fn snapshot_round_trips_and_an_empty_one_is_empty() {
        assert_eq!(Sessions::default().snapshot(), "");
        let sessions = Sessions::default();
        let s = sessions.open(3, "/a", "zed 1", "token:mcp_a", 10);
        sessions.add_work(&s.id, 9, "hello", 20);
        let other = Sessions::default();
        other.restore(&sessions.snapshot()).unwrap();
        assert_eq!(other.list(None), sessions.list(None));
        other.restore("").unwrap();
        assert!(other.list(None).is_empty());
    }

    #[test]
    fn a_session_names_its_opener_and_an_older_one_is_the_owners() {
        let sessions = Sessions::default();
        let s = sessions.open(1, "/a", "zed", "token:mcp_a", 10);
        assert_eq!(s.opener, "token:mcp_a");
        // A snapshot saved before the field existed restores with no opener, which is the owner's.
        let id = &s.id;
        let old = format!(
            "{{\"{id}\":{{\"id\":\"{id}\",\"cid\":1,\"cwd\":\"/a\",\"client\":\"zed\",\"title\":\"\",\"works\":[],\"created_ms\":10,\"updated_ms\":10,\"closed\":false}}}}"
        );
        let restored = Sessions::default();
        restored.restore(&old).unwrap();
        assert_eq!(restored.get(id).unwrap().opener, "");
    }

    #[test]
    fn a_repo_link_is_recorded_and_an_older_snapshot_has_none() {
        let sessions = Sessions::default();
        let s = sessions.open(1, "/a", "zed", "owner", 10);
        assert_eq!(s.repo, None);
        let link = RepoLink {
            name: "app".into(),
            branch: "editor/main".into(),
            commit: "abc1234".into(),
            dirty: true,
        };
        sessions.set_repo(&s.id, link.clone());
        assert_eq!(sessions.get(&s.id).unwrap().repo, Some(link));
        // A snapshot saved before the field existed restores with no link.
        let id = &s.id;
        let old = format!(
            "{{\"{id}\":{{\"id\":\"{id}\",\"cid\":1,\"cwd\":\"/a\",\"client\":\"zed\",\"title\":\"\",\"works\":[],\"created_ms\":10,\"updated_ms\":10,\"closed\":false}}}}"
        );
        let restored = Sessions::default();
        restored.restore(&old).unwrap();
        assert_eq!(restored.get(id).unwrap().repo, None);
    }

    #[test]
    fn a_bad_snapshot_changes_nothing() {
        let sessions = Sessions::default();
        let s = sessions.open(1, "/a", "zed", "owner", 10);
        assert!(sessions.restore("{not json").is_err());
        assert_eq!(sessions.get(&s.id), Some(s));
    }
}
