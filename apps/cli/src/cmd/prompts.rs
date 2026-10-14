//! `agentplaza prompts`: the prompt library. System prompts, AGENTS.md files and fragments: each save is a
//! version with a score and findings, one version is live, and a prompt is bound to a role (a system prompt) or
//! to the plaza (AGENTS.md). The same hub routes the web's prompt editor uses. A body comes from a file, or from
//! stdin with `-`.
//!
//! The helpers at the top are shared with `skills`: the JSON calls, the reply checks, the report and diff lines.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde_json::{Value, json};

use crate::cmd::models::{items, joined};
use crate::ctx::{self, Ctx, confirm};

/// The three kinds a prompt can be (the hub's names).
pub const KINDS: [&str; 3] = ["system", "agents_md", "fragment"];

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The prompts with their live version's score. Archived ones with --archived.
    List {
        /// Only the prompts of this kind.
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(KINDS))]
        kind: Option<String>,
        /// The archived prompts instead of the live ones.
        #[arg(long)]
        archived: bool,
    },
    /// One prompt: its fields and its versions. With --version, that version's findings and stats.
    #[command(disable_version_flag = true)]
    Show {
        /// The prompt's id.
        id: String,
        /// A version to show in full.
        #[arg(long)]
        version: Option<u32>,
    },
    /// A version's markdown, exactly as it is stored (for a file or a pipe).
    #[command(disable_version_flag = true)]
    Raw {
        /// The prompt's id.
        id: String,
        /// The version number.
        version: u32,
    },
    /// Create a prompt at version 1 from a body file (or stdin with -).
    Create {
        /// The kind: system, agents_md or fragment.
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(KINDS))]
        kind: String,
        /// The title.
        #[arg(long)]
        title: String,
        /// The id, a slug. Default: the title's slug.
        #[arg(long)]
        id: Option<String>,
        /// The binding: role:<role> for a system prompt, plaza for AGENTS.md.
        #[arg(long)]
        binding: Option<String>,
        /// A short description.
        #[arg(long)]
        description: Option<String>,
        /// Tags, comma-separated.
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        /// The body (markdown): a file, or - for stdin.
        #[arg(long)]
        file: PathBuf,
    },
    /// Change a prompt's title, description, tags or binding. Only the flags given change; --binding "" unbinds.
    Edit {
        /// The prompt's id.
        id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        description: Option<String>,
        /// The tags, comma-separated (replaces the list).
        #[arg(long, value_delimiter = ',')]
        tags: Option<Vec<String>>,
        /// The binding (replaces it); "" removes it.
        #[arg(long)]
        binding: Option<String>,
    },
    /// Save a new version from a body file (or stdin with -). --base is the version you started from (default:
    /// the latest); a newer version since then refuses the save.
    Save {
        /// The prompt's id.
        id: String,
        /// The body (markdown): a file, or - for stdin.
        #[arg(long)]
        file: PathBuf,
        /// A note on what changed.
        #[arg(long)]
        note: Option<String>,
        /// The version you started from.
        #[arg(long)]
        base: Option<u32>,
    },
    /// Make a version the live one (default: the latest). A version with errors is refused.
    #[command(disable_version_flag = true)]
    Publish {
        /// The prompt's id.
        id: String,
        /// The version number.
        version: Option<u32>,
    },
    /// Take the live version down. The prompt keeps its versions.
    Unpublish {
        /// The prompt's id.
        id: String,
    },
    /// Archive a prompt: it leaves the live set. Asks first unless --yes.
    Archive {
        /// The prompt's id.
        id: String,
        /// Archive without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Bring an archived prompt back.
    Restore {
        /// The prompt's id.
        id: String,
    },
    /// The line diff between two versions.
    Diff {
        /// The prompt's id.
        id: String,
        /// The older version.
        from: u32,
        /// The newer version.
        to: u32,
    },
    /// The prompt's history: each change, who made it, and when.
    History {
        /// The prompt's id.
        id: String,
    },
    /// Score a body for a kind before saving it: the score and its findings, with line numbers.
    Check {
        /// The kind: system, agents_md or fragment.
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(KINDS))]
        kind: String,
        /// The body (markdown): a file, or - for stdin.
        #[arg(long)]
        file: PathBuf,
    },
    /// The rules every prompt is scored against, and the variables a prompt may use.
    Rules,
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List { kind, archived } => list(ctx, kind.as_deref(), archived),
        Cmd::Show { id, version } => show(ctx, &id, version),
        Cmd::Raw { id, version } => {
            let bytes = get_bytes(ctx, &format!("{}/versions/{version}/raw", prompt_path(&id)))?;
            std::io::stdout()
                .write_all(&bytes)
                .map_err(|e| e.to_string())
        }
        Cmd::Create {
            kind,
            title,
            id,
            binding,
            description,
            tags,
            file,
        } => {
            let body = read_text(&file)?;
            let new = create_body(
                &kind,
                &title,
                id.as_deref(),
                binding.as_deref(),
                description.as_deref(),
                &tags,
                &body,
            );
            let made = ctx.client.post("/v1/prompts", &new)?;
            ctx.out(&made, |m| {
                println!("created {} (v1, {kind})", ctx::s(&m["prompt"], "id"))
            });
            Ok(())
        }
        Cmd::Edit {
            id,
            title,
            description,
            tags,
            binding,
        } => edit(ctx, &id, title, description, tags, binding),
        Cmd::Save {
            id,
            file,
            note,
            base,
        } => save(ctx, &id, &file, note.as_deref(), base),
        Cmd::Publish { id, version } => {
            let version = match version {
                Some(v) => v,
                None => latest_version(ctx, &id)?,
            };
            let reply = ctx.client.post(
                &format!("{}/publish", prompt_path(&id)),
                &publish_body(version),
            )?;
            ctx.out(&reply, |_| println!("published v{version} of {id}"));
            Ok(())
        }
        Cmd::Unpublish { id } => {
            let reply = call(
                ctx,
                "POST",
                &format!("{}/unpublish", prompt_path(&id)),
                None,
            )?;
            ctx.out(&reply, |_| println!("unpublished {id}"));
            Ok(())
        }
        Cmd::Archive { id, yes } => {
            if !confirm(
                yes,
                &format!("Archive prompt {id}? It leaves the live set until it is restored."),
            )? {
                return Err("cancelled".into());
            }
            let reply = call(ctx, "POST", &format!("{}/archive", prompt_path(&id)), None)?;
            ctx.out(&reply, |_| println!("archived {id}"));
            Ok(())
        }
        Cmd::Restore { id } => {
            let reply = call(ctx, "POST", &format!("{}/restore", prompt_path(&id)), None)?;
            ctx.out(&reply, |_| println!("restored {id}"));
            Ok(())
        }
        Cmd::Diff { id, from, to } => diff(ctx, &id, from, to),
        Cmd::History { id } => {
            let history = ctx.client.get(&format!("{}/history", prompt_path(&id)))?;
            ctx.out(&history, |h| print_events(&h["events"]));
            Ok(())
        }
        Cmd::Check { kind, file } => {
            let body = read_text(&file)?;
            let report = ctx
                .client
                .post("/v1/prompts/check", &check_body(&kind, &body))?;
            ctx.out(&report, print_report);
            Ok(())
        }
        Cmd::Rules => {
            let rules = ctx.client.get("/v1/prompts/rules")?;
            ctx.out(&rules, print_rules);
            Ok(())
        }
    }
}

