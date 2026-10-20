//! The IDE's diff view. The hub diffs; `apz_ide::parse_unified` gives the hunks. This module lays a
//! file's hunks out as rows (a hunk header, numbered lines with their sign, a note), the change list
//! the Changes panel shows, and the review view that stacks every changed file in the centre.

use apz_ide::{ChangeSet, FileChange, Hunk, LineKind, Status};
use iced::widget::{Space, button, column, container, row};
use iced::{Border, Element, Length, Theme};

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

/// The status's word and tone, as the web's review badge: added (ok), modified (info), deleted (danger), renamed (warn).
fn status_badge<'a>(status: Status) -> Element<'a, Msg> {
    let (word, tone) = match status {
        Status::Added => ("added", ui::Tone::Ok),
        Status::Modified => ("modified", ui::Tone::Info),
        Status::Deleted => ("deleted", ui::Tone::Danger),
        Status::Renamed => ("renamed", ui::Tone::Warn),
    };
    ui::badge(word, tone)
}

/// The summary line of a change set, as the review's totals: files, then added and removed lines.
pub fn summary(set: &ChangeSet) -> String {
    let stats = set.stats();
    format!(
        "{} files · +{} −{}",
        stats.files, stats.additions, stats.deletions
    )
}

/// The review, the web's `.ided-review-body`: every changed file's block, one after another. A file's name picks it
/// (`ShowDiff`), which shows that file alone in the centre.
pub fn review(set: &ChangeSet) -> Element<'static, Msg> {
    let mut col = column![].spacing(16);
    if set.files.is_empty() {
        col = col.push(ui::empty(
            "check",
            "Nothing changed",
            "No file differs from this base.",
        ));
    }
    for file in &set.files {
        col = col.push(file_block(file));
    }
    ui::scroll(col).width(Length::Fill).into()
}

/// One file's block, the web's `.ided-file`: a head with the status badge, the path (picks the file) and the added and
/// removed counts, over its rows. `on` marks the file the centre shows alone.
pub fn file_block(file: &FileChange) -> Element<'static, Msg> {
    let p = theme::palette();
    let (added, removed) = counts(file);
    let name = if file.status == Status::Renamed {
        file.old_path
            .as_deref()
            .map_or_else(|| file.path.clone(), |old| format!("{old} → {}", file.path))
    } else {
        file.path.clone()
    };
    let head = row![
        status_badge(file.status),
        button(ui::mono(name, 12.0, p.ink))
            .on_press(Msg::ShowDiff(file.path.clone()))
            .padding(0)
            .style(|_: &Theme, _| button::Style {
                background: None,
                text_color: theme::palette().ink,
                ..Default::default()
            }),
        Space::new().width(Length::Fill),
        ui::mono(format!("+{added}"), 11.5, p.ok),
        ui::mono(format!("−{removed}"), 11.5, p.crit),
    ]
    .spacing(10)
    .align_y(iced::Alignment::Center);

    let mut block =
        column![
            container(head)
                .padding([10, 14])
                .width(Length::Fill)
                .style(move |_: &Theme| container::Style {
                    background: Some(p.hover.into()),
                    border: Border {
                        color: p.rule,
                        width: 1.0,
                        radius: theme::skin().radius.into(),
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
                LineKind::Add => (Some(p.add_bg), p.add_fg),
                LineKind::Remove => (Some(p.del_bg), p.del_fg),
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
