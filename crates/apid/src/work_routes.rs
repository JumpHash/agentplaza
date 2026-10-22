//! The work engine's routes (Engine 0.3, 0.5, 0.8, 0.10): sessions, seats, threads, budgets, cards and chats.
//!
//! GET   /v1/sessions[?project=&task=&state=&live=true]     → [Session], newest first
//! GET   /v1/sessions/{id}                                   → Session
//! POST  /v1/sessions/{id}/stop                              → Session (ended; its seat goes again)
//! GET   /v1/tasks/{id}/seats                                → [Seat]
//! GET   /v1/tasks/{id}/threads                              → [Thread]
//! GET   /v1/threads/{id}                                    → Thread
//! GET   /v1/projects/{id}/budget                            → BudgetView
//! PUT   /v1/projects/{id}/budget {unit, cap?, warn_at_pct?, monthly_cap?} → BudgetView
//! GET   /v1/tasks/{id}/budget                               → BudgetView
//! PUT   /v1/tasks/{id}/budget {unit, cap?}                  → BudgetView
//! GET   /v1/cards[?open=true&chat=]                         → [Card], newest first
//! GET   /v1/cards/{id}                                      → Card
//! POST  /v1/cards/{id} {choice, amount?}                    → Card (answered, and what it decides done)
//! GET   /v1/chats[?project=&archived=true]                  → [Chat] (without messages), newest activity first
//! POST  /v1/chats {project, title?}                         → 201 Chat (a new chat with the project's planner)
//! GET   /v1/chats/{id}[?limit=]                             → Chat with its last messages (`main`: the orchestrator's)
//! GET   /v1/chats/{id}/messages?after=&limit=               → [Message]
//! POST  /v1/chats/{id}/messages {text}                      → 201 Message (the owner's; delivered to the chat's session)
//! PATCH /v1/chats/{id} {title?, archived?}                  → Chat
//!
//! Amounts are micro-dollars for a `usd` budget and tokens for a `tokens` one.

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::budgets::{Budget, OwnerKind, Unit};
use crate::cards::{Card, Kind as CardKind, State as CardState};
use crate::chats::{Author, MessageKind, Post};
use crate::sessions::{Filter, State};
use crate::state::App;

fn now() -> u64 {
    apz_plaza::now_ms()
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/sessions", list_sessions)
        .doc(RouteDoc::new("List the sessions"))
        .get("/v1/sessions/{id}", |app, _, p| match app.sessions.get(p.get("id")) {
            Some(s) => Response::json(200, &s),
            None => Response::error(404, &format!("no session {}", p.get("id"))),
        })
        .doc(RouteDoc::new("Read a session").errors(&[404]))
        .post("/v1/sessions/{id}/stop", |app, _, p| match crate::work_loop::stop(app, p.get("id")) {
            Ok(s) => Response::json(200, &s),
            Err(e) => Response::error(404, &e),
        })
        .doc(RouteDoc::new("Stop a session: it ends with its handover and its seat goes again").errors(&[404]))
        .get("/v1/tasks/{id}/seats", |app, _, p| Response::json(200, &app.seats.of_task(p.get("id"))))
        .doc(RouteDoc::new("List a task's seats"))
        .get("/v1/tasks/{id}/threads", |app, _, p| Response::json(200, &app.threads.of_task(p.get("id"))))
        .doc(RouteDoc::new("List a task's threads"))
        .get("/v1/threads/{id}", |app, _, p| match app.threads.get(p.get("id")) {
            Some(t) => Response::json(200, &t),
            None => Response::error(404, &format!("no thread {}", p.get("id"))),
        })
        .doc(RouteDoc::new("Read a thread").errors(&[404]))
        .get("/v1/projects/{id}/budget", |app, _, p| budget_view(app, OwnerKind::Project, p.get("id")))
        .doc(RouteDoc::new("Read a project's budget"))
        .route("PUT", "/v1/projects/{id}/budget", put_project_budget)
        .doc(RouteDoc::new("Set a project's budget").errors(&[400, 404]))
        .get("/v1/tasks/{id}/budget", |app, _, p| budget_view(app, OwnerKind::Task, p.get("id")))
        .doc(RouteDoc::new("Read a task's budget"))
        .route("PUT", "/v1/tasks/{id}/budget", put_task_budget)
        .doc(RouteDoc::new("Set a task's budget").errors(&[400, 404]))
        .get("/v1/cards", |app, req, _| {
            let open = req.query_param("open").is_some_and(|v| v == "true" || v == "1");
            Response::json(200, &app.cards.list(open, req.query_param("chat")))
        })
        .doc(RouteDoc::new("List the cards waiting for the owner (and answered ones)"))
        .get("/v1/cards/{id}", |app, _, p| match app.cards.get(p.get("id")) {
            Some(c) => Response::json(200, &c),
            None => Response::error(404, &format!("no card {}", p.get("id"))),
        })
        .doc(RouteDoc::new("Read a card").errors(&[404]))
        .post("/v1/cards/{id}", answer_card)
        .doc(RouteDoc::new("Answer a card").errors(&[400, 404, 409]))
        .get("/v1/chats", |app, req, _| {
            let archived = req.query_param("archived").is_some_and(|v| v == "true" || v == "1");
            app.chats.main(now());
            Response::json(200, &app.chats.list(req.query_param("project"), archived))
        })
        .doc(RouteDoc::new("List the chats"))
        .post("/v1/chats", open_chat)
        .doc(RouteDoc::new("Open a new chat with a project's planner").errors(&[400, 404]))
        .get("/v1/chats/{id}", read_chat)
        .doc(RouteDoc::new("Read a chat with its last messages").errors(&[404]))
        .get("/v1/chats/{id}/messages", |app, req, p| {
            let after = req.query_param("after").and_then(|v| v.parse().ok()).unwrap_or(0);
            let limit = req.query_param("limit").and_then(|v| v.parse().ok()).unwrap_or(200).min(1000);
            match app.chats.messages(p.get("id"), after, limit) {
                Some(m) => Response::json(200, &m),
                None => Response::error(404, &format!("no chat {}", p.get("id"))),
            }
        })
        .doc(RouteDoc::new("Read a chat's messages after one").errors(&[404]))
        .post("/v1/chats/{id}/messages", post_message)
        .doc(RouteDoc::new("Say something in a chat").errors(&[400, 404]))
        .route("PATCH", "/v1/chats/{id}", edit_chat)
        .doc(RouteDoc::new("Rename or archive a chat").errors(&[400, 404]))
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
        live: req.query_param("live").is_some_and(|v| v == "true" || v == "1"),
    };
    Response::json(200, &app.sessions.list(&filter))
}

