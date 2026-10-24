//! The console's design kit, drawn as the Loom and Graphite consoles draw it (`apps/web/src/styles/proto/base.css`):
//! the page on the panel, raised cards with the skin's radius and a card head, soft chips, 30px buttons with the
//! skin's action colour, underline tabs, segmented controls. Screens build from these, never from raw colours.

use iced::font::Weight;
use iced::widget::text::LineHeight;
use iced::widget::{Space, button, column, container, rich_text, row, scrollable, span, svg, text};
use iced::{Alignment, Border, Color, Element, Font, Length, Padding, Shadow, Theme};

use crate::theme::{
    MONO, READING, SANS, TYPE_BADGE, TYPE_CODE, TYPE_ITEM, TYPE_META, TYPE_PROSE, TYPE_UI,
};
pub use crate::theme::{SANS_BOLD, SANS_ITALIC, SANS_MEDIUM, SANS_SEMIBOLD};

/// Martian Mono for code and the ids, hashes and paths where alignment matters: 600 for a code head,
/// 700 for the strongest. The console embeds Regular, Medium and Bold, so 600 renders as Bold.
pub const MONO_SEMIBOLD: Font = Font {
    weight: Weight::Semibold,
    ..MONO
};
pub const MONO_BOLD: Font = Font {
    weight: Weight::Bold,
    ..MONO
};

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
            Tone::Muted => crate::theme::palette().ink_3,
            Tone::Info => crate::theme::palette().info,
            Tone::Ok => crate::theme::palette().ok,
            Tone::Warn => crate::theme::palette().warn,
            Tone::Danger => crate::theme::palette().crit,
            Tone::Signal => crate::theme::palette().signal,
        }
    }
}

/// Martian Mono at `size` in `color`: code, ids, hashes, paths and raw output, where the letters must line up.
pub fn mono<'a>(content: impl text::IntoFragment<'a>, size: f32, color: Color) -> text::Text<'a> {
    text(content).font(MONO).size(size).color(color)
}

/// Inter at `size` in `color`: the interface text (labels, names, values, sentences).
pub fn sans<'a>(content: impl text::IntoFragment<'a>, size: f32, color: Color) -> text::Text<'a> {
    text(content).font(SANS).size(size).color(color)
}

/// Reading prose: Inter at the prose size, at the reading line height.
pub fn prose<'a>(content: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(content)
        .font(SANS)
        .size(TYPE_PROSE)
        .line_height(READING)
        .color(crate::theme::palette().ink)
}

/// Secondary text: Inter at the metadata size, in ink-3.
pub fn faint<'a>(content: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(content)
        .font(SANS)
        .size(TYPE_META)
        .color(crate::theme::palette().ink_3)
}

/// A field label (the web's `.stat-l`, `.kv dt`, `.tbl th`): the skin's label voice (mono capitals at 11px in Loom,
/// sans 12px in Graphite), in ink-3.
pub fn label<'a>(content: &str) -> text::Text<'a> {
    let sk = crate::theme::skin();
    text(sk.label_text(content))
        .font(sk.label_font())
        .size(if sk.label_mono { sk.label_size } else { 12.0 })
        .color(crate::theme::palette().ink_3)
}

/// A lucide icon at `size`, tinted `color`.
pub fn icon<'a, M: 'a>(name: &str, size: f32, color: Color) -> Element<'a, M> {
    match crate::console::icons::svg(name) {
        Some(doc) => svg(svg::Handle::from_memory(doc.as_bytes().to_vec()))
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .style(move |_: &Theme, _| svg::Style { color: Some(color) })
            .into(),
        None => Space::new()
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .into(),
    }
}

/// A provider's mark (`brands.rs`) in its own colours, `size` px square; an empty square for an
/// unknown key.
pub fn brand_logo<'a, M: 'a>(key: &str, size: f32) -> Element<'a, M> {
    match crate::console::brands::get(key) {
        Some(brand) => svg(svg::Handle::from_memory(on_dark(brand.svg).into_bytes()))
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .into(),
        None => Space::new()
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .into(),
    }
}

/// The AgentPlaza mark: the hive (three ink cells around the signal-orange core).
pub fn hive<'a, M: 'a>(size: f32) -> Element<'a, M> {
    svg(svg::Handle::from_memory(on_dark(HIVE).into_bytes()))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .into()
}

/// The marks are drawn in the light theme's ink (`#121613`), which vanishes on this app's dark paper: that ink
/// becomes the theme's crate::theme::palette().ink. Multi-colour marks keep their own colours (they never use that exact ink).
fn on_dark(svg: &str) -> String {
    // Marks drawn in near-black ink are lightened on the dark palette only; the light palette keeps them as drawn.
    if crate::theme::is_light() {
        svg.to_string()
    } else {
        svg.replace("#121613", "#e3e8f4")
    }
}

/// The hive's SVG (the console's geometry; the landing page, JumpHash/agentplaza-landing, draws the same).
const HIVE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><path d="M21.20 7.85L18.60 12.35L13.40 12.35L10.80 7.85L13.40 3.35L18.60 3.35Z" fill="none" stroke="#121613" stroke-width="2.1" stroke-linejoin="miter"/><path d="M11.76 24.20L9.16 28.70L3.96 28.70L1.36 24.20L3.96 19.70L9.16 19.70Z" fill="none" stroke="#121613" stroke-width="2.1" stroke-linejoin="miter"/><path d="M30.64 24.20L28.04 28.70L22.84 28.70L20.24 24.20L22.84 19.70L28.04 19.70Z" fill="none" stroke="#121613" stroke-width="2.1" stroke-linejoin="miter"/><path class="core" fill="#e8590c" d="M21.60 18.75L18.80 23.60L13.20 23.60L10.40 18.75L13.20 13.90L18.80 13.90Z"/></svg>"##;

/// A screen's page, as the web's `.page`: centred at most 1120 wide, padded 32 (56 below), its parts 20 apart.
pub fn sheet<'a, M: 'a>(head: Element<'a, M>, body: Element<'a, M>) -> Element<'a, M> {
    container(
        container(column![head, body].spacing(20))
            .max_width(1120.0)
            .width(Length::Fill)
            .padding(Padding {
                top: 32.0,
                right: 32.0,
                bottom: 56.0,
                left: 32.0,
            }),
    )
    .width(Length::Fill)
    .align_x(Alignment::Center)
    .into()
}