// ---- Shared with skills -----------------------------------------------------------------------

/// A JSON call to the hub: its answer when 2xx; otherwise the status and the hub's `error` text. The status is 0
/// when the hub did not answer at all.
pub(crate) fn call_status(
    ctx: &Ctx,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<Value, (u16, String)> {
    let (status, value) =
        apz_http::client::request_json(&ctx.client.target, method, path, body, ctx.client.timeout)
            .map_err(|e| (0, format!("{} {path}: {e}", ctx.client.base())))?;
    if (200..300).contains(&status) {
        Ok(value)
    } else {
        Err((
            status,
            value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("{path}: HTTP {status}")),
        ))
    }
}

/// [`call_status`] without the status: the answer, or the refusal's text.
pub(crate) fn call(
    ctx: &Ctx,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<Value, String> {
    call_status(ctx, method, path, body).map_err(|(_, message)| message)
}

/// A reply's JSON when 2xx (Null when it is empty), else its `error` text.
pub(crate) fn json_reply(path: &str, reply: &apz_http::client::Reply) -> Result<Value, String> {
    let value = if reply.body.is_empty() {
        Value::Null
    } else {
        reply.json().unwrap_or(Value::Null)
    };
    if (200..300).contains(&reply.status) {
        Ok(value)
    } else {
        Err(value
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("{path}: HTTP {}", reply.status)))
    }
}

