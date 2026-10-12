//! The console's design kit, in the web console's language (`apps/web/DESIGN.md`): paper and
//! sheets, ink rules, the orange signal for the owner's hand, square corners, mono for titles,
//! labels and figures, serif for prose. Screens build from these, never from raw colours.

use iced::widget::{button, column, container, row, svg, text, Space};
use iced::{Alignment, Border, Color, Element, Length, Padding, Theme};

pub use crate::theme::{INK, INK_2, INK_3, PAPER, RULE, RULE_FAINT, SHEET, SIGNAL};
use crate::theme::{CRIT, HOVER, MONO, OK, SERIF, WARN};

/// A semantic tone for badges, notices and buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Muted,
    Info,
    Ok,
    Warn,
    Danger,
    Signal,
}

impl Tone {
    pub fn color(self) -> Color {
        match self {
            Tone::Muted => INK_3,
            Tone::Info => Color::from_rgb8(0x2d, 0x5f, 0x8a),
            Tone::Ok => OK,
            Tone::Warn => WARN,
            Tone::Danger => CRIT,
            Tone::Signal => SIGNAL,
        }
    }
}

/// Mono text at `size` in `color`.
pub fn mono<'a>(content: impl text::IntoFragment<'a>, size: f32, color: Color) -> text::Text<'a> {
    text(content).font(MONO).size(size).color(color)
}

/// Serif prose.
pub fn prose<'a>(content: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(content).font(SERIF).size(15.0).color(INK)
}

/// Faint serif, for secondary lines.
pub fn faint<'a>(content: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(content).font(SERIF).size(13.0).color(INK_3)
}

/// A small uppercase mono label.
pub fn label<'a>(content: &str) -> text::Text<'a> {
    text(content.to_uppercase()).font(MONO).size(11.0).color(INK_3)
}

/// A lucide icon at `size`, tinted `color`.
pub fn icon<'a, M: 'a>(name: &str, size: f32, color: Color) -> Element<'a, M> {
    match crate::console::icons::svg(name) {
        Some(doc) => svg(svg::Handle::from_memory(doc.as_bytes().to_vec()))
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .style(move |_: &Theme, _| svg::Style { color: Some(color) })
            .into(),
        None => Space::new().width(Length::Fixed(size)).height(Length::Fixed(size)).into(),
    }
}

/// A screen's sheet: the page's one bordered surface, uniform padding, full width.
pub fn sheet<'a, M: 'a>(head: Element<'a, M>, body: Element<'a, M>) -> Element<'a, M> {
    container(column![head, body].spacing(22))
        .padding(Padding { top: 0.0, right: 28.0, bottom: 28.0, left: 28.0 })
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(SHEET.into()),
            border: Border { color: RULE, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        })
        .into()
}

/// A sheet's head: the title (uppercase mono) left, actions right, a 2px ink rule under it.
pub fn view_head<'a, M: 'a>(title: &str, actions: Vec<Element<'a, M>>) -> Element<'a, M> {
    let mut line = row![text(title.to_uppercase()).font(MONO).size(28.0).color(INK)].align_y(Alignment::End).spacing(16);
    line = line.push(Space::new().width(Length::Fill));
    for action in actions {
        line = line.push(action);
    }
    column![
        container(line).padding(Padding { top: 24.0, right: 0.0, bottom: 14.0, left: 0.0 }),
        container(Space::new().height(2.0)).width(Length::Fill).style(|_: &Theme| container::Style { background: Some(INK.into()), ..Default::default() }),
    ]
    .into()
}

/// A card: a mono header row over its body, 1px rule borders.
pub fn card<'a, M: 'a>(title: &str, body: Element<'a, M>) -> Element<'a, M> {
    column![
        container(label(title)).padding([8, 12]).width(Length::Fill).style(|_: &Theme| container::Style {
            border: Border { color: RULE, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        }),
        container(body).padding(12).width(Length::Fill).style(|_: &Theme| container::Style {
            border: Border { color: RULE, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        }),
    ]
    .into()
}

/// A square tag: mono uppercase text in a tone, with a 1px border in that tone.
pub fn badge<'a, M: 'a>(content: &str, tone: Tone) -> Element<'a, M> {
    let c = tone.color();
    container(text(content.to_uppercase()).font(MONO).size(10.5).color(c))
        .padding([2, 6])
        .style(move |_: &Theme| container::Style {
            border: Border { color: c, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        })
        .into()
}

/// Button kinds, as on the web console.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The owner's primary act: solid signal.
    Primary,
    /// A plain bordered button.
    Secondary,
    /// Text only.
    Ghost,
    /// Irreversible: solid crit.
    Danger,
}

/// A mono uppercase button with an optional leading icon.
pub fn btn<'a, M: Clone + 'a>(label_text: &str, icon_name: Option<&str>, kind: Kind, on_press: Option<M>) -> Element<'a, M> {
    let fg = match kind {
        Kind::Primary | Kind::Danger => Color::WHITE,
        _ => INK,
    };
    let mut inner = row![].spacing(6).align_y(Alignment::Center);
    if let Some(name) = icon_name {
        inner = inner.push(icon(name, 14.0, fg));
    }
    inner = inner.push(text(label_text.to_uppercase()).font(MONO).size(11.5).color(fg));
    let b = button(inner).padding([7, 12]).style(move |_: &Theme, status: button::Status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let (bg, border) = match kind {
            Kind::Primary => (Some(SIGNAL), INK),
            Kind::Danger => (Some(CRIT), INK),
            Kind::Secondary => (if hovered { Some(HOVER) } else { None }, INK),
            Kind::Ghost => (if hovered { Some(HOVER) } else { None }, Color::TRANSPARENT),
        };
        button::Style {
            background: bg.map(Into::into),
            text_color: fg,
            border: Border { color: border, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        }
    });
    match on_press {
        Some(m) => b.on_press(m).into(),
        None => b.into(),
    }
}

/// An empty state: an icon, a bold mono line, a faint sentence; centred.
pub fn empty<'a, M: 'a>(icon_name: &str, title: &str, sentence: &str) -> Element<'a, M> {
    container(
        column![icon(icon_name, 18.0, INK_3), mono(title.to_string(), 13.0, INK), faint(sentence.to_string())]
            .spacing(6)
            .align_x(Alignment::Center),
    )
    .padding([40, 16])
    .width(Length::Fill)
    .align_x(Alignment::Center)
    .into()
}

/// An inline error line (crit).
pub fn error_line<'a, M: 'a>(message: &str) -> Element<'a, M> {
    text(message.to_string()).font(SERIF).size(13.0).color(CRIT).into()
}

/// A faint "Loading…" line.
pub fn loading<'a, M: 'a>() -> Element<'a, M> {
    mono("Loading…", 12.0, INK_3).into()
}

/// Agent-written markdown, rendered (contract: M-D — a markdown renderer in the design language;
/// until then the text shows as serif prose).
pub fn markdown<'a, M: 'a>(source: &str) -> Element<'a, M> {
    prose(source.to_string()).into()
}

/// The page background.
pub fn paper(_: &Theme) -> container::Style {
    container::Style { background: Some(PAPER.into()), ..Default::default() }
}

/// A hairline rule.
pub fn hairline<'a, M: 'a>() -> Element<'a, M> {
    container(Space::new().height(1.0)).width(Length::Fill).style(|_: &Theme| container::Style { background: Some(RULE_FAINT.into()), ..Default::default() }).into()
}
