//! The work engine's routes (Engine 0.3, 0.5, 0.8, 0.10): sessions, seats, threads, budgets, cards and chats.
//!
//! GET   /v1/sessions[?project=&task=&state=&live=true]     → [Session], newest first
//! GET   /v1/sessions/{id}                                   → Session
//! GET   /v1/sessions/{id}/transcript[?before=&after=&limit=]  → {entries, more}: the newest page before a seq, or the oldest after one
//! GET   /v1/sessions/{id}/context[?work=]                   → the session's context (its last call's, or work's)
//! POST  /v1/sessions/{id}/stop                              → Session (ended; its seat goes again)
//! GET   /v1/tasks/{id}/seats                                → [Seat]
//! GET   /v1/tasks/{id}/threads                              → [Thread]
//! GET   /v1/threads/{id}                                    → Thread
//! GET   /v1/projects/{id}/budget                            → BudgetView, with `tasks`: its tasks' budgets, `disk`: {used_bytes, quota_bytes}
//! PUT   /v1/projects/{id}/budget {unit, cap?, warn_at_pct?, monthly_cap?, disk_quota_bytes?} → BudgetView
//! GET   /v1/projects/{id}/budget/ledger[?limit=]            → [Entry], newest first
//! GET   /v1/tasks/{id}/budget                               → BudgetView
//! PUT   /v1/tasks/{id}/budget {unit, cap?}                  → BudgetView
//! GET   /v1/tasks/{id}/budget/ledger[?limit=]               → [Entry], newest first
//! GET   /v1/cards[?open=true&chat=]                         → [Card], newest first
//! GET   /v1/cards/{id}                                      → Card
//! POST  /v1/cards/{id} {choice, amount?}                    → Card (answered, and what it decides done)
//! GET   /v1/chats[?project=&archived=true]                  → [Chat] (without messages), newest activity first
//! POST  /v1/chats {project, title?}                         → 201 Chat (a new chat with the project's planner)
//! GET   /v1/chats/{id}[?limit=]                             → Chat with its last messages (`main`: the orchestrator's)
//! GET   /v1/chats/{id}/messages?after=&limit=               → [Message]
//! POST  /v1/chats/{id}/messages {text, attachments?}        → 201 Message (the owner's; delivered to the chat's session)
//! GET   /v1/chats/{id}/transcript[?before=&after=&limit=]     → {entries, more}: the turns the chat's session took on it
//! PATCH /v1/chats/{id} {title?, archived?, model?}          → Chat (`model`: the chat's turns run on it; null goes back)
//! The owner's project drafts are the project chats' (`drafts.rs`).
//!
//! Amounts are micro-dollars for a `usd` budget and tokens for a `tokens` one; a disk quota is bytes.

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::budgets::{OwnerKind, Refused, Unit};
use crate::chats::{Author, Message, MessageKind, Post};
use crate::sessions::{Filter, State};
use crate::state::App;
use crate::transcript::{Page, Scope};

