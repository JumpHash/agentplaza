//! The register's visual language: the night-navy "Plaza" palette, the same
//! tokens as the web console (`apps/web/src/styles/tokens.css`), an Inter/Martian Mono pairing, ruled sheets and
//! stamps — as far as iced's widgets carry it (no rotation, no dot leaders:
//! the rules and the space between them stand in).

use apz_engine::style::Style;
use iced::widget::text::LineHeight;
use iced::widget::{button, container, rule, text_input};
use iced::{
    Background, Border, Color, Font, Shadow, Theme, Vector, font, theme::Palette as IcedPalette,
};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// The page behind everything: the console's night navy.
pub const PAPER: Color = Color::from_rgb8(0x0a, 0x0e, 0x19);
/// A sheet laid on the page.
pub const SHEET: Color = Color::from_rgb8(0x11, 0x17, 0x26);
/// A ruled line.
pub const RULE: Color = Color::from_rgb8(0x26, 0x30, 0x49);
/// A fainter rule, for the space between fields.
pub const RULE_FAINT: Color = Color::from_rgb8(0x1a, 0x21, 0x35);
/// The red margin rule.
pub const MARGIN: Color = Color::from_rgb8(0x6e, 0x3a, 0x55);
/// The ink: light type on the dark page.
pub const INK: Color = Color::from_rgb8(0xe3, 0xe8, 0xf4);
/// Secondary ink.
pub const INK_2: Color = Color::from_rgb8(0xa3, 0xad, 0xc4);
/// Tertiary ink, for labels.
pub const INK_3: Color = Color::from_rgb8(0x6c, 0x77, 0x90);
/// The one hot signal.
pub const SIGNAL: Color = Color::from_rgb8(0xff, 0x7a, 0x2e);
/// The signal's ink: a link or a lit label under the pointer (the web's `--signal-ink`, night theme).
pub const SIGNAL_INK: Color = Color::from_rgb8(0xff, 0x9a, 0x5c);
/// The ink on a solid signal fill: the primary button's text (the web's `#120800`).
pub const ON_SIGNAL: Color = Color::from_rgb8(0x12, 0x08, 0x00);
/// Information: a note, a passive state (the web's `--info-ink`, night theme). The register is
/// dark only, so there is no light value.
pub const INFO: Color = Color::from_rgb8(0x6c, 0xc8, 0xff);
/// A refusal, a kill, a failure.
pub const CRIT: Color = Color::from_rgb8(0xff, 0x7c, 0x86);
/// A caution.
pub const WARN: Color = Color::from_rgb8(0xe2, 0xbd, 0x5a);
/// Something finished well.
pub const OK: Color = Color::from_rgb8(0x5f, 0xe0, 0xa0);
/// A stamped, held state: a dark amber fill.
pub const TAINT: Color = Color::from_rgb8(0x2e, 0x2a, 0x17);
/// A hovered row.
pub const HOVER: Color = Color::from_rgb8(0x18, 0x20, 0x36);
/// The machine's substrate region on the fleet map: a hair off the sheet, so
/// the bounded region reads as bounded without becoming a filled block.
pub const SUBSTRATE: Color = Color::from_rgb8(0x0e, 0x13, 0x21);

/// The interface and reading voice: Inter, embedded by the console (`console::run`) and the
/// app's default font, so a text with no font of its own is Inter. Its weights are the TTFs
/// the console embeds (Regular, Medium, SemiBold, Bold, Italic); they share the "Inter" family.
pub const SANS: Font = Font::with_name("Inter");
/// Inter at 500: controls and buttons.
pub const SANS_MEDIUM: Font = Font {
    weight: font::Weight::Medium,
    ..SANS
};
/// Inter at 600: titles, selected tabs, badges and the labels that were uppercase mono.
pub const SANS_SEMIBOLD: Font = Font {
    weight: font::Weight::Semibold,
    ..SANS
};
/// Inter at 700: figures and the empty-state titles.
pub const SANS_BOLD: Font = Font {
    weight: font::Weight::Bold,
    ..SANS
};
/// Inter in italic: the error line and the notes.
pub const SANS_ITALIC: Font = Font {
    style: font::Style::Italic,
    ..SANS
};

