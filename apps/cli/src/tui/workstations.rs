//! The Workstations tab: the workstations enrolled on the hub (`GET /v1/stations`, the list `agentplaza
//! stations list` reads), one of them selected, and its detail as `agentplaza stations show` prints it: the
//! machine's facts, its settings with the takeover policy and its leases in queue order. Beside them, the work
//! folder's sync (a paused banner, the conflicts) and the recent recordings (`agentplaza stations sync` and
//! `recordings`). The tab refreshes on the hub's station, lease, takeover and sync events. `T` takes over the selected
//! station's shared session: a panel under the list takes the commands of `agentplaza stations take` in a line
//! (Enter sends it, Esc gives the session back). Declared from `app.rs`, which reads the selected workstation's sync
//! and recordings and holds the panel.

use std::sync::mpsc::Sender;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Row, Table};
use serde_json::Value;

use super::App;
use crate::cmd::stations;
use crate::cmd::stations::control;
use crate::cmd::stations::{files, recordings};

/// Conflicts listed in the detail before the rest are counted.
const CONFLICTS_SHOWN: usize = 3;
/// The takeover panel's height: its border, and the rows of what the takeover has reported.
const PANEL_ROWS: u16 = 5;
/// The reports a takeover panel keeps.
const PANEL_LOG: usize = 50;

/// A takeover of the selected workstation's session, open on the Workstations tab: the line being typed, the orders
/// to the takeover's thread, and what the thread has reported.
pub struct Panel {
    /// The panel's number: its thread's reports carry it, so a closed panel's late reports are not shown.
    pub number: u64,
    pub station: String,
    /// The line being typed (the commands; Enter sends it).
    pub line: String,
    pub log: Vec<String>,
    pub orders: Sender<control::Order>,
}

impl Panel {
    pub fn new(number: u64, station: String, orders: Sender<control::Order>) -> Panel {
        Panel {
            number,
            station,
            line: String::new(),
            log: Vec::new(),
            orders,
        }
    }

    /// Adds a report, keeping the newest `PANEL_LOG`.
    pub fn push(&mut self, text: String) {
        self.log.push(text);
        if self.log.len() > PANEL_LOG {
            self.log.remove(0);
        }
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let area = match &app.desk {
        Some(panel) => {
            let [main, takeover] =
                Layout::vertical([Constraint::Min(0), Constraint::Length(PANEL_ROWS)]).areas(area);
            takeover_log(frame, takeover, app, panel);
            main
        }
        None => area,
    };
    let [list, detail] = Layout::vertical([Constraint::Min(5), Constraint::Length(28)]).areas(area);
    table(frame, list, app);
    details(frame, detail, app);
}

fn table(frame: &mut Frame, area: Rect, app: &App) {
    let palette = app.palette;
    let all = app.array("/v1/stations");
    let block = Block::bordered().title(format!(" workstations · {} ", all.len()));
    if all.is_empty() {
        let empty = Line::from(Span::styled(
            "no workstations joined yet",
            Style::new().fg(palette.muted),
        ));
        frame.render_widget(Paragraph::new(empty).block(block), area);
        return;
    }
    let head = Row::new(
        [
            "ID",
            "NAME",
            "STATUS",
            "OS",
            "ACCOUNT",
            "LEASES",
            "LAST SEEN",
        ]
        .map(String::from),
    )
    .style(Style::new().fg(palette.muted).add_modifier(Modifier::BOLD));
    let body = all.iter().enumerate().map(|(i, station)| {
        let row = Row::new(stations::row(station));
        if i == app.selected {
            row.style(Style::new().fg(palette.paper).bg(palette.accent))
        } else {
            row
        }
    });
    let widths = [
        Constraint::Length(6),
        Constraint::Length(18),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Fill(1),
        Constraint::Length(7),
        Constraint::Length(12),
    ];
    frame.render_widget(Table::new(body, widths).header(head).block(block), area);
}

fn details(frame: &mut Frame, area: Rect, app: &App) {
    let palette = app.palette;
    let [facts, side] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(area);
    let Some(station) = app.array("/v1/stations").get(app.selected) else {
        frame.render_widget(
            Paragraph::new("").block(Block::bordered().title(" workstation ")),
            area,
        );
        return;
    };

    let mut lines: Vec<Line> = Vec::new();
    if let Some(warning) = stations::account_warning(station) {
        lines.push(Line::from(Span::styled(
            warning,
            Style::new().fg(palette.accent).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::raw(""));
    }
    lines.extend(pairs(&stations::station_fields(station), palette.muted));
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" workstation ")),
        facts,
    );