fn now() -> u64 {
    apz_plaza::now_ms()
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/sessions", list_sessions)
        .doc(RouteDoc::new("List the sessions"))
        .get("/v1/sessions/{id}", |app, _, p| {
            match app.sessions.get(p.get("id")) {
                Some(s) => Response::json(200, &s),
                None => Response::error(404, &format!("no session {}", p.get("id"))),
            }
        })
        .doc(RouteDoc::new("Read a session").errors(&[404]))
        .get("/v1/sessions/{id}/transcript", session_transcript)
        .doc(RouteDoc::new("Read a session's transcript").errors(&[400, 404, 500]))
        .get("/v1/sessions/{id}/context", session_context)
        .doc(RouteDoc::new("Read a session's context").errors(&[400, 404]))
        .post(
            "/v1/sessions/{id}/stop",
            |app, _, p| match crate::work_loop::stop(app, p.get("id")) {
                Ok(s) => Response::json(200, &s),
                Err(e) => Response::error(404, &e),
            },
        )
        .doc(
            RouteDoc::new("Stop a session: it ends with its handover and its seat goes again")
                .errors(&[404]),
        )
        .get("/v1/tasks/{id}/seats", |app, _, p| {
            Response::json(200, &app.seats.of_task(p.get("id")))
        })
        .doc(RouteDoc::new("List a task's seats"))
        .get("/v1/tasks/{id}/threads", |app, _, p| {
            Response::json(200, &app.threads.of_task(p.get("id")))
        })
        .doc(RouteDoc::new("List a task's threads"))
        .get("/v1/threads/{id}", |app, _, p| {
            match app.threads.get(p.get("id")) {
                Some(t) => Response::json(200, &t),
                None => Response::error(404, &format!("no thread {}", p.get("id"))),
            }
        })
        .doc(RouteDoc::new("Read a thread").errors(&[404]))
        .get("/v1/projects/{id}/budget", |app, _, p| {
            project_budget_view(app, p.get("id"))
        })
        .doc(RouteDoc::new("Read a project's budget"))
        .route("PUT", "/v1/projects/{id}/budget", put_project_budget)
        .doc(RouteDoc::new("Set a project's budget").errors(&[400, 404, 409]))
        .get("/v1/projects/{id}/budget/ledger", |app, req, p| {
            ledger_view(app, req, OwnerKind::Project, p.get("id"))
        })
        .doc(RouteDoc::new("Read a project budget's ledger").errors(&[500]))
        .get("/v1/tasks/{id}/budget", |app, _, p| {
            budget_view(app, OwnerKind::Task, p.get("id"))
        })
        .doc(RouteDoc::new("Read a task's budget"))
        .route("PUT", "/v1/tasks/{id}/budget", put_task_budget)
        .doc(RouteDoc::new("Set a task's budget").errors(&[400, 404, 409]))
        .get("/v1/tasks/{id}/budget/ledger", |app, req, p| {
            ledger_view(app, req, OwnerKind::Task, p.get("id"))
        })
        .doc(RouteDoc::new("Read a task budget's ledger").errors(&[500]))
        .get("/v1/cards", |app, req, _| {
            let open = req
                .query_param("open")
                .is_some_and(|v| v == "true" || v == "1");
            Response::json(200, &app.cards.list(open, req.query_param("chat")))
        })
        .doc(RouteDoc::new(
            "List the cards waiting for the owner (and answered ones)",
        ))
        .get("/v1/cards/{id}", |app, _, p| {
            match app.cards.get(p.get("id")) {
                Some(c) => Response::json(200, &c),
                None => Response::error(404, &format!("no card {}", p.get("id"))),
            }
        })
        .doc(RouteDoc::new("Read a card").errors(&[404]))
        .post("/v1/cards/{id}", answer_card)
        .doc(RouteDoc::new("Answer a card").errors(&[400, 404, 409]))
        .get("/v1/chats", |app, req, _| {
            let archived = req
                .query_param("archived")
                .is_some_and(|v| v == "true" || v == "1");
            app.chats.main(now());
            Response::json(200, &app.chats.list(req.query_param("project"), archived))
        })
        .doc(RouteDoc::new("List the chats"))
        .post("/v1/chats", open_chat)
        .doc(RouteDoc::new("Open a new chat with a project's planner").errors(&[400, 404]))
        .get("/v1/chats/{id}", read_chat)
        .doc(RouteDoc::new("Read a chat with its last messages").errors(&[404]))
        .get("/v1/chats/{id}/messages", |app, req, p| {
            let after = req
                .query_param("after")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let limit = req
                .query_param("limit")
                .and_then(|v| v.parse().ok())
                .unwrap_or(200)
                .min(1000);
            match app.chats.messages(p.get("id"), after, limit) {
                Some(m) => Response::json(200, &m),
                None => Response::error(404, &format!("no chat {}", p.get("id"))),
            }
        })
        .doc(RouteDoc::new("Read a chat's messages after one").errors(&[404]))
        .post("/v1/chats/{id}/messages", post_message)
        .doc(RouteDoc::new("Say something in a chat").errors(&[400, 404, 409]))
        .get("/v1/chats/{id}/transcript", chat_transcript)
        .doc(
            RouteDoc::new("Read the turns a chat's session took on its thread")
                .errors(&[400, 404, 500]),
        )
        .route("PATCH", "/v1/chats/{id}", edit_chat)
        .doc(RouteDoc::new("Rename, archive or set the model of a chat").errors(&[400, 404, 409]))
}

