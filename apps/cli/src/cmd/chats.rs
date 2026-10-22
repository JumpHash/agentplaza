//! `agentplaza chats`: the owner's chats: the main chat with the orchestrator, a project's chat with its planner, and a
//! task's. A session the chat is with reads what you post there and answers in it. A card waiting for you shows in its
//! chat with its buttons, answered with `agentplaza inbox answer`; a draft waits in its project's chat with a start card.

use clap::{Args as ClapArgs, Subcommand};
use serde_json::{Value, json};

use crate::ctx::{self, Ctx, text_arg};
use crate::rich;

#[derive(ClapArgs, Debug)]
pub struct Args {
    /// Only this project's chats.
    #[arg(long)]
    pub project: Option<String>,
    /// Include the archived chats.
    #[arg(long)]
    pub archived: bool,
    #[command(subcommand)]
    pub action: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// A chat with its last messages, oldest first; a card in it shows its buttons. `main` is the orchestrator's chat.
    Show {
        /// The chat's id.
        id: String,
        /// How many messages, the newest (at most 1000).
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..=1000))]
        limit: u32,
    },
    /// Say something in a chat: the text, or `-` to read it from stdin.
    Post {
        /// The chat's id.
        id: String,
        /// The message, or `-` to read it from stdin.
        text: String,
    },
    /// Open a chat with a project's planner.
    Open {
        /// The project's id.
        project: String,
        /// The chat's title (the project's own, by default).
        #[arg(long)]
        title: Option<String>,
    },
    /// Archive a chat (`--undo` brings it back).
    Archive {
        /// The chat's id.
        id: String,
        /// Unarchive it.
        #[arg(long)]
        undo: bool,
    },
}

/// The chats, newest activity first, when no action is named.
pub fn run(ctx: &Ctx, args: Args) -> Result<(), String> {
    let Some(cmd) = args.action else {
        return list(ctx, args.project.as_deref(), args.archived);
    };
    match cmd {
        Cmd::Show { id, limit } => show(ctx, &id, limit),
        Cmd::Post { id, text } => post(ctx, &id, &text),
        Cmd::Open { project, title } => open(ctx, &project, title),
        Cmd::Archive { id, undo } => archive(ctx, &id, undo),
    }
}

/// The list's path: the project and the archived ones, as the flags name them.
fn list_path(project: Option<&str>, archived: bool) -> String {
    let mut query: Vec<String> = Vec::new();
    if let Some(project) = project {
        query.push(format!("project={}", ctx::seg(project)));
    }
    if archived {
        query.push("archived=true".to_string());
    }
    if query.is_empty() {
        "/v1/chats".to_string()
    } else {
        format!("/v1/chats?{}", query.join("&"))
    }
}

/// A chat's title, with `(archived)` after it when it is archived.
fn title_text(chat: &Value) -> String {
    let title = ctx::s(chat, "title").to_string();
    if chat.get("archived").and_then(Value::as_bool) == Some(true) {
        format!("{title} (archived)")
    } else {
        title
    }
}

fn list(ctx: &Ctx, project: Option<&str>, archived: bool) -> Result<(), String> {
    let chats = ctx.client.get(&list_path(project, archived))?;
    ctx.out(&chats, |chats| {
        let rows: Vec<Vec<String>> = chats
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|c| {
                vec![
                    ctx::text(c, "id"),
                    ctx::s(c, "kind").to_string(),
                    ctx::text(c, "project"),
                    ctx::text(c, "task"),
                    title_text(c),
                    ctx::ago(&c["updated_ms"]),
                ]
            })
            .collect();
        ctx::table(
            &["chat", "kind", "project", "task", "title", "updated"],
            &rows,
        );
    });
    Ok(())
}

/// The head line of a message: who wrote it (and which session, in which seat), when, and its kind when it is not
/// plain talk.
fn message_head(m: &Value) -> String {
    let who = match (ctx::s(m, "author"), ctx::s(m, "session")) {
        ("session", session) if !session.is_empty() => match ctx::s(m, "seat") {
            "" => format!("session {session}"),
            seat => format!("session {session} ({seat})"),
        },
        (author, _) => author.to_string(),
    };
    let kind = match ctx::s(m, "kind") {
        "" | "message" => String::new(),
        kind => format!(" · {kind}"),
    };
    format!(
        "#{}  {who} · {}{kind}",
        ctx::text(m, "seq"),
        ctx::when(&m["at_ms"])
    )
}

/// A card's buttons as a chat shows them: each choice's id in brackets, its label, and a note when it takes an amount.
fn card_buttons(card: &Value) -> String {
    let buttons: Vec<String> = card
        .get("choices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|c| {
            let amount = if c.get("takes_amount").and_then(Value::as_bool) == Some(true) {
                " (takes --amount)"
            } else {
                ""
            };
            format!("[{}] {}{amount}", ctx::s(c, "id"), ctx::s(c, "label"))
        })
        .collect();
    format!("  buttons: {}", buttons.join(" · "))
}

/// A chat's header lines: its id, title, kind and what it is about.
fn chat_lines(chat: &Value) -> Vec<(&'static str, String)> {
    let mut lines = vec![
        ("chat", ctx::text(chat, "id")),
        ("title", title_text(chat)),
        ("kind", ctx::s(chat, "kind").to_string()),
    ];
    if let Some(project) = chat.get("project").and_then(Value::as_str) {
        lines.push(("project", project.to_string()));
    }
    if let Some(task) = chat.get("task").and_then(Value::as_str) {
        lines.push(("task", task.to_string()));
    }
    lines
}

