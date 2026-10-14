//! `agentplaza lab hf`: Hugging Face. The owner's token (read from a hidden prompt or stdin, never an
//! argument), search and a repo's files, and the import queue: a repo is imported as a base model to
//! fine-tune, or as a servable model.

use std::io::{IsTerminal, Read, Write};

use clap::Subcommand;
use serde_json::{json, Value};

use super::{repo_path, size_text};
use crate::cmd::models::{items, joined, none_if_empty};
use crate::ctx::{self, Ctx};

const SORTS: [&str; 4] = ["downloads", "likes", "lastModified", "trending"];
const LIBRARIES: [&str; 4] = ["onnx", "safetensors", "transformers", "gguf"];
const IMPORT_KINDS: [&str; 2] = ["base", "servable"];
/// The states a cancel can stop. A registering import waits for its model; the others are over.
const CANCELLABLE: [&str; 3] = ["queued", "downloading", "verifying"];

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The token's state, the size cap, the free space and the downloads folder. Never the token.
    Status,
    /// The owner's Hugging Face token.
    #[command(subcommand)]
    Token(TokenCmd),
    /// Search the Hub: sorted by downloads unless --sort says otherwise, 30 a page.
    Search {
        /// The text to search for.
        query: Option<String>,
        /// The sort: downloads, likes, lastModified or trending.
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(SORTS))]
        sort: Option<String>,
        /// The pipeline task, such as text-classification.
        #[arg(long)]
        task: Option<String>,
        /// The library: onnx, safetensors, transformers or gguf (default: any).
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(LIBRARIES))]
        library: Option<String>,
        /// How many to show: 1 to 100.
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=100))]
        limit: Option<u64>,
        /// The next page: the cursor the last search printed.
        #[arg(long)]
        cursor: Option<String>,
    },
    /// One repo in full: its files and sizes, and what it can be imported as.
    Show {
        /// The repo id: name or org/name.
        repo: String,
        /// The revision: a branch, a tag or a commit (default: main).
        #[arg(long)]
        revision: Option<String>,
    },
    /// Import a repo as a base model (to fine-tune) or as a servable model.
    Import {
        /// The repo id: name or org/name.
        repo: String,
        /// base or servable.
        #[arg(long = "as", value_parser = clap::builder::PossibleValuesParser::new(IMPORT_KINDS))]
        kind: String,
        /// The revision (default: main).
        #[arg(long)]
        revision: Option<String>,
        /// Only these files, comma-separated (default: the files the kind needs).
        #[arg(long, value_delimiter = ',')]
        files: Vec<String>,
    },
    /// The import queue, newest first.
    Imports,
    /// Cancel a queued or running import. Its partial files stay, so a new import resumes.
    Cancel {
        /// The import's number.
        n: u64,
    },
    /// Remove an import from the queue: a finished, failed, cancelled or interrupted one. Its partial files go.
    Remove {
        /// The import's number.
        n: u64,
    },
}

