//! The owner's conversations with the orchestrator: the IDE's "New conversation", where a project is talked into
//! being. Each conversation is a thread of its own on the orchestrator, `conv-<n>` (also its id), so two
//! conversations never mix: the harness keeps one chat history per thread. The owner's messages queue there as owner
//! chat (`Origin { owner, conversation }`), and the orchestrator's answer to each is added when its work item ends.
//! Every change publishes a `conversation` event. The turns in between (model calls, tool calls) are the
//! orchestrator's transcript on the thread.
//!
//! Kept in `conversations.json` under the state dir, and with the plaza state (component `conversations`).
//!
//! GET   /v1/owner/conversations[?archived=true]      → [summary], newest first
//! POST  /v1/owner/conversations {text}               → 201 the conversation, its first message queued
//! GET   /v1/owner/conversations/{id}                 → the conversation
//! PATCH /v1/owner/conversations/{id} {title?, archived?} → the conversation
//! POST  /v1/owner/conversations/{id}/messages {text} → the conversation, the message queued
//! GET   /v1/owner/conversations/{id}/transcript?after=&limit= → the orchestrator's entries on the thread

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_proto::wire::WorkKind;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agents::{Agent, Status};
use crate::scheduler::{Origin, WorkItem};
use crate::state::App;

/// The most conversations kept: past it, the oldest archived one goes first, then the oldest.
pub const KEEP: usize = 500;
/// The most messages a conversation keeps; the oldest go first.
pub const MAX_MESSAGES: usize = 1_000;
/// The longest title, in characters.
const TITLE_CHARS: usize = 80;
/// The longest answer kept, in characters (the owner's messages are held to the chat limit).
const MAX_ANSWER_CHARS: usize = 20_000;
/// Who the owner's messages are from.
pub const OWNER: &str = "owner";
/// Who the answers are from.
pub const ORCHESTRATOR: &str = "orchestrator";
/// Who the plaza's notices in a conversation are from (a project started, a task ended).
pub const PLAZA: &str = "plaza";

/// One message of a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// [`OWNER`], [`ORCHESTRATOR`] or [`PLAZA`].
    pub from: String,
    pub text: String,
    pub at_ms: u64,
    /// The work item: the one the owner's message queued, or the one this answer ends (0 while an owner's message
    /// is being queued).
    pub work: u64,
    /// How an answer's work ended: `done`, `failed` or `stopped`. None on the owner's messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// One conversation with the orchestrator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
    /// `conv-<n>`; also the orchestrator's thread for it.
    pub id: String,
    /// The first message's first line, at most 80 characters, until the owner renames it.
    pub title: String,
    /// The orchestrator it talks to.
    pub cid: u64,
    pub messages: Vec<Message>,
    /// The work items its messages queued, oldest first.
    pub works: Vec<u64>,
    pub created_ms: u64,
    pub updated_ms: u64,
    #[serde(default)]
    pub archived: bool,
    /// The project it set up, once there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// The project the orchestrator drafts with the owner, which the owner starts (`drafts`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<crate::drafts::Draft>,
    /// The draft revision the orchestrator last read or wrote: an edit of the owner's after it is pointed out with the
    /// next message.
    #[serde(default)]
    pub draft_seen: u64,
}

impl Conversation {
    /// The owner's messages still waiting for an answer, by work item.
    pub fn pending(&self) -> Vec<u64> {
        self.messages
            .iter()
            .filter(|m| m.from == OWNER && m.work != 0)
            .map(|m| m.work)
            .filter(|work| {
                !self
                    .messages
                    .iter()
                    .any(|m| m.from == ORCHESTRATOR && m.work == *work)
            })
            .collect()
    }

    /// Adds `message`, dropping the oldest past [`MAX_MESSAGES`].
    fn push(&mut self, message: Message) {
        self.updated_ms = message.at_ms;
        self.messages.push(message);
        let over = self.messages.len().saturating_sub(MAX_MESSAGES);
        self.messages.drain(..over);
    }
}

/// The saved form: the id counter and the conversations, oldest first.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Saved {
    next: u64,
    items: Vec<Conversation>,
}

