//! `agentplaza chat`: talk to the orchestrator in one of your conversations, where a project is talked into being. On a
//! terminal it is a line-oriented chat: the conversation so far, then each line you type goes to the orchestrator,
//! whose answer prints as it is written (the hub's `delta` events) and stays once it is in. The project draft the
//! orchestrator keeps in the conversation prints whenever its revision changes, and `/start` starts it at the revision
//! on screen: the hub opens the project, saves its settings and posts its tasks, all or nothing.
//! Off a terminal, a message given as arguments is sent and the answer printed, so a script can talk to the
//! orchestrator too.
//!
//! The routes are `/v1/owner/conversations…` (`crates/apid/src/conversations.rs` and `drafts.rs`). Each change to a
//! conversation comes as an `owner_conversation` event with its whole view; while an answer is awaited the
//! conversation is also read every few seconds, so a dropped events socket only slows the chat down. This file holds
//! the views and what the chat prints (the terminal UI's Chat tab draws the same), and the commands off a terminal;
//! the chat on a terminal is `repl`.

mod repl;

use std::collections::HashSet;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::projects::settings::pairs;
use super::tasks::title_of;
use crate::cmd::prompts::json_reply;
use crate::ctx::{self, Client, Ctx};
use crate::rich;

/// How often a message sent off a terminal checks for its answer.
const ANSWER_POLL: Duration = Duration::from_secs(1);
/// A file to attach can be large (the hub takes up to 100 MiB): its upload may take this long.
const ATTACH_TIMEOUT: Duration = Duration::from_secs(600);
/// The width of a draft's field labels.
const LABEL: usize = 8;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Go on with this conversation (`conv-<n>`, or its number; `--list` shows them). Without it, your first message
    /// opens a new one.
    #[arg(long, value_name = "ID")]
    pub conversation: Option<String>,
    /// List your conversations, newest first, and leave.
    #[arg(long, conflicts_with_all = ["conversation", "message"])]
    pub list: bool,
    /// With --list: the archived conversations too.
    #[arg(long, requires = "list")]
    pub archived: bool,
    /// Off a terminal: the seconds to wait for the answer (0 sends the message and does not wait).
    #[arg(long, default_value_t = 600, value_name = "SECONDS")]
    pub wait: u64,
    /// Attach a file to the message; repeat for more. The files are uploaded first, and the message carries them. With
    /// one, the answer prints and the command ends, even on a terminal.
    #[arg(long = "attach", value_name = "FILE")]
    pub attach: Vec<PathBuf>,
    /// The message; its words are joined with spaces, and `-` reads it from stdin. On a terminal the chat goes on
    /// after it; off one, the answer prints and the command ends.
    pub message: Vec<String>,
}

pub fn run(ctx: &Ctx, args: Args) -> Result<(), String> {
    if args.list {
        return list(ctx, args.archived);
    }
    let id = args
        .conversation
        .as_deref()
        .map(conversation_id)
        .transpose()?;
    let message = (!args.message.is_empty()).then(|| args.message.join(" "));
    if !args.attach.is_empty() {
        let attachments = attach_files(&ctx.client, &args.attach)?;
        let text = message
            .map(|m| ctx::text_arg(&m))
            .transpose()?
            .unwrap_or_default();
        return once(ctx, id.as_deref(), &text, args.wait, &attachments);
    }
    let from_stdin = message.as_deref() == Some("-");
    let terminal = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    if terminal && !ctx.json && !from_stdin {
        return repl::Repl::open(ctx, id)?.run(message);
    }
    match (message, id) {
        (Some(text), id) => once(ctx, id.as_deref(), &ctx::text_arg(&text)?, args.wait, &[]),
        (None, Some(id)) => show(ctx, &id),
        (None, None) => Err(
            "give the message: `agentplaza chat <message>`, or run it on a terminal to chat".into(),
        ),
    }
}

// ---- The hub's views ----------------------------------------------------------------------------------------------

fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}

/// The array at `key` of an object (empty when missing).
fn array_at<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).map(array).unwrap_or_default()
}