fn list_sessions(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let state = match req.query_param("state") {
        None => None,
        Some(s) => match serde_json::from_value::<State>(Value::String(s.to_string())) {
            Ok(s) => Some(s),
            Err(_) => return Response::error(400, &format!("unknown session state {s}")),
        },
    };
    let filter = Filter {
        project: req.query_param("project").map(str::to_string),
        task: req.query_param("task").map(str::to_string),
        state,
        live: req
            .query_param("live")
            .is_some_and(|v| v == "true" || v == "1"),
    };
    Response::json(200, &app.sessions.list(&filter))
}

fn session_transcript(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    let Some(session) = app.sessions.get(id) else {
        return Response::error(404, &format!("no session {id}"));
    };
    transcript_page(app, req, session.cid, Scope::Session(id))
}

/// A page of a transcript (`?before=&after=&limit=`): with `before` (a seq), the newest `limit` entries before it; else
/// the oldest `limit` after `after` (0 by default). Limit: 200 by default, 500 at most. The answer is `{entries, more}`,
/// the entries oldest first, and whether more lie beyond the page in the direction it was paged. `cid` is none for a
/// session or chat with no VM yet: an empty page.
fn transcript_page(app: &App, req: &Request, cid: Option<u64>, scope: Scope<'_>) -> Response {
    let limit = req
        .query_param("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(200)
        .clamp(1, 500);
    let before = match req.query_param("before").map(str::parse::<u64>) {
        None => None,
        Some(Ok(before)) => Some(before),
        Some(Err(_)) => return Response::error(400, "before must be an entry's seq"),
    };
    let Some(cid) = cid else {
        return Response::json(200, &json!({ "entries": [], "more": false }));
    };
    let page = match before {
        Some(before) => match app.transcripts.before(cid, scope, before, limit) {
            Ok(page) => page,
            Err(e) => {
                return Response::error(500, &format!("the transcript cannot be read: {e}"));
            }
        },
        None => {
            let after = req
                .query_param("after")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            // One more than the page, to know whether more follow.
            let mut entries = match scope {
                Scope::Session(session) => {
                    app.transcripts.of_session(cid, session, after, limit + 1)
                }
                Scope::Thread(thread) => app.transcripts.on_thread(cid, thread, after, limit + 1),
            };
            let more = entries.len() > limit;
            entries.truncate(limit);
            Page { entries, more }
        }
    };
    Response::json(200, &json!({ "entries": page.entries, "more": page.more }))
}

fn session_context(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    let Some(session) = app.sessions.get(id) else {
        return Response::error(404, &format!("no session {id}"));
    };
    let work = match req.query_param("work").map(str::parse::<u64>) {
        None => None,
        Some(Ok(w)) => Some(w),
        Some(Err(_)) => return Response::error(400, "work must be a number"),
    };
    let Some(cid) = session.cid else {
        return Response::json(200, &json!({ "session": id, "used": 0, "parts": [] }));
    };
    let mut view = crate::context_track::of_cid(app, cid, work);
    view["session"] = json!(id);
    Response::json(200, &view)
}

fn budget_view(app: &App, kind: OwnerKind, id: &str) -> Response {
    match app.budgets.get(kind, id) {
        Some(b) => Response::json(200, &b.view()),
        None => Response::json(
            200,
            &json!({ "owner": id, "cap": null, "spent": 0, "unit": null }),
        ),
    }
}

/// Whether a project's budget can be read: the plaza's own budget (`plaza`) always can, any other only when its
/// project exists (a read of an unknown project's budget is a 404, a read of an unknown task's is an empty budget).
fn project_budget_known(id: &str, exists: bool) -> bool {
    id == crate::work_loop::PLAZA || exists
}

/// A project's budget, with its tasks' budgets and its disk.
fn project_budget_view(app: &App, id: &str) -> Response {
    if !project_budget_known(id, app.projects.get(id).is_some()) {
        return Response::error(404, &format!("no project {id}"));
    }
    let budget = app.budgets.get(OwnerKind::Project, id);
    let quota = budget.as_ref().and_then(|b| b.disk_quota_bytes);
    let mut view = match budget {
        Some(b) => b.view(),
        None => json!({ "owner": id, "cap": null, "spent": 0, "unit": null }),
    };
    view["tasks"] = app
        .budgets
        .of_project(id)
        .iter()
        .filter(|b| b.owner_kind == OwnerKind::Task)
        .map(|b| b.view())
        .collect();
    view["disk"] = crate::disk::view(app, id, quota);
    Response::json(200, &view)
}

/// A budget's ledger (what was reserved, held, charged and released), newest first.
fn ledger_view(app: &App, req: &Request, kind: OwnerKind, id: &str) -> Response {
    let limit = req
        .query_param("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(200)
        .min(1000);
    let db = app.store.db(crate::store::PLAZA);
    // A charge is queued as its call returns: what is queued now is in the ledger it reads.
    let read = db
        .flush()
        .and_then(|_| db.read(|conn| crate::budgets::Budgets::ledger(conn, kind, id, limit)));
    match read {
        Ok(entries) => Response::json(200, &entries),
        Err(e) => Response::error(500, &format!("the ledger cannot be read: {e}")),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BudgetBody {
    unit: Unit,
    #[serde(default)]
    cap: Option<u64>,
    #[serde(default)]
    warn_at_pct: Option<u8>,
    #[serde(default)]
    monthly_cap: Option<u64>,
    /// A project's disk quota in bytes (`disk.rs`); a task has none.
    #[serde(default)]
    disk_quota_bytes: Option<u64>,
}

/// What a budget body may not say: a disk quota of zero bytes (null is no quota), and a disk quota on a task (its
/// project has the quota).
fn budget_refusal(body: &BudgetBody, task: bool) -> Option<&'static str> {
    match body.disk_quota_bytes {
        Some(0) => Some("a disk quota is more than zero bytes: send null for none"),
        Some(_) if task => Some("a task has no disk quota: its project does"),
        _ => None,
    }
}

fn put_project_budget(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    // The plaza's own budget (`plaza`): what the main orchestrator and VMs that run no session spend from.
    if id != crate::work_loop::PLAZA && app.projects.get(id).is_none() {
        return Response::error(404, &format!("no project {id}"));
    }
    let body: BudgetBody = match req.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &format!("bad budget: {e}")),
    };
    if let Some(refusal) = budget_refusal(&body, false) {
        return Response::error(400, refusal);
    }
    let quota_before = app
        .budgets
        .get(OwnerKind::Project, id)
        .and_then(|b| b.disk_quota_bytes);
    match app.budgets.set_project(
        id,
        body.unit,
        body.cap,
        body.warn_at_pct.unwrap_or(80),
        body.monthly_cap,
        body.disk_quota_bytes,
        now(),
    ) {
        Ok(b) => {
            if b.disk_quota_bytes != quota_before {
                crate::disk::quota_changed(app, id, now());
            }
            crate::work_loop::resume_budget(app, OwnerKind::Project, id);
            Response::json(200, &b.view())
        }
        Err(e) => set_refusal(&e),
    }
}

/// The response to a budget that could not be set: a unit that has spend cannot change (409), a store that refused
/// the write is a failure of the hub's (500).
fn set_refusal(e: &Refused) -> Response {
    match e {
        Refused::UnitLocked { .. } => Response::error(409, &e.to_string()),
        Refused::Store(_) => Response::error(500, &e.to_string()),
    }
}

fn put_task_budget(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    let Some(task) = app.plaza.tasks.find(id) else {
        return Response::error(404, &format!("no task {id}"));
    };
    let Some(project) = task.project().map(str::to_string) else {
        return Response::error(400, "the task belongs to no project");
    };
    let body: BudgetBody = match req.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &format!("bad budget: {e}")),
    };
    if let Some(refusal) = budget_refusal(&body, true) {
        return Response::error(400, refusal);
    }
    match app
        .budgets
        .set_task(id, &project, body.unit, body.cap, now())
    {
        Ok(b) => {
            crate::work_loop::resume_budget(app, OwnerKind::Task, id);
            Response::json(200, &b.view())
        }
        Err(e) => set_refusal(&e),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    choice: String,
    #[serde(default)]
    amount: Option<u64>,
}