/// The conversations and their file.
pub struct Store {
    path: PathBuf,
    inner: Mutex<Saved>,
}

impl Store {
    /// Loads the store; a missing or unreadable file starts empty.
    pub fn load(path: PathBuf) -> Self {
        let inner = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            path,
            inner: Mutex::new(inner),
        }
    }

    fn guard(&self) -> MutexGuard<'_, Saved> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Every conversation, newest first.
    pub fn list(&self) -> Vec<Conversation> {
        let mut items = self.guard().items.clone();
        items.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms).then(b.id.cmp(&a.id)));
        items
    }

    pub fn get(&self, id: &str) -> Option<Conversation> {
        self.guard().items.iter().find(|c| c.id == id).cloned()
    }

    /// Opens a conversation with orchestrator `cid`, titled from `first` and holding it as the owner's first
    /// message (its work item not yet queued).
    pub fn open(&self, cid: u64, first: &str, now: u64) -> std::io::Result<Conversation> {
        let mut saved = self.guard();
        let mut next = saved.clone();
        next.next += 1;
        let mut conversation = Conversation {
            id: format!("conv-{}", next.next),
            title: title_of(first),
            cid,
            messages: Vec::new(),
            works: Vec::new(),
            created_ms: now,
            updated_ms: now,
            archived: false,
            project: None,
            draft: None,
            draft_seen: 0,
        };
        conversation.push(owner_message(first, now));
        next.items.push(conversation.clone());
        keep_within(&mut next.items);
        self.save(&next)?;
        *saved = next;
        Ok(conversation)
    }

    /// Changes conversation `id` with `change`, and writes the file; a refusal or a failed write changes nothing.
    pub fn update(
        &self,
        id: &str,
        change: impl FnOnce(&mut Conversation) -> Result<(), String>,
    ) -> Result<Conversation, (u16, String)> {
        let mut saved = self.guard();
        let mut next = saved.clone();
        let conversation = next
            .items
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| (404, format!("no conversation {id}")))?;
        change(conversation).map_err(|e| (409, e))?;
        let changed = conversation.clone();
        self.save(&next)
            .map_err(|e| (500, format!("conversations not saved: {e}")))?;
        *saved = next;
        Ok(changed)
    }

    /// The whole state as JSON, for Postgres (`crate::db`); "" when there is nothing to save.
    pub fn snapshot(&self) -> String {
        let saved = self.guard();
        if saved.items.is_empty() && saved.next == 0 {
            return String::new();
        }
        serde_json::to_string(&*saved).unwrap_or_default()
    }

    /// Replaces the state with a [`Store::snapshot`] and writes the file; a refusal changes nothing.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let restored: Saved =
            serde_json::from_str(json).map_err(|e| format!("conversations: {e}"))?;
        let mut saved = self.guard();
        self.save(&restored)
            .map_err(|e| format!("conversations: {e}"))?;
        *saved = restored;
        Ok(())
    }

    /// Writes the state to the file atomically (tmp file, then rename).
    fn save(&self, saved: &Saved) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(saved).map_err(std::io::Error::other)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.path)
    }
}

/// The first line of `text`, at most [`TITLE_CHARS`] characters.
fn title_of(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let mut title: String = line.chars().take(TITLE_CHARS).collect();
    if line.chars().count() > TITLE_CHARS {
        title.pop();
        title.push('…');
    }
    if title.is_empty() {
        "Conversation".into()
    } else {
        title
    }
}

fn owner_message(text: &str, now: u64) -> Message {
    Message {
        from: OWNER.into(),
        text: text.to_string(),
        at_ms: now,
        work: 0,
        status: None,
    }
}

/// The conversation that started project `pid`, if one did: the project's line to the orchestrator.
pub fn of_project(app: &App, pid: &str) -> Option<Conversation> {
    app.conversations
        .list()
        .into_iter()
        .find(|c| c.project.as_deref() == Some(pid))
}