/// The code voice: code, diffs, terminals, raw tool output, and the ids, hashes and paths where
/// alignment matters. Martian Mono, embedded by the console (`console::run`). Nothing else.
pub const MONO: Font = Font::with_name("Martian Mono");

/// The type roles (`docs/design/workspace.md`, section 1), in iced points (the web's CSS px).
/// Page title: 20, line 1.3, 600.
pub const TYPE_PAGE: f32 = 20.0;
/// Section title (a card's head): 16, line 1.4, 600.
pub const TYPE_SECTION: f32 = 16.0;
/// Item title: 14, line 1.4, 600.
pub const TYPE_ITEM: f32 = 14.0;
/// Reading prose (transcripts, board posts, conversations): 15, line 1.6, 400.
pub const TYPE_PROSE: f32 = 15.0;
/// Interface text and controls: 13, line 1.4, 500 (600 when selected).
pub const TYPE_UI: f32 = 13.0;
/// Metadata, timestamps, helper text: 12, line 1.4, 400, secondary colour.
pub const TYPE_META: f32 = 12.0;
/// Badges and counts: 11, line 1.3, 600.
pub const TYPE_BADGE: f32 = 11.0;
/// Code: 13, line 1.5, Martian Mono.
pub const TYPE_CODE: f32 = 13.0;
/// The reading line height: 1.6 times the size.
pub const READING: LineHeight = LineHeight::Relative(1.6);

/// How a stamp reads: what colour it is pressed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Nothing to see.
    Neutral,
    /// Something finished well.
    Ok,
    /// Something needs watching.
    Warn,
    /// Something failed, was refused or was killed.
    Crit,
    /// Something is asking for attention.
    Signal,
}

impl Tone {
    /// The tone's colour.
    pub fn color(self) -> Color {
        match self {
            Tone::Neutral => crate::theme::palette().ink_2,
            Tone::Ok => crate::theme::palette().ok,
            Tone::Warn => crate::theme::palette().warn,
            Tone::Crit => crate::theme::palette().crit,
            Tone::Signal => crate::theme::palette().signal,
        }
    }
}

/// The application theme: the Plaza night palette, dark by default, so
/// default-styled widgets (icons, selections, scroll rails) match the register.
pub fn theme() -> Theme {
    let p = palette();
    Theme::custom(
        if is_light() {
            "AgentPlaza register (light)"
        } else {
            "AgentPlaza register"
        },
        IcedPalette {
            background: p.paper,
            text: p.ink,
            primary: p.signal,
            success: p.ok,
            warning: p.warn,
            danger: p.crit,
        },
    )
}

/// The page.
pub fn paper(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(crate::theme::palette().ink),
        background: Some(crate::theme::palette().paper.into()),
        ..container::Style::default()
    }
}

/// The index column, ruled off from the sheet.
pub fn index(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(crate::theme::palette().ink_2),
        background: Some(crate::theme::palette().paper.into()),
        border: Border::default()
            .width(1.0)
            .color(crate::theme::palette().rule),
        snap: true,
        ..container::Style::default()
    }
}

/// A sheet: a slightly brighter page with a hard print shadow.
pub fn sheet(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(crate::theme::palette().ink),
        background: Some(crate::theme::palette().sheet.into()),
        border: Border::default()
            .width(1.0)
            .color(crate::theme::palette().rule),
        shadow: print_shadow(),
        snap: true,
    }
}

/// The red margin rule down the left of the sheet's content.
pub fn margin(_theme: &Theme) -> container::Style {
    container::Style {
        border: Border {
            color: crate::theme::palette().margin,
            width: 2.0,
            ..Border::default()
        },
        snap: true,
        ..container::Style::default()
    }
}