/// The bytes of a GET (a raw body, a file, a zip), or the refusal's text.
pub(crate) fn get_bytes(ctx: &Ctx, path: &str) -> Result<Vec<u8>, String> {
    let reply =
        apz_http::client::request(&ctx.client.target, "GET", path, None, ctx.client.timeout)
            .map_err(|e| format!("{} {path}: {e}", ctx.client.base()))?;
    if (200..300).contains(&reply.status) {
        return Ok(reply.body);
    }
    Err(json_reply(path, &reply)
        .err()
        .unwrap_or_else(|| format!("{path}: HTTP {}", reply.status)))
}

/// A body from a file, or from stdin for `-`. Kept as it is (only blank text is refused).
pub(crate) fn read_text(file: &Path) -> Result<String, String> {
    let text = if file == Path::new("-") {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("reading stdin: {e}"))?;
        buf
    } else {
        std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?
    };
    if text.trim().is_empty() {
        return Err(format!("{}: the text is empty", file.display()));
    }
    Ok(text)
}

/// A string array as "a, b" ("" when there is none).
pub(crate) fn str_list(v: &Value) -> String {
    joined(Some(v))
}

/// The score, errors, warnings and infos of a report, then its findings as a table. A finding's place is its
/// path (skills) and its line; a whole-document finding has neither.
pub(crate) fn print_report(report: &Value) {
    println!(
        "score {}/100 · {} errors · {} warnings · {} infos",
        ctx::text(report, "score"),
        ctx::text(report, "errors"),
        ctx::text(report, "warnings"),
        ctx::text(report, "infos")
    );
    let rows: Vec<Vec<String>> = items(&report["findings"])
        .map(|f| {
            vec![
                ctx::s(f, "severity").to_string(),
                location(f),
                ctx::s(f, "rule").to_string(),
                ctx::s(f, "message").to_string(),
            ]
        })
        .collect();
    if rows.is_empty() {
        println!("no findings");
    } else {
        ctx::table(&["severity", "where", "rule", "message"], &rows);
    }
}

/// Where a finding is: `path:line`, `line N`, the path alone, or the whole document.
fn location(f: &Value) -> String {
    let path = ctx::s(f, "path");
    match (path.is_empty(), f["line"].as_u64().unwrap_or(0)) {
        (true, 0) => "whole document".into(),
        (true, line) => format!("line {line}"),
        (false, 0) => path.to_string(),
        (false, line) => format!("{path}:{line}"),
    }
}

/// A diff's lines: `+` added, `-` removed, a space for the rest.
pub(crate) fn print_diff_lines(lines: &Value) {
    for line in items(lines) {
        let marker = match ctx::s(line, "op") {
            "add" => "+",
            "del" => "-",
            _ => " ",
        };
        println!("{marker} {}", ctx::s(line, "text"));
    }
}

/// How many of a diff's lines are of one op (`add` or `del`).
pub(crate) fn count_op(lines: &Value, op: &str) -> usize {
    items(lines).filter(|l| ctx::s(l, "op") == op).count()
}

/// The history rows: when, what, the version, who.
pub(crate) fn print_events(events: &Value) {
    let rows: Vec<Vec<String>> = items(events)
        .map(|e| {
            vec![
                ctx::when(&e["at_ms"]),
                ctx::s(e, "action").to_string(),
                version_text(&e["version"]),
                ctx::s(e, "actor").to_string(),
            ]
        })
        .collect();
    ctx::table(&["when", "action", "version", "by"], &rows);
}

/// `v3` for a version number, `—` for none.
pub(crate) fn version_text(v: &Value) -> String {
    match v.as_u64() {
        Some(n) => format!("v{n}"),
        None => "—".into(),
    }
}

/// The text, or a dash when it is empty.
pub(crate) fn dash(text: String) -> String {
    if text.is_empty() { "—".into() } else { text }
}

// ---- Paths and bodies -------------------------------------------------------------------------

/// A prompt's path.
fn prompt_path(id: &str) -> String {
    format!("/v1/prompts/{}", ctx::seg(id))
}