/// Holds `items` (oldest first) to [`KEEP`]: the oldest archived conversation goes first, then the oldest.
fn keep_within(items: &mut Vec<Conversation>) {
    while items.len() > KEEP {
        let at = items.iter().position(|c| c.archived).unwrap_or(0);
        items.remove(at);
    }
}

/// The API view of a conversation: its fields, its `thread`, the work items still `pending` an answer, and its draft
/// with what still stops Start (`draft.problems`).
pub fn view(app: &App, conversation: &Conversation) -> Value {
    let mut value = serde_json::to_value(conversation).unwrap_or(Value::Null);
    value["thread"] = json!(conversation.id);
    value["pending"] = json!(conversation.pending());
    if let Some(draft) = &conversation.draft {
        value["draft"] = crate::drafts::view(app, draft);
    }
    value
}

/// A conversation as the list shows it: no messages, only the last one, clipped.
fn summary(conversation: &Conversation) -> Value {
    let last = conversation.messages.last().map(|m| {
        json!({
            "from": m.from,
            "text": m.text.chars().take(200).collect::<String>(),
            "at_ms": m.at_ms,
        })
    });
    json!({
        "id": conversation.id,
        "title": conversation.title,
        "cid": conversation.cid,
        "created_ms": conversation.created_ms,
        "updated_ms": conversation.updated_ms,
        "archived": conversation.archived,
        "project": conversation.project,
        "draft": conversation.draft.as_ref().map(|d| json!({"revision": d.revision, "title": d.title})),
        "messages": conversation.messages.len(),
        "pending": conversation.pending().len(),
        "last": last,
    })
}

/// The orchestrator a new conversation talks to: the agent of role `orchestrator`, a connected one first.
fn orchestrator(app: &App) -> Result<Agent, (u16, String)> {
    let mut found: Vec<Agent> = app
        .agents
        .list()
        .into_iter()
        .filter(|agent| agent.role == "orchestrator")
        .collect();
    found.sort_by_key(|agent| (agent.status == Status::Offline, agent.cid));
    found.into_iter().next().ok_or_else(|| {
        (
            409,
            "no orchestrator is on the roster: conversations are with the plaza's orchestrator, which has not connected"
                .to_string(),
        )
    })
}

/// Checks that orchestrator `agent` can take a message now, waking it when it sleeps.
fn reachable(app: &App, agent: &Agent) -> Result<(), (u16, String)> {
    if agent.status == Status::Offline {
        return Err((409, "the orchestrator is offline".into()));
    }
    crate::sleep::awake_or_503(app, agent.cid).map_err(|_| {
        (
            503,
            "the orchestrator is waking up; try again in a moment".into(),
        )
    })
}

/// Queues `prompt` on conversation `id`'s thread, as the owner's chat (`from` [`OWNER`]) or as a plaza notice
/// ([`PLAZA`]), and records the work item on the message from `from` added last.
fn queue(
    app: &App,
    id: &str,
    cid: u64,
    prompt: &str,
    from: &str,
    mcp: bool,
) -> Result<Conversation, (u16, String)> {
    let origin = Origin {
        owner: from == OWNER,
        mcp,
        conversation: Some(id.to_string()),
        ..Origin::default()
    };
    let work = app.work.enqueue(
        app,
        cid,
        WorkKind::Chat,
        id.to_string(),
        prompt.to_string(),
        origin,
    );
    app.conversations.update(id, |c| {
        // The message added last without a work item is this one.
        if let Some(message) = c
            .messages
            .iter_mut()
            .rev()
            .find(|m| m.from == from && m.work == 0)
        {
            message.work = work;
        }
        if !c.works.contains(&work) {
            c.works.push(work);
        }
        Ok(())
    })
}

/// Opens a conversation with the orchestrator and queues its first message `text`. `mcp`: it came over the hub's
/// MCP server.
pub fn open(app: &App, text: &str, mcp: bool) -> Result<Conversation, (u16, String)> {
    crate::agents::check_chat_text(text).map_err(|e| (400, e))?;
    let agent = orchestrator(app)?;
    reachable(app, &agent)?;
    let opened = app
        .conversations
        .open(agent.cid, text, apz_plaza::now_ms())
        .map_err(|e| (500, format!("conversations not saved: {e}")))?;
    let conversation = queue(app, &opened.id, agent.cid, text, OWNER, mcp)?;
    app.bus.publish("conversation", &view(app, &conversation));
    Ok(conversation)
}