/// A solid colour swatch: the floor legend's status ink, a small filled box.
pub fn swatch(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme: &Theme| container::Style {
        background: Some(Background::Color(color)),
        border: Border::default()
            .width(1.0)
            .color(crate::theme::palette().ink_3),
        snap: true,
        ..container::Style::default()
    }
}

/// A stamp: a boxed word, pressed in `tone`.
pub fn stamp(tone: Tone) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        text_color: Some(tone.color()),
        background: Some(crate::theme::palette().sheet.into()),
        border: Border {
            color: tone.color(),
            width: 2.0,
            ..Border::default()
        },
        snap: true,
        ..container::Style::default()
    }
}

/// A plain bounded box: the serial pane, a key/value block.
pub fn boxed(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(crate::theme::palette().ink),
        background: Some(crate::theme::palette().sheet.into()),
        border: Border::default()
            .width(1.0)
            .color(crate::theme::palette().rule_faint),
        snap: true,
        ..container::Style::default()
    }
}

/// The help sheet's scrim: the register dimmed under a sheet that is over it,
/// not instead of it.
pub fn scrim(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgba8(0x15, 0x19, 0x16, 0.55).into()),
        snap: true,
        ..container::Style::default()
    }
}

/// The notice line: the last action's outcome.
pub fn notice(tone: Tone) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        text_color: Some(crate::theme::palette().ink),
        background: Some(
            if tone == Tone::Crit {
                Color::from_rgba8(0xa3, 0x18, 0x2b, 0.08)
            } else {
                crate::theme::palette().taint
            }
            .into(),
        ),
        border: Border {
            color: tone.color(),
            width: 1.0,
            ..Border::default()
        },
        snap: true,
        ..container::Style::default()
    }
}

