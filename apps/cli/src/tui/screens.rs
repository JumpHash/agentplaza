//! The screens around the tabs: the header, the tab bar, each tab's body and the footer. Every value
//! is shown as the hub sent it. The Agents and Tasks tabs list the project the switcher shows
//! (`scope`), which the header names.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Cell, HighlightSpacing, Paragraph, Row, Table, TableState,
};
use serde_json::Value;

use super::app::{App, Palette, Tab};
use super::onboarding::Stage;
use super::scope;
use super::style::{chip, dot, hints, key_hint, mix, panel, pulse, spinner};
use super::workspace_view::tail;
use crate::cmd::agents::{held_of, label_of, place_of};
use crate::rich;

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
    match app
        .data
        .get(path)
        .and_then(|v| v.pointer(pointer))
        .and_then(Value::as_bool)
    {
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
    let reason = state
        .and_then(|v| v.get("reason"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
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
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        minute_of_day / 60,
        minute_of_day % 60
    )
}

/// What an agent is doing now: its work's kind and prompt, or "-".
fn doing(agent: &Value) -> String {
    match (agent.pointer("/work/kind"), agent.pointer("/work/prompt")) {
        (Some(kind), Some(prompt)) => format!("{}: {}", show(Some(kind)), show(Some(prompt))),
        _ => "-".to_string(),
    }
}

/// `dim` breathing up to `bright` on the clock. Only RGB colours breathe; the 16-colour and the
/// no-colour modes keep `bright` as it is.
fn breathe(bright: Color, dim: Color, clock: f32) -> Color {
    match bright {
        Color::Rgb(..) => mix(dim, bright, pulse(clock, 2.4)),
        _ => bright,
    }
}

/// The header row: the brand with its breathing hexagon, the hub's live dot, name and address, the
/// counts of working and idle agents, the project, and the HELD badge on the right when the plaza is
/// paused.
pub fn header(frame: &mut Frame, area: Rect, app: &App) {
    let p = app.palette;
    let name = show(app.data.get("/v1/hub/identity").and_then(|v| v.get("name")));
    let (working, idle) = app.agents().iter().fold((0usize, 0usize), |(w, i), agent| {
        match agent.get("status").and_then(Value::as_str) {
            Some("working") => (w + 1, i),
            Some("idle") => (w, i + 1),
            _ => (w, i),
        }
    });
    let muted = Style::new().fg(p.muted);
    let row = Style::new().bg(p.raised());
    let brand = breathe(p.accent, p.accent_soft(), app.clock);
    let mut spans = vec![
        Span::raw(" "),
        Span::styled("⬢", Style::new().fg(brand)),
        Span::styled(" agent", Style::new().fg(p.ink2())),
        Span::styled(
            "plaza",
            Style::new().fg(p.ink()).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  │  ", Style::new().fg(p.rule_strong())),
    ];
    if app.link {
        let live = breathe(p.ok, mix(p.ok, p.raised(), 0.55), app.clock);
        spans.push(Span::styled("● ", Style::new().fg(live)));
    } else {
        spans.push(Span::styled("○ offline ", muted));
    }
    spans.push(Span::styled(
        name,
        Style::new().fg(p.ink()).add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled(format!("  {}   ", app.client.base()), muted));
    if working > 0 {
        spans.push(Span::styled(
            format!("{} ", spinner(app.clock)),
            Style::new().fg(p.accent),
        ));
        spans.push(Span::styled(
            format!("{working} working"),
            Style::new().fg(p.accent),
        ));
    } else {
        spans.push(Span::styled("0 working", muted));
    }
    spans.push(Span::raw("   "));
    spans.push(Span::styled("● ", Style::new().fg(p.ok)));
    spans.push(Span::styled(format!("{idle} idle"), muted));
    spans.push(Span::raw("   "));
    spans.push(Span::styled(
        format!(
            "project · {}",
            scope::label(app.scope.as_deref(), app.projects())
        ),
        muted,
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)).style(row), area);
    if app.paused() {
        let badge = Line::from(chip(
            "■ HELD",
            p.on_accent(),
            mix(p.accent, p.crit(), pulse(app.clock, 1.6)),
        ));
        let width = u16::try_from(badge.width()).unwrap_or(0);
        if width < area.width {
            let rect = Rect::new(area.right() - width, area.y, width, area.height);
            frame.render_widget(Paragraph::new(badge).style(row), rect);
        }
    }
}

/// The tab row: every tab as its key and label, the active one as a chip. When the tabs are wider
/// than the row, the window holds the active tab with its neighbours where they fit, and a muted
/// `‹` or `›` shows that tabs are hidden on that side.
pub fn tab_bar(frame: &mut Frame, area: Rect, app: &App) {
    let p = app.palette;
    let n = Tab::ALL.len();
    let active = Tab::ALL.iter().position(|tab| *tab == app.tab).unwrap_or(0);
    // A tab is its label with a space and a key on each side of the label, or the same as a chip.
    let widths: Vec<usize> = Tab::ALL
        .iter()
        .map(|tab| tab.label().chars().count() + 4)
        .collect();
    let budget = usize::from(area.width);
    let used = |lo: usize, hi: usize| {
        widths[lo..=hi].iter().sum::<usize>() + usize::from(lo > 0) + usize::from(hi + 1 < n)
    };
    let (mut lo, mut hi) = (active, active);
    loop {
        let mut grew = false;
        if lo > 0 && used(lo - 1, hi) <= budget {
            lo -= 1;
            grew = true;
        }
        if hi + 1 < n && used(lo, hi + 1) <= budget {
            hi += 1;
            grew = true;
        }
        if !grew {
            break;
        }
    }
    let muted = Style::new().fg(p.muted);
    let mut spans = Vec::new();
    if lo > 0 {
        spans.push(Span::styled("‹", muted));
    }
    for tab in &Tab::ALL[lo..=hi] {
        if *tab == app.tab {
            spans.push(chip(
                &format!("{} {}", tab.key(), tab.label()),
                p.on_accent(),
                p.accent,
            ));
        } else {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(tab.key().to_string(), muted));
            spans.push(Span::styled(
                format!(" {} ", tab.label()),
                Style::new().fg(p.ink2()),
            ));
        }
    }
    if hi + 1 < n {
        spans.push(Span::styled("›", muted));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The keys a tab's status line shows, in the order they drop out when the row is narrow.
fn tab_keys(tab: Tab) -> &'static [(&'static str, &'static str)] {
    match tab {
        Tab::Workspace => &[
            ("^K", "commands"),
            ("Tab", "pane"),
            ("↑↓", "move"),
            ("/", "filter"),
            ("t", "steer"),
            ("i", "context"),
            ("M", "model"),
            ("p", "post"),
            ("P", "pause"),
            ("E", "export"),
            ("o", "IDE"),
            ("Enter", "expand"),
            ("1-8,w,c,m", "tabs"),
            ("q", "quit"),
        ],
        Tab::Workstations => &[
            ("^K", "commands"),
            ("1-8,w,c,m", "tabs"),
            ("Tab", "next"),
            ("↑↓", "move"),
            ("T", "take over"),
            ("r", "refresh"),
            ("?", "tour"),
            ("q", "quit"),
        ],
        Tab::Chat => &[
            ("^K", "commands"),
            ("1-8,w,c,m", "tabs"),
            ("↑↓", "conversation"),
            ("t", "message"),
            ("i", "context"),
            ("M", "model"),
            ("n", "new conversation"),
            ("S", "start the draft"),
            ("r", "refresh"),
            ("q", "quit"),
        ],
        _ => &[
            ("^K", "commands"),
            ("1-8,w,c,m", "tabs"),
            ("Tab", "next"),
            ("↑↓", "move"),
            ("Enter", "details"),
            ("t", "message"),
            ("p/P", "project"),
            ("a/d", "approve/deny"),
            ("s", "stop all"),
            ("r", "refresh"),
            ("?", "tour"),
            ("q", "quit"),
        ],
    }
}

/// The status line: a mode chip (the tab, or what is being typed or confirmed), its text, the keys
/// that fit, and the version on the right. The onboarding, confirm, typing, takeover and error
/// states take the chip and the text in their own colours.
pub fn footer(frame: &mut Frame, area: Rect, app: &App) {
    let p = app.palette;
    let cursor = if ((app.clock * 2.0) as i32) % 2 == 0 {
        "▌"
    } else {
        " "
    };
    let ink = Style::new().fg(p.ink());
    let ink2 = Style::new().fg(p.ink2());
    let accent = Style::new().fg(p.accent);
    let (lead, keys): (Vec<Span<'static>>, &[(&str, &str)]) = if let Some(ob) = &app.onboarding {
        let label = if matches!(ob.stage, Stage::Tour(_)) {
            "TOUR"
        } else {
            "SETUP"
        };
        let (text, style) = match &app.notice {
            Some(notice) => (notice.clone(), accent),
            None => (ob.hint().trim_start().to_string(), Style::new().fg(p.muted)),
        };
        (
            vec![
                chip(label, p.on_accent(), p.accent),
                Span::styled(format!(" {text}"), style),
            ],
            &[],
        )
    } else if let Some(confirm) = &app.confirm {
        (
            vec![
                chip("CONFIRM", p.on_accent(), p.crit()),
                Span::styled(format!(" {} ", confirm.question()), ink),
            ],
            &[("y", "yes"), ("n", "no")],
        )
    } else if let Some((cid, draft)) = &app.draft {
        (
            vec![
                chip("MESSAGE", p.on_accent(), p.accent),
                Span::styled(format!(" to agent {cid}  "), ink2),
                Span::styled(draft.clone(), ink),
                Span::styled(cursor, accent),
            ],
            &[("Enter", "sends"), ("Esc", "cancels")],
        )
    } else if let Some(compose) = &app.compose {
        let room = usize::from(area.width).saturating_sub(60);
        (
            vec![
                chip(&compose.label().to_uppercase(), p.on_accent(), p.accent),
                Span::styled(format!(" {}", tail(compose.text(), room)), ink),
                Span::styled(cursor, accent),
            ],
            &[("Enter", "sends"), ("Esc", "cancels")],
        )
    } else if let Some(panel) = &app.desk {
        (
            vec![
                chip("TAKEOVER", p.on_accent(), p.accent),
                Span::styled(format!(" {} · ", panel.station), ink2),
                Span::styled(format!("> {}", panel.line), ink),
                Span::styled(cursor, accent),
            ],
            &[
                ("Enter", "sends"),
                ("give-back / Esc", "gives the session back"),
            ],
        )
    } else if let Some(error) = &app.error {
        (
            vec![
                chip("OFFLINE", p.on_accent(), p.crit()),
                Span::styled(
                    format!(" hub unreachable: {error}"),
                    Style::new().fg(p.crit()),
                ),
            ],
            &[],
        )
    } else {
        let mut lead = vec![chip(
            &app.tab.label().to_uppercase(),
            p.on_accent(),
            p.accent,
        )];
        if let Some(notice) = &app.notice {
            lead.push(Span::styled(format!(" {notice}"), Style::new().fg(p.ok)));
        }
        (lead, tab_keys(app.tab))
    };
    let right = format!("agentplaza {}  ", env!("CARGO_PKG_VERSION"));
    let right_w = u16::try_from(right.chars().count()).unwrap_or(0);
    let lead_w: usize = lead.iter().map(|span| span.width()).sum();
    // The keys sit after three spaces, and one more column keeps them off the version.
    let room = usize::from(area.width).saturating_sub(lead_w + usize::from(right_w) + 4);
    let mut spans = lead;
    if !keys.is_empty() {
        spans.push(Span::raw("   "));
        let width = u16::try_from(room).unwrap_or(0);
        spans.extend(hints(keys, width, &p).spans);
    }
    let row = Style::new().bg(p.raised());
    frame.render_widget(Paragraph::new(Line::from(spans)).style(row), area);
    if right_w < area.width {
        let rect = Rect::new(area.right() - right_w, area.y, right_w, area.height);
        let text = Line::from(Span::styled(right, Style::new().fg(p.muted)));
        frame.render_widget(Paragraph::new(text).style(row), rect);
    }
}

/// A table's name, with the project the switcher shows when it shows one.
fn scoped(name: &str, app: &App) -> String {
    match app.scope.as_deref() {
        Some(_) => format!(
            "{name} of {}",
            scope::label(app.scope.as_deref(), app.projects())
        ),
        None => name.to_string(),
    }
}

/// A table's title: its name capitalised, the project the switcher shows when it shows one, and the count.
fn titled(name: &str, count: usize, app: &App) -> String {
    let base = scoped(name, app);
    let mut chars = base.chars();
    let head: String = chars
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    format!("{head}{} · {count}", chars.as_str())
}

/// The colour of a status word: working accent, idle and done ok, held or stopping warn, failed or
/// revoked crit, anything else muted.
fn status_colour(status: &str, p: &Palette) -> Color {
    match status {
        "working" => p.accent,
        "idle" | "online" | "done" => p.ok,
        "held" | "paused" | "stopping" => p.warn(),
        "failed" | "error" | "revoked" => p.crit(),
        _ => p.muted,
    }
}

/// A state as a cell: its dot (for `dot_status`, see `style::dot`), then the word in `colour`.
fn state_cell(dot_status: &str, word: &str, colour: Color, app: &App) -> Cell<'static> {
    Cell::from(Line::from(vec![
        dot(dot_status, app.clock, &app.palette),
        Span::styled(format!(" {word}"), Style::new().fg(colour)),
    ]))
}

/// A stat tile: a small rounded panel with its number in bold ink and its label under it in muted.
fn tile(frame: &mut Frame, area: Rect, value: Vec<Span<'static>>, label: &str, p: &Palette) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(p.rule_strong()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines = vec![
        Line::from(value),
        Line::from(Span::styled(label.to_string(), Style::new().fg(p.muted))),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

/// A number in bold ink, the tiles' figure.
fn figure(text: String, p: &Palette) -> Span<'static> {
    Span::styled(text, Style::new().fg(p.ink()).add_modifier(Modifier::BOLD))
}

/// A task state's colour on the overview's counts.
fn task_colour(state: &str, p: &Palette) -> Color {
    match state {
        "open" => p.ink(),
        "claimed" => p.accent,
        "awaiting_review" => p.review(),
        "rework" => p.warn(),
        "done" => p.ok,
        "blocked" | "failed" => p.crit(),
        _ => p.ink2(),
    }
}

pub fn overview(frame: &mut Frame, area: Rect, app: &App) {
    let p = &app.palette;
    let [tiles, rest] = Layout::vertical([Constraint::Length(4), Constraint::Min(0)]).areas(area);
    let [working, idle, open, waiting] = Layout::horizontal([Constraint::Fill(1); 4])
        .spacing(1)
        .areas(tiles);

    // The working tile leads with the spinner while anything works.
    let working_now = app
        .data
        .get("/v1/overview")
        .and_then(|v| v.pointer("/agents/working"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mut busy = Vec::new();
    if working_now > 0 {
        busy.push(Span::styled(
            format!("{} ", spinner(app.clock)),
            Style::new().fg(p.accent),
        ));
    }
    busy.push(figure(at(app, "/v1/overview", "/agents/working"), p));
    tile(frame, working, busy, "working", p);
    tile(
        frame,
        idle,
        vec![figure(at(app, "/v1/overview", "/agents/idle"), p)],
        "idle",
        p,
    );
    tile(
        frame,
        open,
        vec![figure(at(app, "/v1/overview", "/tasks/open"), p)],
        "tasks open",
        p,
    );
    // The poller fetches the approvals while the overview shows; "-" until the first answer.
    let waiting_text = if app.data.contains_key("/v1/approvals") {
        app.array("/v1/approvals")
            .iter()
            .filter(|a| is_pending(a))
            .count()
            .to_string()
    } else {
        "-".to_string()
    };
    tile(
        frame,
        waiting,
        vec![figure(waiting_text, p)],
        "approvals waiting",
        p,
    );

    let [left, right] = Layout::horizontal([Constraint::Fill(3), Constraint::Fill(2)])
        .spacing(1)
        .areas(rest);
    let [system, _, fleet, _] = Layout::vertical([
        Constraint::Length(6),
        Constraint::Length(1),
        Constraint::Length(4),
        Constraint::Min(0),
    ])
    .areas(left);

    let system_lines = vec![
        kv(
            "hub",
            format!(
                "{} {}",
                at(app, "/v1/hub/identity", "/name"),
                at(app, "/v1/hub/identity", "/version")
            ),
            app,
        ),
        kv("url", app.client.base().to_string(), app),
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
    ];
    frame.render_widget(
        Paragraph::new(system_lines).block(panel("System", p, false)),
        system,
    );

    let fleet_lines = vec![
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
    ];
    frame.render_widget(
        Paragraph::new(fleet_lines).block(panel("Fleet", p, false)),
        fleet,
    );

    let counts = app
        .data
        .get("/v1/overview")
        .and_then(|v| v.get("tasks"))
        .and_then(Value::as_object);
    let total: u64 = counts.map_or(0, |m| m.values().filter_map(Value::as_u64).sum());
    let mut task_lines: Vec<Line> = counts
        .into_iter()
        .flat_map(|m| m.iter())
        .map(|(state, count)| {
            Line::from(vec![
                Span::styled(format!("{state:>16}  "), Style::new().fg(p.muted)),
                Span::styled(
                    show(Some(count)),
                    Style::new()
                        .fg(task_colour(state, p))
                        .add_modifier(Modifier::BOLD),
                ),
            ])
        })
        .collect();
    if task_lines.is_empty() {
        task_lines.push(Line::from(Span::styled(
            "○  No tasks yet",
            Style::new().fg(p.muted),
        )));
    }
    frame.render_widget(
        Paragraph::new(task_lines).block(panel(&format!("Tasks · {total}"), p, false)),
        right,
    );
}

/// The keys a list answers, as the bottom edge's hint.
fn keys(items: &[(&str, &str)], p: &Palette) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, (key, label)) in items.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::new().fg(p.muted)));
        }
        spans.extend(key_hint(key, label, p));
    }
    Line::from(spans)
}