/// Adds the owner's `text` to conversation `id` and queues it for its orchestrator.
pub fn say(app: &App, id: &str, text: &str, mcp: bool) -> Result<Conversation, (u16, String)> {
    crate::agents::check_chat_text(text).map_err(|e| (400, e))?;
    let conversation = app
        .conversations
        .get(id)
        .ok_or_else(|| (404, format!("no conversation {id}")))?;
    if conversation.archived {
        return Err((
            409,
            format!("conversation {id} is archived: unarchive it to go on"),
        ));
    }
    let agent = app.agents.get(conversation.cid).ok_or_else(|| {
        (
            409,
            format!(
                "the orchestrator this conversation talks to (agent {}) is gone",
                conversation.cid
            ),
        )
    })?;
    reachable(app, &agent)?;
    let now = apz_plaza::now_ms();
    // The owner's edits of the draft since the orchestrator last read it are pointed out with this message.
    let mut note = None;
    app.conversations.update(id, |c| {
        c.push(owner_message(text, now));
        if let Some(draft) = &c.draft
            && draft.revision > c.draft_seen
            && draft.updated_by == OWNER
        {
            note = Some(draft.revision);
            c.draft_seen = draft.revision;
        }
        Ok(())
    })?;
    let prompt = match note {
        Some(revision) => format!(
            "[plaza] The owner edited the project draft: it is at revision {revision} now. Read it with plaza_project_draft before you change it.\n\n{text}"
        ),
        None => text.to_string(),
    };
    let conversation = queue(app, id, agent.cid, &prompt, OWNER, mcp)?;
    app.bus.publish("conversation", &view(app, &conversation));
    Ok(conversation)
}

/// Tells conversation `id`'s orchestrator `text`, a plaza notice (a project started, a task ended), on the
/// conversation's thread: it shows in the conversation, and so does the orchestrator's answer. A notice for a
/// conversation whose orchestrator is gone is dropped.
pub fn notify(app: &App, id: &str, text: &str) {
    let Some(conversation) = app.conversations.get(id) else {
        return;
    };
    if app.agents.get(conversation.cid).is_none() {
        eprintln!("apid: a notice for {id} dropped: its orchestrator is gone");
        return;
    }
    let first_line = text.lines().next().unwrap_or_default();
    let message = Message {
        from: PLAZA.into(),
        text: first_line.chars().take(MAX_ANSWER_CHARS).collect(),
        at_ms: apz_plaza::now_ms(),
        work: 0,
        status: None,
    };
    if let Err((_, e)) = app.conversations.update(id, |c| {
        c.push(message);
        Ok(())
    }) {
        eprintln!("apid: a notice for {id} not kept: {e}");
        return;
    }
    match queue(app, id, conversation.cid, text, PLAZA, false) {
        Ok(conversation) => app.bus.publish("conversation", &view(app, &conversation)),
        Err((_, e)) => eprintln!("apid: a notice for {id} not queued: {e}"),
    }
}

/// A work item ended as `status` (`done`, `failed`, `stopped`): when a conversation queued it, the orchestrator's
/// answer (or why there is none) is added to it.
pub fn work_ended(app: &App, item: &WorkItem, status: &str, summary: &str) {
    let Some(id) = item.origin.conversation.as_deref() else {
        return;
    };
    let answer = Message {
        from: ORCHESTRATOR.into(),
        text: summary.chars().take(MAX_ANSWER_CHARS).collect(),
        at_ms: apz_plaza::now_ms(),
        work: item.id,
        status: Some(status.to_string()),
    };
    let added = app.conversations.update(id, |c| {
        c.push(answer);
        if !c.works.contains(&item.id) {
            c.works.push(item.id);
        }
        Ok(())
    });
    match added {
        Ok(conversation) => app.bus.publish("conversation", &view(app, &conversation)),
        Err((_, e)) => eprintln!("apid: the answer to work {} not kept: {e}", item.id),
    }
}