fn show(ctx: &Ctx, id: &str, limit: u32) -> Result<(), String> {
    let chat = ctx
        .client
        .get(&format!("/v1/chats/{}?limit={limit}", ctx::seg(id)))?;
    // The buttons of the cards in the chat come from its cards: they are read only when a message has one.
    let has_cards = chat
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|m| m.get("card").is_some_and(|c| !c.is_null()));
    let cards = if has_cards {
        ctx.client
            .get(&format!("/v1/cards?chat={}", ctx::seg(id)))?
    } else {
        Value::Null
    };
    ctx.out(&chat, |chat| {
        ctx::fields(&chat_lines(chat));
        let messages = chat
            .get("messages")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let rich_ctx = rich::context_for(ctx);
        for m in messages {
            println!();
            println!("{}", message_head(m));
            for line in rich::render(ctx::s(m, "text"), &rich_ctx, &ctx.rich).lines() {
                println!("  {line}");
            }
            let card_id = ctx::s(m, "card");
            if !card_id.is_empty() {
                let card = cards
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|c| ctx::s(c, "id") == card_id);
                match card {
                    Some(card) if ctx::s(card, "state") == "open" => {
                        println!("{}", card_buttons(card))
                    }
                    Some(card) => println!("  card {card_id}: {}", ctx::s(card, "state")),
                    None => println!("  card {card_id}"),
                }
            }
        }
    });
    Ok(())
}

fn post(ctx: &Ctx, id: &str, text: &str) -> Result<(), String> {
    let text = text_arg(text)?;
    let message = ctx.client.post(
        &format!("/v1/chats/{}/messages", ctx::seg(id)),
        &json!({ "text": text }),
    )?;
    ctx.out(&message, |m| {
        println!(
            "posted to chat {id} as message #{}; the answer comes in the chat: `agentplaza chats show {id}`",
            ctx::text(m, "seq")
        )
    });
    Ok(())
}

fn open(ctx: &Ctx, project: &str, title: Option<String>) -> Result<(), String> {
    let mut body = json!({ "project": project });
    if let Some(title) = title {
        body["title"] = json!(title);
    }
    let chat = ctx.client.post("/v1/chats", &body)?;
    ctx.out(&chat, |c| {
        println!(
            "opened chat {} with the planner of {project}: `agentplaza chats post {} <text>` writes to it",
            ctx::s(c, "id"),
            ctx::s(c, "id")
        )
    });
    Ok(())
}

fn archive(ctx: &Ctx, id: &str, undo: bool) -> Result<(), String> {
    let chat = ctx.client.patch(
        &format!("/v1/chats/{}", ctx::seg(id)),
        &json!({ "archived": !undo }),
    )?;
    ctx.out(&chat, |_| {
        if undo {
            println!("unarchived chat {id}");
        } else {
            println!("archived chat {id}");
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_asks_for_a_project_and_the_archived_ones_as_asked() {
        assert_eq!(list_path(None, false), "/v1/chats");
        assert_eq!(list_path(Some("pr3"), false), "/v1/chats?project=pr3");
        assert_eq!(list_path(None, true), "/v1/chats?archived=true");
        assert_eq!(
            list_path(Some("pr3"), true),
            "/v1/chats?project=pr3&archived=true"
        );
    }

    #[test]
    fn an_archived_chat_says_so_in_its_title() {
        assert_eq!(
            title_text(&json!({ "title": "Release 0.9", "archived": true })),
            "Release 0.9 (archived)"
        );
        assert_eq!(
            title_text(&json!({ "title": "Release 0.9", "archived": false })),
            "Release 0.9"
        );
    }

    #[test]
    fn a_message_names_its_author_and_its_session_and_seat() {
        let owner =
            json!({ "seq": 3, "author": "owner", "session": null, "kind": "message", "at_ms": 0 });
        assert_eq!(
            message_head(&owner),
            format!("#3  owner · {}", ctx::when(&json!(0)))
        );
        let reply = json!({ "seq": 4, "author": "session", "session": "s12", "seat": "planner", "kind": "question", "at_ms": 0 });
        assert_eq!(
            message_head(&reply),
            format!(
                "#4  session s12 (planner) · {} · question",
                ctx::when(&json!(0))
            )
        );
        let bare = json!({ "seq": 5, "author": "session", "session": "s13", "seat": null, "kind": "message", "at_ms": 0 });
        assert_eq!(
            message_head(&bare),
            format!("#5  session s13 · {}", ctx::when(&json!(0)))
        );
    }

    #[test]
    fn a_card_in_a_chat_shows_its_buttons() {
        let card = json!({ "id": "c7", "state": "open", "choices": [
            { "id": "start", "label": "Start the project", "takes_amount": false },
            { "id": "raise", "label": "Raise", "takes_amount": true }
        ]});
        assert_eq!(
            card_buttons(&card),
            "  buttons: [start] Start the project · [raise] Raise (takes --amount)"
        );
    }

    #[test]
    fn a_chat_header_names_its_project_and_task_where_it_has_them() {
        let task_chat = json!({ "id": "chat-t4", "kind": "task", "title": "Fix it", "project": "pr3", "task": "t4", "archived": false });
        let lines = chat_lines(&task_chat);
        assert_eq!(lines[0], ("chat", "chat-t4".to_string()));
        assert!(lines.contains(&("project", "pr3".to_string())));
        assert!(lines.contains(&("task", "t4".to_string())));
        let main = chat_lines(
            &json!({ "id": "main", "kind": "main", "title": "Orchestrator", "project": null, "task": null }),
        );
        assert!(!main.iter().any(|(k, _)| *k == "project" || *k == "task"));
    }
}