pub fn agents(frame: &mut Frame, area: Rect, app: &App) {
    let p = &app.palette;
    let list = app.shown_agents();
    let (table_area, detail_area) = if app.detail && !list.is_empty() {
        let [table, detail] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(12)]).areas(area);
        (table, Some(detail))
    } else {
        (area, None)
    };
    let rows: Vec<Vec<Cell<'static>>> = list
        .iter()
        .map(|a| {
            let status = show(a.get("status"));
            vec![
                Cell::from(show(a.get("cid"))),
                Cell::from(label_of(a)),
                Cell::from(show(a.get("role_label"))),
                Cell::from(place_of(a)),
                state_cell(&status, &status, status_colour(&status, p), app),
                Cell::from(show(a.get("model"))),
                Cell::from(show(a.get("queue"))),
                Cell::from(doing(a)),
            ]
        })
        .collect();
    grid(
        frame,
        table_area,
        Grid {
            title: titled("agents", list.len(), app),
            empty: "No agents here yet",
            header: &[
                "CID", "AGENT", "ROLE", "PROJECT", "STATUS", "MODEL", "QUEUE", "DOING",
            ],
            widths: &[
                Constraint::Length(6),
                Constraint::Length(18),
                Constraint::Length(16),
                Constraint::Length(8),
                Constraint::Length(11),
                Constraint::Length(24),
                Constraint::Length(6),
                Constraint::Fill(1),
            ],
            rows,
            hint: None,
        },
        app,
    );
    if let (Some(detail), Some(agent)) = (detail_area, list.get(app.selected).copied()) {
        let lines = vec![
            kv("agent", label_of(agent), app),
            kv(
                "role",
                format!(
                    "{} · {}",
                    show(agent.get("role_label")),
                    show(agent.get("tier"))
                ),
                app,
            ),
            kv(
                "model",
                format!(
                    "{} · from {}",
                    show(agent.get("model")),
                    show(agent.get("model_source"))
                ),
                app,
            ),
            kv("status", show(agent.get("status")), app),
            kv("project", place_of(agent), app),
            kv("held", held_of(agent), app),
            kv(
                "run / vm",
                format!("{} / {}", show(agent.get("run")), show(agent.get("vm"))),
                app,
            ),
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
        let block = panel("Agent", p, false).title_bottom(bottom_hint(hints(
            &[("t", "message"), ("Enter", "close")],
            detail.width,
            p,
        )));
        frame.render_widget(Paragraph::new(lines).block(block), detail);
    }
}

