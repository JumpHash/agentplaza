//! The chat on a terminal (`agentplaza chat` with a terminal for its input and output): the conversation so far, then
//! a line-oriented loop. A thread reads the lines typed and another holds the events socket; the loop prints what
//! they bring (the answers as they stream, the messages and drafts as they change) around the prompt, and reads the
//! conversation again every few seconds while an answer is awaited.

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use apz_http::client::Target;
use apz_http::ws::{self, Limits, Message as Frame};
use serde_json::{Value, json};

use super::{
    Seen, array_at, delta_text, draft_lines, draft_revision, fresh, header, message_lines, number,
    path_of, pick_orchestrator, same_text, send, startable, started_line, unanswered,
};
use crate::ctx::{self, Ctx};
use crate::rich;
use crate::tui::context_bar::{self, Context, Owner};

/// The messages printed when a conversation opens; `/history` prints all of them.
const OPENING_MESSAGES: usize = 20;
/// While an answer is awaited, or the events socket is down, how often the conversation is read again.
const POLL_EVERY: Duration = Duration::from_secs(5);
/// How often the chat looks at its input and its events.
const TICK: Duration = Duration::from_millis(250);
/// How long a first message given as arguments waits for the events socket, so its answer streams from the start.
const LINK_WAIT: Duration = Duration::from_millis(1500);
/// How long Start may take: it opens the project, posts its tasks and asks the nodes for the new agents.
const START_TIMEOUT: Duration = Duration::from_secs(120);
/// The prompt of the chat's input line.
const PROMPT: &str = "› ";
/// What `/help` prints.
const HELP: &str = "Type a message and press Enter: it goes to the orchestrator. /draft shows the project draft, \
/start starts it at the revision shown, /history prints the whole conversation, /rename TITLE renames it, /archive \
and /unarchive put it away and back, /model provider/model[@effort]|default sets the model it runs at (max, high, \
medium or low for the effort), /context prints how full its context is, /quit (or Ctrl-D) leaves. Begin a message \
with // to send one that starts with /.";
/// What a command says before there is a conversation.
const NO_CONVERSATION: &str = "No conversation yet: your first message opens one.";

/// What a line typed in the chat asks for.
#[derive(Debug, PartialEq, Eq)]
enum Input {
    /// A message for the orchestrator.
    Say(String),
    Draft,
    Start,
    History,
    /// A new title for the conversation.
    Rename(String),
    /// Archive (true) or unarchive (false) the conversation.
    Archive(bool),
    /// The model the conversation runs at: `provider/model[@effort]`, or `default` (the argument as typed).
    Model(String),
    /// The conversation's context: how full it is, and its breakdown.
    Context,
    Help,
    Quit,
    /// A blank line.
    Nothing,
    /// A command the chat does not know.
    Unknown(String),
}

/// A line as the chat reads it: a `/command`, or a message (`//` begins one that starts with a slash).
fn parse_line(line: &str) -> Input {
    let text = line.trim();
    if text.is_empty() {
        return Input::Nothing;
    }
    let Some(command) = text.strip_prefix('/') else {
        return Input::Say(text.to_string());
    };
    if command.starts_with('/') {
        return Input::Say(command.to_string());
    }
    let (name, rest) = command
        .split_once(char::is_whitespace)
        .unwrap_or((command, ""));
    match name {
        "draft" => Input::Draft,
        "start" => Input::Start,
        "history" => Input::History,
        "rename" => Input::Rename(rest.trim().to_string()),
        "archive" => Input::Archive(true),
        "unarchive" => Input::Archive(false),
        "model" => Input::Model(rest.trim().to_string()),
        "context" => Input::Context,
        "help" | "?" => Input::Help,
        "quit" | "exit" | "q" => Input::Quit,
        other => Input::Unknown(other.to_string()),
    }
}