/// A budget as the consoles read it: the record, what is left, and the share spent.
fn budget_json(b: &Budget) -> Value {
    let mut v = serde_json::to_value(b).unwrap_or(Value::Null);
    v["left"] = json!(b.left());
    v["spent_pct"] = json!(b.cap.filter(|c| *c > 0).map(|c| (b.spent as f64 * 100.0 / c as f64).round()));
    v
}

fn budget_view(app: &App, kind: OwnerKind, id: &str) -> Response {
    match app.budgets.get(kind, id) {
        Some(b) => Response::json(200, &budget_json(&b)),
        None => Response::json(200, &json!({ "owner": id, "cap": null, "spent": 0, "unit": null })),
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
}

fn put_project_budget(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    if app.projects.get(id).is_none() {
        return Response::error(404, &format!("no project {id}"));
    }
    let body: BudgetBody = match req.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &format!("bad budget: {e}")),
    };
    match app.budgets.set_project(id, body.unit, body.cap, body.warn_at_pct.unwrap_or(80), body.monthly_cap, now()) {
        Ok(b) => {
            crate::work_loop::resume_budget(app, OwnerKind::Project, id);
            app.bus.publish("budget", &budget_json(&b));
            Response::json(200, &budget_json(&b))
        }
        Err(e) => Response::error(400, &e),
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
    match app.budgets.set_task(id, &project, body.unit, body.cap, now()) {
        Ok(b) => {
            crate::work_loop::resume_budget(app, OwnerKind::Task, id);
            app.bus.publish("budget", &budget_json(&b));
            Response::json(200, &budget_json(&b))
        }
        Err(e) => Response::error(400, &e),
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
    let card = match app.cards.answer(p.get("id"), &body.choice, body.amount, now()) {
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
    app.bus.publish("chat", &json!({ "chat": chat.id, "opened": true }));
    Response::json(201, &chat)
}

fn read_chat(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    if id == crate::chats::MAIN {
        app.chats.main(now());
    }
    let limit = req.query_param("limit").and_then(|v| v.parse::<usize>().ok()).unwrap_or(100).min(1000);
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
    text: String,
}

fn post_message(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    let body: Say = match req.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &format!("bad message: {e}")),
    };
    let text = body.text.trim();
    if text.is_empty() {
        return Response::error(400, "say something");
    }
    if id == crate::chats::MAIN {
        app.chats.main(now());
    }
    let Some(message) = app.chats.post(
        id,
        Post {
            author: Author::Owner,
            session: None,
            seat: None,
            kind: MessageKind::Message,
            text: text.to_string(),
            card: None,
        },
        now(),
    ) else {
        return Response::error(404, &format!("no chat {id}"));
    };
    // The owner's words are written at once, not on the saver's next pass.
    crate::store::saver::save_now(app, "chats");
    crate::work_loop::deliver_owner_message(app, id, text);
    app.bus.publish("chat", &json!({ "chat": id, "message": message }));
    Response::json(201, &message)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditChat {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    archived: Option<bool>,
}

fn edit_chat(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    let body: EditChat = match req.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &format!("bad edit: {e}")),
    };
    let mut chat = app.chats.get(id).map(|c| c.summary());
    if let Some(title) = body.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        chat = app.chats.rename(id, title);
    }
    if let Some(archived) = body.archived {
        chat = app.chats.set_archived(id, archived);
    }
    match chat {
        Some(chat) => Response::json(200, &chat),
        None => Response::error(404, &format!("no chat {id}")),
    }
}

/// Whether a card is a budget card the owner answered with a raise (for the tests).
#[cfg(test)]
pub fn raised(card: &Card) -> bool {
    card.kind == CardKind::Budget && card.state == CardState::Answered && card.answer.as_deref() == Some("raise_to")
}
