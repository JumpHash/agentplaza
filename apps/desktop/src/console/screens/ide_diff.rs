//! The IDE's diff view. The hub diffs; `apz_ide::parse_unified` gives the hunks. This module lays a
//! file's hunks out as rows (a hunk header, numbered lines with their sign, a note), the change list
//! the Changes panel shows, and the review view that stacks every changed file in the centre.

use apz_ide::{ChangeSet, FileChange, Hunk, LineKind, Status};
use iced::widget::{Space, button, column, container, row};
use iced::{Border, Color, Element, Length, Theme};

use super::ide::Msg;
use crate::console::ui;
use crate::theme;

/// One row of a laid-out diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// `@@ -a,b +c,d @@` and the enclosing header, if the diff gave one.
    Header(String),
    /// A context, added or removed line, with its numbers on each side (the side it is not on is `None`).
    Line {
        kind: LineKind,
        old: Option<u32>,
        new: Option<u32>,
        text: String,
    },
    /// A line the diff marks in words: a binary file, or a missing final newline.
    Note(String),
}

/// The hunk header as the diff writes it.
pub fn hunk_header(h: &Hunk) -> String {
    let mut s = format!(
        "@@ -{},{} +{},{} @@",
        h.old_start, h.old_lines, h.new_start, h.new_lines
    );
    if !h.header.is_empty() {
        s.push(' ');
        s.push_str(&h.header);
    }
    s
}

/// A file's diff as rows, in the order the diff gives them.
pub fn layout(file: &FileChange) -> Vec<Row> {
    if file.binary {
        return vec![Row::Note("Binary file: the change is not shown.".into())];
    }
    if file.hunks.is_empty() {
        return vec![Row::Note("No line changes.".into())];
    }
    let mut rows = Vec::new();
    for hunk in &file.hunks {
        rows.push(Row::Header(hunk_header(hunk)));
        for line in &hunk.lines {
            match line.kind {
                LineKind::NoNewline => rows.push(Row::Note("No newline at end of file.".into())),
                kind => rows.push(Row::Line {
                    kind,
                    old: line.old_no,
                    new: line.new_no,
                    text: line.text.clone(),
                }),
            }
        }
    }
    rows
}

/// The sign a line carries in the diff: `+`, `-` or a space.
pub fn sign(kind: LineKind) -> &'static str {
    match kind {
        LineKind::Add => "+",
        LineKind::Remove => "-",
        LineKind::Context | LineKind::NoNewline => " ",
    }
}

/// Added and removed line counts of one file.
pub fn counts(file: &FileChange) -> (usize, usize) {
    let mut added = 0;
    let mut removed = 0;
    for line in file.hunks.iter().flat_map(|h| &h.lines) {
        match line.kind {
            LineKind::Add => added += 1,
            LineKind::Remove => removed += 1,
            LineKind::Context | LineKind::NoNewline => {}
        }
    }
    (added, removed)
}

/// The one-letter status the lists show.
pub fn status_letter(status: Status) -> &'static str {
    match status {
        Status::Added => "A",
        Status::Modified => "M",
        Status::Deleted => "D",
        Status::Renamed => "R",
    }
}

/// The summary line of a change set: files, added and removed lines.
pub fn summary(set: &ChangeSet) -> String {
    let stats = set.stats();
    format!(
        "{} files  +{}  -{}",
        stats.files, stats.additions, stats.deletions
    )
}

/// The Changes panel's file list: a press shows that file's diff below it.
pub fn file_list(set: &ChangeSet, selected: Option<&str>) -> Element<'static, Msg> {
    let mut col = column![].spacing(0);
    for file in &set.files {
        let (added, removed) = counts(file);
        let on = selected == Some(file.path.as_str());
        let ink = theme::palette().ink;
        let stats = format!("+{added} -{removed}");
        let label = row![
            ui::mono(status_letter(file.status), 11.0, theme::palette().ink_3),
            ui::mono(file.path.clone(), 11.5, ink),
            Space::new().width(Length::Fill),
            ui::mono(stats, 10.5, theme::palette().ink_3),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        col = col.push(
            button(label)
                .on_press(Msg::ShowDiff(file.path.clone()))
                .padding([4, 6])
                .width(Length::Fill)
                .style(theme::register_row(on)),
        );
    }
    col.into()
}

/// The review view: every changed file's diff, one after another, in the centre.
pub fn review(set: &ChangeSet) -> Element<'static, Msg> {
    let mut col = column![ui::mono(summary(set), 11.5, theme::palette().ink_2)].spacing(14);
    if set.files.is_empty() {
        col = col.push(ui::faint("Nothing has changed since this base."));
    }
    for file in &set.files {
        col = col.push(file_block(file));
    }
    ui::scroll(col).width(Length::Fill).into()
}

/// One file's diff: a head (status, path, counts) over its rows.
pub fn file_block(file: &FileChange) -> Element<'static, Msg> {
    let (added, removed) = counts(file);
    let head = row![
        ui::mono(status_letter(file.status), 11.0, theme::palette().ink_3),
        ui::mono(file.path.clone(), 12.0, theme::palette().ink),
        Space::new().width(Length::Fill),
        ui::mono(format!("+{added} -{removed}"), 10.5, theme::palette().ink_3),
    ]
    .spacing(10)
    .align_y(iced::Alignment::Center);
    let mut block =
        column![
            container(head)
                .padding([6, 8])
                .width(Length::Fill)
                .style(|_: &Theme| container::Style {
                    background: Some(theme::palette().hover.into()),
                    border: Border {
                        color: theme::palette().rule,
                        width: 1.0,
                        radius: 0.0.into()
                    },
                    ..Default::default()
                })
        ]
        .spacing(0);
    for r in layout(file) {
        block = block.push(view_row(r));
    }
    block.into()
}

fn view_row(r: Row) -> Element<'static, Msg> {
    let p = theme::palette();
    match r {
        Row::Header(text) => container(ui::mono(text, 11.0, p.ink_2))
            .padding([3, 8])
            .width(Length::Fill)
            .into(),
        Row::Note(text) => container(ui::faint(text))
            .padding([3, 8])
            .width(Length::Fill)
            .into(),
        Row::Line {
            kind,
            old,
            new,
            text,
        } => {
            let (tint, ink) = match kind {
                LineKind::Add => (Some(Color { a: 0.14, ..p.ok }), p.ok),
                LineKind::Remove => (Some(Color { a: 0.14, ..p.crit }), p.crit),
                LineKind::Context | LineKind::NoNewline => (None, p.ink),
            };
            let number = |n: Option<u32>| {
                container(ui::mono(
                    n.map_or_else(String::new, |n| n.to_string()),
                    11.0,
                    p.ink_3,
                ))
                .width(Length::Fixed(44.0))
                .align_x(iced::alignment::Horizontal::Right)
            };
            let content = row![
                number(old),
                number(new),
                ui::mono(sign(kind), 12.0, ink),
                ui::mono(text, 12.0, p.ink)
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center);
            container(content)
                .padding([1, 8])
                .width(Length::Fill)
                .style(move |_: &Theme| container::Style {
                    background: tint.map(Into::into),
                    ..Default::default()
                })
                .into()
        }
    }
}