/// A frame the chat reads (`owner_conversation`, or a `delta` of the orchestrator `cid`), as its kind and data; None
/// for the rest.
fn wanted(text: &str, cid: u64) -> Option<(String, Value)> {
    let frame: Value = serde_json::from_str(text).ok()?;
    let kind = frame.get("event")?.as_str()?;
    let data = frame.get("data").cloned().unwrap_or(Value::Null);
    let keep = match kind {
        "owner_conversation" => true,
        "delta" => cid != 0 && data.get("cid").and_then(Value::as_u64) == Some(cid),
        _ => false,
    };
    keep.then(|| (kind.to_string(), data))
}

/// What the chat's threads send it.
enum Msg {
    /// A line typed.
    Line(String),
    /// The input ended (Ctrl-D).
    End,
    /// A hub event the chat reads (see `wanted`).
    Event(String, Value),
    /// The events socket went up or down.
    Link(bool),
}

/// The input's lines, until it ends.
fn read_lines(tx: Sender<Msg>) {
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if tx.send(Msg::Line(line)).is_err() {
            return;
        }
    }
    let _ = tx.send(Msg::End);
}

/// The events socket of the chat: forwards the `owner_conversation` events, and the `delta` events of the orchestrator
/// `watched` names. Reconnects with a backoff up to 15 s, and says when the socket goes up or down.
fn follow(target: Target, watched: Arc<AtomicU64>, tx: Sender<Msg>) {
    let mut wait = Duration::from_millis(500);
    loop {
        if let Ok(mut socket) = ws::connect(
            &target,
            "/v1/ws",
            Some("apz.events.v1"),
            Duration::from_secs(10),
            Limits::default(),
        ) {
            wait = Duration::from_millis(500);
            if tx.send(Msg::Link(true)).is_err() {
                return;
            }
            loop {
                match socket.recv(Duration::from_secs(1)) {
                    Ok(Some(Frame::Text(text))) => {
                        if let Some((kind, data)) = wanted(&text, watched.load(Ordering::Relaxed))
                            && tx.send(Msg::Event(kind, data)).is_err()
                        {
                            return;
                        }
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        }
        if tx.send(Msg::Link(false)).is_err() {
            return;
        }
        thread::sleep(wait);
        wait = (wait * 2).min(Duration::from_secs(15));
    }
}

fn flush() {
    let _ = std::io::stdout().flush();
}

/// Clears the terminal line the cursor is on: the prompt, and what was typed after it. A terminal that takes no
/// escape codes (`dumb`, or a legacy Windows console) gets a line break instead.
fn clear_line() {
    let var = |name: &str| std::env::var(name).ok();
    let legacy = cfg!(windows) && var("WT_SESSION").is_none() && var("TERM_PROGRAM").is_none();
    if legacy || var("TERM").as_deref() == Some("dumb") {
        println!();
    } else {
        print!("\r\x1b[2K");
    }
    flush();
}

/// The time now as the chat prints it.
fn now() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    ctx::when(&json!(ms))
}

/// The answer being streamed: its work item, the model call its text comes from, that call's text so far, and whether
/// its line is open on screen (a block printed in between closes it, and the next text gets a head again).
struct Live {
    work: u64,
    req: String,
    text: String,
    open: bool,
}

/// The chat on a terminal.
pub(super) struct Repl<'a> {
    ctx: &'a Ctx,
    rich: apz_rich::Ctx,
    /// The conversation as the hub last showed it; None until the first message opens one.
    view: Option<Value>,
    /// The work items the conversation waits on (`unanswered`), whose `delta` events stream.
    waiting: Vec<u64>,
    /// The last message printed, and how many the conversation held then.
    last: Option<Seen>,
    printed: usize,
    /// The draft revision on screen (0: none was shown); `/start` starts this one.
    revision: u64,
    /// The text just sent: the owner's message that echoes it is not printed again.
    echo: Option<String>,
    live: Option<Live>,
    /// Whether the prompt is on screen with nothing printed after it.
    prompt_open: bool,
    /// Whether the events socket is up.
    link: bool,
    /// The orchestrator whose `delta` events the socket forwards.
    watched: Arc<AtomicU64>,
    read_at: Instant,
    /// The last error a background read hit, printed once until it changes.
    failing: Option<String>,
}

impl<'a> Repl<'a> {
    /// The chat on conversation `id`, or on a new one that the first message opens. The agents are read once, when
    /// a new conversation needs the orchestrator it talks to (whose answers stream).
    pub(super) fn open(ctx: &'a Ctx, id: Option<String>) -> Result<Repl<'a>, String> {
        let view = match &id {
            Some(id) => Some(
                ctx.client
                    .get(&path_of(id))
                    .map_err(|e| format!("{id}: {e}"))?,
            ),
            None => None,
        };
        let agents = if view.is_none() {
            ctx.client
                .get("/v1/agents")
                .ok()
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let rich = if rich::draws_labels(ctx.json, &ctx.rich) {
            rich::from_hub(Some(ctx.client.base()))
        } else {
            apz_rich::Ctx::default()
        };
        let cid = view
            .as_ref()
            .and_then(|v| v.get("cid"))
            .and_then(Value::as_u64)
            .or_else(|| pick_orchestrator(&agents))
            .unwrap_or(0);
        Ok(Repl {
            ctx,
            rich,
            view,
            waiting: Vec::new(),
            last: None,
            printed: 0,
            revision: 0,
            echo: None,
            live: None,
            prompt_open: false,
            link: false,
            watched: Arc::new(AtomicU64::new(cid)),
            read_at: Instant::now(),
            failing: None,
        })
    }

    /// The chat until `/quit` or the end of the input: the conversation so far, then `first` when one was given.
    pub(super) fn run(mut self, first: Option<String>) -> Result<(), String> {
        let (tx, rx) = mpsc::channel();
        let lines = tx.clone();
        thread::spawn(move || read_lines(lines));
        let target = self.ctx.client.target.clone();
        let watched = Arc::clone(&self.watched);
        thread::spawn(move || follow(target, watched, tx));
        self.opening();
        // What came in while a first message waited for the socket is taken before anything newer.
        let mut early = VecDeque::new();
        match first {
            Some(text) => {
                let until = Instant::now() + LINK_WAIT;
                while !self.link && Instant::now() < until {
                    match rx.recv_timeout(TICK) {
                        Ok(Msg::Link(up)) => self.link = up,
                        Ok(other) => early.push_back(other),
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                self.say(&text, false);
            }
            None => self.prompt(),
        }
        loop {
            let next = match early.pop_front() {
                Some(msg) => Ok(msg),
                None => rx.recv_timeout(TICK),
            };
            match next {
                Ok(Msg::Line(line)) => {
                    // Enter ended the prompt's line.
                    self.prompt_open = false;
                    if !self.on_line(&line) {
                        return Ok(());
                    }
                }
                Ok(Msg::End) | Err(RecvTimeoutError::Disconnected) => {
                    println!();
                    return Ok(());
                }
                Ok(Msg::Event(kind, data)) => self.on_event(&kind, data),
                Ok(Msg::Link(up)) => {
                    let back = up && !self.link;
                    self.link = up;
                    // What the events carried while they were down is read once they are back.
                    if back {
                        self.refresh();
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            if self.due() {
                self.refresh();
            }
        }
    }

    /// One line typed; false when it says to leave.
    fn on_line(&mut self, line: &str) -> bool {
        match parse_line(line) {
            Input::Quit => return false,
            Input::Nothing => self.prompt(),
            Input::Say(text) => self.say(&text, true),
            Input::Draft => self.draft(),
            Input::Start => self.start(),
            Input::History => self.history(),
            Input::Rename(title) if title.is_empty() => {
                self.note("Give the new title: /rename TITLE");
            }
            Input::Rename(title) => {
                self.patch(json!({ "title": title }), &format!("Renamed: {title}"));
            }
            Input::Archive(true) => self.patch(
                json!({ "archived": true }),
                "Archived: it takes no new messages until you /unarchive it.",
            ),
            Input::Archive(false) => self.patch(json!({ "archived": false }), "Unarchived."),
            Input::Model(text) => self.model(&text),
            Input::Context => self.context(),
            Input::Help => self.note(HELP),
            Input::Unknown(command) => {
                self.note(&format!(
                    "/{command} is not a chat command: /help lists them"
                ));
            }
        }
        true
    }

    fn id(&self) -> Option<String> {
        self.view
            .as_ref()
            .map(|v| ctx::s(v, "id").to_string())
            .filter(|id| !id.is_empty())
    }

    // ---- Output ---------------------------------------------------------------------------------------------------

    fn prompt(&mut self) {
        if !self.prompt_open {
            print!("{PROMPT}");
            flush();
            self.prompt_open = true;
        }
    }

    /// Makes room for a block: the prompt's line is cleared, and a streamed answer's open line is ended.
    fn interrupt(&mut self) {
        if self.prompt_open {
            clear_line();
            self.prompt_open = false;
        }
        if let Some(live) = self.live.as_mut().filter(|live| live.open) {
            println!();
            live.open = false;
        }
    }

    /// Prints lines (after making room for them), without the prompt.
    fn emit(&mut self, lines: &[String]) {
        self.interrupt();
        for line in lines {
            println!("{line}");
        }
        flush();
    }

    /// Prints lines, then the prompt.
    fn block(&mut self, lines: &[String]) {
        self.emit(lines);
        self.settle();
    }

    /// A line of the chat's own, then the prompt.
    fn note(&mut self, text: &str) {
        self.block(&[text.to_string()]);
    }

    /// The prompt, unless an answer is streaming on its open line.
    fn settle(&mut self) {
        if self.live.as_ref().is_none_or(|live| !live.open) {
            self.prompt();
        }
    }

    /// The conversation as it opens: its head, its newest messages, its draft, and what can be typed.
    fn opening(&mut self) {
        let Some(view) = self.view.clone() else {
            self.emit(&[
                "A new conversation with the orchestrator: your first message opens it."
                    .to_string(),
                HELP.to_string(),
                String::new(),
            ]);
            return;
        };
        let messages = array_at(&view, "messages");
        let skip = messages.len().saturating_sub(OPENING_MESSAGES);
        let mut lines = vec![header(&view)];
        if skip > 0 {
            lines.push(format!("({skip} earlier messages: /history prints them)"));
        }
        lines.push(String::new());
        for m in &messages[skip..] {
            lines.extend(message_lines(m, &self.rich, &self.ctx.rich));
        }
        self.emit(&lines);
        self.last = messages.last().map(Seen::of);
        self.printed = messages.len();
        self.waiting = unanswered(&view);
        if let Some(draft) = view.get("draft").filter(|d| d.is_object()) {
            self.show_draft(draft, view.get("project").and_then(Value::as_str));
        }
        let mut closing = vec!["/help lists the chat's commands.".to_string()];
        if view.get("archived").and_then(Value::as_bool) == Some(true) {
            closing.insert(
                0,
                "This conversation is archived: /unarchive it to go on.".to_string(),
            );
        }
        if !self.waiting.is_empty() {
            closing.insert(
                0,
                "The orchestrator has yet to answer: its answer prints when it comes.".to_string(),
            );
        }
        self.emit(&closing);
    }

    /// The draft, and how to start it when nothing stops it; it is the revision `/start` starts.
    fn show_draft(&mut self, draft: &Value, project: Option<&str>) {
        self.revision = number(draft, "revision");
        let mut lines = draft_lines(draft, project);
        if startable(draft) {
            lines.push("Type /start to start it.".to_string());
        }
        lines.push(String::new());
        self.block(&lines);
    }

    // ---- The conversation -----------------------------------------------------------------------------------------

    /// Takes a view of the conversation: prints the messages not printed yet, then the draft when its revision
    /// changed. An older view than the one held (an event that was overtaken) is skipped.
    fn on_view(&mut self, view: Value) {
        if let Some(held) = &self.view
            && number(&view, "updated_ms") < number(held, "updated_ms")
        {
            return;
        }
        let messages = array_at(&view, "messages").to_vec();
        let Some(start) = fresh(&messages, self.last.as_ref(), self.printed) else {
            return;
        };
        if let Some(cid) = view.get("cid").and_then(Value::as_u64) {
            self.watched.store(cid, Ordering::Relaxed);
        }
        let revision = draft_revision(&view);
        let draft = view.get("draft").filter(|d| d.is_object()).cloned();
        let project = view
            .get("project")
            .and_then(Value::as_str)
            .map(str::to_string);
        self.waiting = unanswered(&view);
        self.view = Some(view);
        for m in &messages[start..] {
            self.on_message(m);
        }
        self.last = messages.last().map(Seen::of).or(self.last.take());
        self.printed = messages.len();
        if revision != self.revision
            && let Some(draft) = draft
        {
            self.show_draft(&draft, project.as_deref());
        }
    }

    /// One new message: the owner's echo of a line just sent is skipped; an answer that was streamed is not printed
    /// again when its text is the one on screen.
    fn on_message(&mut self, m: &Value) {
        let from = ctx::s(m, "from");
        let text = ctx::s(m, "text");
        if from == "owner" && self.echo.as_deref() == Some(text) {
            self.echo = None;
            return;
        }
        if from == "orchestrator" {
            let work = number(m, "work");
            if let Some(live) = self.live.take_if(|live| live.work == work) {
                let done = matches!(ctx::s(m, "status"), "" | "done");
                if done && same_text(&live.text, text) {
                    if live.open {
                        println!("\n");
                        flush();
                    }
                    self.settle();
                    return;
                }
                if live.open {
                    println!("\n");
                }
            }
        }
        let lines = message_lines(m, &self.rich, &self.ctx.rich);
        self.block(&lines);
    }

    fn on_event(&mut self, kind: &str, data: Value) {
        match kind {
            "owner_conversation" => {
                let mine = self
                    .id()
                    .is_some_and(|id| ctx::s(&data, "id") == id.as_str());
                if mine {
                    self.on_view(data);
                }
            }
            "delta" => self.on_delta(&data),
            _ => {}
        }
    }

    /// Streams an answer as the orchestrator writes it: the text of a `delta` of a work item the conversation waits
    /// on. A new model call of the same work starts a new paragraph; the answer is the last call's text.
    fn on_delta(&mut self, data: &Value) {
        let Some(text) = delta_text(data) else {
            return;
        };
        let work = number(data, "work");
        if !self.waiting.contains(&work) {
            return;
        }
        let req = ctx::s(data, "req").to_string();
        if self.live.as_ref().is_some_and(|live| live.work != work) {
            // Another answer begins: the one on screen ends its line.
            self.interrupt();
            self.live = None;
        }
        if self.prompt_open {
            clear_line();
            self.prompt_open = false;
        }
        let live = self.live.get_or_insert_with(|| Live {
            work,
            req: req.clone(),
            text: String::new(),
            open: false,
        });
        if !live.open {
            println!("[{}] orchestrator", now());
            live.open = true;
        }
        if live.req != req {
            if !live.text.is_empty() {
                print!("\n\n");
            }
            live.req = req;
            live.text.clear();
        }
        print!("{text}");
        flush();
        live.text.push_str(text);
    }

    /// Reads the conversation again (an answer is awaited, or the events socket is down). An error is said once,
    /// until another one comes.
    fn refresh(&mut self) {
        self.read_at = Instant::now();
        let Some(id) = self.id() else {
            return;
        };
        match self.ctx.client.get(&path_of(&id)) {
            Ok(view) => {
                self.failing = None;
                self.on_view(view);
            }
            Err(e) => {
                if self.failing.as_deref() != Some(e.as_str()) {
                    self.note(&format!("(reading {id}: {e})"));
                    self.failing = Some(e);
                }
            }
        }
    }

    /// Whether the conversation is due a read: an answer is awaited, or the events socket is down.
    fn due(&self) -> bool {
        let Some(view) = &self.view else {
            return false;
        };
        let waiting = !array_at(view, "pending").is_empty() || !self.waiting.is_empty();
        (waiting || !self.link) && self.read_at.elapsed() >= POLL_EVERY
    }

    // ---- The commands ---------------------------------------------------------------------------------------------

    /// Sends a message; the first one opens the conversation. A message `typed` at the prompt is on screen already,
    /// so it is not printed again; one given as arguments is. A refusal (no orchestrator on the roster, an archived
    /// conversation) is printed as the hub says it, and the chat goes on.
    fn say(&mut self, text: &str, typed: bool) {
        let text = text.trim();
        if text.is_empty() {
            return self.prompt();
        }
        let id = self.id();
        match send(&self.ctx.client, id.as_deref(), text) {
            Ok(view) => {
                if id.is_none() {
                    let opened = ctx::s(&view, "id");
                    self.emit(&[format!(
                        "(conversation {opened}: `agentplaza chat --conversation {opened}` goes on with it later)"
                    )]);
                }
                self.echo = typed.then(|| text.to_string());
                self.on_view(view);
                self.settle();
            }
            Err(e) => self.note(&e),
        }
    }

    /// `/draft`: the draft as the hub has it now, with what still stops Start.
    fn draft(&mut self) {
        let Some(id) = self.id() else {
            return self.note(NO_CONVERSATION);
        };
        match self.ctx.client.get(&format!("{}/draft", path_of(&id))) {
            Ok(draft) => {
                let project = self
                    .view
                    .as_ref()
                    .and_then(|v| v.get("project"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                self.show_draft(&draft, project.as_deref());
            }
            Err(e) => self.note(&e),
        }
    }

    /// `/start`: starts the draft at the revision on screen. The hub refuses another revision, a draft something
    /// still stops, or one started already, and says why.
    fn start(&mut self) {
        let Some(id) = self.id() else {
            return self.note(NO_CONVERSATION);
        };
        if self.revision == 0 {
            return self
                .note("No draft is on screen to start: /draft shows the conversation's draft.");
        }
        let mut client = self.ctx.client.clone();
        client.timeout = START_TIMEOUT;
        let path = format!("{}/start", path_of(&id));
        match client.post(&path, &json!({ "revision": self.revision })) {
            Ok(view) => {
                let project = view
                    .pointer("/started/project")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                self.emit(&[
                    started_line(&view["started"]),
                    format!(
                        "Follow it with `agentplaza tasks graph {project}` and `agentplaza projects show {project}`."
                    ),
                    String::new(),
                ]);
                self.on_view(view);
                self.settle();
            }
            Err(e) => self.note(&e),
        }
    }

    /// `/history`: every message of the conversation.
    fn history(&mut self) {
        let Some(view) = self.view.clone() else {
            return self.note(NO_CONVERSATION);
        };
        let mut lines = vec![header(&view), String::new()];
        for m in array_at(&view, "messages") {
            lines.extend(message_lines(m, &self.rich, &self.ctx.rich));
        }
        self.block(&lines);
    }

    /// `/model`: sets the conversation's model (`default` clears it), then says what it is now.
    fn model(&mut self, text: &str) {
        let body = match model_body(text) {
            Ok(body) => body,
            Err(e) => return self.note(&e),
        };
        let now = body["model"].as_str().unwrap_or("the default").to_string();
        self.patch(body, &format!("Model: {now}."));
    }

    /// `/context`: how full the conversation's context is, and what fills it (the tool calls, the parts).
    fn context(&mut self) {
        let Some(id) = self.id() else {
            return self.note(NO_CONVERSATION);
        };
        match self.ctx.client.get(&Owner::Conversation(id).context_path()) {
            Ok(answer) => match context_block(&answer) {
                Some(lines) => self.block(&lines),
                None => self.note("The hub's answer has no context in it."),
            },
            Err(e) => self.note(&e),
        }
    }

    /// `/rename`, `/archive` and `/unarchive`: changes the conversation, then says `done`.
    fn patch(&mut self, body: Value, done: &str) {
        let Some(id) = self.id() else {
            return self.note(NO_CONVERSATION);
        };
        match self.ctx.client.patch(&path_of(&id), &body) {
            Ok(view) => {
                self.note(done);
                self.on_view(view);
            }
            Err(e) => self.note(&e),
        }
    }
}

/// The body of `/model`: `{model}` with the entry (`provider/model[@effort]`), or null for `default`.
fn model_body(text: &str) -> Result<Value, String> {
    Ok(json!({ "model": context_bar::model_entry(text)? }))
}

/// The lines `/context` prints: the bar's text, then the breakdown. None when the answer has no `used`.
fn context_block(answer: &Value) -> Option<Vec<String>> {
    let context = Context::parse(answer)?;
    let mut lines = vec![format!("context {}", context_bar::bar_text(&context))];
    lines.extend(context_bar::plain(&context));
    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_a_message_or_a_command() {
        assert_eq!(
            parse_line("  Fix the flaky tests "),
            Input::Say("Fix the flaky tests".into())
        );
        assert_eq!(parse_line("/draft"), Input::Draft);
        assert_eq!(parse_line("/start"), Input::Start);
        assert_eq!(parse_line("/quit"), Input::Quit);
        assert_eq!(parse_line("/exit"), Input::Quit);
        assert_eq!(parse_line("   "), Input::Nothing);
        assert_eq!(
            parse_line("/rename  Release 0.9.1 "),
            Input::Rename("Release 0.9.1".into())
        );
        assert_eq!(parse_line("/rename"), Input::Rename(String::new()));
        assert_eq!(parse_line("/unarchive"), Input::Archive(false));
        assert_eq!(
            parse_line("//usr/bin is where"),
            Input::Say("/usr/bin is where".into())
        );
        assert_eq!(parse_line("/deploy now"), Input::Unknown("deploy".into()));
    }

    #[test]
    fn the_socket_keeps_the_conversations_and_the_orchestrators_text() {
        let conversation = r#"{"event":"owner_conversation","data":{"id":"conv-3"}}"#;
        assert_eq!(
            wanted(conversation, 0).map(|(kind, _)| kind).as_deref(),
            Some("owner_conversation")
        );
        let delta =
            r#"{"event":"delta","data":{"cid":3,"work":42,"delta":{"type":"text","text":"a"}}}"#;
        assert!(wanted(delta, 3).is_some());
        assert!(wanted(delta, 4).is_none(), "another agent's text");
        assert!(wanted(delta, 0).is_none(), "no orchestrator known yet");
        // The agents' direct messages are `conversation` events, which the chat leaves alone.
        assert!(wanted(r#"{"event":"conversation","data":{}}"#, 3).is_none());
        assert!(wanted("not json", 3).is_none());
    }

    #[test]
    fn model_and_context_are_commands_and_model_keeps_its_argument() {
        assert_eq!(
            parse_line("/model anthropic/claude-sonnet-4-5@high"),
            Input::Model("anthropic/claude-sonnet-4-5@high".into())
        );
        assert_eq!(parse_line("/model default"), Input::Model("default".into()));
        assert_eq!(parse_line("/model"), Input::Model(String::new()));
        assert_eq!(parse_line("/context"), Input::Context);
    }

    #[test]
    fn model_sends_the_entry_with_its_effort_or_null_for_the_default() {
        assert_eq!(
            model_body("anthropic/claude-sonnet-4-5@max").unwrap(),
            json!({"model": "anthropic/claude-sonnet-4-5@max"})
        );
        assert_eq!(model_body("default").unwrap(), json!({"model": null}));
        assert!(model_body("").unwrap_err().starts_with("usage: /model"));
        assert!(model_body("sonnet").unwrap_err().contains("provider/model"));
    }

    #[test]
    fn context_prints_the_bar_then_the_breakdown() {
        let answer = json!({
            "cid": 3, "work": 9, "model": "anthropic/claude-sonnet-4-5", "window": 200000,
            "used": 41200, "output": 300,
            "parts": [{"part": "system", "tokens": 2000}, {"part": "history", "tokens": 39200}],
            "tools": [{"tool": "bash", "calls": 12, "errors": 2, "result_tokens": 9000}],
            "calls": 7, "compactions": 1, "updated_ms": 5
        });
        let lines = context_block(&answer).expect("a context");
        assert_eq!(lines[0], "context 41.2k / 200k · 21%");
        assert!(
            lines
                .iter()
                .any(|l| l.contains("history") && l.contains("95%"))
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("bash") && l.contains("2 errors"))
        );
        assert_eq!(lines.last().unwrap(), "7 model calls · 1 compactions");
        assert!(context_block(&json!({"cid": 3})).is_none());
    }
}