/// A page head, as the web's `.page-head`: the title (600 at 23px) left, the actions right; no rule under it.
pub fn view_head<'a, M: 'a>(title: &str, actions: Vec<Element<'a, M>>) -> Element<'a, M> {
    head_line(title, None, actions)
}

/// A page head with its lede under the title, in ink-2 at the prose size: the web's `.page-head` with `.lede`.
pub fn view_head_with_intro<'a, M: 'a>(
    title: &str,
    intro: &str,
    actions: Vec<Element<'a, M>>,
) -> Element<'a, M> {
    head_line(title, Some(intro), actions)
}

/// The page title's size (the web's `--fs-h1`).
pub const TYPE_H1: f32 = 23.0;

fn head_line<'a, M: 'a>(
    title: &str,
    intro: Option<&str>,
    actions: Vec<Element<'a, M>>,
) -> Element<'a, M> {
    let mut line = row![
        text(title.to_string())
            .font(SANS_SEMIBOLD)
            .size(TYPE_H1)
            .line_height(LineHeight::Relative(1.25))
            .color(crate::theme::palette().ink)
    ]
    .align_y(Alignment::Center)
    .spacing(12);
    line = line.push(Space::new().width(Length::Fill));
    if !actions.is_empty() {
        line = line.push(row(actions).spacing(8).align_y(Alignment::Center));
    }
    let mut head = column![line].spacing(6);
    if let Some(intro) = intro {
        head = head.push(
            text(intro.to_string())
                .font(SANS)
                .size(TYPE_PROSE)
                .line_height(READING)
                .color(crate::theme::palette().ink_2)
                .width(Length::Fixed(620.0)),
        );
    }
    head.into()
}

/// A card, as the web's `.card`: raised, ruled, the skin's large radius and card shadow; its head (`.card-h`, at least
/// 44 tall, the title in the skin's label voice) over a rule, the body padded 16.
pub fn card<'a, M: 'a>(title: &str, body: Element<'a, M>) -> Element<'a, M> {
    card_with(title, Vec::new(), body)
}

/// A card with actions at the right of its head.
pub fn card_with<'a, M: 'a>(
    title: &str,
    actions: Vec<Element<'a, M>>,
    body: Element<'a, M>,
) -> Element<'a, M> {
    let mut head = row![card_title(title)]
        .spacing(8)
        .align_y(Alignment::Center);
    head = head.push(Space::new().width(Length::Fill));
    for a in actions {
        head = head.push(a);
    }
    let head = container(head)
        .padding([8, 16])
        .height(Length::Shrink)
        .width(Length::Fill)
        .center_y(Length::Fixed(44.0))
        .style(|_: &Theme| container::Style {
            border: Border::default(),
            ..Default::default()
        });
    container(column![
        head,
        rule_line(),
        container(body).padding(16).width(Length::Fill)
    ])
    .width(Length::Fill)
    .style(card_style)
    .into()
}

/// A card's own surface: the web's `.card` (raised, ruled, the skin's large radius, its card shadow).
pub fn card_style(_: &Theme) -> container::Style {
    container::Style {
        text_color: Some(crate::theme::palette().ink),
        background: Some(crate::theme::palette().raised.into()),
        border: Border {
            color: crate::theme::palette().rule,
            width: 1.0,
            radius: crate::theme::skin().radius_lg.into(),
        },
        shadow: crate::theme::skin().shadow_card,
        snap: true,
    }
}

/// A card head's title (`.card-h h3`): mono capitals at 11px in Loom, sans 12.5px in Graphite, ink-3.
pub fn card_title<'a>(title: &str) -> text::Text<'a> {
    let sk = crate::theme::skin();
    text(sk.label_text(title))
        .font(sk.label_font())
        .size(if sk.label_mono { sk.label_size } else { 12.5 })
        .color(crate::theme::palette().ink_3)
}

/// A 1px rule across.
fn rule_line<'a, M: 'a>() -> Element<'a, M> {
    container(Space::new().height(1.0))
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(crate::theme::palette().rule.into()),
            ..Default::default()
        })
        .into()
}

/// A chip's text and soft ground in a tone, as the web's `.chip.t-<tone>`.
fn chip_colors(tone: Tone) -> (Color, Color) {
    let p = crate::theme::palette();
    match tone {
        Tone::Muted => (p.ink_3, p.hover),
        Tone::Info => (p.info, p.info_soft),
        Tone::Ok => (p.ok, p.ok_soft),
        Tone::Warn => (p.warn, p.warn_soft),
        Tone::Danger => (p.crit, p.crit_soft),
        Tone::Signal => (p.signal_ink, p.signal_soft),
    }
}

/// What a contributor model says of itself: its provider trains future models on what it is sent.
pub const TRAINS_NOTE: &str = "Meta trains future models on everything sent to and returned by these models, so use them only for data you are happy to share.";

/// The warning badge on a contributor model (one that trains its provider's future models).
pub fn trains_badge<'a, M: 'a>() -> Element<'a, M> {
    badge("WARNING: trains on your data", Tone::Warn)
}

/// A badge, as the web's `.chip.t-<tone>`: the tone's text on its soft ground, 500 at 11px, radius 6.
pub fn badge<'a, M: 'a>(content: &str, tone: Tone) -> Element<'a, M> {
    let (ink, ground) = chip_colors(tone);
    let loom = crate::theme::skin().label_mono;
    container(
        text(content.to_string())
            .font(SANS_MEDIUM)
            .size(TYPE_BADGE)
            .color(ink),
    )
    .padding(if loom { [4, 7] } else { [3, 6] })
    .style(move |_: &Theme| container::Style {
        background: Some(ground.into()),
        border: Border::default().rounded(6.0),
        ..Default::default()
    })
    .into()
}

/// A badge led by a dot in the tone (the web's `.chip` with `<i class="dot">`).
pub fn badge_dot<'a, M: 'a>(content: &str, tone: Tone) -> Element<'a, M> {
    let (ink, ground) = chip_colors(tone);
    container(
        row![
            dot(ink),
            text(content.to_string())
                .font(SANS_MEDIUM)
                .size(TYPE_BADGE)
                .color(ink)
        ]
        .spacing(5)
        .align_y(Alignment::Center),
    )
    .padding([3, 6])
    .style(move |_: &Theme| container::Style {
        background: Some(ground.into()),
        border: Border::default().rounded(6.0),
        ..Default::default()
    })
    .into()
}