fn number(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// A conversation's id as the hub names it, `conv-<n>`, from `conv-<n>` or the bare number.
fn conversation_id(text: &str) -> Result<String, String> {
    let trimmed = text.trim();
    let digits = trimmed.strip_prefix("conv-").unwrap_or(trimmed);
    digits
        .parse::<u64>()
        .map(|n| format!("conv-{n}"))
        .map_err(|_| {
            format!(
                "`{text}` is a conversation id, like conv-3 (`agentplaza chat --list` shows them)"
            )
        })
}

fn path_of(id: &str) -> String {
    format!("/v1/owner/conversations/{}", ctx::seg(id))
}

/// Sends `text` to the orchestrator: in conversation `id`, or as the first message of a new conversation. The hub
/// answers with the conversation, the message queued.
fn send(client: &Client, id: Option<&str>, text: &str) -> Result<Value, String> {
    send_with(client, id, text, &[])
}

/// [`send`] with files attached: their ids, from `PUT /v1/files`.
fn send_with(
    client: &Client,
    id: Option<&str>,
    text: &str,
    attachments: &[String],
) -> Result<Value, String> {
    let mut body = json!({ "text": text });
    if !attachments.is_empty() {
        body["attachments"] = json!(attachments);
    }
    match id {
        Some(id) => client.post(&format!("{}/messages", path_of(id)), &body),
        None => client.post("/v1/owner/conversations", &body),
    }
}

/// Uploads each file (`PUT /v1/files?name=`, with its type from its extension) and returns the ids in order. Nothing
/// is sent until every file is in.
fn attach_files(client: &Client, paths: &[PathBuf]) -> Result<Vec<String>, String> {
    paths
        .iter()
        .map(|path| {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| format!("{}: the name is not text", path.display()))?;
            let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let mut target = client.target.clone();
            target.headers.push((
                "Content-Type".to_string(),
                content_type_of(path).to_string(),
            ));
            let url = format!("/v1/files?name={}", ctx::seg(name));
            let reply =
                apz_http::client::request(&target, "PUT", &url, Some(&bytes), ATTACH_TIMEOUT)
                    .map_err(|e| format!("{} {name}: {e}", client.base()))?;
            let file = json_reply(name, &reply)?;
            file["id"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("{name}: the hub gave no id for the file"))
        })
        .collect()
}

/// A file's content type from its extension; `application/octet-stream` when it is not one of the common ones.
fn content_type_of(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "txt" | "log" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "xml" => "application/xml",
        "yaml" | "yml" => "application/yaml",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "zip" => "application/zip",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
}

/// The work items of a conversation still waiting for an answer: the owner's messages and the plaza's notices that
/// queued one, with no answer of that work yet.
pub(crate) fn unanswered(view: &Value) -> Vec<u64> {
    let messages = array_at(view, "messages");
    let answered: HashSet<u64> = messages
        .iter()
        .filter(|m| ctx::s(m, "from") == "orchestrator")
        .map(|m| number(m, "work"))
        .collect();
    messages
        .iter()
        .filter(|m| ctx::s(m, "from") != "orchestrator")
        .map(|m| number(m, "work"))
        .filter(|work| *work != 0 && !answered.contains(work))
        .collect()
}

/// The orchestrator's answer to work item `work`, once it is in.
fn answer_to(view: &Value, work: u64) -> Option<&Value> {
    array_at(view, "messages")
        .iter()
        .find(|m| ctx::s(m, "from") == "orchestrator" && number(m, "work") == work)
}

/// The work item the owner's newest message queued.
fn last_owner_work(view: &Value) -> Option<u64> {
    array_at(view, "messages")
        .iter()
        .rev()
        .find(|m| ctx::s(m, "from") == "owner")
        .map(|m| number(m, "work"))
        .filter(|work| *work != 0)
}

/// The revision of the conversation's draft (0 when it has none).
fn draft_revision(view: &Value) -> u64 {
    view.get("draft").map_or(0, |d| number(d, "revision"))
}

/// The text a `delta` event carries: the model's answer as it is written (its thinking and tool calls carry none).
pub(crate) fn delta_text(data: &Value) -> Option<&str> {
    let delta = data.get("delta")?;
    (delta.get("type")?.as_str()? == "text")
        .then(|| delta.get("text")?.as_str())
        .flatten()
}

/// The orchestrator a new conversation talks to, as the hub picks it: of the agents of role `orchestrator`, a
/// connected one first, then the lowest cid.
fn pick_orchestrator(agents: &[Value]) -> Option<u64> {
    agents
        .iter()
        .filter(|a| ctx::s(a, "role") == "orchestrator")
        .filter_map(|a| Some((ctx::s(a, "status") == "offline", a.get("cid")?.as_u64()?)))
        .min()
        .map(|(_, cid)| cid)
}