/// A register row: an entry in the index, or a run in the fleet table.
///
/// `selected` presses the row; hover lifts it.
pub fn register_row(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let background = match (selected, status) {
            (true, _) => Some(crate::theme::palette().taint.into()),
            (false, button::Status::Hovered | button::Status::Pressed) => {
                Some(crate::theme::palette().hover.into())
            }
            (false, button::Status::Active | button::Status::Disabled) => None,
        };
        button::Style {
            background,
            text_color: crate::theme::palette().ink,
            border: Border::default(),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// A committing press in `tone`: it has to look like one, at every step.
///
/// `armed` is the second press's own look — the tone filled, its label spent —
/// while an unarmed press wears the tone's ink on the sheet. It is the one body
/// behind every arm-then-confirm press in the register: a run's kill, a task's
/// withdraw, an agent's stop, and the owner's ratify and veto.
pub fn act_button(tone: Tone, armed: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| button::Style {
        background: Some(
            if armed {
                tone.color()
            } else {
                match status {
                    button::Status::Hovered | button::Status::Pressed => Color {
                        a: 0.12,
                        ..tone.color()
                    },
                    button::Status::Active | button::Status::Disabled => {
                        crate::theme::palette().sheet
                    }
                }
            }
            .into(),
        ),
        text_color: if armed {
            crate::theme::palette().sheet
        } else {
            tone.color()
        },
        border: Border::default().width(2.0).color(tone.color()),
        shadow: Shadow::default(),
        snap: true,
    }
}

/// A destructive action: it has to look like one, at every step — [`act_button`]
/// in the register's red.
pub fn kill_button(armed: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    act_button(Tone::Crit, armed)
}

/// A form field: ruled, on the sheet.
pub fn input(_theme: &Theme, status: text_input::Status) -> text_input::Style {
    text_input::Style {
        background: if matches!(
            status,
            text_input::Status::Focused { .. } | text_input::Status::Hovered
        ) {
            Background::Color(crate::theme::palette().hover)
        } else {
            Background::Color(crate::theme::palette().sheet)
        },
        border: Border::default()
            .width(1.0)
            .color(crate::theme::palette().rule),
        icon: crate::theme::palette().ink_3,
        placeholder: crate::theme::palette().ink_3,
        value: crate::theme::palette().ink,
        selection: crate::theme::palette().signal,
    }
}

/// The hard offset shadow the console prints boxed things with. iced drops the text of a box
/// whose shadow has exactly zero blur, so a hair of blur keeps it crisp and the text drawn.
fn print_shadow() -> Shadow {
    Shadow {
        color: Color::BLACK,
        offset: Vector::new(2.0, 2.0),
        blur_radius: 0.6,
    }
}

/// A ruled line.
pub fn rule(_theme: &Theme) -> rule::Style {
    ruled(crate::theme::palette().rule)
}

/// A ruled line drawn in ink: under a heading, above a total.
pub fn rule_ink(_theme: &Theme) -> rule::Style {
    ruled(crate::theme::palette().ink)
}

/// A full-width ruled line in `color`.
fn ruled(color: Color) -> rule::Style {
    rule::Style {
        color,
        radius: 0.0.into(),
        fill_mode: rule::FillMode::Full,
        snap: true,
    }
}

/// The pick list as the web's select: no fill, the value set in ink, the signal rule while open.
/// iced borders are uniform, so the web's bottom-only rule becomes a full hairline.
pub fn pick_list(
    _theme: &Theme,
    status: iced::widget::pick_list::Status,
) -> iced::widget::pick_list::Style {
    use iced::widget::pick_list::Status;
    let (edge, width) = match status {
        Status::Opened { .. } => (crate::theme::palette().signal, 2.0),
        Status::Active | Status::Hovered => (crate::theme::palette().ink, 1.0),
    };
    iced::widget::pick_list::Style {
        text_color: crate::theme::palette().ink,
        placeholder_color: crate::theme::palette().ink_3,
        handle_color: crate::theme::palette().ink_2,
        background: Background::Color(Color::TRANSPARENT),
        border: Border::default().width(width).color(edge),
    }
}

/// One full set of the register's colours. [`Palette::dark`] is the night console (the constants above);
/// [`Palette::light`] maps the same tokens to the web's light register (`tokens.css`, `:root`). The
/// tokens the web does not carry (`on_signal`, `taint`, `substrate`) get light values with the same role.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub paper: Color,
    pub sheet: Color,
    pub rule: Color,
    pub rule_faint: Color,
    pub margin: Color,
    pub ink: Color,
    pub ink_2: Color,
    pub ink_3: Color,
    pub signal: Color,
    pub signal_ink: Color,
    pub on_signal: Color,
    pub info: Color,
    pub crit: Color,
    pub warn: Color,
    pub ok: Color,
    pub taint: Color,
    pub hover: Color,
    pub substrate: Color,
}

impl Palette {
    /// The night console: exactly the constants above.
    pub const fn dark() -> Self {
        Self {
            paper: PAPER,
            sheet: SHEET,
            rule: RULE,
            rule_faint: RULE_FAINT,
            margin: MARGIN,
            ink: INK,
            ink_2: INK_2,
            ink_3: INK_3,
            signal: SIGNAL,
            signal_ink: SIGNAL_INK,
            on_signal: ON_SIGNAL,
            info: INFO,
            crit: CRIT,
            warn: WARN,
            ok: OK,
            taint: TAINT,
            hover: HOVER,
            substrate: SUBSTRATE,
        }
    }

    /// The web's light register (`apps/web/src/styles/tokens.css`, `:root`).
    pub const fn light() -> Self {
        Self {
            paper: Color::from_rgb8(0xe9, 0xec, 0xe5),
            sheet: Color::from_rgb8(0xf4, 0xf6, 0xf1),
            rule: Color::from_rgb8(0xc7, 0xcf, 0xc3),
            rule_faint: Color::from_rgb8(0xdb, 0xe1, 0xd7),
            margin: Color::from_rgb8(0xd0, 0x7a, 0x72),
            ink: Color::from_rgb8(0x15, 0x19, 0x16),
            ink_2: Color::from_rgb8(0x4c, 0x54, 0x4d),
            ink_3: Color::from_rgb8(0x7d, 0x86, 0x7e),
            signal: Color::from_rgb8(0xe8, 0x59, 0x0c),
            signal_ink: Color::from_rgb8(0xa6, 0x3c, 0x00),
            on_signal: Color::from_rgb8(0x12, 0x08, 0x00),
            info: Color::from_rgb8(0x2d, 0x5f, 0x8a),
            crit: Color::from_rgb8(0xa3, 0x18, 0x2b),
            warn: Color::from_rgb8(0x85, 0x64, 0x0e),
            ok: Color::from_rgb8(0x2e, 0x6a, 0x40),
            taint: Color::from_rgb8(0xf5, 0xee, 0xd2),
            hover: Color::from_rgb8(0xe6, 0xea, 0xe2),
            substrate: Color::from_rgb8(0xec, 0xee, 0xe7),
        }
    }
}

/// The palette on screen: light only when the owner chose it, or the OS is light and nothing is chosen.
static LIGHT: AtomicBool = AtomicBool::new(false);
/// Whether the owner has chosen a theme; once chosen, the OS appearance no longer decides.
static CHOSEN: AtomicBool = AtomicBool::new(false);

/// The palette in use now.
pub fn palette() -> Palette {
    if is_light() {
        Palette::light()
    } else {
        Palette::dark()
    }
}

/// Whether the light palette is in use.
pub fn is_light() -> bool {
    LIGHT.load(Ordering::Relaxed)
}

/// The owner's choice, from the switch in the top bar. It is kept for the rest of the run.
pub fn choose(light: bool) {
    LIGHT.store(light, Ordering::Relaxed);
    CHOSEN.store(true, Ordering::Relaxed);
}

/// The saved choice at start (`"light"` or `"dark"` in `desktop.json`); nothing when none is saved.
pub fn choose_saved(value: Option<&str>) {
    match value {
        Some("light") => choose(true),
        Some("dark") => choose(false),
        _ => {}
    }
}

/// The OS appearance: it decides only until the owner chooses. An unknown mode stays dark.
pub fn follow_system(mode: iced::theme::Mode) {
    if !CHOSEN.load(Ordering::Relaxed) {
        LIGHT.store(matches!(mode, iced::theme::Mode::Light), Ordering::Relaxed);
    }
}

/// The machines' style as an index: 0 Ink (the default), 1 Blueprint, 2 Night.
static MACHINE_STYLE: AtomicU8 = AtomicU8::new(0);

/// The style the machines are drawn in: the grid, the cards and a machine's own view.
pub fn machine_style() -> Style {
    match MACHINE_STYLE.load(Ordering::Relaxed) {
        1 => Style::Blueprint,
        2 => Style::Night,
        _ => Style::Ink,
    }
}

/// Sets the machines' style. The owner picks it on the Machines screen; it is kept for the run and saved.
pub fn set_machine_style(style: Style) {
    let index = match style {
        Style::Ink => 0,
        Style::Blueprint => 1,
        Style::Night => 2,
    };
    MACHINE_STYLE.store(index, Ordering::Relaxed);
}

/// The style a saved name (`desktop.json`, key `machine_style`) names. Ink when none is saved or the name is unknown.
pub fn saved_machine_style(value: Option<&str>) -> Style {
    value.and_then(Style::parse).unwrap_or(Style::Ink)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_style_round_trips_through_set_and_get() {
        for style in [Style::Ink, Style::Blueprint, Style::Night] {
            set_machine_style(style);
            assert_eq!(machine_style(), style);
        }
        set_machine_style(Style::Ink);
    }

    #[test]
    fn saved_machine_style_falls_back_to_ink() {
        assert_eq!(saved_machine_style(Some("night")), Style::Night);
        assert_eq!(saved_machine_style(Some("Blueprint")), Style::Blueprint);
        assert_eq!(saved_machine_style(Some("neon")), Style::Ink);
        assert_eq!(saved_machine_style(None), Style::Ink);
    }
}