pub fn tasks(frame: &mut Frame, area: Rect, app: &App) {
    let p = &app.palette;
    let rich_ctx = apz_rich::Ctx::default();
    let shown = app.shown_tasks();
    let rows: Vec<Vec<Cell<'static>>> = shown
        .iter()
        .map(|t| {
            // The tasks it runs after (the Work screen's links); "-" for none.
            let after: Vec<&str> = t
                .get("after")
                .and_then(Value::as_array)
                .map(|ids| ids.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let word = crate::cmd::tasks::state_word(t);
            let (dot_status, colour) = match word {
                "working" => ("working", p.accent),
                "needs you" | "sent back" => ("held", p.warn()),
                "in review" => ("other", p.review()),
                "ready" => ("other", p.info()),
                "done" => ("idle", p.ok),
                "blocked" | "failed" => ("failed", p.crit()),
                _ => ("other", p.muted),
            };
            vec![
                Cell::from(clip(&show(t.get("id")), 10)),
                Cell::from(show(t.get("project"))),
                state_cell(dot_status, word, colour, app),
                Cell::from(show(t.get("role"))),
                Cell::from(show(t.get("agent_cid"))),
                Cell::from(if after.is_empty() {
                    "-".to_string()
                } else {
                    clip(&after.join(", "), 14)
                }),
                Cell::from(Line::from(rich::spans(&show(t.get("goal")), &rich_ctx))),
            ]
        })
        .collect();
    grid(
        frame,
        area,
        Grid {
            title: titled("tasks", shown.len(), app),
            empty: "No tasks yet",
            header: &["ID", "PROJECT", "STATE", "ROLE", "AGENT", "AFTER", "GOAL"],
            widths: &[
                Constraint::Length(10),
                Constraint::Length(8),
                Constraint::Length(13),
                Constraint::Length(12),
                Constraint::Length(7),
                Constraint::Length(14),
                Constraint::Fill(1),
            ],
            rows,
            hint: None,
        },
        app,
    );
}

/// Whether an approval waits for the owner's answer.
fn is_pending(approval: &Value) -> bool {
    approval.get("state").and_then(Value::as_str) == Some("pending")
}

/// An approval's state as a cell: a pulsing "waiting" chip while it waits, its word otherwise.
fn approval_state(state: &str, app: &App) -> Line<'static> {
    let p = &app.palette;
    match state {
        "pending" => {
            let bg = mix(
                p.accent_soft(),
                p.accent,
                0.25 + 0.5 * pulse(app.clock, 2.4),
            );
            Line::from(chip("waiting", p.ink(), bg))
        }
        "approved" => Line::from(Span::styled(state.to_string(), Style::new().fg(p.ok))),
        "denied" => Line::from(Span::styled(state.to_string(), Style::new().fg(p.crit()))),
        other => Line::from(Span::styled(other.to_string(), Style::new().fg(p.muted))),
    }
}

