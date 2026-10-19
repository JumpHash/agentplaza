//! The rich-content classifier (docs/design/workspace.md, section 3): which references in agent text become
//! chips and hover cards, and what each one is. Pure: no I/O, no clock, no network. The web's TypeScript port
//! and the desktop and CLI run the same vectors (`tests/vectors.json`), so every app recognises the same things.
//!
//! # Output
//! [`classify`] splits a text into spans. Concatenating every span's raw text gives the input back:
//! - `Span::Text(s)`: plain text.
//! - `Span::Code(s)`: an inline code span with its backticks (`` `x` ``). Never rewritten.
//! - `Span::Ref { raw, r, text }`: a reference. `raw` is the matched text: for `[label](url)` and `![alt](url)`
//!   the whole markdown. `text` is the label of a markdown link or image, else `None`.
//!
//! # JSON (the shape the vectors and the TypeScript port use)
//! - Span: `{"kind":"text","value":"hi"}`, `{"kind":"code","value":"`x`"}`,
//!   `{"kind":"ref","value":{"raw":"#528","r":{"kind":"task","number":528},"text":null}}`.
//! - Ref: `{"kind":"<snake_case kind>", ...fields}`, every field always present (`null` when absent). One example
//!   per kind is in the table on [`Ref`].
//! - Ctx: `{"project":null,"paths":[],"agents":[],"hub_host":null}`, every field optional. `paths` lists the
//!   workspace's files, and folders with a trailing `/`.
//! - A vector: `{"name","text","ctx","spans"}`; `tests/vectors.json` is an array of them.
//!
//! # Rules that are not obvious
//! - Bare URLs are `https?://` followed by non-space characters, with trailing `.,;:!?'"*_)]` removed (the web
//!   renderer's rule, `apps/web/src/ui/markdown.ts`). Scheme and host compare case-insensitively; paths do not.
//!   `www.` without a scheme is not a link.
//! - `#N` is a task only when `ctx.project` is set. `task #N` is always a task. `PR #N` and `pull #N` are text,
//!   and their `#N` is not a task.
//! - `@Name` is an agent only when `Name` is in `ctx.agents` (exact, case-sensitive); `@all` is `mention_all`.
//! - A path is recognised only when it is in `ctx.paths`: a file as given, a folder with or without its `/`.
//!   `path:12` and `path#L12` give the line.
//! - GitHub: `github.com` and `www.github.com`. The branch in `blob/<ref>/...` is the first segment after `blob`:
//!   a branch name with a `/` is read as the ref. The fragment `#L10` or `#L10-L20` gives the line.
//! - Hub library links: `/v1/files/<id>` (a deliverable) and `/v1/library/<name>/<version>` (a library tool),
//!   relative or on `ctx.hub_host`.
//! - Artifacts: `artifact:<id>` or `artifact:<id>@<n>` in text and in markdown link targets, where the id is 8 to 40
//!   characters of `[a-z0-9-]`. Hub links `https://<hub host>/artifact/<id>` and `https://agentplaza.io/artifact/<id>`
//!   are artifacts too, with no version. Trailing punctuation is not part of the id.

mod classify;
mod url;

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

pub use classify::classify;
pub use url::{classify_url, host_of};

/// What the classifier may match against: the project, the workspace's paths, the agent names and the hosts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ctx {
    /// The project in view (its id, such as `pr12`). Presence makes `#N` a task.
    pub project: Option<String>,
    /// The workspace's known paths: files, and folders with a trailing `/`.
    pub paths: BTreeSet<String>,
    /// The agents that `@Name` may name.
    pub agents: Vec<AgentName>,
    /// The hub's own host. Its `/v1/files` and `/v1/library` links are library references.
    pub hub_host: Option<String>,
}

/// An agent `@Name` can name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentName {
    pub name: String,
    pub cid: u64,
}

/// One piece of a classified text. See the crate docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Span {
    Text(String),
    Code(String),
    Ref {
        /// The matched text, exactly as it appears in the input.
        raw: String,
        r: Ref,
        /// The label of a markdown link or image (`[label](url)`), else None.
        text: Option<String>,
    },
}