/// A 7px dot in `color` (the web's `.dot`).
pub fn dot<'a, M: 'a>(color: Color) -> Element<'a, M> {
    container(Space::new())
        .width(Length::Fixed(7.0))
        .height(Length::Fixed(7.0))
        .style(move |_: &Theme| container::Style {
            background: Some(color.into()),
            border: Border::default().rounded(4.0),
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
    /// A stop or a cancel: crit in an outline, quiet at rest and tinted under the pointer.
    DangerOutline,
}

/// A button with an optional leading icon, as the web's `.btn`: 30 tall, radius 8, 500 at 13px.
pub fn btn<'a, M: Clone + 'a>(
    label_text: &str,
    icon_name: Option<&str>,
    kind: Kind,
    on_press: Option<M>,
) -> Element<'a, M> {
    sized_btn(label_text, icon_name, kind, on_press, BTN)
}

/// The small twin of `btn`: the same kinds at a smaller size, for dense rows and card heads.
pub fn btn_sm<'a, M: Clone + 'a>(
    label_text: &str,
    icon_name: Option<&str>,
    kind: Kind,
    on_press: Option<M>,
) -> Element<'a, M> {
    sized_btn(label_text, icon_name, kind, on_press, BTN_SM)
}

/// The button both sizes share; `size` sets the icon, the label, the height and the radius.
fn sized_btn<'a, M: Clone + 'a>(
    label_text: &str,
    icon_name: Option<&str>,
    kind: Kind,
    on_press: Option<M>,
    size: BtnSize,
) -> Element<'a, M> {
    let p = crate::theme::palette();
    let action_ink = crate::theme::skin().action_ink;
    // The primary is the skin's action: ink in Loom, the signal fill in Graphite.
    let (primary_fill, on_primary) = if action_ink {
        (p.action, p.on_action)
    } else {
        (p.signal_fill, p.on_signal)
    };
    let fg = match kind {
        Kind::Primary => on_primary,
        Kind::Danger => p.on_hue,
        Kind::DangerOutline => p.crit,
        Kind::Ghost => p.ink_2,
        Kind::Secondary => p.ink,
    };
    let mut inner = row![].spacing(size.gap).align_y(Alignment::Center);
    if let Some(name) = icon_name {
        inner = inner.push(icon(name, size.icon, fg));
    }
    if !label_text.is_empty() {
        // The label sets no colour of its own: it takes the button style's text_color, so the hover ink reaches it.
        inner = inner.push(
            text(label_text.to_string())
                .font(SANS_MEDIUM)
                .size(size.text),
        );
    }
    let b = button(container(inner).center_y(Length::Fill))
        .padding([0.0, size.padding[1]])
        .height(Length::Fixed(size.height))
        .style(move |_: &Theme, status: button::Status| {
            let p = crate::theme::palette();
            let disabled = status == button::Status::Disabled;
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            let (bg, border, ink) = match kind {
                Kind::Primary => {
                    let fill = if hovered {
                        Color {
                            a: 0.88,
                            ..primary_fill
                        }
                    } else {
                        primary_fill
                    };
                    (Some(fill), fill, on_primary)
                }
                Kind::Danger => (Some(p.crit), p.crit, p.on_hue),
                Kind::DangerOutline => (
                    Some(if hovered { p.crit_soft } else { p.raised }),
                    Color { a: 0.35, ..p.crit },
                    p.crit,
                ),
                Kind::Secondary => (
                    Some(if hovered { p.hover } else { p.raised }),
                    p.rule_strong,
                    p.ink,
                ),
                Kind::Ghost => (
                    hovered.then_some(p.hover),
                    Color::TRANSPARENT,
                    if hovered { p.ink } else { p.ink_2 },
                ),
            };
            let fade = |c: Color| {
                if disabled {
                    Color { a: c.a * 0.5, ..c }
                } else {
                    c
                }
            };
            button::Style {
                background: bg.map(|c| fade(c).into()),
                text_color: fade(ink),
                border: Border {
                    color: fade(border),
                    width: 1.0,
                    radius: size.radius.into(),
                },
                ..Default::default()
            }
        });
    match on_press {
        Some(m) => b.on_press(m).into(),
        None => b.into(),
    }
}

/// A text link: underlined in ink-3, ink on the sheet, signal-ink under the pointer. Not uppercased,
/// for a line's own action ("Reply", "Show more", "see usage"); `mono` sets it in the mono voice,
/// otherwise serif.
pub fn link<'a, M: Clone + 'a>(label_text: &str, msg: M, mono: bool) -> Element<'a, M> {
    let body = if mono {
        text(label_text.to_string()).font(MONO).size(TYPE_META)
    } else {
        text(label_text.to_string()).font(SANS).size(TYPE_UI)
    };
    let underline = container(Space::new().height(1.0))
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(crate::theme::palette().ink_3.into()),
            ..Default::default()
        });
    // The text carries no colour of its own: it takes the button's, so the hover signal reaches it.
    button(column![body, underline].spacing(1))
        .padding(0.0)
        .style(|_: &Theme, status: button::Status| button::Style {
            background: None,
            text_color: if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                crate::theme::palette().signal_ink
            } else {
                crate::theme::palette().ink
            },
            border: Border::default(),
            ..Default::default()
        })
        .on_press(msg)
        .into()
}

/// A link that keeps its text's width, for a name among other things in a row: the label in ink, signal-ink under the
/// pointer, no rule under it ([`link`]'s rule spans the space it is given).
pub fn text_link<'a, M: Clone + 'a>(label_text: &str, msg: M) -> Element<'a, M> {
    button(text(label_text.to_string()).font(SANS_MEDIUM).size(TYPE_UI))
        .padding(0.0)
        .style(|_: &Theme, status: button::Status| button::Style {
            background: None,
            text_color: if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                crate::theme::palette().signal_ink
            } else {
                crate::theme::palette().ink
            },
            border: Border::default(),
            ..Default::default()
        })
        .on_press(msg)
        .into()
}