/// Whether two texts say the same, whatever the spacing: a streamed answer and the answer the hub kept.
fn same_text(a: &str, b: &str) -> bool {
    a.split_whitespace().eq(b.split_whitespace())
}

/// A message's identity in the chat: who it is from, when, and its text (the owner's message gets its work item only
/// once it is queued, so the work is left out).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seen {
    from: String,
    at_ms: u64,
    text: String,
}

impl Seen {
    fn of(m: &Value) -> Seen {
        Seen {
            from: ctx::s(m, "from").to_string(),
            at_ms: number(m, "at_ms"),
            text: ctx::s(m, "text").to_string(),
        }
    }

    fn is(&self, m: &Value) -> bool {
        number(m, "at_ms") == self.at_ms
            && ctx::s(m, "from") == self.from
            && ctx::s(m, "text") == self.text
    }
}

/// Where the messages not printed yet start in `messages`: after the last one printed (`last`), or at 0 when none
/// was. When the last one printed is not listed, a list at least as long as the one printed lost its oldest (the hub
/// keeps a conversation's newest 1000), and the ones past the count printed are new; a shorter one is an older view of
/// the conversation, and None says to skip it.
fn fresh(messages: &[Value], last: Option<&Seen>, printed: usize) -> Option<usize> {
    let Some(last) = last else {
        return Some(0);
    };
    if let Some(at) = messages.iter().rposition(|m| last.is(m)) {
        return Some(at + 1);
    }
    (messages.len() >= printed).then_some(printed.min(messages.len()))
}

// ---- What the chat prints -----------------------------------------------------------------------------------------

/// Who a message is from, as the chat names them.
pub(crate) fn speaker(from: &str) -> &str {
    match from {
        "owner" => "you",
        "" => "?",
        other => other,
    }
}

/// The line over a message: when it was written and who it is from; an answer whose work did not end done says how
/// it ended.
fn head(m: &Value, at: &str) -> String {
    let who = speaker(ctx::s(m, "from"));
    match ctx::s(m, "status") {
        "" | "done" => format!("[{at}] {who}"),
        status => format!("[{at}] {who} ({status})"),
    }
}

/// One message as lines: its head, its text (references drawn as labels on a terminal), and a blank line.
fn message_lines(m: &Value, rich_ctx: &apz_rich::Ctx, mode: &rich::Mode) -> Vec<String> {
    let mut lines = vec![head(m, &ctx::when(&m["at_ms"]))];
    let text = rich::render(ctx::s(m, "text"), rich_ctx, mode);
    lines.extend(text.lines().map(str::to_string));
    lines.push(String::new());
    lines
}

/// The line a conversation opens with: its id and title, the orchestrator it talks to, its project once it started
/// one, and whether it is archived.
fn header(view: &Value) -> String {
    let mut line = format!(
        "Conversation {}: {} (with the orchestrator, agent {})",
        ctx::s(view, "id"),
        ctx::s(view, "title"),
        ctx::text(view, "cid")
    );
    if let Some(project) = view.get("project").and_then(Value::as_str) {
        line.push_str(&format!(" · project {project}"));
    }
    if view.get("archived").and_then(Value::as_bool) == Some(true) {
        line.push_str(" · archived");
    }
    line
}

/// Cuts `text` to `max` characters; the last one becomes an ellipsis when it was cut.
fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        text.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
    }
}

/// A field of a draft: its label, then its text, each further line of the text under the first.
fn field(lines: &mut Vec<String>, label: &str, text: &str) {
    let text = if text.trim().is_empty() { "-" } else { text };
    for (i, line) in text.lines().enumerate() {
        let label = if i == 0 { label } else { "" };
        lines.push(format!("{label:>LABEL$}  {line}").trim_end().to_string());
    }
}