fn answer_card(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let body: Answer = match req.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &format!("bad answer: {e}")),
    };
    let card = match app
        .cards
        .answer(p.get("id"), &body.choice, body.amount, now())
    {
        Ok(card) => card,
        Err(e @ crate::cards::Refused::NotFound(_)) => return Response::error(404, &e.to_string()),
        Err(e @ crate::cards::Refused::NotOpen(_)) => return Response::error(409, &e.to_string()),
        Err(e) => return Response::error(400, &e.to_string()),
    };
    let done = crate::work_loop::on_card_answered(app, &card);
    app.bus.publish("card", &card);
    Response::json(200, &json!({ "card": card, "done": done }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenChat {
    project: String,
    #[serde(default)]
    title: Option<String>,
}

fn open_chat(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let body: OpenChat = match req.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &format!("bad chat: {e}")),
    };
    let Some(project) = app.projects.get(&body.project) else {
        return Response::error(404, &format!("no project {}", body.project));
    };
    let title = body.title.unwrap_or_else(|| project.title.clone());
    let chat = app.chats.open_project(&body.project, &title, now());
    app.bus
        .publish("chat", &json!({ "chat": chat.id, "opened": true }));
    Response::json(201, &chat)
}

fn read_chat(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    if id == crate::chats::MAIN {
        app.chats.main(now());
    }
    let limit = req
        .query_param("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(100)
        .min(1000);
    match app.chats.get(id) {
        Some(mut chat) => {
            let skip = chat.messages.len().saturating_sub(limit);
            chat.messages.drain(..skip);
            Response::json(200, &chat)
        }
        None => Response::error(404, &format!("no chat {id}")),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Say {
    #[serde(default)]
    text: String,
    /// The owner's files (ids from `PUT /v1/files`) that go with the message.
    #[serde(default)]
    attachments: Vec<String>,
}

fn post_message(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let body: Say = match req.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &format!("bad message: {e}")),
    };
    match say(app, p.get("id"), &body.text, &body.attachments) {
        Ok(message) => Response::json(201, &message),
        Err((code, e)) => Response::error(code, &e),
    }
}

/// The owner says `text` in chat `id`, with the files `attachments` (ids): the message is kept, and it is delivered to
/// the session the chat is with. A message with no words but files says so. Its prompt names the files, and starts
/// with the note of the draft's edits the orchestrator has not read (`drafts::edit_note`).
pub fn say(
    app: &App,
    id: &str,
    text: &str,
    attachments: &[String],
) -> Result<Message, (u16, String)> {
    let text = text.trim();
    if text.is_empty() && attachments.is_empty() {
        return Err((400, "say something".into()));
    }
    let text = if text.is_empty() {
        "(files attached)"
    } else {
        text
    };
    crate::agents::check_chat_text(text).map_err(|e| (400, e))?;
    if id == crate::chats::MAIN {
        app.chats.main(now());
    }
    let chat = app
        .chats
        .get(id)
        .ok_or_else(|| (404, format!("no chat {id}")))?;
    if chat.archived {
        return Err((409, format!("chat {id} is archived: unarchive it to go on")));
    }
    let files = if attachments.is_empty() {
        String::new()
    } else {
        let cid = crate::work_loop::chat_cid(app, &chat).ok_or_else(|| {
            (
                409,
                "no session reads this chat now: its files wait for one".to_string(),
            )
        })?;
        crate::deliverables::attach(app, attachments, cid).map_err(|e| (400, e))?
    };
    let note = crate::drafts::edit_note(&app.chats, id);
    let message = app
        .chats
        .post(
            id,
            Post {
                author: Author::Owner,
                session: None,
                seat: None,
                kind: MessageKind::Message,
                text: text.to_string(),
                card: None,
                attachments: crate::deliverables::attachment_views(app, attachments),
            },
            now(),
        )
        .ok_or_else(|| (404, format!("no chat {id}")))?;
    // The owner's words are written at once, not on the saver's next pass.
    crate::store::saver::save_now(app, "chats");
    let said = match note {
        Some(note) => format!("{note}\n\n{text}"),
        None => text.to_string(),
    };
    let prompt = if files.is_empty() {
        said
    } else {
        format!("{said}\n\n{files}")
    };
    crate::work_loop::deliver_owner_message(app, id, &prompt);
    app.bus
        .publish("chat", &json!({ "chat": id, "message": message }));
    Ok(message)
}

/// `?before=&after=&limit=` (see [`transcript_page`]): the turns the session of the chat took on its thread.
fn chat_transcript(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    let Some(chat) = app.chats.get(id) else {
        return Response::error(404, &format!("no chat {id}"));
    };
    let cid = crate::work_loop::chat_cid(app, &chat);
    transcript_page(app, req, cid, Scope::Thread(&format!("chat:{id}")))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditChat {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    archived: Option<bool>,
    /// `Some(None)` (JSON null) goes back to the agent's model.
    #[serde(default, deserialize_with = "some_or_null")]
    model: Option<Option<String>>,
}

/// A present field as `Some`, JSON null included (as `Some(None)`); an absent one stays `None`.
fn some_or_null<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Option<String>>, D::Error> {
    Ok(Some(Option::<String>::deserialize(d)?))
}

/// Body: `{title?, archived?, model?}`.
fn edit_chat(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    let body: EditChat = match req.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &format!("bad edit: {e}")),
    };
    let title = body
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    // A chat's model is held to the orchestrator's policy, as the gateway holds every call.
    if let Some(Some(model)) = &body.model
        && let Err(e) = app
            .settings
            .allowed(crate::settings::Tier::Orchestrator, model)
    {
        return Response::error(400, &e);
    }
    if id == crate::chats::MAIN {
        app.chats.main(now());
    }
    let changed = app.chats.update(id, |c| {
        if let Some(title) = title {
            c.title = title;
        }
        if let Some(model) = body.model.clone() {
            c.model = model.filter(|m| !m.trim().is_empty());
        }
        // The main chat cannot be archived.
        if let Some(archived) = body.archived
            && c.kind != crate::chats::Kind::Main
        {
            c.archived = archived;
        }
        c.updated_ms = now();
        Ok(())
    });
    match changed {
        Ok(chat) => {
            crate::store::saver::save_now(app, "chats");
            app.bus
                .publish("chat", &json!({ "chat": id, "updated": true }));
            Response::json(200, &chat.summary())
        }
        Err((code, e)) => Response::error(code, &e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(disk_quota_bytes: Option<u64>) -> BudgetBody {
        BudgetBody {
            unit: Unit::Usd,
            cap: None,
            warn_at_pct: None,
            monthly_cap: None,
            disk_quota_bytes,
        }
    }

    #[test]
    fn a_project_budget_is_read_only_for_a_project_that_exists() {
        assert!(!project_budget_known("pr999999", false));
        assert!(project_budget_known("pr1", true));
        assert!(project_budget_known(crate::work_loop::PLAZA, false));
    }

    #[test]
    fn a_disk_quota_is_more_than_zero_bytes_and_a_task_has_none() {
        assert!(budget_refusal(&body(Some(0)), false).is_some());
        assert_eq!(budget_refusal(&body(Some(1 << 30)), false), None);
        assert_eq!(budget_refusal(&body(None), false), None);
        assert!(budget_refusal(&body(Some(1 << 30)), true).is_some());
        assert_eq!(budget_refusal(&body(None), true), None);
    }
}