    let [settings_area, sync_area] =
        Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(side);
    let mut settings = pairs(
        &stations::settings_fields(&station["settings"]),
        palette.muted,
    );
    settings.push(Line::raw(""));
    settings.push(Line::from(Span::styled(
        "leases",
        Style::new().fg(palette.muted),
    )));
    let leases = stations::leases(station);
    if leases.is_empty() {
        settings.push(Line::raw("  none"));
    }
    for lease in leases {
        settings.push(Line::raw(format!(
            "  {}",
            stations::lease_row(lease).join("  ")
        )));
    }
    frame.render_widget(
        Paragraph::new(settings).block(Block::bordered().title(" settings and leases ")),
        settings_area,
    );
    sync_and_recordings(frame, sync_area, app, station);
}

/// The takeover panel's border, with the newest reports in it (the prompt is the footer's line).
fn takeover_log(frame: &mut Frame, area: Rect, app: &App, panel: &Panel) {
    let palette = app.palette;
    let shown = (PANEL_ROWS as usize).saturating_sub(2);
    let lines: Vec<Line> = if panel.log.is_empty() {
        vec![Line::from(Span::styled(
            "taking the session",
            Style::new().fg(palette.muted),
        ))]
    } else {
        let from = panel.log.len().saturating_sub(shown);
        panel.log[from..]
            .iter()
            .map(|text| Line::raw(text.clone()))
            .collect()
    };
    let title = format!(" takeover · {} ", panel.station);
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(title)),
        area,
    );
}

/// The work folder's sync (its paused banner, its state and its conflicts) and the recent recordings.
fn sync_and_recordings(frame: &mut Frame, area: Rect, app: &App, station: &Value) {
    let palette = app.palette;
    let id = station
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut lines: Vec<Line> = Vec::new();
    match app.station_sync.get(id) {
        Some(state) => {
            if let Some(banner) = files::paused_banner(state) {
                lines.push(Line::from(Span::styled(
                    banner,
                    Style::new().fg(palette.accent).add_modifier(Modifier::BOLD),
                )));
            }
            lines.extend(pairs(&files::sync_fields(state), palette.muted));
            let conflicts = state
                .get("conflicts")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            for conflict in conflicts.iter().take(CONFLICTS_SHOWN) {
                let cells = files::conflict_row(conflict);
                lines.push(Line::raw(format!(
                    "  conflict  {}  (copy of {})",
                    cells[0], cells[1]
                )));
            }
            if conflicts.len() > CONFLICTS_SHOWN {
                lines.push(Line::from(Span::styled(
                    format!("  and {} more conflicts", conflicts.len() - CONFLICTS_SHOWN),
                    Style::new().fg(palette.muted),
                )));
            }
        }
        None => lines.push(Line::from(Span::styled(
            "sync: reading",
            Style::new().fg(palette.muted),
        ))),
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "recent recordings",
        Style::new().fg(palette.muted),
    )));
    let recent = app
        .station_recent
        .get(id)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if recent.is_empty() {
        lines.push(Line::raw("  none"));
    }
    for recording in recent.iter().take(super::RECENT_RECORDINGS as usize) {
        lines.push(Line::raw(format!(
            "  {}",
            recordings::recording_brief(recording)
        )));
    }
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" sync and recordings ")),
        area,
    );
}

/// `label  value` lines, the labels in the muted colour.
fn pairs(pairs: &[(&'static str, String)], muted: Color) -> Vec<Line<'static>> {
    let width = pairs
        .iter()
        .map(|(label, _)| label.len())
        .max()
        .unwrap_or(0);
    pairs
        .iter()
        .map(|(label, value)| {
            Line::from(vec![
                Span::styled(format!("{label:>width$}  "), Style::new().fg(muted)),
                Span::raw(value.clone()),
            ])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_pair_line_has_its_label_then_its_value() {
        let lines = pairs(
            &[
                ("takeover", "agent-waits".to_string()),
                ("record fps", "2".to_string()),
            ],
            Color::Gray,
        );
        assert_eq!(lines.len(), 2);
        let text: String = lines[1]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(
            text, "record fps  2",
            "labels right-align to the longest label"
        );
    }

    #[test]
    fn the_detail_names_the_owner_account_before_the_facts() {
        let station =
            json!({ "id": "w1", "facts": { "account": { "mode": "owner", "name": "irfan" } } });
        assert_eq!(
            stations::account_warning(&station).as_deref(),
            Some("warning: agents act as irfan (the owner's own account)")
        );
        assert_eq!(
            stations::station_fields(&station)[0],
            ("id", "w1".to_string())
        );
    }
}
