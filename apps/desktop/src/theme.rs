//! The register's visual language: the "Plaza" palette from
//! `docs/ux-draft/owner-console.html`, a mono/serif pairing, ruled sheets and
//! stamps — as far as iced's widgets carry it (no rotation, no dot leaders:
//! the rules and the space between them stand in).

use iced::widget::{button, container, rule, text_input};
use iced::{Background, Border, Color, Font, Shadow, Theme, Vector, theme::Palette};

/// The page behind everything.
pub const PAPER: Color = Color::from_rgb8(0xe9, 0xec, 0xe5);
/// A sheet laid on the page.
pub const SHEET: Color = Color::from_rgb8(0xf4, 0xf6, 0xf1);
/// A ruled line.
pub const RULE: Color = Color::from_rgb8(0xc7, 0xcf, 0xc3);
/// A fainter rule, for the space between fields.
pub const RULE_FAINT: Color = Color::from_rgb8(0xdb, 0xe1, 0xd7);
/// The red margin rule.
pub const MARGIN: Color = Color::from_rgb8(0xd0, 0x7a, 0x72);
/// The ink.
pub const INK: Color = Color::from_rgb8(0x15, 0x19, 0x16);
/// Secondary ink.
pub const INK_2: Color = Color::from_rgb8(0x4c, 0x54, 0x4d);
/// Tertiary ink, for labels.
pub const INK_3: Color = Color::from_rgb8(0x7d, 0x86, 0x7e);
/// The one hot signal.
pub const SIGNAL: Color = Color::from_rgb8(0xe8, 0x59, 0x0c);
/// A refusal, a kill, a failure.
pub const CRIT: Color = Color::from_rgb8(0xa3, 0x18, 0x2b);
/// A caution.
pub const WARN: Color = Color::from_rgb8(0x85, 0x64, 0x0e);
/// Something finished well.
pub const OK: Color = Color::from_rgb8(0x2e, 0x6a, 0x40);
/// A stamped, held state.
pub const TAINT: Color = Color::from_rgb8(0xef, 0xe2, 0xbd);
/// A hovered row.
pub const HOVER: Color = Color::from_rgb8(0xe6, 0xea, 0xe2);
/// The machine's substrate region on the fleet map: a hair off the sheet, so
/// the bounded region reads as bounded without becoming a filled block.
pub const SUBSTRATE: Color = Color::from_rgb8(0xee, 0xf1, 0xe9);

/// The monospace voice: ids, states, counts, serial.
pub const MONO: Font = Font::MONOSPACE;

/// The serif voice: prose and the sheet's lede. Falls back to the default face
/// on a host without DejaVu Serif.
pub const SERIF: Font = Font::with_name("DejaVu Serif");

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
            Tone::Neutral => INK_2,
            Tone::Ok => OK,
            Tone::Warn => WARN,
            Tone::Crit => CRIT,
            Tone::Signal => SIGNAL,
        }
    }
}

/// The application theme: the Plaza paper and ink, so default-styled widgets
/// (icons, selections, scroll rails) do not fall back to iced's dark palette.
pub fn theme() -> Theme {
    Theme::custom(
        "AgentPlaza register",
        Palette {
            background: PAPER,
            text: INK,
            primary: SIGNAL,
            success: OK,
            warning: WARN,
            danger: CRIT,
        },
    )
}

/// The page.
pub fn paper(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(INK),
        background: Some(PAPER.into()),
        ..container::Style::default()
    }
}

/// The index column, ruled off from the sheet.
pub fn index(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(INK_2),
        background: Some(PAPER.into()),
        border: Border::default().width(1.0).color(RULE),
        snap: true,
        ..container::Style::default()
    }
}

/// A sheet: a slightly brighter page with a hard print shadow.
pub fn sheet(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(INK),
        background: Some(SHEET.into()),
        border: Border::default().width(1.0).color(RULE),
        shadow: print_shadow(),
        snap: true,
    }
}

/// The red margin rule down the left of the sheet's content.
pub fn margin(_theme: &Theme) -> container::Style {
    container::Style {
        border: Border {
            color: MARGIN,
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
        border: Border::default().width(1.0).color(INK_3),
        snap: true,
        ..container::Style::default()
    }
}

/// A stamp: a boxed word, pressed in `tone`.
pub fn stamp(tone: Tone) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        text_color: Some(tone.color()),
        background: Some(SHEET.into()),
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
        text_color: Some(INK),
        background: Some(SHEET.into()),
        border: Border::default().width(1.0).color(RULE_FAINT),
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
        text_color: Some(INK),
        background: Some(
            if tone == Tone::Crit {
                Color::from_rgba8(0xa3, 0x18, 0x2b, 0.08)
            } else {
                TAINT
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
            (true, _) => Some(TAINT.into()),
            (false, button::Status::Hovered | button::Status::Pressed) => Some(HOVER.into()),
            (false, button::Status::Active | button::Status::Disabled) => None,
        };
        button::Style {
            background,
            text_color: INK,
            border: Border::default(),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// The primary action: pressed in ink on the sheet, with the print shadow.
pub fn primary_button(_theme: &Theme, status: button::Status) -> button::Style {
    button::Style {
        background: Some(
            match status {
                button::Status::Hovered | button::Status::Pressed => HOVER,
                button::Status::Active | button::Status::Disabled => SHEET,
            }
            .into(),
        ),
        text_color: INK,
        border: Border::default().width(1.0).color(INK),
        shadow: print_shadow(),
        snap: true,
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
                    button::Status::Active | button::Status::Disabled => SHEET,
                }
            }
            .into(),
        ),
        text_color: if armed { SHEET } else { tone.color() },
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
            Background::Color(HOVER)
        } else {
            Background::Color(SHEET)
        },
        border: Border::default().width(1.0).color(RULE),
        icon: INK_3,
        placeholder: INK_3,
        value: INK,
        selection: SIGNAL,
    }
}

/// The hard offset shadow the console prints boxed things with. iced drops the text of a box
/// whose shadow has exactly zero blur, so a hair of blur keeps it crisp and the text drawn.
fn print_shadow() -> Shadow {
    Shadow {
        color: INK,
        offset: Vector::new(2.0, 2.0),
        blur_radius: 0.6,
    }
}

/// A ruled line.
pub fn rule(_theme: &Theme) -> rule::Style {
    ruled(RULE)
}

/// A ruled line drawn in ink: under a heading, above a total.
pub fn rule_ink(_theme: &Theme) -> rule::Style {
    ruled(INK)
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