/// A segmented control, as the web's `.seg`: a soft well (radius 9, 2px inset) of cells; the chosen one is raised.
/// Each option pairs a value with its label; a press sends `on_select(value)`.
pub fn seg<'a, T, M>(
    options: &[(T, &str)],
    selected: T,
    on_select: impl Fn(T) -> M,
) -> Element<'a, M>
where
    T: Copy + PartialEq,
    M: Clone + 'a,
{
    let mut strip: Vec<Element<'a, M>> = Vec::new();
    for (value, name) in options.iter() {
        let on = *value == selected;
        let cell = button(text(name.to_string()).font(SANS).size(12.5))
            .padding([5, 10])
            .style(move |_: &Theme, status: button::Status| {
                let p = crate::theme::palette();
                let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
                button::Style {
                    background: on.then(|| p.raised.into()),
                    text_color: if on || hovered { p.ink } else { p.ink_2 },
                    border: Border::default().rounded(7.0),
                    shadow: if on {
                        Shadow {
                            color: Color {
                                a: 0.12,
                                ..Color::BLACK
                            },
                            offset: iced::Vector::new(0.0, 1.0),
                            blur_radius: 2.0,
                        }
                    } else {
                        Shadow::default()
                    },
                    snap: true,
                }
            })
            .on_press(on_select(*value));
        strip.push(cell.into());
    }
    container(
        iced::widget::Row::with_children(strip)
            .spacing(2)
            .align_y(Alignment::Center)
            .wrap(),
    )
    .padding(2)
    .style(|_: &Theme| container::Style {
        background: Some(crate::theme::palette().hover.into()),
        border: Border::default().rounded(9.0),
        ..Default::default()
    })
    .into()
}

/// Underline tabs, as the web's `.tabs`: 500 at 13px, ink-3 until the pointer or the choice reaches them; the chosen
/// tab is ink with a 2px ink rule under it, and a rule runs under the strip. A press sends `on_select(value)`.
pub fn tabs<'a, T, M>(
    options: &[(T, &str)],
    selected: T,
    on_select: impl Fn(T) -> M,
) -> Element<'a, M>
where
    T: Copy + PartialEq,
    M: Clone + 'a,
{
    let mut strip = row![].spacing(22);
    for (value, name) in options.iter() {
        let on = *value == selected;
        // The text carries no colour of its own: it takes the button's state colour.
        // As wide as its label, as the web's tabs are: the underline is measured, not stretched.
        let wide = super::screens::metrics::measure::width(name, SANS_MEDIUM, TYPE_UI);
        let body = column![
            container(text(name.to_string()).font(SANS_MEDIUM).size(TYPE_UI))
                .center_y(Length::Fixed(34.0)),
            container(Space::new().height(2.0))
                .width(Length::Fixed(wide))
                .style(move |_: &Theme| container::Style {
                    background: on.then(|| crate::theme::palette().action.into()),
                    ..Default::default()
                }),
        ];
        let cell = button(body)
            .padding(0.0)
            .style(move |_: &Theme, status: button::Status| {
                let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
                button::Style {
                    background: None,
                    text_color: if on || hovered {
                        crate::theme::palette().ink
                    } else {
                        crate::theme::palette().ink_3
                    },
                    border: Border::default(),
                    ..Default::default()
                }
            })
            .on_press(on_select(*value));
        strip = strip.push(cell);
    }
    column![strip, rule_line()].into()
}

/// An empty state, as the web's `.empty`: the icon at 22, the title in ink 500, the sentence in ink-3; centred.
pub fn empty<'a, M: 'a>(icon_name: &str, title: &str, sentence: &str) -> Element<'a, M> {
    container(
        column![
            icon(icon_name, 22.0, crate::theme::palette().ink_3),
            text(title.to_string())
                .font(SANS_MEDIUM)
                .size(TYPE_ITEM)
                .color(crate::theme::palette().ink),
            text(sentence.to_string())
                .font(SANS)
                .size(TYPE_ITEM)
                .color(crate::theme::palette().ink_3),
        ]
        .spacing(6)
        .align_x(Alignment::Center),
    )
    .padding([44, 16])
    .width(Length::Fill)
    .align_x(Alignment::Center)
    .into()
}

/// An inline error line: serif italic in crit, as the web's `.error-text`.
pub fn error_line<'a, M: 'a>(message: &str) -> Element<'a, M> {
    text(message.to_string())
        .font(SANS_ITALIC)
        .size(TYPE_UI)
        .color(crate::theme::palette().crit)
        .into()
}

/// A faint "Loading…" line.
pub fn loading<'a, M: 'a>() -> Element<'a, M> {
    sans("Loading…", TYPE_META, crate::theme::palette().ink_3).into()
}

/// Agent-written markdown in the design language: mono headings (a hairline under the first),
/// serif prose, inline spans, sheet code blocks, ruled quotes and grid tables. Unknown syntax
/// renders as plain text; nothing is dropped.
pub fn markdown<'a, M: 'a>(source: &str) -> Element<'a, M> {
    markdown_with(source, &plain_para)
}

/// The paragraph renderer the blocks share: a body of text, its base font, size and colour.
pub(crate) type Para<'p, 'a, M> = &'p dyn Fn(&str, Font, f32, Color) -> Element<'a, M>;

/// The same blocks as `markdown`, with each paragraph, list item and table cell drawn by `para` (the rich renderer
/// draws references in them as chips and cards; see `console::rich`).
pub(crate) fn markdown_with<'a, M: 'a>(source: &str, para: Para<'_, 'a, M>) -> Element<'a, M> {
    blocks_view(parse_blocks(source), para)
}

/// The plain paragraph: inline spans as one wrapping rich text.
fn plain_para<'a, M: 'a>(body: &str, base: Font, size: f32, color: Color) -> Element<'a, M> {
    rich_para(inline(body), base, size, color)
}

/// A parsed block, before layout.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Block {
    Heading(u8, String),
    Para(String),
    List(Vec<Item>),
    Code(String),
    Quote(Vec<Block>),
    Table {
        head: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    Rule,
}

/// One list item. `depth` 1 nests under the item before it; `marker` numbers an ordered item.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    depth: usize,
    marker: Option<usize>,
    text: String,
}

/// An inline span of a paragraph, heading or cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Inline {
    Text(String),
    Bold(String),
    Italic(String),
    Code(String),
    /// A link's label, shown underlined in signal; the target is not followed.
    Link(String),
}