#[derive(Subcommand, Debug)]
pub enum TokenCmd {
    /// Store the token. It is read from a hidden prompt, or from stdin when stdin is not a terminal. Never an argument.
    Set,
    /// Remove the stored token.
    Rm,
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::Status => {
            let status = ctx.client.get("/v1/foundry/hf/status")?;
            ctx.out(&status, print_status);
            Ok(())
        }
        Cmd::Token(TokenCmd::Set) => {
            let value = token_from_terminal()?;
            let reply = ctx.client.put("/v1/foundry/hf/token", &json!({ "value": value }))?;
            ctx.out(&reply, |_| println!("Hugging Face token stored"));
            Ok(())
        }
        Cmd::Token(TokenCmd::Rm) => {
            let reply = ctx.client.delete("/v1/foundry/hf/token")?;
            ctx.out(&reply, |_| println!("Hugging Face token removed"));
            Ok(())
        }
        Cmd::Search { query, sort, task, library, limit, cursor } => {
            let path = search_path(&[
                ("search", query),
                ("sort", sort),
                ("task", task),
                ("library", library),
                ("limit", limit.map(|n| n.to_string())),
                ("cursor", cursor),
            ]);
            let page = ctx.client.get(&path)?;
            ctx.out(&page, print_search);
            Ok(())
        }
        Cmd::Show { repo, revision } => {
            let mut path = format!("/v1/foundry/hf/models/{}", repo_path(&repo));
            if let Some(revision) = revision {
                path.push_str(&format!("?revision={}", ctx::seg(&revision)));
            }
            let detail = ctx.client.get(&path)?;
            ctx.out(&detail, print_detail);
            Ok(())
        }
        Cmd::Import { repo, kind, revision, files } => {
            let body = import_body(&repo, revision.as_deref(), &kind, &files);
            let reply = ctx.client.post("/v1/foundry/hf/import", &body)?;
            ctx.out(&reply, |r| {
                let n = ctx::text(&r["import"], "n");
                if r.get("already").and_then(Value::as_bool) == Some(true) {
                    println!("already imported as #{n} ({})", ctx::text(&r["import"], "state"));
                } else {
                    println!("import #{n} queued");
                }
            });
            Ok(())
        }
        Cmd::Imports => {
            let list = ctx.client.get("/v1/foundry/hf/imports")?;
            ctx.out(&list, |v| {
                let rows: Vec<Vec<String>> = items(&v["imports"])
                    .map(|i| {
                        vec![
                            format!("#{}", ctx::text(i, "n")),
                            ctx::s(i, "id").to_string(),
                            ctx::s(i, "as").to_string(),
                            ctx::s(i, "state").to_string(),
                            format!("{} of {}", size_text(i["bytes"].as_f64().unwrap_or(0.0)), size_text(i["total"].as_f64().unwrap_or(0.0))),
                            ctx::when(&i["updated_ms"]),
                            outcome(i),
                        ]
                    })
                    .collect();
                ctx::table(&["no.", "repo", "as", "state", "progress", "updated", "model or error"], &rows);
            });
            Ok(())
        }
        Cmd::Cancel { n } => {
            let import = ctx.client.get(&format!("/v1/foundry/hf/imports/{n}"))?;
            let state = ctx::text(&import["import"], "state");
            if !CANCELLABLE.contains(&state.as_str()) {
                return Err(format!("import #{n} is {state}: only a queued or running import cancels (use `lab hf remove {n}`)"));
            }
            let reply = ctx.client.delete(&format!("/v1/foundry/hf/imports/{n}"))?;
            ctx.out(&reply, |_| println!("import #{n} cancelled"));
            Ok(())
        }
        Cmd::Remove { n } => {
            let reply = ctx.client.delete(&format!("/v1/foundry/hf/imports/{n}"))?;
            // The hub cancels an import that is still running rather than removing it, and says which it did.
            ctx.out(&reply, |r| {
                if r.get("removed").is_some() {
                    println!("removed import #{n}");
                } else {
                    println!("import #{n} was still running, so it was cancelled");
                }
            });
            Ok(())
        }
    }
}

/// The search's path: each value encoded, and only the flags given (the hub's defaults fill the rest).
fn search_path(query: &[(&str, Option<String>)]) -> String {
    let base = "/v1/foundry/hf/models";
    let pairs: Vec<String> = query
        .iter()
        .filter_map(|(key, value)| value.as_ref().filter(|v| !v.is_empty()).map(|v| format!("{key}={}", ctx::seg(v))))
        .collect();
    if pairs.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", pairs.join("&"))
    }
}

/// The import request: the repo, the revision when one is named, the kind, and the files when some are named.
pub fn import_body(repo: &str, revision: Option<&str>, kind: &str, files: &[String]) -> Value {
    let mut body = json!({ "id": repo, "as": kind });
    if let Some(revision) = revision {
        body["revision"] = json!(revision);
    }
    if !files.is_empty() {
        body["files"] = json!(files);
    }
    body
}

/// Whether a repo is open, or gated (a gate kind such as `auto` or `manual`).
fn access(gated: &Value) -> &'static str {
    match gated {
        Value::Bool(false) | Value::Null => "open",
        Value::String(kind) if kind.is_empty() => "open",
        _ => "gated",
    }
}