// ---- Routes ----------------------------------------------------------------------------------------------------------

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/owner/conversations", list_route)
        .doc(RouteDoc::new(
            "List the owner's conversations with the orchestrator",
        ))
        .post("/v1/owner/conversations", open_route)
        .doc(
            RouteDoc::new("Start a conversation with the orchestrator")
                .status(201)
                .errors(&[400, 409, 500, 503]),
        )
        .get("/v1/owner/conversations/{id}", get_route)
        .doc(RouteDoc::new("Read a conversation").errors(&[404]))
        .route("PATCH", "/v1/owner/conversations/{id}", patch_route)
        .doc(RouteDoc::new("Rename or archive a conversation").errors(&[400, 404, 500]))
        .post("/v1/owner/conversations/{id}/messages", message_route)
        .doc(
            RouteDoc::new("Send the orchestrator a message in a conversation")
                .errors(&[400, 404, 409, 500, 503]),
        )
        .get("/v1/owner/conversations/{id}/transcript", transcript_route)
        .doc(RouteDoc::new("Read the orchestrator's turns in a conversation").errors(&[404]))
}

#[derive(Deserialize)]
struct TextBody {
    text: String,
}

fn text_body(req: &Request) -> Result<String, Response> {
    req.json::<TextBody>()
        .map(|body| body.text)
        .map_err(|e| Response::error(400, &format!("bad body: {e}")))
}

fn answer(app: &App, result: Result<Conversation, (u16, String)>, status: u16) -> Response {
    match result {
        Ok(conversation) => Response::json(status, &view(app, &conversation)),
        Err((code, e)) => Response::error(code, &e),
    }
}

/// `?archived=true` lists the archived conversations too.
fn list_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let archived = req.query_param("archived") == Some("true");
    let rows: Vec<Value> = app
        .conversations
        .list()
        .iter()
        .filter(|c| archived || !c.archived)
        .map(summary)
        .collect();
    Response::json(200, &rows)
}

/// Body: `{text}`, the first message.
fn open_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    match text_body(req) {
        Ok(text) => answer(app, open(app, &text, false), 201),
        Err(refused) => refused,
    }
}

fn get_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    match app.conversations.get(p.get("id")) {
        Some(conversation) => Response::json(200, &view(app, &conversation)),
        None => Response::error(404, "no such conversation"),
    }
}

#[derive(Deserialize)]
struct PatchBody {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    archived: Option<bool>,
}

/// Body: `{title?, archived?}`.
fn patch_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let body: PatchBody = match req.json() {
        Ok(body) => body,
        Err(e) => return Response::error(400, &format!("bad body: {e}")),
    };
    let title = match body.title.as_deref().map(str::trim) {
        Some("") => return Response::error(400, "a title must not be empty"),
        Some(title) if title.chars().count() > TITLE_CHARS => {
            return Response::error(400, &format!("a title is at most {TITLE_CHARS} characters"));
        }
        other => other.map(str::to_string),
    };
    let result = app.conversations.update(p.get("id"), |c| {
        if let Some(title) = title {
            c.title = title;
        }
        if let Some(archived) = body.archived {
            c.archived = archived;
        }
        c.updated_ms = apz_plaza::now_ms();
        Ok(())
    });
    if let Ok(conversation) = &result {
        app.bus.publish("conversation", &view(app, conversation));
    }
    answer(app, result, 200)
}

/// Body: `{text}`.
fn message_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    match text_body(req) {
        Ok(text) => answer(app, say(app, p.get("id"), &text, false), 200),
        Err(refused) => refused,
    }
}

