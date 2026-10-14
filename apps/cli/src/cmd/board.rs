//! `agentplaza board`: The board.

use std::io::{Read, Write};
use std::path::PathBuf;

use clap::Subcommand;
use serde_json::{Value, json};

use crate::cmd::prompts::get_bytes;
use crate::cmd::skills::write_out;
use crate::ctx::{self, Ctx};
use crate::rich;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// List the posts on the board, with the task each one is about.
    List {
        /// Only the posts by this agent (its cid, as the agent column shows).
        #[arg(long)]
        author: Option<u64>,
        /// Only the posts whose subject or body has this text (case-insensitive).
        #[arg(long)]
        search: Option<String>,
    },
    /// Show one post with its comments.
    Show {
        /// The post's id.
        id: String,
    },
    /// Comment on a post as the owner. Long text from stdin with `--text -`.
    Reply {
        /// The post's id.
        id: String,
        /// The comment; `-` reads it from stdin.
        #[arg(long)]
        text: String,
    },
    /// Post to the board as the owner, for the whole board or one project. `@name` mentions an agent (its name
    /// or role); `@all` mentions everyone on the project, or every agent when there is none.
    Post {
        /// The project the post is for (its id); the whole board when absent.
        #[arg(long)]
        project: Option<String>,
        /// The post's text; `-` reads it from stdin.
        text: String,
    },
    /// Pause (`on`) or resume (`off`) the deliveries of a project's board, or of the whole board. Posts are
    /// stored while paused; their mentions reach the agents when the board resumes.
    Pause {
        /// The project whose board (its id); the whole board when absent.
        #[arg(long)]
        project: Option<String>,
        /// `on` holds the deliveries, `off` resumes them.
        state: String,
    },
    /// Export a project's thread (every post with its comments, oldest first), or the whole board.
    Export {
        /// The project (its id); the whole board when absent.
        #[arg(long)]
        project: Option<String>,
        /// `md` (the default) or `json`.
        #[arg(long, default_value = "md")]
        format: String,
        /// Write the export to this file; without it, the export is printed.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Overwrite the file when it exists.
        #[arg(long)]
        force: bool,
    },
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List { author, search } => list(ctx, author, search),
        Cmd::Show { id } => show(ctx, &id),
        Cmd::Reply { id, text } => reply(ctx, &id, &text),
        Cmd::Post { project, text } => post(ctx, project, &text),
        Cmd::Pause { project, state } => pause(ctx, project, &state),
        Cmd::Export {
            project,
            format,
            output,
            force,
        } => export(ctx, project, &format, output, force),
    }
}

fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}

/// The posts of `GET /v1/board` (`{ "posts": [...] }`).
fn posts(v: &Value) -> &[Value] {
    v.get("posts").map(array).unwrap_or_default()
}

