//! The Workstations tab: the workstations enrolled on the hub (`GET /v1/stations`, the list `agentplaza
//! stations list` reads), one of them selected, and its detail as `agentplaza stations show` prints it: the
//! machine's facts, its settings with the takeover policy, and its leases in queue order. Read-only; the
//! tab refreshes on the hub's station and lease events. Declared from `app.rs`.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Row, Table};

use super::App;
use crate::cmd::stations;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let [list, detail] = Layout::vertical([Constraint::Min(5), Constraint::Length(18)]).areas(area);
    table(frame, list, app);
    details(frame, detail, app);
}

fn table(frame: &mut Frame, area: Rect, app: &App) {
    let palette = app.palette;
    let all = app.array("/v1/stations");
    let block = Block::bordered().title(format!(" workstations · {} ", all.len()));
    if all.is_empty() {
        let empty = Line::from(Span::styled("no workstations joined yet", Style::new().fg(palette.muted)));
        frame.render_widget(Paragraph::new(empty).block(block), area);
        return;
    }
    let head = Row::new(["ID", "NAME", "STATUS", "OS", "ACCOUNT", "LEASES", "LAST SEEN"].map(String::from))
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
    let [facts, side] = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(area);
    let Some(station) = app.array("/v1/stations").get(app.selected) else {
        frame.render_widget(Paragraph::new("").block(Block::bordered().title(" workstation ")), area);
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
    frame.render_widget(Paragraph::new(lines).block(Block::bordered().title(" workstation ")), facts);

    let mut settings = pairs(&stations::settings_fields(&station["settings"]), palette.muted);
    settings.push(Line::raw(""));
    settings.push(Line::from(Span::styled("leases", Style::new().fg(palette.muted))));
    let leases = stations::leases(station);
    if leases.is_empty() {
        settings.push(Line::raw("  none"));
    }
    for lease in leases {
        settings.push(Line::raw(format!("  {}", stations::lease_row(lease).join("  "))));
    }
    frame.render_widget(
        Paragraph::new(settings).block(Block::bordered().title(" settings and leases ")),
        side,
    );
}

/// `label  value` lines, the labels in the muted colour.
fn pairs(pairs: &[(&'static str, String)], muted: Color) -> Vec<Line<'static>> {
    let width = pairs.iter().map(|(label, _)| label.len()).max().unwrap_or(0);
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
            &[("takeover", "agent-waits".to_string()), ("record fps", "2".to_string())],
            Color::Gray,
        );
        assert_eq!(lines.len(), 2);
        let text: String = lines[1].spans.iter().map(|span| span.content.as_ref()).collect();
        assert_eq!(text, "record fps  2", "labels right-align to the longest label");
    }

    #[test]
    fn the_detail_names_the_owner_account_before_the_facts() {
        let station = json!({ "id": "w1", "facts": { "account": { "mode": "owner", "name": "irfan" } } });
        assert_eq!(
            stations::account_warning(&station).as_deref(),
            Some("warning: agents act as irfan (the owner's own account)")
        );
        assert_eq!(stations::station_fields(&station)[0], ("id", "w1".to_string()));
    }
}