/// `?after=<seq>&limit=<n>` (at most 1000, default 200).
fn transcript_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let Some(conversation) = app.conversations.get(p.get("id")) else {
        return Response::error(404, "no such conversation");
    };
    let after = req
        .query_param("after")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let limit = req
        .query_param("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(200)
        .min(1000);
    Response::json(
        200,
        &app.transcripts
            .on_thread(conversation.cid, &conversation.id, after, limit),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> Store {
        let dir =
            std::env::temp_dir().join(format!("apid-conversations-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Store::load(dir.join("conversations.json"))
    }

    fn answer_to(work: u64, text: &str) -> Message {
        Message {
            from: ORCHESTRATOR.into(),
            text: text.into(),
            at_ms: 30,
            work,
            status: Some("done".into()),
        }
    }

    #[test]
    fn each_conversation_is_a_thread_of_its_own_with_its_answers() {
        let conversations = store("threads");
        let first = conversations
            .open(9, "The scheduler tests are flaky\nand I want 0.9 out", 10)
            .unwrap();
        let second = conversations.open(9, "Something else", 11).unwrap();
        assert_eq!(first.id, "conv-1");
        assert_eq!(second.id, "conv-2");
        assert_eq!(first.title, "The scheduler tests are flaky");
        assert_eq!(first.messages[0].from, OWNER);
        assert_eq!(first.pending(), Vec::<u64>::new(), "not queued yet");

        let queued = conversations
            .update("conv-1", |c| {
                c.messages[0].work = 41;
                c.works.push(41);
                Ok(())
            })
            .unwrap();
        assert_eq!(queued.pending(), [41]);
        let answered = conversations
            .update("conv-1", |c| {
                c.push(answer_to(41, "Which repo?"));
                Ok(())
            })
            .unwrap();
        assert_eq!(answered.pending(), Vec::<u64>::new());
        assert_eq!(answered.updated_ms, 30);
        // The other conversation is untouched, and the newest is listed first.
        assert_eq!(conversations.get("conv-2").unwrap().messages.len(), 1);
        let ids: Vec<String> = conversations.list().into_iter().map(|c| c.id).collect();
        assert_eq!(ids, ["conv-1", "conv-2"]);
        assert_eq!(
            conversations.update("conv-9", |_| Ok(())).unwrap_err().0,
            404
        );
    }

    #[test]
    fn conversations_survive_a_restart_and_a_snapshot() {
        let conversations = store("restart");
        conversations.open(9, "Hello", 5).unwrap();
        let again = Store::load(conversations.path.clone());
        assert_eq!(again.get("conv-1").unwrap().title, "Hello");
        // The counter is kept: a new conversation never reuses an id.
        assert_eq!(again.open(9, "Again", 6).unwrap().id, "conv-2");
        let other = store("restore");
        other.restore(&again.snapshot()).unwrap();
        assert_eq!(other.list().len(), 2);
        assert!(other.restore("not json").is_err());
        assert_eq!(other.list().len(), 2, "a refused restore changes nothing");
    }

    #[test]
    fn titles_are_a_first_line_and_the_oldest_archived_go_first() {
        assert_eq!(title_of("\n  Release 0.9  \nmore"), "Release 0.9");
        assert_eq!(title_of("   "), "Conversation");
        let long = "x".repeat(100);
        let title = title_of(&long);
        assert_eq!(title.chars().count(), TITLE_CHARS);
        assert!(title.ends_with('…'));

        let mut items: Vec<Conversation> = (0..=KEEP as u64)
            .map(|n| Conversation {
                id: format!("conv-{n}"),
                title: String::new(),
                cid: 9,
                messages: Vec::new(),
                works: Vec::new(),
                created_ms: n,
                updated_ms: n,
                archived: n == 7,
                project: None,
                draft: None,
                draft_seen: 0,
            })
            .collect();
        keep_within(&mut items);
        assert_eq!(items.len(), KEEP);
        assert!(
            items.iter().all(|c| c.id != "conv-7"),
            "the archived one went first"
        );
        let mut conversation = items.remove(0);
        for n in 0..(MAX_MESSAGES as u64 + 5) {
            conversation.push(answer_to(n, "a"));
        }
        assert_eq!(conversation.messages.len(), MAX_MESSAGES);
        assert_eq!(
            conversation.messages[0].work, 5,
            "the oldest messages go first"
        );
    }
}