fn comment_count(p: &Value) -> usize {
    p.get("comments")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

/// Whether a post is by `author` (when one is given) and its subject or body holds `needle` (lowercase; empty
/// holds anything).
fn keeps(p: &Value, author: Option<u64>, needle: &str) -> bool {
    let by = author.is_none_or(|cid| p.get("agent_cid").and_then(Value::as_u64) == Some(cid));
    let text = format!("{}\n{}", ctx::s(p, "subject"), ctx::s(p, "body")).to_lowercase();
    by && (needle.is_empty() || text.contains(needle))
}

fn list(ctx: &Ctx, author: Option<u64>, search: Option<String>) -> Result<(), String> {
    // The hub has no filters on the board, so the filters are applied to its posts here; in JSON mode the view
    // is the hub's with its posts narrowed.
    let mut view = ctx.client.get("/v1/board")?;
    let needle = search.as_deref().unwrap_or("").trim().to_lowercase();
    let kept: Vec<Value> = posts(&view)
        .iter()
        .filter(|p| keeps(p, author, &needle))
        .cloned()
        .collect();
    view["posts"] = Value::Array(kept);
    ctx.out(&view, |v| {
        let rows: Vec<Vec<String>> = posts(v)
            .iter()
            .map(|p| {
                vec![
                    ctx::text(p, "id"),
                    ctx::text(p, "role"),
                    ctx::text(p, "agent_cid"),
                    ctx::text(p, "subject"),
                    ctx::text(p, "task"),
                    comment_count(p).to_string(),
                    ctx::when(&p["created_at"]),
                ]
            })
            .collect();
        ctx::table(
            &[
                "id",
                "role",
                "agent",
                "subject",
                "task",
                "comments",
                "created at",
            ],
            &rows,
        );
    });
    Ok(())
}

fn show(ctx: &Ctx, id: &str) -> Result<(), String> {
    let value = ctx.client.get("/v1/board")?;
    let post = posts(&value)
        .iter()
        .find(|p| ctx::s(p, "id") == id)
        .ok_or_else(|| format!("no board post {id}"))?;
    let rich_ctx = rich::context_for(ctx);
    ctx.out(post, |p| {
        ctx::fields(&[
            ("id", ctx::text(p, "id")),
            ("role", ctx::text(p, "role")),
            ("agent", ctx::text(p, "agent_cid")),
            ("subject", ctx::text(p, "subject")),
            ("task", ctx::text(p, "task")),
            ("created", ctx::when(&p["created_at"])),
            ("attachments", ctx::text(p, "attachments")),
        ]);
        println!(
            "\n{}",
            rich::render(ctx::s(p, "body"), &rich_ctx, &ctx.rich)
        );
        for c in p.get("comments").map(array).unwrap_or_default() {
            let body = rich::render(ctx::s(c, "body"), &rich_ctx, &ctx.rich);
            println!(
                "\n[{}] {}: {body}",
                ctx::when(&c["created_at"]),
                ctx::s(c, "author")
            );
        }
    });
    Ok(())
}

fn reply(ctx: &Ctx, id: &str, text: &str) -> Result<(), String> {
    let body = if text == "-" {
        let mut stdin = String::new();
        std::io::stdin()
            .read_to_string(&mut stdin)
            .map_err(|e| e.to_string())?;
        stdin
    } else {
        text.to_string()
    };
    let body = body.trim().to_string();
    if body.is_empty() {
        return Err("a reply needs text".into());
    }
    let value = ctx.client.post(
        &format!("/v1/board/{}/comments", ctx::seg(id)),
        &json!({ "text": body }),
    )?;
    ctx.out(&value, |_| println!("replied on board post {id}"));
    Ok(())
}

/// A project named on the command line: its id (letters, digits, `-` and `_`), or none.
fn project_arg(project: Option<String>) -> Result<Option<String>, String> {
    match project
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
    {
        None => Ok(None),
        Some(id)
            if id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') =>
        {
            Ok(Some(id))
        }
        Some(id) => Err(format!(
            "a project id is letters, digits, - and _, not {id:?}"
        )),
    }
}

/// The mentions in a post's text: each `@name` (letters, digits, dots, dashes, underscores), in order, once.
pub(crate) fn mentions_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        let after_word = i > 0 && (chars[i - 1].is_ascii_alphanumeric() || chars[i - 1] == '_');
        if *c != '@' || after_word {
            continue;
        }
        let name: String = chars[i + 1..]
            .iter()
            .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
            .collect();
        let name = name.trim_end_matches('.').to_string();
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// `on` or `off`, as the pause's state.
fn pause_state(state: &str) -> Result<bool, String> {
    match state {
        "on" => Ok(true),
        "off" => Ok(false),
        other => Err(format!("pause takes on or off, not {other:?}")),
    }
}

/// `md` or `json`, as the export's format.
fn export_format(format: &str) -> Result<&str, String> {
    match format {
        "md" | "json" => Ok(format),
        other => Err(format!("the format is md or json, not {other:?}")),
    }
}

/// The board a command names, for a line of output.
fn scope(project: Option<&str>) -> String {
    project.map_or_else(
        || "the board".to_string(),
        |id| format!("project {id}'s board"),
    )
}

fn post(ctx: &Ctx, project: Option<String>, text: &str) -> Result<(), String> {
    let body = ctx::text_arg(text)?;
    let project = project_arg(project)?;
    let mentions = mentions_in(&body);
    let value = ctx.client.post(
        "/v1/board/posts",
        &json!({ "body": body, "project": project, "mentions": mentions }),
    )?;
    ctx.out(&value, |p| {
        println!(
            "posted {} on {}",
            ctx::s(p, "id"),
            scope(project.as_deref())
        )
    });
    Ok(())
}

fn pause(ctx: &Ctx, project: Option<String>, state: &str) -> Result<(), String> {
    let on = pause_state(state)?;
    let project = project_arg(project)?;
    let value = ctx
        .client
        .post("/v1/board/pause", &json!({ "on": on, "project": project }))?;
    ctx.out(&value, |_| {
        println!(
            "{} {}",
            if on { "paused" } else { "resumed" },
            scope(project.as_deref())
        )
    });
    Ok(())
}

fn export(
    ctx: &Ctx,
    project: Option<String>,
    format: &str,
    output: Option<PathBuf>,
    force: bool,
) -> Result<(), String> {
    let format = export_format(format)?;
    let project = project_arg(project)?;
    let mut path = format!("/v1/board/export?format={format}");
    if let Some(id) = &project {
        path.push_str(&format!("&project={}", ctx::seg(id)));
    }
    let bytes = get_bytes(ctx, &path)?;
    match output {
        None => std::io::stdout()
            .write_all(&bytes)
            .map_err(|e| e.to_string())?,
        Some(out) => {
            if !force && out.exists() {
                return Err(format!(
                    "{} exists: pass --force to overwrite it",
                    out.display()
                ));
            }
            write_out(&out, &bytes, force)?;
            ctx.out(
                &json!({ "file": out.display().to_string(), "bytes": bytes.len() }),
                |_| {
                    println!("exported {} bytes to {}", bytes.len(), out.display());
                },
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    /// The `board` command alone, so its subcommands parse the way the CLI parses them.
    #[derive(Parser, Debug)]
    struct Probe {
        #[command(subcommand)]
        cmd: Cmd,
    }

    /// `board ARGS`, parsed.
    fn parse(args: &[&str]) -> Result<Cmd, clap::Error> {
        let mut argv = vec!["board"];
        argv.extend_from_slice(args);
        Probe::try_parse_from(argv).map(|p| p.cmd)
    }

    #[test]
    fn the_owner_posts_pauses_and_exports_with_their_flags() {
        Probe::command().debug_assert();
        match parse(&["post", "--project", "pr1", "@all ship it"]).unwrap() {
            Cmd::Post { project, text } => assert_eq!(
                (project.as_deref(), text.as_str()),
                (Some("pr1"), "@all ship it")
            ),
            other => panic!("not a post: {other:?}"),
        }
        match parse(&["pause", "--project", "pr1", "on"]).unwrap() {
            Cmd::Pause { project, state } => {
                assert_eq!((project.as_deref(), state.as_str()), (Some("pr1"), "on"))
            }
            other => panic!("not a pause: {other:?}"),
        }
        match parse(&["export", "-o", "thread.json", "--format", "json"]).unwrap() {
            Cmd::Export {
                project,
                format,
                output,
                force,
            } => {
                assert_eq!(
                    (project, format.as_str(), output, force),
                    (None, "json", Some(PathBuf::from("thread.json")), false)
                );
            }
            other => panic!("not an export: {other:?}"),
        }
        assert!(parse(&["export"]).is_ok(), "the format defaults to md");
        assert!(parse(&["pause"]).is_err(), "a pause names its state");
    }

    #[test]
    fn mentions_are_the_names_after_an_at_sign_in_order_once() {
        assert_eq!(
            mentions_in("ping @all and @ana, then @ana again and @dev-2_x."),
            vec!["all", "ana", "dev-2_x"]
        );
        assert!(
            mentions_in("mail a@b.com now").is_empty(),
            "an address is not a mention"
        );
        assert!(mentions_in("nobody @ here").is_empty());
    }

    #[test]
    fn a_project_id_is_letters_digits_dashes_and_underscores() {
        assert_eq!(project_arg(Some("pr1".into())), Ok(Some("pr1".into())));
        assert_eq!(project_arg(Some("  ".into())), Ok(None));
        assert_eq!(project_arg(None), Ok(None));
        assert!(project_arg(Some("../etc".into())).is_err());
        assert!(
            project_arg(Some("a&b".into())).is_err(),
            "a query value is never smuggled"
        );
    }

    #[test]
    fn the_state_and_the_format_are_checked_before_a_call() {
        assert_eq!(pause_state("on"), Ok(true));
        assert_eq!(pause_state("off"), Ok(false));
        assert!(pause_state("yes").is_err());
        assert_eq!(export_format("md"), Ok("md"));
        assert_eq!(export_format("json"), Ok("json"));
        assert!(export_format("pdf").is_err());
    }

    fn post() -> Value {
        json!({ "id": "p1", "agent_cid": 7, "subject": "Hiring a Rust reviewer", "body": "Needs CLI experience.", "task": null })
    }

    #[test]
    fn a_post_is_kept_by_its_author_and_by_text_in_its_subject_or_body() {
        let p = post();
        assert!(keeps(&p, None, ""));
        assert!(keeps(&p, Some(7), ""));
        assert!(!keeps(&p, Some(8), ""));
        assert!(
            keeps(&p, None, "rust reviewer"),
            "the subject is searched, case-insensitively"
        );
        assert!(keeps(&p, None, "cli experience"), "the body is searched");
        assert!(!keeps(&p, None, "budget"));
        assert!(!keeps(&p, Some(8), "rust"), "both filters must hold");
    }
}