/// A recognised reference. `kind` is the snake_case name; every field is always present in the JSON.
///
/// | Kind | Example | Fields |
/// |---|---|---|
/// | `github_pull` | `{"kind":"github_pull","owner":"o","repo":"r","number":12}` | owner, repo, number |
/// | `github_issue` | `{"kind":"github_issue","owner":"o","repo":"r","number":7}` | owner, repo, number |
/// | `github_commit` | `{"kind":"github_commit","owner":"o","repo":"r","sha":"abc1234..."}` | owner, repo, sha |
/// | `github_repo` | `{"kind":"github_repo","owner":"o","repo":"r"}` | owner, repo |
/// | `github_file` | `{"kind":"github_file","owner":"o","repo":"r","ref":"main","path":"a.rs","line":10,"dir":false}` | owner, repo, ref, path, line, dir |
/// | `github_release` | `{"kind":"github_release","owner":"o","repo":"r","tag":"v1"}` | owner, repo, tag |
/// | `github_run` | `{"kind":"github_run","owner":"o","repo":"r","id":9}` | owner, repo, id |
/// | `gist` | `{"kind":"gist","id":"abc123"}` | id |
/// | `agent` | `{"kind":"agent","name":"Ada","cid":null}` or `{"kind":"agent","name":null,"cid":42}` | name or cid |
/// | `mention_all` | `{"kind":"mention_all"}` | none |
/// | `task` | `{"kind":"task","number":528}` | number |
/// | `path` | `{"kind":"path","path":"src/main.rs","line":42,"dir":false}` | path, line, dir |
/// | `library` | `{"kind":"library","id":"d1f2"}` (a deliverable, `/v1/files/d1f2`) | id |
/// | `library_tool` | `{"kind":"library_tool","name":"search","version":3}` (`/v1/library/search/3`) | name, version |
/// | `artifact` | `{"kind":"artifact","id":"a7k2m9qp","version":3}` (`artifact:a7k2m9qp@3`) | id, version |
/// | `image` | `{"kind":"image","url":"https://x/a.png"}` | url |
/// | `video` | `{"kind":"video","url":"https://x/a.mp4"}` | url |
/// | `html` | `{"kind":"html","url":"https://x/a.html"}` | url |
/// | `link` | `{"kind":"link","url":"https://example.com/x"}` | url |
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Ref {
    GithubPull {
        owner: String,
        repo: String,
        number: u64,
    },
    GithubIssue {
        owner: String,
        repo: String,
        number: u64,
    },
    GithubCommit {
        owner: String,
        repo: String,
        sha: String,
    },
    GithubRepo {
        owner: String,
        repo: String,
    },
    GithubFile {
        owner: String,
        repo: String,
        #[serde(rename = "ref")]
        reference: String,
        path: String,
        line: Option<u32>,
        dir: bool,
    },
    GithubRelease {
        owner: String,
        repo: String,
        tag: String,
    },
    GithubRun {
        owner: String,
        repo: String,
        id: u64,
    },
    Gist {
        id: String,
    },
    Agent {
        name: Option<String>,
        cid: Option<u64>,
    },
    MentionAll,
    Task {
        number: u64,
    },
    Path {
        path: String,
        line: Option<u32>,
        dir: bool,
    },
    Library {
        id: String,
    },
    LibraryTool {
        name: String,
        version: u32,
    },
    /// A hub artifact, written as `artifact:<id>` or `artifact:<id>@<n>`, or as a hub `/artifact/<id>` link.
    Artifact {
        id: String,
        version: Option<u32>,
    },
    Image {
        url: String,
    },
    Video {
        url: String,
    },
    Html {
        url: String,
    },
    Link {
        url: String,
    },
}

/// The short chip label (docs/design/workspace.md, section 3.1): `o/r #12`, `o/r @abc1234`, `r: path:10`,
/// `#528`, `@Name`, `src/main.rs:42`. Used by the CLI and the desktop.
pub fn label(r: &Ref) -> String {
    match r {
        Ref::GithubPull {
            owner,
            repo,
            number,
        }
        | Ref::GithubIssue {
            owner,
            repo,
            number,
        } => format!("{owner}/{repo} #{number}"),
        Ref::GithubCommit { owner, repo, sha } => {
            let short: String = sha.chars().take(7).collect();
            format!("{owner}/{repo} @{short}")
        }
        Ref::GithubRepo { owner, repo } => format!("{owner}/{repo}"),
        Ref::GithubFile {
            repo,
            path,
            line,
            dir,
            ..
        } => {
            let shown = folder_path(path, *dir);
            match line {
                Some(line) => format!("{repo}: {shown}:{line}"),
                None => format!("{repo}: {shown}"),
            }
        }
        Ref::GithubRelease { owner, repo, tag } => format!("{owner}/{repo} release {tag}"),
        Ref::GithubRun { owner, repo, id } => format!("{owner}/{repo} run {id}"),
        Ref::Gist { id } => format!("gist {id}"),
        Ref::Agent {
            name: Some(name), ..
        } => format!("@{name}"),
        Ref::Agent { name: None, cid } => format!("agent:{}", cid.unwrap_or(0)),
        Ref::MentionAll => "@all".to_string(),
        Ref::Task { number } => format!("#{number}"),
        Ref::Path { path, line, dir } => {
            let shown = folder_path(path, *dir);
            match line {
                Some(line) => format!("{shown}:{line}"),
                None => shown,
            }
        }
        Ref::Library { id } => format!("file {id}"),
        Ref::LibraryTool { name, version } => format!("{name} v{version}"),
        Ref::Artifact { id, version } => match version {
            Some(version) => format!("artifact {id}@{version}"),
            None => format!("artifact {id}"),
        },
        Ref::Image { url } | Ref::Video { url } | Ref::Html { url } => {
            let path = url::strip_query(url);
            path.rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or(path)
                .to_string()
        }
        Ref::Link { url } => {
            let bare = url::strip_query(url);
            let rest = url::strip_scheme(bare).unwrap_or(bare);
            rest.trim_end_matches('/').to_string()
        }
    }
}

fn folder_path(path: &str, dir: bool) -> String {
    if dir && !path.ends_with('/') {
        format!("{path}/")
    } else {
        path.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gh(path: &str, line: Option<u32>, dir: bool) -> Ref {
        Ref::GithubFile {
            owner: "o".into(),
            repo: "r".into(),
            reference: "main".into(),
            path: path.into(),
            line,
            dir,
        }
    }

    #[test]
    fn ref_json_uses_the_kind_and_the_ref_field_name() {
        let json = serde_json::to_value(gh("a.rs", None, false)).unwrap();
        assert_eq!(json["kind"], "github_file");
        assert_eq!(json["ref"], "main");
        assert!(json.get("reference").is_none());
        let back: Ref = serde_json::from_value(json).unwrap();
        assert_eq!(back, gh("a.rs", None, false));
    }
}
