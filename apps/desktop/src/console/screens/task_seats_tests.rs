use serde_json::json;

use super::*;
use crate::console::client::Client;

fn ctx() -> Ctx {
    Ctx {
        client: Client::new("http://127.0.0.1:1", None, None),
        project: None,
    }
}

fn event(kind: &str, data: serde_json::Value) -> events::Event {
    events::Event {
        kind: kind.into(),
        data,
    }
}

#[test]
fn a_task_frame_of_this_task_reloads_the_seats_and_another_does_not() {
    let ctx = ctx();
    let mut panel = Panel::new("t3");
    let _ = update(
        &mut panel,
        Msg::Event(event("task", json!({ "id": "t9" }))),
        &ctx,
    );
    assert!(!panel.loading);
    let _ = update(
        &mut panel,
        Msg::Event(event("task", json!({ "id": "t3" }))),
        &ctx,
    );
    assert!(panel.loading);
}

#[test]
fn a_session_of_this_task_reloads_the_seats_and_a_frame_during_a_read_asks_for_one_more() {
    let ctx = ctx();
    let mut panel = Panel::new("t3");
    let _ = update(
        &mut panel,
        Msg::Event(event("session", json!({ "task": "t3" }))),
        &ctx,
    );
    assert!(panel.loading);
    let _ = update(
        &mut panel,
        Msg::Event(event("session", json!({ "task": "t3" }))),
        &ctx,
    );
    assert!(panel.again);
}

#[test]
fn a_failed_reload_keeps_the_seats_shown() {
    let ctx = ctx();
    let mut panel = Panel::new("t3");
    let seat = Seat {
        name: "builder".into(),
        ..Default::default()
    };
    let _ = update(&mut panel, Msg::Loaded(Ok(vec![seat])), &ctx);
    let _ = update(&mut panel, Msg::Loaded(Err("hub down".into())), &ctx);
    assert_eq!(panel.seats.as_ref().map(Vec::len), Some(1));
    assert_eq!(panel.failure.as_deref(), Some("hub down"));
}

#[test]
fn the_expert_view_is_kept() {
    let ctx = ctx();
    let mut panel = Panel::default();
    let _ = update(&mut panel, Msg::Expert(true), &ctx);
    assert!(panel.expert);
}
