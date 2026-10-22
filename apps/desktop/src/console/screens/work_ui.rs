//! What the work engine's screens share on the page: the table grid (a row of cells at their shares of the width, the
//! head over a strong rule, a rule between rows), the clickable row, the expert switch, and a fact's label and value.
//! The sessions, seats, cards, chats and ledger draw with it, so their tables read alike.

use iced::widget::{Space, button, checkbox, column, container};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding, Theme};

use crate::console::ui;
use crate::theme;

/// The inset of a table cell's text, as the web's `.tbl td`.
pub const CELL_PAD: f32 = 12.0;

/// The card's side inset for a flush table: the first and the last cells start and end this far in.
pub const EDGE: f32 = 16.0;

/// One column: its share of the width (`portion`), or a fixed width, and whether its cells read to the right.
#[derive(Debug, Clone, Copy)]
pub struct Col {
    pub portion: u16,
    pub width: Option<f32>,
    pub right: bool,
}

impl Col {
    pub fn flex(portion: u16) -> Col {
        Col {
            portion,
            width: None,
            right: false,
        }
    }

    pub fn right(portion: u16) -> Col {
        Col {
            right: true,
            ..Col::flex(portion)
        }
    }

    pub fn fixed(width: f32) -> Col {
        Col {
            portion: 0,
            width: Some(width),
            right: false,
        }
    }
}

/// One table row: its cells in `cols`, each at its share, the outer two inset by `edge`.
pub fn grid_row<'a, M: 'a>(cols: &[Col], cells: Vec<Element<'a, M>>, edge: f32) -> Element<'a, M> {
    let last = cols.len().saturating_sub(1);
    let row_cells: Vec<Element<'a, M>> = cols
        .iter()
        .zip(cells)
        .enumerate()
        .map(|(i, (col, cell))| {
            let padding = Padding {
                top: 0.0,
                right: if i == last { edge } else { CELL_PAD },
                bottom: 0.0,
                left: if i == 0 { edge } else { CELL_PAD },
            };
            let cell = container(cell)
                .padding(padding)
                .clip(true)
                .align_x(if col.right {
                    Alignment::End
                } else {
                    Alignment::Start
                });
            let cell = match col.width {
                Some(width) => cell.width(Length::Fixed(width)),
                None => cell.width(Length::FillPortion(col.portion)),
            };
            cell.into()
        })
        .collect();
    iced::widget::Row::with_children(row_cells)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into()
}

/// The table: its head (the labels in the label voice) over a strong rule, then the rows, a rule between each pair.
/// Each row is a [`grid_row`] the screen built.
pub fn table<'a, M: 'a>(
    cols: &[Col],
    labels: &[&str],
    rows: Vec<Element<'a, M>>,
    edge: f32,
) -> Element<'a, M> {
    let head: Vec<Element<'a, M>> = labels
        .iter()
        .map(|label| {
            if label.is_empty() {
                Space::new().into()
            } else {
                ui::label(label).into()
            }
        })
        .collect();
    let mut lines: Vec<Element<'a, M>> = vec![
        container(grid_row(cols, head, edge))
            .padding([8, 0])
            .width(Length::Fill)
            .into(),
        rule(theme::palette().rule_strong),
    ];
    let n = rows.len();
    for (slot, line) in rows.into_iter().enumerate() {
        lines.push(line);
        if slot + 1 < n {
            lines.push(rule(theme::palette().rule));
        }
    }
    column(lines).width(Length::Fill).into()
}

/// A clickable row: the hover ground of the web's `tr.clickable`, its content inside.
pub fn clickable<'a, M: Clone + 'a>(content: Element<'a, M>, on_press: M) -> Element<'a, M> {
    button(content)
        .on_press(on_press)
        .style(row_style)
        .padding(0)
        .width(Length::Fill)
        .into()
}

fn row_style(_: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: hovered.then_some(Background::Color(theme::palette().hover)),
        text_color: theme::palette().ink,
        border: Border::default(),
        ..Default::default()
    }
}

/// The rule the web draws between table rows and under its head.
pub fn rule<'a, M: 'a>(color: Color) -> Element<'a, M> {
    container(Space::new().height(Length::Fixed(1.0)))
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(Background::Color(color)),
            ..Default::default()
        })
        .into()
}

/// The expert switch, or any on-off filter of a screen's head: a checkbox with its label, as the Machines and the
/// Maintenance heads have it.
pub fn switch<'a, M: 'a>(
    label: &'static str,
    on: bool,
    on_toggle: impl Fn(bool) -> M + 'a,
) -> Element<'a, M> {
    checkbox(on)
        .label(label)
        .on_toggle(on_toggle)
        .size(14.0)
        .text_size(13.0)
        .into()
}

/// A fact's label and its value, one under the other: the label in the muted voice, the value under it.
pub fn fact<'a, M: 'a>(label: &str, value: Element<'a, M>) -> Element<'a, M> {
    column![ui::faint(label.to_string()), value]
        .spacing(2)
        .width(Length::Fill)
        .into()
}

/// The facts of a screen in rows of `per_row` (a row ends when it is full): the facts a page leads with.
pub fn facts<'a, M: 'a>(items: Vec<Element<'a, M>>, per_row: usize) -> Element<'a, M> {
    let mut lines: Vec<Element<'a, M>> = Vec::new();
    let mut items = items.into_iter().peekable();
    while items.peek().is_some() {
        let line: Vec<Element<'a, M>> = items.by_ref().take(per_row.max(1)).collect();
        lines.push(
            iced::widget::Row::with_children(line)
                .spacing(24)
                .width(Length::Fill)
                .into(),
        );
    }
    column(lines).spacing(14).width(Length::Fill).into()
}

/// A line of muted words, as the screens' notes and empty sentences.
pub fn muted<'a, M: 'a>(words: impl Into<String>) -> Element<'a, M> {
    ui::faint(words.into()).into()
}

/// A text of the palette's ink in the sans voice, at `size`.
pub fn ink<'a, M: 'a>(words: impl Into<String>, size: f32) -> Element<'a, M> {
    ui::sans(words.into(), size, theme::palette().ink).into()
}