/// The token's state, the cap, the free space and the downloads folder. Never the token.
fn print_status(s: &Value) {
    let token = if s["hf_token_set"].as_bool() == Some(true) { "set" } else { "not set" };
    let free = s["free_bytes"].as_f64().map_or_else(|| "—".to_string(), size_text);
    ctx::fields(&[
        ("token", token.to_string()),
        ("size cap", format!("{} GiB", ctx::text(s, "hf_max_gb"))),
        ("free space", free),
        ("downloads", ctx::s(s, "downloads_dir").to_string()),
    ]);
}

fn print_search(page: &Value) {
    let rows: Vec<Vec<String>> = items(&page["items"])
        .map(|m| {
            vec![
                ctx::s(m, "id").to_string(),
                ctx::text(m, "task"),
                ctx::text(m, "library"),
                ctx::text(m, "downloads"),
                ctx::text(m, "likes"),
                ctx::text(m, "last_modified"),
                access(&m["gated"]).to_string(),
                ctx::text(m, "license"),
            ]
        })
        .collect();
    ctx::table(&["model", "task", "library", "downloads", "likes", "updated", "access", "license"], &rows);
    if let Some(next) = page["next_cursor"].as_str() {
        println!("more: --cursor {next}");
    }
}

/// A repo in full: its identity, what it can be imported as, and its files.
fn print_detail(d: &Value) {
    let importable = &d["importable"];
    ctx::fields(&[
        ("id", ctx::s(d, "id").to_string()),
        ("author", ctx::text(d, "author")),
        ("revision", short_sha(ctx::s(d, "sha"))),
        ("task", none_if_empty(ctx::text(d, "task"))),
        ("library", none_if_empty(ctx::text(d, "library"))),
        ("license", none_if_empty(ctx::text(d, "license"))),
        ("access", access(&d["gated"]).to_string()),
        ("downloads", ctx::text(d, "downloads")),
        ("likes", ctx::text(d, "likes")),
        ("tags", none_if_empty(joined(d.get("tags")))),
        ("as base", importable_text(importable, "base", "base_reason")),
        ("as servable", importable_text(importable, "servable", "servable_reason")),
    ]);
    let rows: Vec<Vec<String>> = items(&d["files"])
        .map(|f| {
            vec![
                ctx::s(f, "path").to_string(),
                size_text(f["size"].as_f64().unwrap_or(0.0)),
                if f["lfs"].as_bool() == Some(true) { "lfs" } else { "" }.to_string(),
            ]
        })
        .collect();
    println!();
    ctx::table(&["file", "size", "storage"], &rows);
}

/// `yes`, or `no: the reason`, for one kind of import.
fn importable_text(importable: &Value, kind: &str, reason: &str) -> String {
    if importable[kind].as_bool() == Some(true) {
        "yes".to_string()
    } else {
        format!("no: {}", none_if_empty(ctx::text(importable, reason)))
    }
}

/// The first twelve characters of a commit.
fn short_sha(sha: &str) -> String {
    sha.chars().take(12).collect()
}

/// An import's outcome in the queue: its model when it is done, its error when it failed, else its reason.
fn outcome(i: &Value) -> String {
    match (i["model_id"].as_str(), i["error"].as_str(), i["reason"].as_str()) {
        (Some(model), _, _) => model.to_string(),
        (None, Some(error), _) => error.to_string(),
        (None, None, Some(reason)) => reason.to_string(),
        _ => String::new(),
    }
}

/// The token from a hidden prompt on the terminal, or one read from stdin when stdin is not a terminal.
/// It is never echoed and never an argument.
fn token_from_terminal() -> Result<String, String> {
    let text = if std::io::stdin().is_terminal() {
        eprint!("Hugging Face token: ");
        let _ = std::io::stderr().flush();
        read_hidden()?
    } else {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf).map_err(|e| format!("reading stdin: {e}"))?;
        buf
    };
    let token = text.trim().to_string();
    if token.is_empty() {
        Err("no token was entered".into())
    } else {
        Ok(token)
    }
}

