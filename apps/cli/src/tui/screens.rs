//! The screens around the floor: the header, the tab bar, each tab's body, the event log and the
//! footer. Every value is shown as the hub sent it.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Row, Table};
use serde_json::Value;

use super::app::{App, Tab};

/// A value as text: strings bare, missing or null as "-", anything else as its JSON.
pub fn show(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "-".to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Bool(on)) => if *on { "yes" } else { "no" }.to_string(),
        Some(other) => other.to_string(),
    }
}

fn flag(value: Option<&Value>) -> bool {
    value.and_then(Value::as_bool).unwrap_or(false)
}

/// Cuts `text` to `max` characters; the last one becomes an ellipsis when it was cut.
pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
        cut.push('…');
        cut
    }
}

/// One value of the header of a detail block: a muted label, then the value.
fn kv(label: &str, value: String, app: &App) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:>12}  "), Style::new().fg(app.palette.muted)),
        Span::raw(value),
    ])
}

/// A field of an endpoint's answer, by JSON pointer.
fn at(app: &App, path: &str, pointer: &str) -> String {
    show(app.data.get(path).and_then(|v| v.pointer(pointer)))
}

/// A service's flag from an endpoint's answer as a word: `up` or `down`.
fn up(app: &App, path: &str, pointer: &str) -> String {
    match app.data.get(path).and_then(|v| v.pointer(pointer)).and_then(Value::as_bool) {
        Some(true) => "up".to_string(),
        Some(false) => "down".to_string(),
        None => "-".to_string(),
    }
}

/// The plaza's state: `running`, or `HELD by <who> since <when>`, with the reason after a colon
/// only when there is one.
fn plaza(app: &App) -> String {
    if !app.paused() {
        return "running".to_string();
    }
    let state = app.data.get("/v1/plaza/state");
    let by = show(state.and_then(|v| v.get("by")));
    let since = state
        .and_then(|v| v.get("since_ms"))
        .and_then(Value::as_i64)
        .map(|ms| format!(" since {}", local_time(ms)))
        .unwrap_or_default();
    let reason = state.and_then(|v| v.get("reason")).and_then(Value::as_str).unwrap_or_default().trim();
    if reason.is_empty() {
        format!("HELD by {by}{since}")
    } else {
        format!("HELD by {by}{since}: {reason}")
    }
}

/// Epoch milliseconds as `YYYY-MM-DD HH:MM`. The crate carries no time-zone database, so this is
/// UTC; a local conversion can replace the body once one is a dependency.
pub fn local_time(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400) + 719_468;
    let minute_of_day = secs.rem_euclid(86_400) / 60;
    let era = days.div_euclid(146_097);
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}", minute_of_day / 60, minute_of_day % 60)
}

/// What an agent is doing now: its work's kind and prompt, or "-".
fn doing(agent: &Value) -> String {
    match (agent.pointer("/work/kind"), agent.pointer("/work/prompt")) {
        (Some(kind), Some(prompt)) => format!("{}: {}", show(Some(kind)), show(Some(prompt))),
        _ => "-".to_string(),
    }
}