pub fn approvals(frame: &mut Frame, area: Rect, app: &App) {
    let p = &app.palette;
    let items = app.array("/v1/approvals");
    let rows: Vec<Vec<Cell<'static>>> = items
        .iter()
        .map(|a| {
            let id = clip(&show(a.get("id")), 10);
            // A waiting approval leads with the warn diamond; the others line up under it.
            let id = if is_pending(a) {
                Line::from(vec![
                    Span::styled("◆ ", Style::new().fg(p.warn())),
                    Span::styled(id, Style::new().fg(p.ink())),
                ])
            } else {
                Line::from(format!("  {id}"))
            };
            vec![
                Cell::from(id),
                Cell::from(show(a.get("cid"))),
                Cell::from(show(a.get("tool"))),
                Cell::from(approval_state(&show(a.get("state")), app)),
                Cell::from(show(a.get("input"))),
            ]
        })
        .collect();
    grid(
        frame,
        area,
        Grid {
            title: titled("approvals", items.len(), app),
            empty: "No approvals waiting",
            header: &["ID", "AGENT", "TOOL", "STATE", "INPUT"],
            widths: &[
                Constraint::Length(12),
                Constraint::Length(7),
                Constraint::Length(22),
                Constraint::Length(10),
                Constraint::Fill(1),
            ],
            rows,
            hint: Some(keys(&[("a", "approve"), ("d", "deny")], p)),
        },
        app,
    );
}