/// The tags without blanks, each trimmed.
fn clean_tags(tags: &[String]) -> Vec<String> {
    tags.iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// The create body. `id` and `binding` are sent only when given (an absent id is the title's slug).
fn create_body(
    kind: &str,
    title: &str,
    id: Option<&str>,
    binding: Option<&str>,
    description: Option<&str>,
    tags: &[String],
    body: &str,
) -> Value {
    let mut out = json!({
        "kind": kind,
        "title": title,
        "description": description.unwrap_or(""),
        "tags": clean_tags(tags),
        "binding": binding,
        "body": body,
    });
    if let Some(id) = id {
        out["id"] = json!(id);
    }
    out
}

/// The edit body: the current title, description, tags and binding, with the given ones replacing them. An empty
/// binding removes it.
fn edit_body(
    current: &Value,
    title: Option<&str>,
    description: Option<&str>,
    tags: Option<&[String]>,
    binding: Option<&str>,
) -> Value {
    let binding = match binding {
        Some("") => Value::Null,
        Some(b) => json!(b),
        None => current["binding"].clone(),
    };
    json!({
        "title": title.unwrap_or(ctx::s(current, "title")),
        "description": description.unwrap_or(ctx::s(current, "description")),
        "tags": tags.map(clean_tags).map(|t| json!(t)).unwrap_or_else(|| current["tags"].clone()),
        "binding": binding,
    })
}

/// A new version's body: the markdown, the note, and the version it was made from.
fn version_body(body: &str, note: Option<&str>, base: u32) -> Value {
    json!({ "body": body, "note": note.unwrap_or(""), "base_version": base })
}

fn publish_body(version: u32) -> Value {
    json!({ "version": version })
}

fn check_body(kind: &str, body: &str) -> Value {
    json!({ "kind": kind, "body": body })
}

/// The newest version number of a prompt, read from its detail.
fn latest_version(ctx: &Ctx, id: &str) -> Result<u32, String> {
    let detail = ctx.client.get(&prompt_path(id))?;
    detail["prompt"]["latest_version"]
        .as_u64()
        .map(|v| v as u32)
        .ok_or_else(|| format!("{id}: no latest version"))
}

// ---- Commands ---------------------------------------------------------------------------------

fn list(ctx: &Ctx, kind: Option<&str>, archived: bool) -> Result<(), String> {
    let mut query = Vec::new();
    if let Some(kind) = kind {
        query.push(format!("kind={kind}"));
    }
    if archived {
        query.push("archived=true".to_string());
    }
    let path = if query.is_empty() {
        "/v1/prompts".to_string()
    } else {
        format!("/v1/prompts?{}", query.join("&"))
    };
    let list = ctx.client.get(&path)?;
    ctx.out(&list, |v| {
        let rows: Vec<Vec<String>> = items(&v["prompts"])
            .map(|p| {
                vec![
                    ctx::s(p, "id").to_string(),
                    ctx::s(p, "kind").to_string(),
                    ctx::s(p, "title").to_string(),
                    version_text(&p["active_version"]),
                    version_text(&p["latest_version"]),
                    dash(ctx::text(p, "score")),
                    dash(ctx::text(p, "binding")),
                    ctx::when(&p["updated_ms"]),
                ]
            })
            .collect();
        ctx::table(
            &[
                "id", "kind", "title", "live", "latest", "score", "binding", "updated",
            ],
            &rows,
        );
    });
    Ok(())
}

fn show(ctx: &Ctx, id: &str, version: Option<u32>) -> Result<(), String> {
    match version {
        None => {
            let detail = ctx.client.get(&prompt_path(id))?;
            ctx.out(&detail, print_detail);
        }
        Some(v) => {
            let view = ctx
                .client
                .get(&format!("{}/versions/{v}", prompt_path(id)))?;
            ctx.out(&view, print_version);
        }
    }
    Ok(())
}

fn print_detail(d: &Value) {
    let p = &d["prompt"];
    let live = if p["active_version"].is_null() {
        "not published".to_string()
    } else {
        version_text(&p["active_version"])
    };
    let mut pairs = vec![
        ("id", ctx::s(p, "id").to_string()),
        ("kind", ctx::s(p, "kind").to_string()),
        ("title", ctx::s(p, "title").to_string()),
        ("description", dash(ctx::s(p, "description").to_string())),
        ("tags", dash(joined(Some(&p["tags"])))),
        ("binding", dash(ctx::text(p, "binding"))),
        ("live", live),
        ("latest", version_text(&p["latest_version"])),
        ("best", version_text(&p["best_version"])),
        ("score", dash(ctx::text(p, "score"))),
        ("updated", ctx::when(&p["updated_ms"])),
    ];
    if !p["archived_ms"].is_null() {
        pairs.push(("archived", ctx::when(&p["archived_ms"])));
    }
    ctx::fields(&pairs);
    println!();
    let rows: Vec<Vec<String>> = items(&d["versions"])
        .map(|v| {
            vec![
                version_text(&v["version"]),
                ctx::text(v, "score"),
                ctx::text(v, "errors"),
                ctx::text(v, "warnings"),
                ctx::s(v, "author").to_string(),
                ctx::when(&v["created_ms"]),
                dash(ctx::s(v, "note").to_string()),
            ]
        })
        .collect();
    ctx::table(
        &[
            "version", "score", "errors", "warnings", "author", "created", "note",
        ],
        &rows,
    );
}

fn print_version(v: &Value) {
    ctx::fields(&[
        ("version", version_text(&v["version"])),
        ("author", ctx::s(v, "author").to_string()),
        ("created", ctx::when(&v["created_ms"])),
        ("note", dash(ctx::s(v, "note").to_string())),
    ]);
    if v["stale"].as_bool() == Some(true) {
        println!(
            "scored under older rules; today's score is {}",
            ctx::text(&v["recheck"], "score")
        );
    }
    println!();
    print_report(v);
}

fn edit(
    ctx: &Ctx,
    id: &str,
    title: Option<String>,
    description: Option<String>,
    tags: Option<Vec<String>>,
    binding: Option<String>,
) -> Result<(), String> {
    if title.is_none() && description.is_none() && tags.is_none() && binding.is_none() {
        return Err("give --title, --description, --tags or --binding".into());
    }
    let detail = ctx.client.get(&prompt_path(id))?;
    let body = edit_body(
        &detail["prompt"],
        title.as_deref(),
        description.as_deref(),
        tags.as_deref(),
        binding.as_deref(),
    );
    let updated = ctx.client.put(&prompt_path(id), &body)?;
    ctx.out(&updated, |_| println!("updated {id}"));
    Ok(())
}

fn save(
    ctx: &Ctx,
    id: &str,
    file: &Path,
    note: Option<&str>,
    base: Option<u32>,
) -> Result<(), String> {
    let body = read_text(file)?;
    let base = match base {
        Some(v) => v,
        None => latest_version(ctx, id)?,
    };
    let path = format!("{}/versions", prompt_path(id));
    let reply = match call_status(ctx, "POST", &path, Some(&version_body(&body, note, base))) {
        Ok(reply) => reply,
        Err((409, _)) => return Err("a newer version exists; reload with `prompts show`".into()),
        Err((_, message)) => return Err(message),
    };
    ctx.out(&reply, |r| {
        if r["created"].as_bool() == Some(true) {
            println!("saved {} of {id}", version_text(&r["version"]["version"]));
        } else {
            println!(
                "no change: {} already has this body",
                version_text(&r["version"]["version"])
            );
        }
    });
    Ok(())
}

fn diff(ctx: &Ctx, id: &str, from: u32, to: u32) -> Result<(), String> {
    let d = ctx
        .client
        .get(&format!("{}/diff?from={from}&to={to}", prompt_path(id)))?;
    ctx.out(&d, |d| {
        println!(
            "v{from} → v{to}: {} added, {} removed",
            count_op(&d["lines"], "add"),
            count_op(&d["lines"], "del")
        );
        print_diff_lines(&d["lines"]);
    });
    Ok(())
}

fn print_rules(r: &Value) {
    println!("rules version {}", ctx::text(r, "rules_version"));
    let rows: Vec<Vec<String>> = items(&r["rules"])
        .map(|rule| {
            vec![
                ctx::s(rule, "id").to_string(),
                ctx::s(rule, "severity").to_string(),
                str_list(&rule["kinds"]),
                ctx::s(rule, "title").to_string(),
            ]
        })
        .collect();
    ctx::table(&["rule", "severity", "kinds", "title"], &rows);
    println!();
    println!("variables: {}", str_list(&r["variables"]));
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    /// The `prompts` command alone, so its subcommands parse the way the CLI parses them.
    #[derive(Parser, Debug)]
    struct Probe {
        #[command(subcommand)]
        cmd: Cmd,
    }

    /// `prompts ARGS`, parsed.
    fn parse(args: &[&str]) -> Result<Cmd, clap::Error> {
        let mut argv = vec!["prompts"];
        argv.extend_from_slice(args);
        Probe::try_parse_from(argv).map(|p| p.cmd)
    }

    #[test]
    fn the_commands_are_all_there() {
        let cmd = Probe::command();
        for name in [
            "list",
            "show",
            "raw",
            "create",
            "edit",
            "save",
            "publish",
            "unpublish",
            "archive",
            "restore",
            "diff",
            "history",
            "check",
            "rules",
        ] {
            assert!(cmd.find_subcommand(name).is_some(), "{name} is missing");
        }
    }

    #[test]
    fn the_kinds_and_the_required_flags_are_checked() {
        assert!(
            parse(&[
                "create", "--kind", "system", "--title", "T", "--file", "a.md"
            ])
            .is_ok()
        );
        assert!(
            parse(&[
                "create", "--kind", "bogus", "--title", "T", "--file", "a.md"
            ])
            .is_err()
        );
        assert!(
            parse(&["create", "--title", "T", "--file", "a.md"]).is_err(),
            "a kind is required"
        );
        assert!(parse(&["list", "--kind", "nope"]).is_err());
        assert!(parse(&["check", "--kind", "fragment", "--file", "-"]).is_ok());
        assert!(
            parse(&["diff", "x", "1"]).is_err(),
            "diff takes two versions"
        );
        assert!(parse(&["raw", "x", "two"]).is_err());
    }

    #[test]
    fn tags_are_split_on_commas() {
        match parse(&["edit", "x", "--tags", "a,b"]) {
            Ok(Cmd::Edit { tags, .. }) => {
                assert_eq!(tags, Some(vec!["a".to_string(), "b".to_string()]))
            }
            other => panic!("edit did not parse: {other:?}"),
        }
    }

    #[test]
    fn a_prompt_path_encodes_its_id() {
        assert_eq!(prompt_path("planner"), "/v1/prompts/planner");
        assert_eq!(prompt_path("a b"), "/v1/prompts/a%20b");
    }

    #[test]
    fn the_create_body_sends_only_what_was_given() {
        let body = create_body(
            "system",
            "Planner",
            None,
            Some("role:planner"),
            None,
            &[" a ".into(), "".into()],
            "# Hi\n",
        );
        assert_eq!(body["kind"], "system");
        assert_eq!(body["binding"], "role:planner");
        assert_eq!(body["description"], "");
        assert_eq!(body["tags"], json!(["a"]));
        assert!(body.get("id").is_none(), "the title's slug is the default");
        let with_id = create_body(
            "fragment",
            "Tone",
            Some("tone"),
            None,
            Some("Short."),
            &[],
            "x",
        );
        assert_eq!(with_id["id"], "tone");
        assert!(with_id["binding"].is_null());
    }

    #[test]
    fn an_edit_changes_only_the_fields_given() {
        let current =
            json!({ "title": "Old", "description": "d", "tags": ["x"], "binding": "plaza" });
        let kept = edit_body(&current, Some("New"), None, None, None);
        assert_eq!(
            kept,
            json!({ "title": "New", "description": "d", "tags": ["x"], "binding": "plaza" })
        );
        let unbound = edit_body(&current, None, None, Some(&["y".to_string()]), Some(""));
        assert_eq!(unbound["title"], "Old");
        assert_eq!(unbound["tags"], json!(["y"]));
        assert!(unbound["binding"].is_null(), "an empty binding removes it");
    }

    #[test]
    fn the_version_publish_and_check_bodies_have_the_hubs_fields() {
        assert_eq!(
            version_body("b", None, 3),
            json!({ "body": "b", "note": "", "base_version": 3 })
        );
        assert_eq!(version_body("b", Some("why"), 1)["note"], "why");
        assert_eq!(publish_body(2), json!({ "version": 2 }));
        assert_eq!(
            check_body("agents_md", "x"),
            json!({ "kind": "agents_md", "body": "x" })
        );
    }

    #[test]
    fn a_finding_says_where_it_is() {
        assert_eq!(
            location(&json!({ "path": "", "line": 0 })),
            "whole document"
        );
        assert_eq!(location(&json!({ "path": "", "line": 4 })), "line 4");
        assert_eq!(
            location(&json!({ "path": "SKILL.md", "line": 0 })),
            "SKILL.md"
        );
        assert_eq!(
            location(&json!({ "path": "scripts/run.sh", "line": 12 })),
            "scripts/run.sh:12"
        );
    }

    #[test]
    fn a_body_file_is_read_whole_but_a_blank_one_is_refused() {
        let dir = std::env::temp_dir().join(format!("apz-cli-prompts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let full = dir.join("body.md");
        std::fs::write(&full, "# Title\n\n  indented line\n").unwrap();
        assert_eq!(read_text(&full).unwrap(), "# Title\n\n  indented line\n");
        let blank = dir.join("blank.md");
        std::fs::write(&blank, " \n\t\n").unwrap();
        assert!(read_text(&blank).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
