use serde_json::json;

use super::*;
use crate::console::client::Client;
use crate::console::work_api::CardAnswer;

fn ctx(project: Option<&str>) -> Ctx {
    Ctx {
        client: Client::new("http://127.0.0.1:1", None, None),
        project: project.map(String::from),
    }
}

fn project(id: &str, title: &str) -> Value {
    json!({ "id": id, "title": title, "stage": "plan", "created_ms": 1 })
}

fn chat(id: &str, kind: &str) -> Chat {
    Chat {
        id: id.into(),
        kind: kind.into(),
        title: "Orchestrator".into(),
        ..Default::default()
    }
}

fn event(kind: &str, data: Value) -> events::Event {
    events::Event {
        kind: kind.into(),
        data,
    }
}

#[test]
fn a_new_chat_needs_a_live_project() {
    let ctx = ctx(None);
    let mut state = opened(None);
    let _ = update(&mut state, Msg::OpenNewChat, &ctx);
    assert!(state.new_chat.is_none());
    assert_eq!(
        state.note.as_deref(),
        Some("There is no live project to start a chat in.")
    );
}

#[test]
fn a_new_chat_opens_with_the_scope_s_project_when_it_is_live() {
    let ctx = ctx(Some("pr2"));
    let mut state = opened(None);
    state.projects = vec![project("pr1", "One"), project("pr2", "Two")];
    let _ = update(&mut state, Msg::OpenNewChat, &ctx);
    assert_eq!(
        state.new_chat.as_ref().and_then(|n| n.project.clone()),
        Some("pr2".into())
    );
    let _ = update(&mut state, Msg::NewChatProject("pr1".into()), &ctx);
    let _ = update(&mut state, Msg::NewChatTitle("Launch".into()), &ctx);
    let new = state.new_chat.as_ref().unwrap();
    assert_eq!(
        (new.project.as_deref(), new.title.as_str()),
        (Some("pr1"), "Launch")
    );
    let _ = update(&mut state, Msg::CloseNewChat, &ctx);
    assert!(state.new_chat.is_none());
}

#[test]
fn a_rename_needs_a_title_to_save() {
    let ctx = ctx(None);
    let mut state = opened(Some("main".into()));
    state.chat = Some(chat("main", "main"));
    let _ = update(&mut state, Msg::StartRename, &ctx);
    assert_eq!(state.renaming.as_deref(), Some("Orchestrator"));
    let _ = update(&mut state, Msg::RenameTo("   ".into()), &ctx);
    let _ = update(&mut state, Msg::SaveRename, &ctx);
    assert_eq!(state.note.as_deref(), Some("A chat needs a title."));
    let _ = update(&mut state, Msg::CancelRename, &ctx);
    assert!(state.renaming.is_none());
}

#[test]
fn an_edited_chat_says_whether_it_was_archived_or_saved() {
    let ctx = ctx(None);
    let mut state = opened(Some("pr1".into()));
    let archived = Chat {
        archived: true,
        ..chat("pr1", "project")
    };
    let _ = update(&mut state, Msg::Edited(Ok(archived)), &ctx);
    assert_eq!(state.note.as_deref(), Some("Chat archived"));
    let _ = update(&mut state, Msg::Edited(Ok(chat("pr1", "project"))), &ctx);
    assert_eq!(state.note.as_deref(), Some("Chat saved"));
    let _ = update(&mut state, Msg::Edited(Err("nope".into())), &ctx);
    assert_eq!(state.note.as_deref(), Some("nope"));
}

#[test]
fn a_message_is_sent_once_at_a_time_and_a_blank_one_is_not_sent() {
    let ctx = ctx(None);
    let mut state = opened(Some("main".into()));
    let _ = update(&mut state, Msg::Draft("   ".into()), &ctx);
    let _ = update(&mut state, Msg::Send, &ctx);
    assert!(!state.sending, "a blank message is not sent");
    let _ = update(&mut state, Msg::Draft("hello".into()), &ctx);
    let _ = update(&mut state, Msg::Send, &ctx);
    assert!(state.sending);
    let _ = update(&mut state, Msg::Send, &ctx);
    assert!(state.sending, "a second send waits for the first");
}

#[test]
fn a_sent_message_clears_the_draft_and_a_refused_one_keeps_it() {
    let ctx = ctx(None);
    let mut state = opened(Some("main".into()));
    state.sending = true;
    state.draft = "hello".into();
    let _ = update(
        &mut state,
        Msg::Sent(Ok(ChatMessage {
            seq: 3,
            ..Default::default()
        })),
        &ctx,
    );
    assert!(!state.sending);
    assert!(state.draft.is_empty());

    state.sending = true;
    state.draft = "again".into();
    let _ = update(&mut state, Msg::Sent(Err("chat is archived".into())), &ctx);
    assert!(!state.sending);
    assert_eq!(state.draft, "again");
    assert_eq!(state.note.as_deref(), Some("chat is archived"));
}

#[test]
fn a_chat_frame_reloads_the_list_and_a_card_frame_does_not() {
    let ctx = ctx(None);
    let mut state = opened(None);
    let _ = update(&mut state, Msg::Event(event("card", json!({}))), &ctx);
    assert!(!state.loading);
    let _ = update(
        &mut state,
        Msg::Event(event("chat", json!({ "chat": "pr1" }))),
        &ctx,
    );
    assert!(state.loading);
}

#[test]
fn an_unreadable_chat_says_why_and_a_read_clears_it() {
    let ctx = ctx(None);
    let mut state = opened(Some("pr9".into()));
    let _ = update(&mut state, Msg::Read(Err("no chat pr9".into())), &ctx);
    assert_eq!(state.missing.as_deref(), Some("no chat pr9"));
    let _ = update(&mut state, Msg::Read(Ok(chat("pr9", "project"))), &ctx);
    assert!(state.missing.is_none());
    assert_eq!(state.chat.as_ref().map(|c| c.id.as_str()), Some("pr9"));
}

#[test]
fn a_question_answered_in_place_settles_its_card() {
    let ctx = ctx(None);
    let mut state = opened(Some("main".into()));
    let question = Card {
        id: "c1".into(),
        state: "open".into(),
        ..Default::default()
    };
    state.cards.insert("c1".into(), question.clone());
    let answered = CardAnswer {
        card: Card {
            state: "answered".into(),
            answer: Some("approve".into()),
            ..question
        },
        done: "Approved".into(),
    };
    let _ = update(
        &mut state,
        Msg::Answers(inbox::Msg::Answered("c1".into(), Ok(answered))),
        &ctx,
    );
    assert_eq!(
        state.cards.get("c1").map(|c| c.state.as_str()),
        Some("answered")
    );
    assert_eq!(state.note.as_deref(), Some("Approved"));
}

#[test]
fn a_chat_row_or_a_title_opens_the_chat_page() {
    assert_eq!(
        go(&Msg::Go(Id::Chat("pr1".into()))),
        Some(Id::Chat("pr1".into()))
    );
    assert_eq!(go(&Msg::Archived(true)), None);
}