pub fn header(frame: &mut Frame, area: Rect, app: &App) {
    let palette = app.palette;
    let name = show(app.data.get("/v1/hub/identity").and_then(|v| v.get("name")));
    let (working, idle) = app.agents().iter().fold((0usize, 0usize), |(w, i), agent| {
        match agent.get("status").and_then(Value::as_str) {
            Some("working") => (w + 1, i),
            Some("idle") => (w, i + 1),
            _ => (w, i),
        }
    });
    let mut spans = vec![
        Span::styled(format!(" {name} "), Style::new().add_modifier(Modifier::BOLD)),
        Span::styled(format!("{}   ", app.client.base()), Style::new().fg(palette.muted)),
        Span::raw(format!("working {working}  idle {idle}  ")),
    ];
    if app.paused() {
        spans.push(Span::styled(
            " HELD ",
            Style::new().fg(palette.paper).bg(palette.accent).add_modifier(Modifier::BOLD),
        ));
    }
    if !app.link {
        spans.push(Span::styled("  events offline", Style::new().fg(palette.muted)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub fn tab_bar(frame: &mut Frame, area: Rect, app: &App) {
    let palette = app.palette;
    let mut spans = Vec::new();
    for (i, tab) in Tab::ALL.iter().enumerate() {
        let style = if *tab == app.tab {
            Style::new().fg(palette.paper).bg(palette.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(palette.muted)
        };
        spans.push(Span::styled(format!(" {} {} ", i + 1, tab.label()), style));
        spans.push(Span::raw(" "));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The last events the floor's log holds, as many as the area has lines for.
pub fn event_log(frame: &mut Frame, area: Rect, app: &App) {
    let palette = app.palette;
    let rows = usize::from(area.height.saturating_sub(2));
    let width = usize::from(area.width.saturating_sub(2));
    let skip = app.log.len().saturating_sub(rows);
    let lines: Vec<Line> = app
        .log
        .iter()
        .skip(skip)
        .map(|line| Line::from(Span::styled(clip(line, width), Style::new().fg(palette.muted))))
        .collect();
    frame.render_widget(Paragraph::new(lines).block(Block::bordered().title(" events ")), area);
}

pub fn footer(frame: &mut Frame, area: Rect, app: &App) {
    let palette = app.palette;
    let line = if let Some(ob) = &app.onboarding {
        match &app.notice {
            Some(notice) => Line::from(Span::styled(format!(" {notice}"), Style::new().fg(palette.accent))),
            None => Line::from(Span::styled(ob.hint(), Style::new().fg(palette.muted))),
        }
    } else if let Some(confirm) = &app.confirm {
        Line::from(Span::styled(
            format!(" {}  [y] yes   [n] no ", confirm.question()),
            Style::new().fg(palette.paper).bg(palette.accent),
        ))
    } else if let Some((cid, draft)) = &app.draft {
        Line::from(vec![
            Span::styled(format!(" message to agent {cid}: "), Style::new().fg(palette.accent)),
            Span::raw(format!("{draft}_")),
            Span::styled("   Enter sends · Esc cancels", Style::new().fg(palette.muted)),
        ])
    } else if let Some(error) = &app.error {
        Line::from(Span::styled(format!(" hub unreachable: {error}"), Style::new().fg(palette.accent)))
    } else if let Some(notice) = &app.notice {
        Line::from(Span::styled(format!(" {notice}"), Style::new().fg(palette.ok)))
    } else {
        Line::from(Span::styled(
            " 1-9 tabs · Tab next · ↑↓ move · Enter details · t message · a/d approve/deny · s stop all · r refresh · ? tour · q quit",
            Style::new().fg(palette.muted),
        ))
    };
    frame.render_widget(Paragraph::new(line), area);
}

pub fn overview(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines = vec![
        kv("hub", format!("{} {}", at(app, "/v1/hub/identity", "/name"), at(app, "/v1/hub/identity", "/version")), app),
        kv("url", app.client.base().to_string(), app),
        kv(
            "agents",
            format!(
                "{} total · {} working · {} idle · {} offline",
                at(app, "/v1/overview", "/agents/total"),
                at(app, "/v1/overview", "/agents/working"),
                at(app, "/v1/overview", "/agents/idle"),
                at(app, "/v1/overview", "/agents/offline"),
            ),
            app,
        ),
        kv(
            "models",
            format!(
                "{} default · {} orchestrator",
                at(app, "/v1/overview", "/default_model"),
                at(app, "/v1/overview", "/orchestrator_model"),
            ),
            app,
        ),
        kv(
            "services",
            format!(
                "hub {} · noded {}",
                up(app, "/v1/overview", "/hub"),
                up(app, "/v1/overview", "/noded"),
            ),
            app,
        ),
        kv("plaza", plaza(app), app),
        Line::from(""),
        Line::from(Span::styled("tasks", Style::new().fg(app.palette.muted))),
    ];
    if let Some(tasks) = app.data.get("/v1/overview").and_then(|v| v.get("tasks")).and_then(Value::as_object) {
        for (state, count) in tasks {
            lines.push(kv(state, count.to_string(), app));
        }
    }
    frame.render_widget(Paragraph::new(lines).block(Block::bordered().title(" overview ")), area);
}

pub fn agents(frame: &mut Frame, area: Rect, app: &App) {
    let list = app.agents();
    let (table_area, detail_area) = if app.detail && !list.is_empty() {
        let [table, detail] = Layout::vertical([Constraint::Min(0), Constraint::Length(10)]).areas(area);
        (table, Some(detail))
    } else {
        (area, None)
    };
    let rows: Vec<Vec<String>> = list
        .iter()
        .map(|a| {
            vec![
                show(a.get("cid")),
                show(a.get("name")),
                show(a.get("role_label")),
                show(a.get("status")),
                show(a.get("model")),
                show(a.get("queue")),
                doing(a),
            ]
        })
        .collect();
    grid(
        frame,
        table_area,
        "agents",
        &["CID", "NAME", "ROLE", "STATUS", "MODEL", "QUEUE", "DOING"],
        rows,
        &[
            Constraint::Length(6),
            Constraint::Length(14),
            Constraint::Length(16),
            Constraint::Length(9),
            Constraint::Length(24),
            Constraint::Length(6),
            Constraint::Fill(1),
        ],
        app,
    );
    if let (Some(detail), Some(agent)) = (detail_area, list.get(app.selected)) {
        let lines = vec![
            kv("name", show(agent.get("name")), app),
            kv("role", format!("{} · {}", show(agent.get("role_label")), show(agent.get("tier"))), app),
            kv(
                "model",
                format!("{} · from {}", show(agent.get("model")), show(agent.get("model_source"))),
                app,
            ),
            kv("status", show(agent.get("status")), app),
            kv("run / vm", format!("{} / {}", show(agent.get("run")), show(agent.get("vm"))), app),
            kv("queue", show(agent.get("queue")), app),
            kv("doing", doing(agent), app),
            kv(
                "tokens",
                format!(
                    "in {} · out {}",
                    show(agent.pointer("/usage/input_tokens")),
                    show(agent.pointer("/usage/output_tokens")),
                ),
                app,
            ),
        ];
        frame.render_widget(
            Paragraph::new(lines).block(Block::bordered().title(" agent · t message · Enter close ")),
            detail,
        );
    }
}

pub fn tasks(frame: &mut Frame, area: Rect, app: &App) {
    let rows: Vec<Vec<String>> = app
        .wrapped("/v1/tasks", "tasks")
        .iter()
        .map(|t| {
            vec![
                clip(&show(t.get("id")), 10),
                show(t.get("state")),
                show(t.get("role")),
                show(t.get("agent_cid")),
                show(t.get("goal")),
            ]
        })
        .collect();
    grid(
        frame,
        area,
        "tasks",
        &["ID", "STATE", "ROLE", "AGENT", "GOAL"],
        rows,
        &[
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(7),
            Constraint::Fill(1),
        ],
        app,
    );
}

pub fn approvals(frame: &mut Frame, area: Rect, app: &App) {
    let rows: Vec<Vec<String>> = app
        .array("/v1/approvals")
        .iter()
        .map(|a| {
            vec![
                clip(&show(a.get("id")), 10),
                show(a.get("cid")),
                show(a.get("tool")),
                show(a.get("state")),
                show(a.get("input")),
            ]
        })
        .collect();
    grid(
        frame,
        area,
        "approvals · a approve · d deny",
        &["ID", "AGENT", "TOOL", "STATE", "INPUT"],
        rows,
        &[
            Constraint::Length(10),
            Constraint::Length(7),
            Constraint::Length(22),
            Constraint::Length(10),
            Constraint::Fill(1),
        ],
        app,
    );
}

pub fn board(frame: &mut Frame, area: Rect, app: &App) {
    let rows: Vec<Vec<String>> = app
        .wrapped("/v1/board", "posts")
        .iter()
        .map(|p| {
            let comments = p.get("comments").and_then(Value::as_array).map_or(0, Vec::len);
            vec![
                format!("{} #{}", show(p.get("role")), show(p.get("agent_cid"))),
                comments.to_string(),
                show(p.get("subject")),
            ]
        })
        .collect();
    grid(
        frame,
        area,
        "board",
        &["FROM", "COMMENTS", "SUBJECT"],
        rows,
        &[Constraint::Length(20), Constraint::Length(9), Constraint::Fill(1)],
        app,
    );
}

pub fn conversations(frame: &mut Frame, area: Rect, app: &App) {
    let rows: Vec<Vec<String>> = app
        .wrapped("/v1/conversations", "conversations")
        .iter()
        .map(|c| {
            vec![
                format!("{} #{}", show(c.get("from_role")), show(c.get("from_cid"))),
                format!("{} #{}", show(c.get("to_role")), show(c.get("to_cid"))),
                if flag(c.get("delivered")) { "yes" } else { "no" }.to_string(),
                show(c.get("text")),
            ]
        })
        .collect();
    grid(
        frame,
        area,
        "conversations",
        &["FROM", "TO", "DELIVERED", "TEXT"],
        rows,
        &[
            Constraint::Length(20),
            Constraint::Length(20),
            Constraint::Length(10),
            Constraint::Fill(1),
        ],
        app,
    );
}

pub fn nodes(frame: &mut Frame, area: Rect, app: &App) {
    let rows: Vec<Vec<String>> = app
        .array("/v1/nodes")
        .iter()
        .map(|n| {
            let state = if flag(n.get("revoked")) {
                "revoked"
            } else if flag(n.get("draining")) {
                "draining"
            } else if flag(n.get("online")) {
                "online"
            } else {
                "offline"
            };
            let memory = match n.pointer("/machine/memory_mib") {
                Some(mib) if !mib.is_null() => format!("{} MiB", show(Some(mib))),
                _ => "-".to_string(),
            };
            vec![
                show(n.get("name")),
                state.to_string(),
                show(n.get("agents")),
                show(n.pointer("/machine/cpus")),
                memory,
                show(n.get("address")),
            ]
        })
        .collect();
    grid(
        frame,
        area,
        "nodes",
        &["NAME", "STATE", "AGENTS", "CPUS", "MEMORY", "ADDRESS"],
        rows,
        &[
            Constraint::Length(14),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(6),
            Constraint::Length(12),
            Constraint::Fill(1),
        ],
        app,
    );
}

/// A table of rows under a header; the selected row is drawn in the accent colour.
fn grid(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    header: &[&str],
    rows: Vec<Vec<String>>,
    widths: &[Constraint],
    app: &App,
) {
    let palette = app.palette;
    let block = Block::bordered().title(format!(" {title} · {} ", rows.len()));
    if rows.is_empty() {
        let empty = Line::from(Span::styled("nothing here yet", Style::new().fg(palette.muted)));
        frame.render_widget(Paragraph::new(empty).block(block), area);
        return;
    }
    let head = Row::new(header.iter().map(|h| h.to_string()))
        .style(Style::new().fg(palette.muted).add_modifier(Modifier::BOLD));
    let body = rows.into_iter().enumerate().map(|(i, cells)| {
        let row = Row::new(cells);
        if i == app.selected {
            row.style(Style::new().fg(palette.paper).bg(palette.accent))
        } else {
            row
        }
    });
    frame.render_widget(Table::new(body, widths.to_vec()).header(head).block(block), area);
}