/// A draft's tasks as aligned rows: the key, the role (`any role` for none), the goal's first line, and the keys of
/// the tasks it runs after.
fn task_rows(tasks: &[Value]) -> Vec<String> {
    if tasks.is_empty() {
        return vec!["none yet".to_string()];
    }
    let cells: Vec<[String; 4]> = tasks
        .iter()
        .map(|t| {
            let after: Vec<&str> = array_at(t, "after")
                .iter()
                .filter_map(Value::as_str)
                .collect();
            [
                ctx::s(t, "key").trim().to_string(),
                match ctx::s(t, "role") {
                    "" => "any role".to_string(),
                    role => role.to_string(),
                },
                cut(title_of(ctx::s(t, "goal")), 56),
                if after.is_empty() {
                    String::new()
                } else {
                    format!("after {}", after.join(", "))
                },
            ]
        })
        .collect();
    let width = |i: usize| {
        cells
            .iter()
            .map(|c| c[i].chars().count())
            .max()
            .unwrap_or(0)
    };
    let (key, role, title) = (width(0), width(1), width(2));
    cells
        .iter()
        .map(|c| {
            format!(
                "{:<key$}  {:<role$}  {:<title$}  {}",
                c[0], c[1], c[2], c[3]
            )
            .trim_end()
            .to_string()
        })
        .collect()
}

/// A project draft as lines (`GET …/draft`, or a conversation's `draft`): its revision, title and kind, and who edited
/// it last; then the goal, where the work goes, the tasks with what they run after, and the settings; then
/// what still stops Start, or that nothing does. A started draft says so instead, with its project when it is known.
pub(crate) fn draft_lines(draft: &Value, project: Option<&str>) -> Vec<String> {
    let by = match ctx::s(draft, "updated_by") {
        "owner" => "you",
        "orchestrator" => "the orchestrator",
        "" => "-",
        other => other,
    };
    let kind = match ctx::s(draft, "kind") {
        "" => "code",
        kind => kind,
    };
    let mut lines = vec![format!(
        "Draft r{}: {} ({kind}), last edited by {by}",
        ctx::text(draft, "revision"),
        ctx::s(draft, "title")
    )];
    field(&mut lines, "goal", ctx::s(draft, "goal"));
    if let Some(repo) = draft
        .get("repo")
        .and_then(Value::as_str)
        .filter(|r| !r.trim().is_empty())
    {
        field(&mut lines, "repo", repo);
    }
    field(
        &mut lines,
        "tasks",
        &task_rows(array_at(draft, "tasks")).join("\n"),
    );
    let settings: Vec<String> = pairs(draft.get("settings").unwrap_or(&Value::Null))
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    if !settings.is_empty() {
        field(&mut lines, "settings", &settings.join(" · "));
    }
    let problems: Vec<&str> = array_at(draft, "problems")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    if draft.get("started_ms").is_some_and(|ms| !ms.is_null()) {
        lines.push(match project {
            Some(project) => format!("Started as project {project}."),
            None => "Started.".to_string(),
        });
    } else if problems.is_empty() {
        lines.push("Nothing stops Start.".to_string());
    } else {
        lines.push("Before it can start:".to_string());
        lines.extend(problems.iter().map(|p| format!("  - {p}")));
    }
    lines
}

/// Whether a draft can be started now: it is not started, and nothing stops it.
pub(crate) fn startable(draft: &Value) -> bool {
    draft.get("started_ms").is_none_or(Value::is_null) && array_at(draft, "problems").is_empty()
}