fn parse_blocks(source: &str) -> Vec<Block> {
    let lines: Vec<&str> = source.lines().collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        if line.is_empty() {
            i += 1;
        } else if line.starts_with("```") {
            // A fence runs to its closing line or to the end of the document; unterminated, its text stays.
            let mut code = Vec::new();
            i += 1;
            while i < lines.len() && lines[i].trim() != "```" {
                code.push(lines[i]);
                i += 1;
            }
            i += 1;
            blocks.push(Block::Code(code.join("\n")));
        } else if let Some((level, title)) = heading(line) {
            blocks.push(Block::Heading(level, title.to_string()));
            i += 1;
        } else if is_rule(line) {
            blocks.push(Block::Rule);
            i += 1;
        } else if line.starts_with('>') {
            let mut inner = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with('>') {
                let body = lines[i].trim_start().strip_prefix('>').unwrap_or_default();
                inner.push(body.strip_prefix(' ').unwrap_or(body));
                i += 1;
            }
            blocks.push(Block::Quote(parse_blocks(&inner.join("\n"))));
        } else if list_item(lines[i]).is_some() {
            let mut items = Vec::new();
            while i < lines.len() {
                if let Some(item) = list_item(lines[i]) {
                    items.push(item);
                } else if let (true, Some(last)) = (
                    lines[i].starts_with(' ') && !lines[i].trim().is_empty(),
                    items.last_mut(),
                ) {
                    // An indented line continues the item above.
                    last.text = format!("{} {}", last.text, lines[i].trim());
                } else {
                    break;
                }
                i += 1;
            }
            blocks.push(Block::List(items));
        } else if is_table_start(&lines, i) {
            let head = split_row(lines[i]);
            i += 2;
            let mut rows = Vec::new();
            while i < lines.len() && lines[i].contains('|') && !lines[i].trim().is_empty() {
                rows.push(split_row(lines[i]));
                i += 1;
            }
            blocks.push(Block::Table { head, rows });
        } else {
            // A paragraph runs to a blank line or the start of another block; its lines join with spaces.
            let mut para = vec![line];
            i += 1;
            while i < lines.len() && !lines[i].trim().is_empty() && !starts_block(&lines, i) {
                para.push(lines[i].trim());
                i += 1;
            }
            blocks.push(Block::Para(para.join(" ")));
        }
    }
    blocks
}

/// `#`, `##` or `###` and a title: the line's level and its text.
fn heading(line: &str) -> Option<(u8, &str)> {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    let title = line.get(hashes..)?.strip_prefix(' ')?;
    matches!(hashes, 1..=3).then_some((hashes as u8, title.trim()))
}

/// Three or more of one of `-`, `*`, `_` alone on a line.
fn is_rule(line: &str) -> bool {
    line.len() >= 3
        && ['-', '*', '_']
            .iter()
            .any(|&c| line.chars().all(|x| x == c))
}

/// A list item line: a bullet (`-`, `*`, `+`) or a number (`1.`, `1)`), at depth 0 or, indented, 1.
fn list_item(line: &str) -> Option<Item> {
    let indent = line.len() - line.trim_start().len();
    let t = line.trim();
    let depth = usize::from(indent >= 2);
    if let Some(body) = t
        .strip_prefix(['-', '*', '+'])
        .and_then(|r| r.strip_prefix(' '))
    {
        return Some(Item {
            depth,
            marker: None,
            text: body.trim().to_string(),
        });
    }
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    if !(1..=9).contains(&digits) {
        return None;
    }
    let rest = t.get(digits..)?;
    let body = rest
        .strip_prefix(". ")
        .or_else(|| rest.strip_prefix(") "))?;
    let marker = t[..digits].parse().ok()?;
    Some(Item {
        depth,
        marker: Some(marker),
        text: body.trim().to_string(),
    })
}

fn starts_block(lines: &[&str], i: usize) -> bool {
    let Some(raw) = lines.get(i) else {
        return false;
    };
    let line = raw.trim();
    line.starts_with("```")
        || heading(line).is_some()
        || is_rule(line)
        || line.starts_with('>')
        || list_item(raw).is_some()
        || is_table_start(lines, i)
}

fn is_table_start(lines: &[&str], i: usize) -> bool {
    match (lines.get(i), lines.get(i + 1)) {
        (Some(head), Some(sep)) => {
            head.contains('|') && is_table_sep(sep) && split_row(head).len() == split_row(sep).len()
        }
        _ => false,
    }
}

fn is_table_sep(line: &str) -> bool {
    let t = line.trim();
    t.contains('-') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// The cells of a table row, outer pipes dropped.
fn split_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

/// Inline spans: `code`, **bold**, *italic*, [label](target). Anything else is text.
pub(crate) fn inline(source: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut plain = String::new();
    let mut rest = source;
    while !rest.is_empty() {
        if let Some((token, used)) = inline_token(rest) {
            if !plain.is_empty() {
                out.push(Inline::Text(std::mem::take(&mut plain)));
            }
            out.push(token);
            rest = &rest[used..];
        } else {
            let width = rest.chars().next().map_or(1, char::len_utf8);
            plain.push_str(&rest[..width]);
            rest = &rest[width..];
        }
    }
    if !plain.is_empty() {
        out.push(Inline::Text(plain));
    }
    out
}

/// The span at the start of `s` and how many bytes it used, if `s` opens one that closes.
fn inline_token(s: &str) -> Option<(Inline, usize)> {
    if let Some(body) = s.strip_prefix('`') {
        let end = body.find('`')?;
        Some((Inline::Code(body[..end].to_string()), end + 2))
    } else if let Some(body) = s.strip_prefix("**") {
        let end = body.find("**")?;
        Some((Inline::Bold(body[..end].to_string()), end + 4))
    } else if let Some(body) = s.strip_prefix('*') {
        let end = body.find('*')?;
        if end == 0 || body.starts_with(char::is_whitespace) {
            return None;
        }
        Some((Inline::Italic(body[..end].to_string()), end + 2))
    } else if let Some(body) = s.strip_prefix('[') {
        let close = body.find(']')?;
        let end = body[close + 1..].strip_prefix('(')?.find(')')?;
        Some((Inline::Link(body[..close].to_string()), close + end + 4))
    } else {
        None
    }
}

fn blocks_view<'a, M: 'a>(blocks: Vec<Block>, para: Para<'_, 'a, M>) -> Element<'a, M> {
    column(
        blocks
            .into_iter()
            .map(|b| block_view(b, para))
            .collect::<Vec<_>>(),
    )
    .spacing(12)
    .width(Length::Fill)
    .into()
}