pub fn board(frame: &mut Frame, area: Rect, app: &App) {
    let rich_ctx = apz_rich::Ctx::default();
    let posts = app.wrapped("/v1/board", "posts");
    let rows: Vec<Vec<Cell<'static>>> = posts
        .iter()
        .map(|p| {
            let comments = p
                .get("comments")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            vec![
                Cell::from(format!(
                    "{} #{}",
                    show(p.get("role")),
                    show(p.get("agent_cid"))
                )),
                Cell::from(comments.to_string()),
                Cell::from(Line::from(rich::spans(&show(p.get("subject")), &rich_ctx))),
            ]
        })
        .collect();
    grid(
        frame,
        area,
        Grid {
            title: titled("board", posts.len(), app),
            empty: "The board is quiet",
            header: &["FROM", "COMMENTS", "SUBJECT"],
            widths: &[
                Constraint::Length(20),
                Constraint::Length(9),
                Constraint::Fill(1),
            ],
            rows,
            hint: None,
        },
        app,
    );
}

pub fn nodes(frame: &mut Frame, area: Rect, app: &App) {
    let p = &app.palette;
    let items = app.array("/v1/nodes");
    let rows: Vec<Vec<Cell<'static>>> = items
        .iter()
        .map(|n| {
            let hub = n.get("state").and_then(Value::as_str).unwrap_or("");
            // The word and the dot that shows it: the same states the list always showed.
            let (word, dot_status, colour) = if flag(n.get("revoked")) {
                ("revoked".to_string(), "failed", p.crit())
            } else if hub == "stopping" {
                let word = match n.get("left").and_then(Value::as_u64) {
                    Some(left) => format!("stopping ({left} to move)"),
                    None => "stopping".to_string(),
                };
                (word, "held", p.warn())
            } else if hub == "stopped" || (hub.is_empty() && flag(n.get("draining"))) {
                ("stopped".to_string(), "other", p.muted)
            } else if flag(n.get("online")) {
                ("online".to_string(), "idle", p.ok)
            } else {
                ("offline".to_string(), "other", p.muted)
            };
            let memory = match n.pointer("/machine/memory_mib") {
                Some(mib) if !mib.is_null() => format!("{} MiB", show(Some(mib))),
                _ => "-".to_string(),
            };
            vec![
                Cell::from(show(n.get("name"))),
                state_cell(dot_status, &word, colour, app),
                Cell::from(show(n.get("agents"))),
                Cell::from(show(n.pointer("/machine/cpus"))),
                Cell::from(memory),
                Cell::from(show(n.get("address"))),
            ]
        })
        .collect();
    grid(
        frame,
        area,
        Grid {
            title: titled("nodes", items.len(), app),
            empty: "No nodes yet",
            header: &["NAME", "STATE", "AGENTS", "CPUS", "MEMORY", "ADDRESS"],
            widths: &[
                Constraint::Length(14),
                Constraint::Length(23),
                Constraint::Length(8),
                Constraint::Length(6),
                Constraint::Length(12),
                Constraint::Fill(1),
            ],
            rows,
            hint: None,
        },
        app,
    );
}

