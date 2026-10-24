use serde_json::json;

use super::*;
use crate::console::client::Client;
use crate::console::events::Event;

fn ctx() -> Ctx {
    Ctx {
        client: Client::new("http://127.0.0.1:1", None, None),
        project: None,
    }
}

fn frame(kind: &str) -> Event {
    Event {
        kind: kind.into(),
        data: json!({}),
    }
}

#[test]
fn a_session_frame_reloads_the_list_and_a_frame_during_a_load_asks_for_one_more() {
    let ctx = ctx();
    let mut state = State::default();
    let _ = update(&mut state, Msg::Event(frame("transcript")), &ctx);
    assert!(
        !state.loading,
        "a transcript frame does not reload the list"
    );
    let _ = update(&mut state, Msg::Event(frame("session")), &ctx);
    assert!(state.loading);
    let _ = update(&mut state, Msg::Event(frame("session")), &ctx);
    assert!(
        state.again,
        "a frame that comes during the load asks for one more"
    );
}

#[test]
fn a_failed_reload_keeps_the_rows_already_shown() {
    let ctx = ctx();
    let mut state = State::default();
    let first = vec![Session {
        id: "s1".into(),
        ..Default::default()
    }];
    let _ = update(&mut state, Msg::Loaded(Ok(first)), &ctx);
    let _ = update(&mut state, Msg::Loaded(Err("hub down".into())), &ctx);
    assert_eq!(state.sessions.as_ref().map(Vec::len), Some(1));
    assert_eq!(state.failure.as_deref(), Some("hub down"));
}

#[test]
fn the_live_filter_and_show_more_reset_the_rows_drawn() {
    let ctx = ctx();
    let mut state = State::default();
    let _ = update(&mut state, Msg::MoreRows, &ctx);
    assert!(state.show_all);
    let _ = update(&mut state, Msg::LiveOnly(true), &ctx);
    assert!(state.live_only);
    assert!(!state.show_all, "a new filter draws the first rows again");
}

#[test]
fn the_expert_view_is_kept_while_the_screen_is_open() {
    let ctx = ctx();
    let mut state = State::default();
    let _ = update(&mut state, Msg::Expert(true), &ctx);
    assert!(state.expert);
}

#[test]
fn a_row_opens_its_session_page() {
    assert_eq!(
        go(&Msg::Go(Id::Session("s12".into()))),
        Some(Id::Session("s12".into()))
    );
    assert_eq!(go(&Msg::MoreRows), None);
}
