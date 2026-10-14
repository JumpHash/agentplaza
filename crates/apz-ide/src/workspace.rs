//! Workspace identities: `agent:<cid>` for an agent's `/workspace`, `repo:<name>@<ref>` for a project's
//! Forgejo repo at one branch, and `station:<id>` for a workstation's work folder (`<id>` is `w<seq>`).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// What a workspace is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Agent,
    Repo,
    Station,
}

/// The key of the IDE state. Its text form is the wire form.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WorkspaceId {
    /// The `/workspace` of agent `cid`.
    Agent(u64),
    /// Project repo `name` at branch or ref `git_ref` (which may contain `/`).
    Repo { name: String, git_ref: String },
    /// The work folder of workstation `id` (`w<seq>`).
    Station(String),
}

/// Why a workspace id did not parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdError {
    /// Neither `agent:` nor `repo:`.
    Prefix,
    /// An agent cid that is not a canonical decimal number.
    Cid,
    /// A repo name outside `[A-Za-z0-9._-]+`, or `.` and `..`.
    Name,
    /// A missing `@ref`, or a ref git would refuse.
    Ref,
    /// A station id that is not `w` and a canonical decimal number.
    Station,
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            IdError::Prefix => "workspace id must start with agent:, repo: or station:",
            IdError::Cid => "agent cid must be a decimal number without leading zeros",
            IdError::Name => "repo name must be [A-Za-z0-9._-]+ and not . or ..",
            IdError::Ref => "repo id needs @<ref>, and the ref must be a valid git ref",
            IdError::Station => "station id must be w followed by a decimal number without leading zeros",
        })
    }
}

impl std::error::Error for IdError {}

impl FromStr for WorkspaceId {
    type Err = IdError;

    /// Parses the canonical form only: `format(parse(s)) == s` for every accepted `s`.
    fn from_str(s: &str) -> Result<Self, IdError> {
        if let Some(cid) = s.strip_prefix("agent:") {
            let canonical = cid == "0" || (!cid.is_empty() && !cid.starts_with('0'));
            if !canonical || !cid.bytes().all(|b| b.is_ascii_digit()) {
                return Err(IdError::Cid);
            }
            return cid
                .parse()
                .map(WorkspaceId::Agent)
                .map_err(|_| IdError::Cid);
        }
        if let Some(rest) = s.strip_prefix("repo:") {
            // Names never hold '@', so the first one splits name from ref.
            let (name, git_ref) = rest.split_once('@').ok_or(IdError::Ref)?;
            check_name(name)?;
            check_ref(git_ref)?;
            return Ok(WorkspaceId::Repo {
                name: name.to_string(),
                git_ref: git_ref.to_string(),
            });
        }
        if let Some(id) = s.strip_prefix("station:") {
            let digits = id.strip_prefix('w').unwrap_or_default();
            let canonical = digits == "0" || (!digits.is_empty() && !digits.starts_with('0'));
            if !canonical || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err(IdError::Station);
            }
            return Ok(WorkspaceId::Station(id.to_string()));
        }
        Err(IdError::Prefix)
    }
}

impl fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkspaceId::Agent(cid) => write!(f, "agent:{cid}"),
            WorkspaceId::Repo { name, git_ref } => write!(f, "repo:{name}@{git_ref}"),
            WorkspaceId::Station(id) => write!(f, "station:{id}"),
        }
    }
}

impl Serialize for WorkspaceId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for WorkspaceId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

fn check_name(name: &str) -> Result<(), IdError> {
    let ok = !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok { Ok(()) } else { Err(IdError::Name) }
}

/// A conservative subset of `git check-ref-format`: enough that a ref cannot
/// escape a URL path or smuggle a control character into a log line.
fn check_ref(r: &str) -> Result<(), IdError> {
    let bad_char = |c: char| c.is_control() || c == ' ' || "~^:?*[\\".contains(c);
    let ok = !r.is_empty()
        && !r.chars().any(bad_char)
        && !r.contains("..")
        && !r.contains("//")
        && !r.contains("@{")
        && !r.starts_with('/')
        && !r.ends_with('/')
        && !r.ends_with('.');
    if ok { Ok(()) } else { Err(IdError::Ref) }
}

/// A workspace the IDE opens. `kind`, `owner` and `ref` follow from `id`; use [`Workspace::new`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Workspace {
    pub id: WorkspaceId,
    pub kind: Kind,
    /// The agent's cid, the repo name, or the station id.
    pub owner: String,
    /// The branch of a repo workspace; `None` for an agent.
    pub r#ref: Option<String>,
    /// Where the files live, as the hub names it (`/workspace`, or the repo's path).
    pub root: String,
    pub writable: bool,
}