/// One list's table: its title (with the count), the header, the rows, the column widths, the line a
/// list with no rows shows, and the keys its bottom edge hints.
struct Grid<'a, C> {
    title: String,
    empty: &'a str,
    header: &'a [&'a str],
    widths: &'a [Constraint],
    rows: Vec<Vec<C>>,
    hint: Option<Line<'static>>,
}

/// A panel's keys on its bottom edge: at the right, with a space kept from the corner and the frame.
fn bottom_hint(line: Line<'static>) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    spans.extend(line.spans);
    spans.push(Span::raw(" "));
    Line::from(spans).right_aligned()
}

/// A list's panel: its title, and its keys on the bottom edge at the right.
fn list_block(title: &str, hint: Option<Line<'static>>, p: &Palette) -> Block<'static> {
    let block = panel(title, p, false);
    match hint {
        Some(line) => block.title_bottom(bottom_hint(line)),
        None => block,
    }
}

/// A list with nothing in it: a muted glyph and line, centred in its panel.
fn empty_state(frame: &mut Frame, area: Rect, block: Block<'static>, text: &str, p: &Palette) {
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let row = Rect::new(inner.x, inner.y + (inner.height - 1) / 2, inner.width, 1);
    let line = Line::from(vec![
        Span::styled("○  ", Style::new().fg(p.muted)),
        Span::styled(text.to_string(), Style::new().fg(p.muted)),
    ])
    .centered();
    frame.render_widget(Paragraph::new(line), row);
}

/// A list as a table in a rounded panel. The selected row is marked by an accent bar and a raised
/// background; a rule sits under the header. Cells are text, or styled lines for agent text
/// (`rich::spans`).
fn grid<C: Into<Cell<'static>>>(frame: &mut Frame, area: Rect, spec: Grid<'_, C>, app: &App) {
    let p = &app.palette;
    let Grid {
        title,
        empty,
        header,
        widths,
        rows,
        hint,
    } = spec;
    let block = list_block(&title, hint, p);
    if rows.is_empty() {
        return empty_state(frame, area, block, empty, p);
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    // The header's bottom margin is the row the rule is drawn in.
    let head = Row::new(header.iter().copied())
        .style(Style::new().fg(p.muted).add_modifier(Modifier::BOLD))
        .bottom_margin(1);
    let body = rows.into_iter().map(Row::new);
    let table = Table::new(body, widths.to_vec())
        .header(head)
        .style(Style::new().fg(p.ink2()))
        .row_highlight_style(
            Style::new()
                .fg(p.ink())
                .bg(p.sheet())
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(Line::from("▌").style(Style::new().fg(p.accent)))
        .highlight_spacing(HighlightSpacing::Always)
        .column_spacing(2);
    let mut state = TableState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(table, inner, &mut state);
    if inner.height > 1 {
        let rule = Rect::new(inner.x, inner.y + 1, inner.width, 1);
        frame.render_widget(
            Paragraph::new("─".repeat(usize::from(inner.width)))
                .style(Style::new().fg(p.rule_strong())),
            rule,
        );
    }
}
