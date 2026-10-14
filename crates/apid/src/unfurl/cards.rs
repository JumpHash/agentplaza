//! The cards: what the unfurl shows for a reference, built from the body of one GitHub or Forgejo answer. Pure, so the
//! tests run them on recorded fixtures. GitHub and Forgejo answer the same shapes, except where a field name differs
//! (`stargazers_count` and `stars_count`); the readers take whichever is there.

use apz_rich::Ref;
use serde_json::{Value, json};

use crate::git::{base64_decode, pct, seg};
use crate::ide::rfc3339_ms;

/// The most characters of a body a card shows.
const EXCERPT: usize = 280;
/// The first lines of a file, and the first names of a folder, a card shows.
const LINES: usize = 12;
const NAMES: usize = 12;

/// How long an open item's answer is kept (ms): 10 minutes.
pub const OPEN_TTL: u64 = 10 * 60 * 1000;
/// Merged, closed and completed items, and commits (which do not change): 1 hour.
pub const SETTLED_TTL: u64 = 60 * 60 * 1000;
/// A failure is retried after a minute.
pub const FAILURE_TTL: u64 = 60 * 1000;

/// The Ref's kind, as the JSON names it (`github_pull`).
pub fn kind_of(r: &Ref) -> String {
    serde_json::to_value(r)
        .ok()
        .and_then(|v| v["kind"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// The card for `r` from its answer's `body`: the reference's kind and its web link are added here.
pub fn card(r: &Ref, body: &Value, origin: &str) -> Value {
    let mut card = match r {
        Ref::GithubPull { .. } | Ref::PlazaGitPull { .. } => pull(body),
        Ref::GithubIssue { .. } | Ref::PlazaGitIssue { .. } => issue(body),
        Ref::GithubCommit { .. } | Ref::PlazaGitCommit { .. } => commit(body),
        Ref::GithubRepo { .. } | Ref::PlazaGitRepo { .. } => repo(body),
        Ref::GithubFile { .. } | Ref::PlazaGitFile { .. } => file(r, body),
        Ref::GithubRelease { .. } | Ref::PlazaGitRelease { .. } => release(body),
        Ref::GithubRun { .. } => run(body),
        Ref::Gist { .. } => gist(body),
        _ => Value::Null,
    };
    card["kind"] = json!(kind_of(r));
    card["url"] = web_url(r, origin).map_or(Value::Null, Value::String);
    card
}

/// The TTL of a stored answer, in ms: failures a minute, commits and settled items an hour, the rest 10 minutes.
pub fn ttl_ms(item: &Value) -> u64 {
    if item["ok"].as_bool() != Some(true) {
        return FAILURE_TTL;
    }
    let card = &item["card"];
    match (card["kind"].as_str(), card["state"].as_str()) {
        (Some("github_commit" | "plaza_git_commit"), _) => SETTLED_TTL,
        (_, Some("merged" | "closed" | "completed")) => SETTLED_TTL,
        _ => OPEN_TTL,
    }
}

/// The page a reference links to on GitHub, or on the plaza's git host (`origin`, which may be unset).
pub fn web_url(r: &Ref, origin: &str) -> Option<String> {
    let github = "https://github.com";
    let url = match r {
        Ref::GithubPull {
            owner,
            repo,
            number,
        } => format!("{github}/{owner}/{repo}/pull/{number}"),
        Ref::GithubIssue {
            owner,
            repo,
            number,
        } => format!("{github}/{owner}/{repo}/issues/{number}"),
        Ref::GithubCommit { owner, repo, sha } => format!("{github}/{owner}/{repo}/commit/{sha}"),
        Ref::GithubRepo { owner, repo } => format!("{github}/{owner}/{repo}"),
        Ref::GithubFile {
            owner,
            repo,
            reference,
            path,
            line,
            dir,
        } => {
            let kind = if *dir { "tree" } else { "blob" };
            let anchor = line.map(|n| format!("#L{n}")).unwrap_or_default();
            format!(
                "{github}/{owner}/{repo}/{kind}/{}/{}{anchor}",
                seg(reference),
                pct(path, true)
            )
        }
        Ref::GithubRelease { owner, repo, tag } => {
            format!("{github}/{owner}/{repo}/releases/tag/{}", seg(tag))
        }
        Ref::GithubRun { owner, repo, id } => format!("{github}/{owner}/{repo}/actions/runs/{id}"),
        Ref::Gist { id } => format!("https://gist.github.com/{id}"),
        _ if origin.is_empty() => return None,
        Ref::PlazaGitPull {
            owner,
            repo,
            number,
        } => format!("{origin}/{owner}/{repo}/pulls/{number}"),
        Ref::PlazaGitIssue {
            owner,
            repo,
            number,
        } => format!("{origin}/{owner}/{repo}/issues/{number}"),
        Ref::PlazaGitCommit { owner, repo, sha } => format!("{origin}/{owner}/{repo}/commit/{sha}"),
        Ref::PlazaGitRepo { owner, repo } => format!("{origin}/{owner}/{repo}"),
        Ref::PlazaGitFile {
            owner,
            repo,
            reference,
            path,
            line,
            ..
        } => {
            let anchor = line.map(|n| format!("#L{n}")).unwrap_or_default();
            format!(
                "{origin}/{owner}/{repo}/src/branch/{}/{}{anchor}",
                seg(reference),
                pct(path, true)
            )
        }
        Ref::PlazaGitRelease { owner, repo, tag } => {
            format!("{origin}/{owner}/{repo}/releases/tag/{}", seg(tag))
        }
        Ref::PlazaGitRun { .. } => return None,
        _ => return None,
    };
    Some(url)
}

fn pull(body: &Value) -> Value {
    let merged = body["merged"].as_bool() == Some(true) || body["merged_at"].is_string();
    let state = if merged {
        "merged"
    } else if body["state"].as_str() == Some("closed") {
        "closed"
    } else if body["draft"].as_bool() == Some(true) {
        "draft"
    } else {
        "open"
    };
    json!({
        "title": body["title"],
        "author": author(&body["user"]),
        "state": state,
        "head": body["head"]["ref"],
        "base": body["base"]["ref"],
        "additions": body["additions"],
        "deletions": body["deletions"],
        "comments": body["comments"],
        "created_ms": ms(&body["created_at"]),
        "body_excerpt": excerpt(&body["body"]),
    })
}

fn issue(body: &Value) -> Value {
    let labels: Vec<&str> = body["labels"]
        .as_array()
        .map(|items| items.iter().filter_map(|l| l["name"].as_str()).collect())
        .unwrap_or_default();
    json!({
        "title": body["title"],
        "author": author(&body["user"]),
        "state": body["state"],
        "labels": labels,
        "comments": body["comments"],
        "created_ms": ms(&body["created_at"]),
        "body_excerpt": excerpt(&body["body"]),
    })
}

fn commit(body: &Value) -> Value {
    let message = body["commit"]["message"].as_str().unwrap_or("");
    json!({
        "title": message.lines().next().unwrap_or(""),
        "message": excerpt(&body["commit"]["message"]),
        "author": {
            "name": body["commit"]["author"]["name"],
            "login": body["author"]["login"],
            "avatar_url": body["author"]["avatar_url"],
        },
        "date_ms": ms(&body["commit"]["author"]["date"]),
        "files": body["files"].as_array().map_or(Value::Null, |files| json!(files.len())),
        "additions": body["stats"]["additions"],
        "deletions": body["stats"]["deletions"],
    })
}

fn repo(body: &Value) -> Value {
    let stars = if body["stargazers_count"].is_null() {
        &body["stars_count"]
    } else {
        &body["stargazers_count"]
    };
    json!({
        "name": body["full_name"],
        "description": body["description"],
        "stars": stars,
        "language": body["language"],
        "default_branch": body["default_branch"],
        "updated_ms": ms(&body["updated_at"]),
    })
}

/// A file: its first lines (`lines`, null for a binary file, `truncated` when the host sent no content). A folder: its
/// entry count and first names.
fn file(r: &Ref, body: &Value) -> Value {
    let (reference, path, line) = match r {
        Ref::GithubFile {
            reference,
            path,
            line,
            ..
        }
        | Ref::PlazaGitFile {
            reference,
            path,
            line,
            ..
        } => (reference, path, line),
        _ => return Value::Null,
    };
    if let Some(entries) = body.as_array() {
        let names: Vec<&str> = entries
            .iter()
            .filter_map(|e| e["name"].as_str())
            .take(NAMES)
            .collect();
        return json!({ "path": path, "ref": reference, "dir": true, "entries": entries.len(), "names": names });
    }
    let bytes = body["content"].as_str().and_then(|text| {
        let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        base64_decode(&compact).ok()
    });
    let (lines, binary) = match bytes {
        Some(bytes) if bytes.contains(&0) => (Value::Null, true),
        Some(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            (json!(text.lines().take(LINES).collect::<Vec<_>>()), false)
        }
        None => (Value::Null, false),
    };
    json!({
        "path": path,
        "ref": reference,
        "dir": false,
        "size": body["size"],
        "binary": binary,
        "line": line,
        "lines": lines,
        "truncated": body["content"].is_null(),
    })
}

fn release(body: &Value) -> Value {
    json!({
        "name": body["name"].as_str().filter(|n| !n.is_empty()).or(body["tag_name"].as_str()),
        "tag": body["tag_name"],
        "author": author(&body["author"]),
        "prerelease": body["prerelease"],
        "draft": body["draft"],
        "created_ms": ms(if body["published_at"].is_null() { &body["created_at"] } else { &body["published_at"] }),
        "body_excerpt": excerpt(&body["body"]),
    })
}

fn run(body: &Value) -> Value {
    json!({
        "name": body["name"],
        "title": body["display_title"],
        "state": body["status"],
        "conclusion": body["conclusion"],
        "branch": body["head_branch"],
        "event": body["event"],
        "actor": body["actor"]["login"],
        "created_ms": ms(if body["run_started_at"].is_null() { &body["created_at"] } else { &body["run_started_at"] }),
    })
}

fn gist(body: &Value) -> Value {
    let first_file = body["files"]
        .as_object()
        .and_then(|files| files.keys().next().cloned());
    let title = body["description"]
        .as_str()
        .filter(|d| !d.is_empty())
        .map(str::to_string)
        .or(first_file);
    json!({
        "title": title,
        "author": author(&body["owner"]),
        "files": body["files"].as_object().map_or(Value::Null, |files| json!(files.len())),
        "updated_ms": ms(&body["updated_at"]),
    })
}

/// A user as the cards show it.
fn author(user: &Value) -> Value {
    json!({ "login": user["login"], "avatar_url": user["avatar_url"] })
}

/// An RFC 3339 instant as milliseconds, or null.
fn ms(value: &Value) -> Value {
    value
        .as_str()
        .and_then(rfc3339_ms)
        .map_or(Value::Null, |ms| json!(ms))
}

/// The first characters of a body, with the markdown marks removed and the spaces collapsed; null without a body.
fn excerpt(body: &Value) -> Value {
    let Some(text) = body.as_str() else {
        return Value::Null;
    };
    let plain: String = text
        .chars()
        .filter(|c| !matches!(c, '#' | '*' | '`' | '>' | '[' | ']'))
        .collect();
    let collapsed = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    json!(collapsed.chars().take(EXCERPT).collect::<String>())
}