impl Workspace {
    /// A workspace for `id`, with `kind`, `owner` and `ref` taken from it.
    pub fn new(id: WorkspaceId, root: impl Into<String>, writable: bool) -> Self {
        let (kind, owner, r#ref) = match &id {
            WorkspaceId::Agent(cid) => (Kind::Agent, cid.to_string(), None),
            WorkspaceId::Repo { name, git_ref } => {
                (Kind::Repo, name.clone(), Some(git_ref.clone()))
            }
            WorkspaceId::Station(id) => (Kind::Station, id.clone(), None),
        };
        Workspace {
            id,
            kind,
            owner,
            r#ref,
            root: root.into(),
            writable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_id_round_trips() {
        let id: WorkspaceId = "agent:42".parse().unwrap();
        assert_eq!(id, WorkspaceId::Agent(42));
        assert_eq!(id.to_string(), "agent:42");
        assert_eq!(
            "agent:0".parse::<WorkspaceId>().unwrap(),
            WorkspaceId::Agent(0)
        );
    }

    #[test]
    fn repo_id_keeps_slashes_in_the_ref() {
        let id: WorkspaceId = "repo:plaza-web@feature/ide-spike".parse().unwrap();
        assert_eq!(
            id,
            WorkspaceId::Repo {
                name: "plaza-web".into(),
                git_ref: "feature/ide-spike".into()
            }
        );
        assert_eq!(id.to_string(), "repo:plaza-web@feature/ide-spike");
    }

    #[test]
    fn repo_id_splits_at_the_first_at_sign() {
        let id: WorkspaceId = "repo:x@a@b".parse().unwrap();
        assert_eq!(
            id,
            WorkspaceId::Repo {
                name: "x".into(),
                git_ref: "a@b".into()
            }
        );
    }

    #[test]
    fn accepted_ids_format_back_to_the_same_text() {
        for s in [
            "agent:7",
            "agent:18446744073709551615",
            "repo:a.b_c-d@main",
            "repo:x@v1.2.3",
        ] {
            assert_eq!(s.parse::<WorkspaceId>().unwrap().to_string(), s);
        }
    }

    #[test]
    fn agent_cid_must_be_canonical_decimal() {
        for s in [
            "agent:",
            "agent:x",
            "agent:+4",
            "agent:-1",
            "agent:007",
            "agent: 4",
            "agent:1 ",
            "agent:99999999999999999999999",
        ] {
            assert_eq!(s.parse::<WorkspaceId>(), Err(IdError::Cid), "{s}");
        }
    }

    #[test]
    fn repo_name_and_ref_are_checked() {
        let cases = [
            ("repo:@main", IdError::Name),
            ("repo:bad name@main", IdError::Name),
            ("repo:..@main", IdError::Name),
            ("repo:.@main", IdError::Name),
            ("repo:x/y@main", IdError::Name),
            ("repo:x", IdError::Ref),
            ("repo:x@", IdError::Ref),
            ("repo:x@a..b", IdError::Ref),
            ("repo:x@/a", IdError::Ref),
            ("repo:x@a/", IdError::Ref),
            ("repo:x@a b", IdError::Ref),
            ("repo:x@a//b", IdError::Ref),
            ("repo:x@a~1", IdError::Ref),
            ("repo:x@a\nb", IdError::Ref),
            ("repo:x@a.", IdError::Ref),
        ];
        for (s, want) in cases {
            assert_eq!(s.parse::<WorkspaceId>(), Err(want), "{s}");
        }
    }

    #[test]
    fn station_id_round_trips_and_must_be_w_then_a_canonical_number() {
        let id: WorkspaceId = "station:w12".parse().unwrap();
        assert_eq!(id, WorkspaceId::Station("w12".into()));
        assert_eq!(id.to_string(), "station:w12");
        assert_eq!("station:w0".parse::<WorkspaceId>().unwrap().to_string(), "station:w0");
        for s in ["station:", "station:w", "station:12", "station:w01", "station:wx", "station:w1/..", "station:w 1"] {
            assert_eq!(s.parse::<WorkspaceId>(), Err(IdError::Station), "{s}");
        }
        let ws = Workspace::new(id, "~/AgentPlaza", true);
        assert_eq!(ws.kind, Kind::Station);
        assert_eq!(ws.owner, "w12");
        assert_eq!(ws.r#ref, None);
        assert_eq!(serde_json::to_value(&ws).unwrap()["kind"], "station");
    }

    #[test]
    fn unknown_prefix_is_refused() {
        for s in ["", "project:1", "Agent:1", "agent 1"] {
            assert_eq!(s.parse::<WorkspaceId>(), Err(IdError::Prefix), "{s}");
        }
    }

    #[test]
    fn id_serializes_as_its_text() {
        let id = WorkspaceId::Agent(7);
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"agent:7\"");
        let back: WorkspaceId = serde_json::from_str("\"repo:a@main\"").unwrap();
        assert_eq!(back.to_string(), "repo:a@main");
        assert!(serde_json::from_str::<WorkspaceId>("\"nope\"").is_err());
    }

    #[test]
    fn workspace_fields_follow_the_id() {
        let agent = Workspace::new(WorkspaceId::Agent(7), "/workspace", true);
        assert_eq!(agent.kind, Kind::Agent);
        assert_eq!(agent.owner, "7");
        assert_eq!(agent.r#ref, None);

        let repo: WorkspaceId = "repo:plaza-web@main".parse().unwrap();
        let repo = Workspace::new(repo, "repos/plaza-web", false);
        assert_eq!(repo.kind, Kind::Repo);
        assert_eq!(repo.owner, "plaza-web");
        assert_eq!(repo.r#ref.as_deref(), Some("main"));
    }

    #[test]
    fn workspace_serializes_snake_case() {
        let ws = Workspace::new(WorkspaceId::Agent(7), "/workspace", true);
        let v = serde_json::to_value(&ws).unwrap();
        assert_eq!(v["id"], "agent:7");
        assert_eq!(v["kind"], "agent");
        assert_eq!(v["owner"], "7");
        assert!(v["ref"].is_null());
        assert_eq!(v["root"], "/workspace");
        assert_eq!(v["writable"], true);
        let back: Workspace = serde_json::from_value(v).unwrap();
        assert_eq!(back, ws);
    }
}