fn block_view<'a, M: 'a>(block: Block, para: Para<'_, 'a, M>) -> Element<'a, M> {
    match block {
        Block::Heading(level, title) => {
            let size = match level {
                1 => 18.0,
                2 => 15.0,
                _ => 13.0,
            };
            let head = text(title)
                .font(SANS_SEMIBOLD)
                .size(size)
                .color(crate::theme::palette().ink);
            if level == 1 {
                column![head, hairline()]
                    .spacing(8)
                    .width(Length::Fill)
                    .into()
            } else {
                head.into()
            }
        }
        Block::Para(body) => para(&body, SANS, TYPE_PROSE, crate::theme::palette().ink),
        Block::List(items) => column(
            items
                .into_iter()
                .map(|i| item_view(i, para))
                .collect::<Vec<_>>(),
        )
        .spacing(4)
        .width(Length::Fill)
        .into(),
        Block::Code(code) => container(scroll_x(
            text(code)
                .font(MONO)
                .size(TYPE_CODE)
                .color(crate::theme::palette().ink),
        ))
        .padding(10)
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(crate::theme::palette().sheet.into()),
            border: Border {
                color: crate::theme::palette().rule,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into(),
        Block::Quote(inner) => {
            let bar = container(Space::new())
                .width(Length::Fixed(3.0))
                .height(Length::Fill)
                .style(|_: &Theme| container::Style {
                    background: Some(crate::theme::palette().ink_3.into()),
                    ..Default::default()
                });
            row![bar, blocks_view(inner, para)]
                .spacing(14)
                .width(Length::Fill)
                .into()
        }
        Block::Table { head, rows } => {
            let cols = rows
                .iter()
                .map(Vec::len)
                .chain(std::iter::once(head.len()))
                .max()
                .unwrap_or(1)
                .max(1);
            let mut grid = vec![table_row(&head, cols, true, para)];
            grid.extend(rows.iter().map(|r| table_row(r, cols, false, para)));
            column(grid).width(Length::Fill).into()
        }
        Block::Rule => hairline(),
    }
}

fn item_view<'a, M: 'a>(item: Item, para: Para<'_, 'a, M>) -> Element<'a, M> {
    let marker = item
        .marker
        .map_or_else(|| "•".to_string(), |n| format!("{n}."));
    let indent = if item.depth > 0 { 24.0 } else { 0.0 };
    row![
        Space::new().width(Length::Fixed(indent)),
        sans(marker, TYPE_UI, crate::theme::palette().ink_2).width(Length::Fixed(22.0)),
        para(&item.text, SANS, TYPE_PROSE, crate::theme::palette().ink),
    ]
    .spacing(6)
    .width(Length::Fill)
    .into()
}

/// A table row over `cols` equal cells; the header's cells are mono.
fn table_row<'a, M: 'a>(
    cells: &[String],
    cols: usize,
    header: bool,
    para: Para<'_, 'a, M>,
) -> Element<'a, M> {
    let row_cells = (0..cols)
        .map(|k| -> Element<'a, M> {
            let cell = cells.get(k).map_or("", String::as_str);
            let content = if header {
                para(cell, SANS_SEMIBOLD, TYPE_META, crate::theme::palette().ink)
            } else {
                para(cell, SANS, TYPE_UI, crate::theme::palette().ink)
            };
            container(content)
                .padding([5, 8])
                .width(Length::FillPortion(1))
                .style(|_: &Theme| container::Style {
                    border: Border {
                        color: crate::theme::palette().rule,
                        width: 1.0,
                        radius: 0.0.into(),
                    },
                    ..Default::default()
                })
                .into()
        })
        .collect::<Vec<_>>();
    row(row_cells).width(Length::Fill).into()
}

/// A run of inline spans as one wrapping rich text.
pub(crate) fn rich_para<'a, M: 'a>(
    parts: Vec<Inline>,
    base: Font,
    size: f32,
    color: Color,
) -> Element<'a, M> {
    let spans: Vec<text::Span<'a>> = parts
        .into_iter()
        .map(|part| part_span(part, base))
        .collect();
    rich_text(spans)
        .font(base)
        .size(size)
        .line_height(READING)
        .color(color)
        .width(Length::Fill)
        .into()
}

fn part_span<'a>(part: Inline, base: Font) -> text::Span<'a> {
    match part {
        Inline::Text(s) => span(s),
        Inline::Bold(s) => span(s).font(Font {
            weight: iced::font::Weight::Semibold,
            ..base
        }),
        Inline::Italic(s) => span(s).font(Font {
            style: iced::font::Style::Italic,
            ..base
        }),
        Inline::Code(s) => span(s)
            .font(MONO)
            .background(crate::theme::palette().rule_faint),
        Inline::Link(label) => span(label)
            .color(crate::theme::palette().signal)
            .underline(true),
    }
}

/// The page background.
pub fn paper(_: &Theme) -> container::Style {
    container::Style {
        background: Some(crate::theme::palette().paper.into()),
        ..Default::default()
    }
}

/// A hairline rule.
pub fn hairline<'a, M: 'a>() -> Element<'a, M> {
    container(Space::new().height(1.0))
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(crate::theme::palette().rule_faint.into()),
            ..Default::default()
        })
        .into()
}

/// The sizes a button is drawn at.
#[derive(Clone, Copy)]
struct BtnSize {
    icon: f32,
    text: f32,
    padding: [f32; 2],
    height: f32,
    radius: f32,
    gap: f32,
}

const BTN: BtnSize = BtnSize {
    icon: 15.0,
    text: TYPE_UI,
    padding: [0.0, 11.0],
    height: 30.0,
    radius: 8.0,
    gap: 7.0,
};
const BTN_SM: BtnSize = BtnSize {
    icon: 14.0,
    text: 12.5,
    padding: [0.0, 9.0],
    height: 26.0,
    radius: 7.0,
    gap: 5.0,
};

/// Lays cards out in rows of three equal columns; a short last row keeps the same column widths.
const CARD_GRID_COLS: usize = 3;

