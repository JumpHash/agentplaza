//! The Machines tab: the machines the plaza runs on (`GET /v1/machines`, the list `agentplaza
//! machines list` reads), one of them selected, and its detail as `agentplaza machines show` prints
//! it: hardware, load, the plaza's usage, and its runs. Declared from `app.rs`.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Row, Table};
use serde_json::Value;

use super::style::{chip, dot, panel};
use super::{App, Palette};
use crate::tui::screens::{clip, show};

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let [list, detail] = Layout::vertical([Constraint::Min(5), Constraint::Length(18)]).areas(area);
    table(frame, list, app);
    details(frame, detail, app);
}

fn table(frame: &mut Frame, area: Rect, app: &App) {
    let palette = app.palette;
    let machines = app.array("/v1/machines");
    let block = panel(&format!("Machines · {}", machines.len()), &palette, true);
    if machines.is_empty() {
        let empty = Line::from(Span::styled(
            "no machines reported yet",
            Style::new().fg(palette.muted),
        ));
        frame.render_widget(Paragraph::new(empty).block(block), area);
        return;
    }
    let head =
        Row::new(["NAME", "STATE", "AGENTS", "RUNS", "CPUS", "MEMORY", "CPU"].map(String::from))
            .style(Style::new().fg(palette.ink2()).add_modifier(Modifier::BOLD));
    let body = machines.iter().enumerate().map(|(i, m)| {
        let selected = i == app.selected;
        let name = Line::from(vec![
            Span::styled(
                if selected { "▌ " } else { "  " },
                Style::new().fg(palette.accent),
            ),
            Span::styled(
                show(m.get("name")),
                if selected {
                    Style::new().fg(palette.ink()).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(palette.ink2())
                },
            ),
        ]);
        let cells = vec![
            name,
            state_cell(m, app.clock, palette),
            Line::raw(show(m.get("agents"))),
            Line::raw(runs(m).len().to_string()),
            Line::raw(show(m.get("cpus"))),
            Line::raw(memory(m)),
            Line::raw(figure(stat(m, "cpu_percent"), "%")),
        ];
        let row = Row::new(cells);
        if selected {
            row.style(Style::new().bg(palette.sheet()))
        } else {
            row
        }
    });
    let widths = [
        Constraint::Length(18),
        Constraint::Length(11),
        Constraint::Length(8),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Length(17),
        Constraint::Fill(1),
    ];
    frame.render_widget(Table::new(body, widths).header(head).block(block), area);
}

/// The state as a dot and its word: green while the machine answers, crit when it does not.
fn state_cell(m: &Value, clock: f32, palette: Palette) -> Line<'static> {
    let word = state(m);
    let status = match word {
        "online" => "idle",
        "offline" => "failed",
        _ => "gone",
    };
    Line::from(vec![
        dot(status, clock, &palette),
        Span::raw(format!(" {word}")),
    ])
}

fn details(frame: &mut Frame, area: Rect, app: &App) {
    let palette = app.palette;
    let [facts, runs_area] =
        Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(area);
    let Some(machine) = app.array("/v1/machines").get(app.selected) else {
        frame.render_widget(panel("Machine", &palette, false), facts);
        frame.render_widget(panel("Runs", &palette, false), runs_area);
        return;
    };
    let kind = show(machine.get("kind"));
    let mut head = vec![Span::styled(
        show(machine.get("name")),
        Style::new().fg(palette.ink()).add_modifier(Modifier::BOLD),
    )];
    if kind != "-" {
        head.push(Span::raw("  "));
        head.push(chip(&kind, palette.ink2(), palette.sheet()));
    }
    let mut lines: Vec<Line> = vec![Line::from(head), Line::raw("")];
    lines.extend(detail_fields(machine).into_iter().map(|(label, value)| {
        Line::from(vec![
            Span::styled(format!("{label:>12}  "), Style::new().fg(palette.muted)),
            Span::styled(value, Style::new().fg(palette.ink2())),
        ])
    }));
    frame.render_widget(
        Paragraph::new(lines).block(panel("Machine", &palette, false)),
        facts,
    );

    let run_list = runs(machine);
    let mut lines: Vec<Line> = Vec::new();
    if run_list.is_empty() {
        lines.push(Line::from(Span::styled(
            "  none",
            Style::new().fg(palette.muted),
        )));
    }
    for run in run_list {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {}  ", clip(&show(run.get("id")), 12)),
                Style::new().fg(palette.ink()),
            ),
            Span::styled(show(run.get("state")), Style::new().fg(palette.ink2())),
            Span::styled(
                format!(
                    "  {}  {} vms",
                    show(run.get("role")),
                    run.get("vms").and_then(Value::as_array).map_or(0, Vec::len)
                ),
                Style::new().fg(palette.muted),
            ),
        ]));
    }
    let title = format!("Runs · {}", run_list.len());
    frame.render_widget(
        Paragraph::new(lines).block(panel(&title, &palette, false)),
        runs_area,
    );
}

