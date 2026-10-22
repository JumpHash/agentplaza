use serde_json::json;

use super::*;
use crate::console::client::Client;
use crate::console::events::Event;
use crate::console::work_api::SessionState;

fn ctx() -> Ctx {
    Ctx {
        client: Client::new("http://127.0.0.1:1", None, None),
        project: None,
    }
}

fn session(id: &str, cid: Option<u64>, kind: SessionKind, state: SessionState) -> Session {
    Session {
        id: id.into(),
        project: "pr1".into(),
        kind,
        state,
        cid,
        ..Default::default()
    }
}

fn entry(seq: u64, kind: &str) -> TranscriptEntry {
    TranscriptEntry {
        cid: 105,
        seq,
        kind: kind.into(),
        ..Default::default()
    }
}

fn read(
    session: Session,
    owner: &str,
    fresh: bool,
    entries: Vec<TranscriptEntry>,
    cursor: u64,
) -> Fetched {
    Fetched {
        session: Some(session),
        context: None,
        owner: owner.into(),
        fresh_log: fresh,
        entries,
        cursor,
        failure: None,
    }
}

fn frame(kind: &str, data: serde_json::Value) -> Event {
    Event {
        kind: kind.into(),
        data,
    }
}

#[test]
fn a_read_puts_the_session_and_its_log_on_the_page() {
    let ctx = ctx();
    let mut state = opened("s1".into());
    let s = session("s1", Some(105), SessionKind::Step, SessionState::Running);
    let entries = vec![entry(1, "work_start"), entry(2, "model_response")];
    let _ = update(
        &mut state,
        Msg::Loaded(Ok(read(s, "s1@105", true, entries, 2))),
        &ctx,
    );
    assert!(state.loaded);
    assert_eq!(state.session.as_ref().map(|s| s.id.as_str()), Some("s1"));
    let log = state.log.as_ref().expect("a log for the session");
    assert_eq!(log.entries.len(), 2);
    assert_eq!(log.cursor, 2);
}

#[test]
fn the_next_read_adds_only_the_entries_after_the_ones_held() {
    let ctx = ctx();
    let mut state = opened("s1".into());
    let s = session("s1", Some(105), SessionKind::Step, SessionState::Running);
    let _ = update(
        &mut state,
        Msg::Loaded(Ok(read(
            s.clone(),
            "s1@105",
            true,
            vec![entry(1, "work_start")],
            1,
        ))),
        &ctx,
    );
    let _ = update(
        &mut state,
        Msg::Loaded(Ok(read(
            s,
            "s1@105",
            false,
            vec![entry(2, "model_response"), entry(3, "work_end")],
            3,
        ))),
        &ctx,
    );
    let log = state.log.as_ref().unwrap();
    let seqs: Vec<u64> = log.entries.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, [1, 2, 3]);
    assert_eq!(log.cursor, 3);
}

#[test]
fn a_new_session_on_the_vm_starts_a_log_of_its_own() {
    let ctx = ctx();
    let mut state = opened("105".into());
    let first = session("s1", Some(105), SessionKind::Step, SessionState::Ended);
    let _ = update(
        &mut state,
        Msg::Loaded(Ok(read(
            first,
            "s1@105",
            true,
            vec![entry(1, "work_start")],
            1,
        ))),
        &ctx,
    );
    let second = session("s2", Some(105), SessionKind::Step, SessionState::Running);
    let _ = update(
        &mut state,
        Msg::Loaded(Ok(read(
            second,
            "s2@105",
            true,
            vec![entry(7, "work_start")],
            7,
        ))),
        &ctx,
    );
    let log = state.log.as_ref().unwrap();
    assert_eq!(log.owner, "s2@105");
    assert_eq!(
        log.entries.len(),
        1,
        "the first session's entries are not carried over"
    );
}

#[test]
fn a_log_keeps_only_the_newest_entries_and_counts_the_rest() {
    let ctx = ctx();
    let mut state = opened("s1".into());
    let s = session("s1", Some(105), SessionKind::Step, SessionState::Running);
    let entries: Vec<TranscriptEntry> = (1..=(LOG_HELD as u64 + 5))
        .map(|seq| entry(seq, "tool_request"))
        .collect();
    let cursor = LOG_HELD as u64 + 5;
    let _ = update(
        &mut state,
        Msg::Loaded(Ok(read(s, "s1@105", true, entries, cursor))),
        &ctx,
    );
    let log = state.log.as_ref().unwrap();
    assert_eq!(log.entries.len(), LOG_HELD);
    assert_eq!(log.dropped, 5);
    assert_eq!(log.entries[0].seq, 6, "the oldest held entry is the sixth");
}

