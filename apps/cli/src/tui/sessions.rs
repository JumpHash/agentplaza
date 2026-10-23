//! The Sessions tab: the work engine's sessions, live first, then the recent ones (`GET /v1/sessions`). Each row is one
//! session: its state, its seat, task, project, model and what it has cost. Enter opens the selected one's details under
//! the table: its node and VM, its usage, and how it ended. Its transcript and context are read with
//! `agentplaza sessions transcript` and `context`. The project switcher (`p` and `P`) filters it, as it filters the
//! Tasks tab. Declared from `app.rs`, which holds the tab's state.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Cell, Paragraph};
use serde_json::Value;

use super::App;
use super::style::{hints, panel};
use crate::cmd::sessions::cost_text;
use crate::tui::screens::{Grid, bottom_hint, grid, kv, show, state_cell, status_colour, titled};

/// The height of the details under the table.
const DETAIL_HEIGHT: u16 = 12;

/// A session's state as the dot and the colour word the table shows it with: the working states, the paused, the queued,
/// the ended and the failed.
fn state_look(state: &str) -> (&'static str, &'static str) {
    match state {
        "running" | "forking" | "placing" | "compacting" | "handing_over" | "ending" => {
            ("working", "working")
        }
        "paused" => ("held", "paused"),
        "queued" => ("other", "queued"),
        "ended" => ("idle", "done"),
        "failed" => ("failed", "failed"),
        _ => ("other", "other"),
    }
}

/// A count from a session's usage (0 when missing).
fn count(session: &Value, key: &str) -> u64 {
    session
        .pointer(&format!("/usage/{key}"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

/// A session's state, and why it is paused when it is.
fn state_line(session: &Value) -> String {
    match session.get("paused_by").and_then(Value::as_str) {
        Some(why) => format!("{} (paused by {why})", show(session.get("state"))),
        None => show(session.get("state")),
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let p = &app.palette;
    let shown = app.shown_sessions();
    let (table_area, detail_area) = if app.detail && !shown.is_empty() {
        let [table, detail] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(DETAIL_HEIGHT)]).areas(area);
        (table, Some(detail))
    } else {
        (area, None)
    };
    let rows: Vec<Vec<Cell<'static>>> = shown
        .iter()
        .map(|s| {
            let state = show(s.get("state"));
            let (dot, word) = state_look(&state);
            vec![
                Cell::from(show(s.get("id"))),
                state_cell(dot, &state, status_colour(word, p), app),
                Cell::from(show(s.get("seat"))),
                Cell::from(show(s.get("task"))),
                Cell::from(show(s.get("project"))),
                Cell::from(show(s.get("model"))),
                Cell::from(cost_text(s)),
            ]
        })
        .collect();
    grid(
        frame,
        table_area,
        Grid {
            title: titled("sessions", shown.len(), app),
            empty: "No sessions yet",
            header: &["ID", "STATE", "SEAT", "TASK", "PROJECT", "MODEL", "COST"],
            widths: &[
                Constraint::Length(7),
                Constraint::Length(16),
                Constraint::Length(14),
                Constraint::Length(10),
                Constraint::Length(8),
                Constraint::Length(24),
                Constraint::Fill(1),
            ],
            rows,
            hint: None,
        },
        app,
    );
    if let (Some(detail), Some(session)) = (detail_area, shown.get(app.selected).copied()) {
        let lines = vec![
            kv("session", show(session.get("id")), app),
            kv("state", state_line(session), app),
            kv(
                "seat",
                format!(
                    "{} · {}",
                    show(session.get("seat")),
                    show(session.get("role"))
                ),
                app,
            ),
            kv(
                "task",
                format!(
                    "{} · {}",
                    show(session.get("task")),
                    show(session.get("project"))
                ),
                app,
            ),
            kv("model", show(session.get("model")), app),
            kv(
                "node / vm",
                format!(
                    "{} / {}",
                    show(session.get("node")),
                    show(session.get("cid"))
                ),
                app,
            ),
            kv(
                "tokens",
                format!(
                    "in {} · out {} · cached {}",
                    count(session, "tokens_in"),
                    count(session, "tokens_out"),
                    count(session, "tokens_cached")
                ),
                app,
            ),
            kv("cost", cost_text(session), app),
            kv("attempt", show(session.get("attempt")), app),
            kv("outcome", show(session.get("outcome")), app),
        ];
        let block = panel("Session", p, false).title_bottom(bottom_hint(hints(
            &[("Enter", "close")],
            detail.width,
            p,
        )));
        frame.render_widget(Paragraph::new(lines).block(block), detail);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use serde_json::json;

    use super::*;
    use crate::ctx::Client;

    #[test]
    fn a_session_state_has_a_dot_and_a_colour_word() {
        assert_eq!(state_look("running"), ("working", "working"));
        assert_eq!(state_look("paused"), ("held", "paused"));
        assert_eq!(state_look("ended"), ("idle", "done"));
        assert_eq!(state_look("failed"), ("failed", "failed"));
        assert_eq!(state_look("queued"), ("other", "queued"));
    }

    #[test]
    fn a_paused_session_says_who_paused_it() {
        let paused = json!({ "state": "paused", "paused_by": "budget" });
        assert_eq!(state_line(&paused), "paused (paused by budget)");
        assert_eq!(
            state_line(&json!({ "state": "running", "paused_by": null })),
            "running"
        );
    }

    /// The pane lists the sessions live first, and its details open under the table with the selected session.
    #[test]
    fn the_pane_lists_the_live_sessions_first_and_opens_the_selected_one() {
        let mut app = App::new(Client::new("http://127.0.0.1:9", None, None));
        app.data.insert(
            "/v1/sessions",
            json!([
                { "id": "s9", "state": "ended", "seat": "builder", "project": "pr3", "model": "m/a", "usage": {} },
                { "id": "s8", "state": "running", "seat": "reviewer:1", "project": "pr3", "model": "m/b", "usage": { "cost_micros": 420000 } }
            ]),
        );
        app.select_tab(super::super::Tab::Sessions);
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        app.detail = true;
        terminal
            .draw(|frame| render(frame, frame.area(), &app))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        let live = text.find("s8").expect("the running session is listed");
        let ended = text.find("s9").expect("the ended session is listed");
        assert!(live < ended, "the live session comes first");
        assert!(text.contains("$0.42"), "its cost shows");
        assert!(text.contains("reviewer:1 · "), "the details name its seat");
    }
}