fn plural(n: u64, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// What Start did, in a sentence (`started` of its answer): the project it opened and the tasks it posted.
fn started_line(started: &Value) -> String {
    let tasks: Vec<&str> = array_at(started, "tasks")
        .iter()
        .map(|t| ctx::s(t, "id"))
        .collect();
    format!(
        "Started project {}: {} posted ({}).",
        ctx::s(started, "project"),
        plural(tasks.len() as u64, "task"),
        tasks.join(", ")
    )
}

/// One conversation of the list as a row: its id, title (an archived one says so), project, the draft's revision, how
/// many messages it holds and how many wait for an answer, and when it last changed.
fn summary_row(c: &Value) -> Vec<String> {
    let mut title = ctx::s(c, "title").to_string();
    if c.get("archived").and_then(Value::as_bool) == Some(true) {
        title.push_str(" (archived)");
    }
    let draft = c
        .pointer("/draft/revision")
        .and_then(Value::as_u64)
        .map_or_else(|| "-".to_string(), |r| format!("r{r}"));
    let project = match ctx::s(c, "project") {
        "" => "-".to_string(),
        project => project.to_string(),
    };
    vec![
        ctx::s(c, "id").to_string(),
        title,
        project,
        draft,
        ctx::text(c, "messages"),
        ctx::text(c, "pending"),
        ctx::ago(&c["updated_ms"]),
    ]
}

// ---- Off a terminal -----------------------------------------------------------------------------------------------

/// `chat --list`: the conversations, newest first (the archived ones too with `--archived`).
fn list(ctx: &Ctx, archived: bool) -> Result<(), String> {
    let path = if archived {
        "/v1/owner/conversations?archived=true"
    } else {
        "/v1/owner/conversations"
    };
    let all = ctx.client.get(path)?;
    ctx.out(&all, |v| {
        let rows: Vec<Vec<String>> = array(v).iter().map(summary_row).collect();
        ctx::table(
            &[
                "id", "title", "project", "draft", "messages", "waiting", "updated",
            ],
            &rows,
        );
    });
    Ok(())
}

/// No message, off a terminal: the conversation as it is, and its draft.
fn show(ctx: &Ctx, id: &str) -> Result<(), String> {
    let view = ctx
        .client
        .get(&path_of(id))
        .map_err(|e| format!("{id}: {e}"))?;
    let rich_ctx = rich::context_for(ctx);
    ctx.out(&view, |v| {
        println!("{}\n", header(v));
        for m in array_at(v, "messages") {
            for line in message_lines(m, &rich_ctx, &ctx.rich) {
                println!("{line}");
            }
        }
        if let Some(draft) = v.get("draft").filter(|d| d.is_object()) {
            let project = v.get("project").and_then(Value::as_str);
            for line in draft_lines(draft, project) {
                println!("{line}");
            }
        }
    });
    Ok(())
}

/// A message off a terminal: sends `text` (in conversation `id`, or opening a new one), waits up to `wait` seconds for
/// the orchestrator's answer to it, and prints the answer (`--json`: the conversation), then the draft when the answer
/// changed it. A new conversation's id goes to stderr, so a script can go on with it. An answer whose work failed or
/// stopped is an error with its reason.
fn once(
    ctx: &Ctx,
    id: Option<&str>,
    text: &str,
    wait: u64,
    attachments: &[String],
) -> Result<(), String> {
    let sent = send_with(&ctx.client, id, text, attachments)?;
    let conversation = ctx::s(&sent, "id").to_string();
    if id.is_none() && !ctx.json {
        eprintln!(
            "conversation {conversation}: go on with `agentplaza chat --conversation {conversation}`"
        );
    }
    let work = last_owner_work(&sent)
        .ok_or_else(|| format!("{conversation}: the hub did not queue the message"))?;
    if wait == 0 {
        ctx.out(&sent, |_| println!("sent to {conversation} (work {work})"));
        return Ok(());
    }
    let revision = draft_revision(&sent);
    let deadline = Instant::now() + Duration::from_secs(wait);
    let mut view = sent;
    let mut failing: Option<String> = None;
    let answer = loop {
        if let Some(answer) = answer_to(&view, work) {
            break answer.clone();
        }
        if Instant::now() >= deadline {
            let why = failing
                .map(|e| format!(" (the last read said: {e})"))
                .unwrap_or_default();
            return Err(format!(
                "no answer in {wait} s{why}; it will come in {conversation}: `agentplaza chat --conversation {conversation}` shows it"
            ));
        }
        thread::sleep(ANSWER_POLL);
        match ctx.client.get(&path_of(&conversation)) {
            Ok(next) => view = next,
            Err(e) => failing = Some(e),
        }
    };
    let status = ctx::s(&answer, "status");
    let text = ctx::s(&answer, "text");
    if !matches!(status, "" | "done") {
        if ctx.json {
            ctx.out(&view, |_| {});
        }
        return Err(format!("the orchestrator's work on it {status}: {text}"));
    }
    let rich_ctx = rich::context_for(ctx);
    ctx.out(&view, |v| {
        println!("{}", rich::render(text, &rich_ctx, &ctx.rich));
        if let Some(draft) = v
            .get("draft")
            .filter(|d| d.is_object() && draft_revision(v) != revision)
        {
            println!();
            for line in draft_lines(draft, v.get("project").and_then(Value::as_str)) {
                println!("{line}");
            }
            if startable(draft) {
                println!(
                    "Start it on a terminal: `agentplaza chat --conversation {conversation}`, then /start."
                );
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(from: &str, text: &str, at_ms: u64, work: u64) -> Value {
        json!({ "from": from, "text": text, "at_ms": at_ms, "work": work })
    }

    /// A conversation as `GET /v1/owner/conversations/{id}` gives it: the owner asked twice, and the orchestrator
    /// answered the first.
    fn conversation() -> Value {
        json!({
            "id": "conv-3", "title": "Release 0.9", "cid": 3, "updated_ms": 30,
            "messages": [
                message("owner", "Fix the flaky tests", 10, 41),
                { "from": "orchestrator", "text": "Which repo?", "at_ms": 20, "work": 41, "status": "done" },
                message("owner", "agentplaza/main", 30, 42)
            ],
            "works": [41, 42], "pending": [42]
        })
    }

    #[test]
    fn an_attached_file_goes_up_with_its_type_from_its_extension() {
        assert_eq!(
            content_type_of(Path::new("notes/Plan.PDF")),
            "application/pdf"
        );
        assert_eq!(content_type_of(Path::new("shot.png")), "image/png");
        assert_eq!(
            content_type_of(Path::new("data")),
            "application/octet-stream"
        );
        assert_eq!(
            content_type_of(Path::new("deck.pptx")),
            "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        );
    }

    #[test]
    fn a_conversation_id_is_conv_and_its_number() {
        assert_eq!(conversation_id("conv-3").unwrap(), "conv-3");
        assert_eq!(conversation_id(" 12 ").unwrap(), "conv-12");
        assert!(conversation_id("conv-x").is_err());
        assert!(conversation_id("pr3").is_err());
    }

    #[test]
    fn the_answers_awaited_are_the_messages_with_no_answer_of_their_work() {
        let view = conversation();
        assert_eq!(unanswered(&view), [42]);
        assert_eq!(last_owner_work(&view), Some(42));
        assert_eq!(
            answer_to(&view, 41).map(|m| ctx::s(m, "text")),
            Some("Which repo?")
        );
        assert!(answer_to(&view, 42).is_none());
        // A plaza notice queues work too, and its answer is awaited the same way.
        let notice = json!({ "messages": [message("plaza", "[plaza] Task t4 ended", 5, 50)] });
        assert_eq!(unanswered(&notice), [50]);
        // An owner's message not queued yet (work 0) waits on nothing.
        assert!(unanswered(&json!({ "messages": [message("owner", "hi", 1, 0)] })).is_empty());
    }

    #[test]
    fn only_the_messages_after_the_last_printed_are_new() {
        let view = conversation();
        let messages = array_at(&view, "messages");
        assert_eq!(fresh(messages, None, 0), Some(0));
        let first = Seen::of(&messages[0]);
        assert_eq!(fresh(messages, Some(&first), 1), Some(1));
        let last = Seen::of(&messages[2]);
        assert_eq!(fresh(messages, Some(&last), 3), Some(3));
        // An owner's message that got its work item since is the same message.
        let queued = message("owner", "Fix the flaky tests", 10, 0);
        assert_eq!(fresh(messages, Some(&Seen::of(&queued)), 1), Some(1));
        // An older view, which does not hold the last message printed, is skipped.
        let older = &messages[..1];
        assert_eq!(fresh(older, Some(&last), 3), None);
    }

    #[test]
    fn a_streamed_answer_is_the_kept_one_whatever_the_spacing() {
        assert!(same_text("Which repo?\n", " Which  repo?"));
        assert!(!same_text("Which repo?", "Which repository?"));
        let text =
            json!({ "cid": 3, "work": 42, "req": "r1", "delta": { "type": "text", "text": "Wh" } });
        assert_eq!(delta_text(&text), Some("Wh"));
        let thinking =
            json!({ "cid": 3, "work": 42, "delta": { "type": "thinking", "text": "hm" } });
        assert_eq!(delta_text(&thinking), None);
        let tool = json!({ "cid": 3, "work": 42, "delta": { "type": "tool_use", "name": "bash" } });
        assert_eq!(delta_text(&tool), None);
    }

    #[test]
    fn the_orchestrator_is_a_connected_one_first() {
        let agents = json!([
            { "cid": 3, "role": "orchestrator", "status": "offline" },
            { "cid": 9, "role": "orchestrator", "status": "idle" },
            { "cid": 5, "role": "builder", "status": "idle" }
        ]);
        assert_eq!(pick_orchestrator(array(&agents)), Some(9));
        assert_eq!(pick_orchestrator(&[]), None);
    }

    #[test]
    fn a_message_head_says_who_wrote_it_and_how_its_work_ended() {
        let view = conversation();
        let messages = array_at(&view, "messages");
        assert_eq!(head(&messages[0], "12:00"), "[12:00] you");
        assert_eq!(head(&messages[1], "12:01"), "[12:01] orchestrator");
        let failed =
            json!({ "from": "orchestrator", "text": "agent went offline", "status": "failed" });
        assert_eq!(head(&failed, "12:02"), "[12:02] orchestrator (failed)");
        assert_eq!(
            header(&json!({ "id": "conv-3", "title": "Release 0.9", "cid": 3, "project": "pr3" })),
            "Conversation conv-3: Release 0.9 (with the orchestrator, agent 3) · project pr3"
        );
    }

    /// A draft as `GET …/draft` gives it.
    fn draft() -> Value {
        json!({
            "revision": 3, "title": "Release 0.9", "goal": "Fix the flaky scheduler tests\nand tag 0.9.",
            "kind": "code", "repo": "agentplaza/main",
            "tasks": [
                { "key": "1", "goal": "Survey the flaky tests", "role": "qa" },
                { "key": "2", "goal": "Fix the double-charge\nin detail", "role": "builder", "after": ["1"] },
                { "key": "3", "goal": "Tag 0.9", "after": ["1", "2"] }
            ],
            "settings": { "effort": "high", "role_models": { "qa": "openai/gpt-6" } },
            "updated_by": "orchestrator", "updated_ms": 50,
            "problems": ["task 3 is for a release, and no review follows it"]
        })
    }

    #[test]
    fn a_draft_prints_its_plan_and_what_stops_start() {
        assert_eq!(
            draft_lines(&draft(), None),
            [
                "Draft r3: Release 0.9 (code), last edited by the orchestrator",
                "    goal  Fix the flaky scheduler tests",
                "          and tag 0.9.",
                "    repo  agentplaza/main",
                "   tasks  1  qa        Survey the flaky tests",
                "          2  builder   Fix the double-charge   after 1",
                "          3  any role  Tag 0.9                 after 1, 2",
                "settings  effort=high · role_models.qa=openai/gpt-6",
                "Before it can start:",
                "  - task 3 is for a release, and no review follows it",
            ]
        );
        assert!(!startable(&draft()));
    }

    #[test]
    fn a_draft_with_nothing_in_the_way_can_start_and_a_started_one_names_its_project() {
        let mut ready = draft();
        ready["problems"] = json!([]);
        assert_eq!(
            draft_lines(&ready, None).last().map(String::as_str),
            Some("Nothing stops Start.")
        );
        assert!(startable(&ready));
        let mut started = ready.clone();
        started["started_ms"] = json!(70);
        assert_eq!(
            draft_lines(&started, Some("pr3"))
                .last()
                .map(String::as_str),
            Some("Started as project pr3.")
        );
        assert!(!startable(&started));
        let bare = json!({ "revision": 1, "title": "Notes", "goal": "Write them", "updated_by": "owner", "problems": ["the draft has no tasks yet"] });
        assert_eq!(
            draft_lines(&bare, None),
            [
                "Draft r1: Notes (code), last edited by you",
                "    goal  Write them",
                "   tasks  none yet",
                "Before it can start:",
                "  - the draft has no tasks yet",
            ]
        );
    }

    #[test]
    fn start_says_what_it_opened_and_posted() {
        let started = json!({
            "project": "pr3",
            "tasks": [{ "key": "1", "id": "t12" }, { "key": "2", "id": "t13" }]
        });
        assert_eq!(
            started_line(&started),
            "Started project pr3: 2 tasks posted (t12, t13)."
        );
        let alone = json!({ "project": "pr4", "tasks": [{ "id": "t20" }] });
        assert_eq!(
            started_line(&alone),
            "Started project pr4: 1 task posted (t20)."
        );
    }

    #[test]
    fn the_list_shows_each_conversations_project_draft_and_waiting_answers() {
        let summary = json!({
            "id": "conv-3", "title": "Release 0.9", "cid": 3, "archived": true, "project": null,
            "draft": { "revision": 3, "title": "Release 0.9" }, "messages": 4, "pending": 1, "updated_ms": null
        });
        assert_eq!(
            summary_row(&summary),
            ["conv-3", "Release 0.9 (archived)", "-", "r3", "4", "1", "—"]
                .map(String::from)
                .to_vec()
        );
    }
}