#[test]
fn a_read_that_finds_no_session_leaves_the_page_saying_so() {
    let ctx = ctx();
    let mut state = opened("105".into());
    let _ = update(&mut state, Msg::Loaded(Ok(Fetched::default())), &ctx);
    assert!(state.loaded);
    assert!(state.session.is_none());
    assert!(state.log.is_none());
}

#[test]
fn a_failed_read_keeps_the_session_shown_and_says_why() {
    let ctx = ctx();
    let mut state = opened("s1".into());
    let s = session("s1", None, SessionKind::Step, SessionState::Running);
    let _ = update(
        &mut state,
        Msg::Loaded(Ok(read(s, "s1@", true, vec![], 0))),
        &ctx,
    );
    let _ = update(
        &mut state,
        Msg::Loaded(Err("Could not load the session: gone".into())),
        &ctx,
    );
    assert!(state.session.is_some(), "the session stays on the page");
    assert_eq!(
        state.failure.as_deref(),
        Some("Could not load the session: gone")
    );
}

#[test]
fn stopping_a_planner_asks_first_and_a_step_stops_at_once() {
    let ctx = ctx();
    let mut state = opened("s1".into());
    state.session = Some(session(
        "s1",
        None,
        SessionKind::Long,
        SessionState::Running,
    ));
    let _ = update(&mut state, Msg::AskStop, &ctx);
    assert!(state.confirm_stop, "a long-lived session asks first");
    assert!(!state.stopping);
    let _ = update(&mut state, Msg::CancelStop, &ctx);
    assert!(!state.confirm_stop);

    state.session = Some(session(
        "s1",
        None,
        SessionKind::Step,
        SessionState::Running,
    ));
    let _ = update(&mut state, Msg::AskStop, &ctx);
    assert!(state.stopping, "a step stops at once");
    assert!(!state.confirm_stop);
}

#[test]
fn a_stopped_session_is_shown_ended_with_its_note() {
    let ctx = ctx();
    let mut state = opened("s1".into());
    state.stopping = true;
    let ended = session("s1", None, SessionKind::Step, SessionState::Ended);
    let _ = update(&mut state, Msg::Stopped(Ok(ended)), &ctx);
    assert!(!state.stopping);
    assert_eq!(state.note.as_deref(), Some("Session stopped"));
    assert_eq!(
        state.session.as_ref().map(|s| s.state),
        Some(SessionState::Ended)
    );
}

#[test]
fn a_session_frame_reloads_the_page_only_for_its_own_session() {
    let ctx = ctx();
    let mut state = opened("s1".into());
    state.session = Some(session(
        "s1",
        Some(105),
        SessionKind::Step,
        SessionState::Running,
    ));
    let _ = update(
        &mut state,
        Msg::Event(frame("session", json!({ "id": "s2" }))),
        &ctx,
    );
    assert!(!state.loading);
    let _ = update(
        &mut state,
        Msg::Event(frame("session", json!({ "id": "s1" }))),
        &ctx,
    );
    assert!(state.loading);
}

#[test]
fn a_page_opened_on_a_vm_follows_every_session_frame() {
    let ctx = ctx();
    let mut state = opened("105".into());
    let _ = update(
        &mut state,
        Msg::Event(frame("session", json!({ "id": "s9" }))),
        &ctx,
    );
    assert!(state.loading);
}

#[test]
fn a_transcript_frame_reloads_the_page_when_it_is_of_the_pages_vm() {
    let ctx = ctx();
    let mut state = opened("s1".into());
    state.session = Some(session(
        "s1",
        Some(105),
        SessionKind::Step,
        SessionState::Running,
    ));
    let _ = update(
        &mut state,
        Msg::Event(frame("transcript", json!({ "cid": 106 }))),
        &ctx,
    );
    assert!(!state.loading);
    let _ = update(
        &mut state,
        Msg::Event(frame("transcript", json!({ "cid": 105 }))),
        &ctx,
    );
    assert!(state.loading);
}

#[test]
fn the_context_tab_reads_the_context() {
    let ctx = ctx();
    let mut state = opened("s1".into());
    let _ = update(&mut state, Msg::Tab(Tab::Context), &ctx);
    assert_eq!(state.tab, Tab::Context);
    assert!(state.loading);
}

#[test]
fn a_page_names_its_reference_and_a_link_goes_to_the_list() {
    assert_eq!(opened("105".into()).reference, "105");
    assert_eq!(go(&Msg::Go(Id::Sessions)), Some(Id::Sessions));
}