/// One line from the terminal with the echo turned off. The settings are put back afterwards.
#[cfg(unix)]
fn read_hidden() -> Result<String, String> {
    let fd = libc::STDIN_FILENO;
    // SAFETY: termios is plain old data, and tcgetattr fills it in before it is read.
    let mut saved: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: fd is the terminal on stdin and `saved` is a valid out-parameter.
    if unsafe { libc::tcgetattr(fd, &mut saved) } != 0 {
        return Err("cannot read the terminal settings".into());
    }
    let mut quiet = saved;
    quiet.c_lflag &= !libc::ECHO;
    // SAFETY: as above; `quiet` is a copy of the settings just read, with echo off.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &quiet) } != 0 {
        return Err("cannot turn off the echo".into());
    }
    let mut line = String::new();
    let read = std::io::stdin().read_line(&mut line);
    // SAFETY: restores the settings read above.
    unsafe {
        libc::tcsetattr(fd, libc::TCSANOW, &saved);
    }
    eprintln!();
    read.map_err(|e| format!("reading the prompt: {e}"))?;
    Ok(line)
}

#[cfg(not(unix))]
fn read_hidden() -> Result<String, String> {
    Err("a hidden prompt needs a Unix terminal here: pipe the token on stdin".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    #[test]
    fn a_search_names_only_the_flags_given_and_encodes_their_values() {
        assert_eq!(
            search_path(&[("search", Some("sst 2".into())), ("sort", Some("likes".into())), ("task", None)]),
            "/v1/foundry/hf/models?search=sst%202&sort=likes"
        );
        assert_eq!(search_path(&[("search", None), ("cursor", Some(String::new()))]), "/v1/foundry/hf/models");
        assert_eq!(search_path(&[("limit", Some("5".into())), ("cursor", Some("a+b".into()))]), "/v1/foundry/hf/models?limit=5&cursor=a%2Bb");
    }

    #[test]
    fn an_import_names_the_repo_and_kind_and_only_the_revision_and_files_given() {
        assert_eq!(import_body("org/m", None, "base", &[]), json!({ "id": "org/m", "as": "base" }));
        assert_eq!(
            import_body("m", Some("abc1234"), "servable", &["model.onnx".into()]),
            json!({ "id": "m", "as": "servable", "revision": "abc1234", "files": ["model.onnx"] })
        );
    }

    #[test]
    fn a_repo_is_open_unless_the_hub_names_a_gate() {
        assert_eq!(access(&json!(false)), "open");
        assert_eq!(access(&Value::Null), "open");
        assert_eq!(access(&json!("")), "open");
        assert_eq!(access(&json!("auto")), "gated");
        assert_eq!(access(&json!("manual")), "gated");
    }

    #[test]
    fn an_import_says_what_it_made_or_why_it_failed() {
        assert_eq!(outcome(&json!({ "model_id": "hf-m-1a2b", "error": null })), "hf-m-1a2b");
        assert_eq!(outcome(&json!({ "model_id": null, "error": "disk full" })), "disk full");
        assert_eq!(outcome(&json!({ "model_id": null, "error": null, "reason": "no onnx" })), "no onnx");
        assert_eq!(outcome(&json!({})), "");
    }

    /// `lab hf ARGS`, parsed. The token is never a value on the command line.
    #[derive(Parser, Debug)]
    struct Probe {
        #[command(subcommand)]
        cmd: Cmd,
    }

    #[test]
    fn the_import_kind_and_the_search_sort_are_checked_by_the_parser() {
        assert!(Probe::command().find_subcommand("token").and_then(|t| t.find_subcommand("set")).is_some());
        assert!(Probe::try_parse_from(["hf", "token", "set", "hf_x"]).is_err());
        assert!(Probe::try_parse_from(["hf", "import", "m", "--as", "base"]).is_ok());
        assert!(Probe::try_parse_from(["hf", "import", "m", "--as", "gguf"]).is_err());
        assert!(Probe::try_parse_from(["hf", "search", "--sort", "stars"]).is_err());
        assert!(Probe::try_parse_from(["hf", "search", "--sort", "trending", "--library", "onnx"]).is_ok());
    }
}