/// The fields `machines show` prints for one machine, as (label, value) pairs.
fn detail_fields(m: &Value) -> Vec<(&'static str, String)> {
    let gpus: Vec<&str> = m
        .get("gpus")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|g| g.get("name").and_then(Value::as_str))
        .collect();
    vec![
        ("id", show(m.get("id"))),
        ("name", show(m.get("name"))),
        ("kind", show(m.get("kind"))),
        ("arch", show(m.get("arch"))),
        ("backend", show(m.get("backend"))),
        ("cpus", show(m.get("cpus"))),
        ("memory", format!("{} MiB", show(m.get("memory_mib")))),
        ("state", state(m).to_string()),
        (
            "gpus",
            if gpus.is_empty() {
                "none".to_string()
            } else {
                gpus.join(", ")
            },
        ),
        ("cpu", figure(stat(m, "cpu_percent"), "%")),
        ("load", load(m)),
        ("memory used", figure(stat(m, "mem_used_mib"), " MiB")),
        ("vms running", figure(stat(m, "vms_running"), "")),
        ("agents", show(m.get("agents"))),
    ]
}

/// The machine's state as a word: online while it answers, else offline (`-` when unknown).
pub fn state(m: &Value) -> &'static str {
    match m.get("reachable").and_then(Value::as_bool) {
        Some(true) => "online",
        Some(false) => "offline",
        None => "-",
    }
}

/// The machine's runs (its `runs` array; none when it lists none).
fn runs(m: &Value) -> &[Value] {
    m.get("runs")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// One figure from the machine's live stats (`-` when the node reports none).
fn stat<'a>(m: &'a Value, key: &str) -> Option<&'a Value> {
    m.get("stats")
        .filter(|s| !s.is_null())
        .and_then(|s| s.get(key))
}

fn figure(v: Option<&Value>, unit: &str) -> String {
    match v.and_then(Value::as_f64) {
        Some(n) => format!("{n:.0}{unit}"),
        None => "-".to_string(),
    }
}

/// Used of total memory: "2048/8192 MiB", or the total alone when the usage is unknown.
fn memory(m: &Value) -> String {
    let total = show(m.get("memory_mib"));
    match stat(m, "mem_used_mib").and_then(Value::as_f64) {
        Some(used) => format!("{used:.0}/{total} MiB"),
        None => format!("-/{total} MiB"),
    }
}

/// The 1, 5 and 15 minute load averages, as `machines show` prints them.
fn load(m: &Value) -> String {
    stat(m, "load")
        .and_then(Value::as_array)
        .map(|l| {
            l.iter()
                .map(|n| format!("{:.2}", n.as_f64().unwrap_or(0.0)))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_else(|| "-".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn runs_are_listed_or_none() {
        assert_eq!(
            runs(&json!({ "runs": [{ "id": "r1" }, { "id": "r2" }] })).len(),
            2
        );
        assert!(runs(&json!({})).is_empty());
    }
}