pub fn card_grid<'a, M: 'a>(cards: Vec<Element<'a, M>>) -> Element<'a, M> {
    let mut out = column(Vec::<Element<'a, M>>::new())
        .spacing(16)
        .width(Length::Fill);
    let mut line: Vec<Element<'a, M>> = Vec::new();
    for card in cards {
        line.push(container(card).width(Length::FillPortion(1)).into());
        if line.len() == CARD_GRID_COLS {
            out = out.push(
                row(std::mem::take(&mut line))
                    .spacing(16)
                    .width(Length::Fill),
            );
        }
    }
    if !line.is_empty() {
        while line.len() < CARD_GRID_COLS {
            line.push(Space::new().width(Length::FillPortion(1)).into());
        }
        out = out.push(row(line).spacing(16).width(Length::Fill));
    }
    out.into()
}

/// Chips in a wrapping row: a chip that runs past the row's width flows onto the next line.
pub fn chip_row<'a, M: 'a>(chips: Vec<Element<'a, M>>) -> Element<'a, M> {
    row(chips).spacing(6).wrap().vertical_spacing(6).into()
}

/// A bordered panel, optionally filled: the web's cards, tiles and mono frames.
pub fn frame(fill: Option<Color>, edge: Color) -> impl Fn(&Theme) -> container::Style {
    move |_: &Theme| container::Style {
        background: fill.map(Into::into),
        border: Border {
            color: edge,
            width: 1.0,
            radius: crate::theme::skin().radius.into(),
        },
        ..Default::default()
    }
}

/// The web's dialog (`modal()` in `ui/components.ts`: `.scrim > .sheet` with `.sheet-h`, `.sheet-b` and
/// `footer.sheet-f`): a sheet `width` wide over the scrim, its title and close button at the head, the body
/// scrolling, and the buttons right-aligned in the foot. Draw it over a screen with [`with_modal`].
pub fn modal<'a, M: Clone + 'a>(
    title: &'a str,
    body: Element<'a, M>,
    foot: Vec<Element<'a, M>>,
    on_close: M,
    width: f32,
) -> Element<'a, M> {
    let p = crate::theme::palette();
    let on_close_scrim = on_close.clone();
    let head = container(
        row![
            text(title)
                .font(SANS_SEMIBOLD)
                .size(15.0)
                .color(p.ink)
                .width(Length::Fill),
            btn_sm("", Some("x"), Kind::Ghost, Some(on_close)),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .padding(Padding {
        top: 14.0,
        right: 12.0,
        bottom: 6.0,
        left: 20.0,
    });
    // The body scrolls within what the sheet leaves it, so the foot's buttons always show.
    let body_max = if foot.is_empty() {
        MODAL_MAX - 60.0
    } else {
        MODAL_MAX - 124.0
    };
    let mut sheet = column![
        head,
        container(
            scroll(container(body).padding(Padding {
                top: 6.0,
                right: 20.0,
                bottom: 16.0,
                left: 20.0,
            }))
            .height(Length::Shrink),
        )
        .max_height(body_max),
    ];
    if !foot.is_empty() {
        let line = container(Space::new().height(1.0))
            .width(Length::Fill)
            .style(move |_: &Theme| container::Style {
                background: Some(crate::theme::palette().rule.into()),
                ..Default::default()
            });
        sheet = sheet.push(line).push(
            container(
                row![Space::new().width(Length::Fill)]
                    .extend(foot)
                    .spacing(8)
                    .align_y(Alignment::Center),
            )
            .padding(Padding {
                top: 12.0,
                right: 16.0,
                bottom: 12.0,
                left: 16.0,
            }),
        );
    }
    let sheet = container(sheet)
        .width(Length::Fixed(width))
        .max_height(MODAL_MAX)
        .style(crate::theme::pop);
    // A press on the scrim closes the dialog, as on the web; the sheet itself takes its own presses.
    let scrim = iced::widget::mouse_area(
        container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(crate::theme::scrim),
    )
    .on_press(on_close_scrim);
    iced::widget::stack![
        scrim,
        container(iced::widget::opaque(sheet))
            .width(Length::Fill)
            .height(Length::Fill)
            .center(Length::Fill),
    ]
    .into()
}

/// The tallest a dialog's sheet gets.
const MODAL_MAX: f32 = 760.0;

/// A screen with an optional dialog over it.
pub fn with_modal<'a, M: 'a>(
    base: Element<'a, M>,
    dialog: Option<Element<'a, M>>,
) -> Element<'a, M> {
    match dialog {
        Some(dialog) => iced::widget::stack![base, dialog].into(),
        None => base,
    }
}

/// CSS `hsl()`: hue in degrees, saturation and lightness 0 to 1.
pub fn hsl(h: f32, s: f32, l: f32) -> Color {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - ((hp % 2.0) - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    Color::from_rgb(r + m, g + m, b + m)
}

/// A transcript disclosure: a marker and a mono label in a tone, that opens and shuts its body.
pub fn disclosure<'a, M: Clone + 'a>(
    label: String,
    tone: Color,
    opened: bool,
    msg: M,
) -> Element<'a, M> {
    let marker = if opened { "▾" } else { "▸" };
    button(
        row![
            sans(marker, TYPE_BADGE, crate::theme::palette().ink_3),
            sans(label, TYPE_META, tone)
        ]
        .spacing(6),
    )
    .padding(0)
    .style(|_: &Theme, _: button::Status| button::Style {
        background: None,
        text_color: crate::theme::palette().ink,
        border: Border::default(),
        ..Default::default()
    })
    .on_press(msg)
    .into()
}

/// A tool call's name as a chip: bordered, mono, opening its input.
pub fn chip<'a, M: Clone + 'a>(label: String, opened: bool, msg: M) -> Element<'a, M> {
    let marker = if opened { "▾" } else { "▸" };
    button(
        row![
            sans(marker, TYPE_BADGE, crate::theme::palette().ink_3),
            sans(label, TYPE_BADGE, crate::theme::palette().ink_2)
        ]
        .spacing(6),
    )
    .padding([2, 8])
    .style(|_: &Theme, _: button::Status| button::Style {
        background: Some(crate::theme::palette().hover.into()),
        text_color: crate::theme::palette().ink_2,
        border: Border {
            color: crate::theme::palette().rule,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    })
    .on_press(msg)
    .into()
}

/// `s` cut to `max_chars` characters, the last of them an ellipsis when it had to be cut.
/// A helper only: iced cannot clip text with a trailing ellipsis natively.
pub fn ellipsize(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    if max_chars == 0 {
        return String::new();
    }
    let kept: String = s.chars().take(max_chars - 1).collect();
    format!("{}…", kept.trim_end())
}

/// How long ago an epoch-millisecond instant was, in the web's short words: "42s ago", "3m ago", "2h ago", "5d ago".
pub fn ago(ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(ms);
    let secs = now.saturating_sub(ms) / 1000;
    match secs {
        0..=59 => format!("{secs}s ago"),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

/// Dollars to the cent with separators; a non-zero amount under a cent reads as "<$0.01".
pub fn usd(v: f64) -> String {
    if v != 0.0 && v.abs() < 0.005 {
        return "<$0.01".to_string();
    }
    let sign = if v < 0.0 { "-" } else { "" };
    let s = format!("{:.2}", v.abs());
    let (int, frac) = s.split_once('.').unwrap_or((&s, "00"));
    format!("{sign}${}.{frac}", commas(int))
}

/// A whole number with thousands separators: 15000 is "15,000".
pub fn thousands(n: i64) -> String {
    let sign = if n < 0 { "-" } else { "" };
    format!("{sign}{}", commas(&n.unsigned_abs().to_string()))
}

/// Digits with thousands separators.
fn commas(digits: &str) -> String {
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A sidebar section heading (the web's `.sec`): the skin's label voice in ink-3.
pub fn section<'a>(content: &str) -> text::Text<'a> {
    label(content)
}

/// A sidebar count (the web's `.count`): 500 at 11px in ink-3; `hot` (waiting on the owner) on the signal fill.
pub fn count<'a, M: 'a>(n: u32) -> Element<'a, M> {
    text(n.to_string())
        .font(SANS_MEDIUM)
        .size(TYPE_BADGE)
        .color(crate::theme::palette().ink_3)
        .into()
}

/// A count that needs the owner (`.count.hot`): on-signal on the signal fill, rounded.
pub fn count_hot<'a, M: 'a>(n: u32) -> Element<'a, M> {
    container(
        text(n.to_string())
            .font(SANS_MEDIUM)
            .size(TYPE_BADGE)
            .color(crate::theme::palette().on_signal),
    )
    .padding([3, 6])
    .style(|_: &Theme| container::Style {
        background: Some(crate::theme::palette().signal_fill.into()),
        border: Border::default().rounded(9.0),
        ..Default::default()
    })
    .into()
}

/// The scrollbar's width in px: thin, as the web's `scrollbar-width: thin`.
const BAR_WIDTH: f32 = 6.0;

fn bar() -> scrollable::Scrollbar {
    scrollable::Scrollbar::new()
        .width(BAR_WIDTH)
        .scroller_width(BAR_WIDTH)
        .margin(0.0)
}

/// The app's scrollbar, shared by every scroll area: a transparent rail with a rounded thumb in
/// the rule colour, lighter while the bar is hovered and brighter while it is dragged.
fn bar_style(theme: &Theme, status: scrollable::Status) -> scrollable::Style {
    let (hot, dragged) = match status {
        scrollable::Status::Active { .. } => (false, false),
        scrollable::Status::Hovered {
            is_vertical_scrollbar_hovered,
            is_horizontal_scrollbar_hovered,
            ..
        } => (
            is_vertical_scrollbar_hovered || is_horizontal_scrollbar_hovered,
            false,
        ),
        scrollable::Status::Dragged {
            is_vertical_scrollbar_dragged,
            is_horizontal_scrollbar_dragged,
            ..
        } => (
            true,
            is_vertical_scrollbar_dragged || is_horizontal_scrollbar_dragged,
        ),
    };
    let thumb = if dragged {
        crate::theme::palette().ink_2
    } else if hot {
        crate::theme::palette().ink_3
    } else {
        crate::theme::palette().rule
    };
    let rail = scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller {
            background: thumb.into(),
            border: Border {
                radius: 3.0.into(),
                ..Default::default()
            },
        },
    };
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail,
        horizontal_rail: rail,
        gap: None,
        auto_scroll: scrollable::default(theme, status).auto_scroll,
    }
}

/// A vertical scroll area in the app's look. Every `scrollable(...)` in the console goes through
/// this (or `scroll_x`), so the scrollbars match everywhere.
pub fn scroll<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> scrollable::Scrollable<'a, M> {
    scrollable(content)
        .direction(scrollable::Direction::Vertical(bar()))
        .style(bar_style)
}

/// A horizontal scroll area in the app's look (see `scroll`).
pub fn scroll_x<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> scrollable::Scrollable<'a, M> {
    scrollable(content)
        .direction(scrollable::Direction::Horizontal(bar()))
        .style(bar_style)
}

/// Whether [`open_url`] may open `url`: an `https://` link with a host, and no whitespace or
/// control character anywhere (a leading space, a newline or a tab is refused).
pub fn openable(url: &str) -> bool {
    url.len() > "https://".len()
        && url.starts_with("https://")
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Opens an `https://` link in the system's browser: `open` on macOS, the URL protocol handler on
/// Windows, `xdg-open` elsewhere. The URL is passed as one argument (no shell), and any other scheme
/// is refused.
pub fn open_url(url: &str) -> Result<(), String> {
    if !openable(url) {
        return Err(format!("not an https link: {url:?}"));
    }
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = std::process::Command::new("xdg-open");
    let mut child = command
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("opening {url}: {e}"))?;
    // Reap the opener when it exits, so it never lingers as a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod open_url_tests {
    use super::{commas, openable, usd};

    #[test]
    fn openable_accepts_https_links_and_refuses_every_other_url() {
        for (url, want) in [
            // accepts_an_https_link
            ("https://huggingface.co/settings/tokens", true),
            ("https://huggingface.co/meta-llama/Llama-3.2-1B", true),
            // refuses_every_other_scheme
            ("http://huggingface.co/", false),
            ("file:///etc/passwd", false),
            ("javascript:alert(1)", false),
            ("data:text/html,<p>x</p>", false),
            ("ftp://example.com/", false),
            ("mailto:owner@example.com", false),
            ("HTTPS://huggingface.co/", false),
            ("https:/huggingface.co/", false),
            ("https://", false),
            ("", false),
            // refuses_whitespace_and_control_characters
            (" https://huggingface.co/", false),
            ("https://huggingface.co/\nrm -rf /", false),
            ("https://huggingface.co/a b", false),
            ("https://huggingface.co/\t", false),
            ("https://huggingface.co/\r", false),
            ("https://huggingface.co/\u{0}", false),
        ] {
            assert_eq!(openable(url), want, "openable({url:?})");
        }
    }

    #[test]
    fn a_past_instant_reads_in_the_short_words() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert_eq!(super::ago(now - 42_000), "42s ago");
        assert_eq!(super::ago(now - 90_000), "1m ago");
        assert_eq!(super::ago(now - 7_200_000), "2h ago");
        assert_eq!(super::ago(now - 5 * 86_400_000), "5d ago");
    }

    #[test]
    fn money_reads_to_the_cent_with_separators() {
        assert_eq!(usd(1234.5), "$1,234.50");
        assert_eq!(usd(0.0), "$0.00");
        assert_eq!(usd(0.004), "<$0.01");
        assert_eq!(usd(-12.0), "-$12.00");
        assert_eq!(commas("1234567"), "1,234,567");
    }
}
