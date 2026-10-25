//! The consoles' visual language on the desktop: the theme a console applies is resolved by `apz-theme` (the same
//! crate, files and rules the web console reads), so both consoles draw the same colours, radii, shadows and type.
//! A theme is a skin (Loom or Graphite: shape and type), a palette (every colour token for light and dark) and a
//! background. [`apply`] resolves the owner's choice for both modes; [`palette()`] and [`skin()`] are what the
//! style functions and screens read. Token names are the web's (`--ink-3` is `palette().ink_3`); a test fails when
//! the list in `apz_theme::tokens::TOKENS` and the [`Palette`] fields differ.

use apz_engine::style::Style;
use apz_theme::{Appearance, Mode, ModeChoice, Resolved, ThemeFile};
use iced::widget::text::LineHeight;
use iced::widget::{button, container, rule, text_editor, text_input};
use iced::{
    Background, Border, Color, Font, Shadow, Theme, Vector, font, theme::Palette as IcedPalette,
};
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

/// The interface and reading voice: Geist, embedded by the console (`console::run`) and the app's default font.
/// Its weights are the TTFs the console embeds (Regular, Medium, SemiBold, Bold, Italic); they share the family.
pub const SANS: Font = Font::with_name("Geist");
/// Geist at 500: controls and buttons.
pub const SANS_MEDIUM: Font = Font {
    weight: font::Weight::Medium,
    ..SANS
};
/// Geist at 600: titles, selected tabs and badges.
pub const SANS_SEMIBOLD: Font = Font {
    weight: font::Weight::Semibold,
    ..SANS
};
/// Geist at 700: figures and the empty-state titles.
pub const SANS_BOLD: Font = Font {
    weight: font::Weight::Bold,
    ..SANS
};
/// Geist in italic: the error line and the notes.
pub const SANS_ITALIC: Font = Font {
    style: font::Style::Italic,
    ..SANS
};

/// The code voice: code, diffs, terminals, raw tool output, the ids, hashes and paths where alignment matters, and
/// (in the Loom skin) section labels. Geist Mono, embedded by the console.
pub const MONO: Font = Font::with_name("Geist Mono");
/// Geist Mono at 500: Loom's section labels.
pub const MONO_MEDIUM: Font = Font {
    weight: font::Weight::Medium,
    ..MONO
};

/// The type roles (`docs/design/workspace.md`, section 1), in iced points (the web's CSS px, `styles/tokens.css`).
/// Page title: 22, line 1.25, 600.
pub const TYPE_PAGE: f32 = 22.0;
/// Section title (a card's head): 15, line 1.35, 600.
pub const TYPE_SECTION: f32 = 15.0;
/// Item title: 13.5, line 1.4, 600.
pub const TYPE_ITEM: f32 = 13.5;
/// Reading prose (transcripts, board posts, conversations): 14.5, line 1.6, 400.
pub const TYPE_PROSE: f32 = 14.5;
/// Interface text and controls: 13, line 1.4, 500 (600 when selected).
pub const TYPE_UI: f32 = 13.0;
/// Metadata, timestamps, helper text: 12, line 1.4, 400, secondary colour.
pub const TYPE_META: f32 = 12.0;
/// Badges and counts: 11, line 1.3, 600.
pub const TYPE_BADGE: f32 = 11.0;
/// Code: 12.5, line 1.5, Geist Mono.
pub const TYPE_CODE: f32 = 12.5;
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
    /// The tone's colour, for text and marks.
    pub fn color(self) -> Color {
        let p = palette();
        match self {
            Tone::Neutral => p.ink_2,
            Tone::Ok => p.ok,
            Tone::Warn => p.warn,
            Tone::Crit => p.crit,
            Tone::Signal => p.signal_ink,
        }
    }

    /// The tone's soft tint, behind chips and notices.
    pub fn soft(self) -> Color {
        let p = palette();
        match self {
            Tone::Neutral => p.sheet_2,
            Tone::Ok => p.ok_soft,
            Tone::Warn => p.warn_soft,
            Tone::Crit => p.crit_soft,
            Tone::Signal => p.signal_soft,
        }
    }

    /// A solid fill in the tone and the text that reads on it.
    pub fn fill(self) -> (Color, Color) {
        let p = palette();
        match self {
            Tone::Neutral => (p.action, p.on_action),
            Tone::Signal => (p.signal_fill, p.on_signal),
            other => (other.color(), p.sheet),
        }
    }
}

macro_rules! palette {
    ($($field:ident = $name:literal,)*) => {
        /// One full set of colours, resolved for one mode: a field per token of `apz_theme::tokens::TOKENS`.
        #[derive(Debug, Clone, Copy, PartialEq)]
        pub struct Palette {
            $(pub $field: Color,)*
        }

        impl Palette {
            /// The token names, in the list's order.
            pub const NAMES: &'static [&'static str] = &[$($name),*];

            /// The palette of resolved tokens.
            pub fn from_tokens(t: &apz_theme::Tokens) -> Palette {
                Palette { $($field: color(t.get($name)),)* }
            }

            /// The colour of a token by its name.
            pub fn get(&self, name: &str) -> Option<Color> {
                match name {
                    $($name => Some(self.$field),)*
                    _ => None,
                }
            }
        }
    };
}

palette! {
    paper = "paper",
    paper_side = "paper-side",
    panel = "panel",
    sheet = "sheet",
    raised = "raised",
    sheet_2 = "sheet-2",
    hover = "hover",
    press = "press",
    pattern = "pattern",
    amb_1 = "amb-1",
    amb_2 = "amb-2",
    amb_3 = "amb-3",
    rule = "rule",
    rule_strong = "rule-strong",
    rule_faint = "rule-faint",
    ink = "ink",
    ink_2 = "ink-2",
    ink_3 = "ink-3",
    signal = "signal",
    signal_fill = "signal-fill",
    signal_ink = "signal-ink",
    on_signal = "on-signal",
    signal_soft = "signal-soft",
    action = "action",
    on_action = "on-action",
    ok = "ok",
    ok_soft = "ok-soft",
    warn = "warn",
    warn_soft = "warn-soft",
    crit = "crit",
    crit_soft = "crit-soft",
    info = "info",
    info_soft = "info-soft",
    review = "review",
    review_soft = "review-soft",
    add_bg = "add-bg",
    add_fg = "add-fg",
    add_bar = "add-bar",
    del_bg = "del-bg",
    del_fg = "del-fg",
    del_bar = "del-bar",
    syn_keyword = "syn-keyword",
    syn_string = "syn-string",
    syn_type = "syn-type",
    syn_number = "syn-number",
    syn_comment = "syn-comment",
    syn_function = "syn-function",
    syn_variable = "syn-variable",
    term_bg = "term-bg",
    term_fg = "term-fg",
    ansi_0 = "ansi-0",
    ansi_1 = "ansi-1",
    ansi_2 = "ansi-2",
    ansi_3 = "ansi-3",
    ansi_4 = "ansi-4",
    ansi_5 = "ansi-5",
    ansi_6 = "ansi-6",
    ansi_7 = "ansi-7",
    ansi_8 = "ansi-8",
    ansi_9 = "ansi-9",
    ansi_10 = "ansi-10",
    ansi_11 = "ansi-11",
    ansi_12 = "ansi-12",
    ansi_13 = "ansi-13",
    ansi_14 = "ansi-14",
    ansi_15 = "ansi-15",
    chart_1 = "chart-1",
    chart_2 = "chart-2",
    chart_3 = "chart-3",
    chart_4 = "chart-4",
    chart_5 = "chart-5",
    chart_6 = "chart-6",
    chart_7 = "chart-7",
    chart_8 = "chart-8",
    chart_other = "chart-other",
    chart_grid = "chart-grid",
    chart_axis = "chart-axis",
    hue_0 = "hue-0",
    hue_1 = "hue-1",
    hue_2 = "hue-2",
    hue_3 = "hue-3",
    hue_4 = "hue-4",
    hue_5 = "hue-5",
    on_hue = "on-hue",
    toast_bg = "toast-bg",
    toast_fg = "toast-fg",
    scrim = "scrim",
    amb_lit = "amb-lit",
    amb_line = "amb-line",
}

/// An `apz_theme` colour as iced's.
pub fn color(c: apz_theme::Rgba) -> Color {
    Color {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}

/// The skin's shape and type, as the style functions read it.
#[derive(Debug, Clone, PartialEq)]
pub struct Skin {
    pub id: String,
    pub radius_sm: f32,
    pub radius: f32,
    pub radius_lg: f32,
    /// A list row's height, after density.
    pub row: f32,
    /// Section labels in mono capitals (Loom) or sans (Graphite).
    pub label_mono: bool,
    pub label_upper: bool,
    pub label_size: f32,
    /// Primary actions in ink (Loom) or in the signal colour (Graphite).
    pub action_ink: bool,
    pub shadow_card: Shadow,
    pub shadow_pop: Shadow,
}

/// The first layer of a skin's shadow list as iced's (iced draws one).
fn shadow(layers: &[apz_theme::skin::Shadow]) -> Shadow {
    match layers.last() {
        Some(s) => Shadow {
            color: Color {
                a: s.alpha,
                ..Color::BLACK
            },
            offset: Vector::new(0.0, s.y),
            blur_radius: s.blur,
        },
        None => Shadow::default(),
    }
}

impl Skin {
    fn from(r: &Resolved, density: f32) -> Skin {
        let s = &r.skin;
        let sh = if r.mode == Mode::Light {
            &s.light
        } else {
            &s.dark
        };
        Skin {
            id: s.id.clone(),
            radius_sm: (s.radius - 2.0).max(0.0),
            radius: s.radius,
            radius_lg: s.radius_lg,
            row: s.row * density,
            label_mono: s.label.mono,
            label_upper: s.label.upper,
            label_size: s.label.size,
            action_ink: s.action == apz_theme::skin::ActionFrom::Ink,
            shadow_card: shadow(&sh.card),
            shadow_pop: shadow(&sh.pop),
        }
    }

    /// The face section labels are set in.
    pub fn label_font(&self) -> Font {
        if self.label_mono {
            MONO_MEDIUM
        } else {
            SANS_MEDIUM
        }
    }

    /// A label as the skin sets it: capitals in Loom, as written in Graphite.
    pub fn label_text(&self, text: &str) -> String {
        if self.label_upper {
            text.to_uppercase()
        } else {
            text.to_string()
        }
    }
}

/// What is on screen: the owner's choice resolved for both modes.
#[derive(Debug, Clone)]
pub struct Look {
    pub appearance: Appearance,
    pub light: Resolved,
    pub dark: Resolved,
    pub light_palette: Palette,
    pub dark_palette: Palette,
    pub light_skin: Skin,
    pub dark_skin: Skin,
}

impl Look {
    fn new(appearance: &Appearance, saved: &[ThemeFile]) -> Look {
        let light = appearance.resolve(saved, true);
        let dark = appearance.resolve(saved, false);
        let density = appearance.density.factor();
        Look {
            appearance: appearance.clone(),
            light_palette: Palette::from_tokens(&light.tokens),
            dark_palette: Palette::from_tokens(&dark.tokens),
            light_skin: Skin::from(&light, density),
            dark_skin: Skin::from(&dark, density),
            light,
            dark,
        }
    }
}

/// The look in use. Each [`apply`] leaks one small `Look` so readers hold a `&'static` reference after a brief read
/// lock; the owner changes themes a handful of times per run.
static LOOK: RwLock<Option<&'static Look>> = RwLock::new(None);

fn look_ref() -> &'static Look {
    if let Some(l) = *LOOK.read().unwrap_or_else(|e| e.into_inner()) {
        return l;
    }
    let mut slot = LOOK.write().unwrap_or_else(|e| e.into_inner());
    slot.get_or_insert_with(|| Box::leak(Box::new(Look::new(&Appearance::default(), &[]))))
}

/// The owner's choice and saved themes: resolves both modes and puts them on screen. The mode follows the choice
/// (System leaves it to the OS through [`follow_system`]).
pub fn apply(appearance: &Appearance, saved: &[ThemeFile]) {
    let fresh: &'static Look = Box::leak(Box::new(Look::new(appearance, saved)));
    *LOOK.write().unwrap_or_else(|e| e.into_inner()) = Some(fresh);
    match appearance.mode {
        ModeChoice::Light => choose(true),
        ModeChoice::Dark => choose(false),
        ModeChoice::System => {
            LIGHT.store(SYSTEM_LIGHT.load(Ordering::Relaxed), Ordering::Relaxed);
            CHOSEN.store(false, Ordering::Relaxed);
        }
    }
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// The look in use.
pub fn look() -> &'static Look {
    look_ref()
}

/// The owner's appearance in use.
pub fn appearance() -> Appearance {
    look_ref().appearance.clone()
}

/// The resolved theme in use, for the current mode.
pub fn resolved() -> &'static Resolved {
    let l = look_ref();
    if is_light() { &l.light } else { &l.dark }
}

/// The palette in use now.
pub fn palette() -> &'static Palette {
    let l = look_ref();
    if is_light() {
        &l.light_palette
    } else {
        &l.dark_palette
    }
}

/// How many times the palette in use has been replaced: by [`apply`], by a light or dark switch, or by the OS
/// appearance. Canvas caches key on it, so they redraw after a change without hashing every colour.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// The skin in use now.
pub fn skin() -> &'static Skin {
    let l = look_ref();
    if is_light() {
        &l.light_skin
    } else {
        &l.dark_skin
    }
}

/// The application theme, so default-styled widgets (icons, selections, scroll rails) match.
pub fn theme() -> Theme {
    let p = palette();
    Theme::custom_with_fn(
        if is_light() {
            "AgentPlaza (light)"
        } else {
            "AgentPlaza (dark)"
        },
        IcedPalette {
            background: p.paper,
            text: p.ink,
            primary: p.signal,
            success: p.ok,
            warning: p.warn,
            danger: p.crit,
        },
        |base| {
            // A widget left on iced's default style (a pick list, a scroll rail) takes the kit's colours: the
            // web's `.select` is on raised with a rule-strong edge and ink-3 placeholders.
            use iced::theme::palette::{Extended, Pair};
            let mut x = Extended::generate(base);
            x.background.weak = Pair::new(p.raised, p.ink);
            x.background.strong = Pair::new(p.rule_strong, p.ink);
            x.secondary.base = Pair::new(p.ink_3, p.ink);
            x.primary.weak = Pair::new(p.signal_soft, p.ink);
            x
        },
    )
}

/// The page.
pub fn paper(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(palette().ink),
        background: (!crate::console::backdrop::drawn()).then(|| palette().paper.into()),
        ..container::Style::default()
    }
}

/// The side column (the sidebar and side panels), ruled off from the page.
pub fn index(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(palette().ink_2),
        background: Some(palette().paper_side.into()),
        border: Border::default().width(1.0).color(palette().rule),
        snap: true,
        ..container::Style::default()
    }
}

/// A sheet: a card laid on the page, with the skin's radius and card shadow.
pub fn sheet(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(palette().ink),
        background: Some(palette().sheet.into()),
        border: Border::default()
            .width(1.0)
            .color(palette().rule)
            .rounded(skin().radius_lg),
        shadow: skin().shadow_card,
        snap: true,
    }
}

/// A popover or dialog: the sheet lifted with the skin's popover shadow.
pub fn pop(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(palette().ink),
        background: Some(palette().sheet.into()),
        border: Border::default()
            .width(1.0)
            .color(palette().rule_strong)
            .rounded(skin().radius_lg),
        shadow: skin().shadow_pop,
        snap: true,
    }
}

/// An emphasised edge down the left of a block's content (a quoted note, the selected row's edge).
pub fn margin(_theme: &Theme) -> container::Style {
    container::Style {
        border: Border {
            color: palette().rule_strong,
            width: 2.0,
            ..Border::default()
        },
        snap: true,
        ..container::Style::default()
    }
}

/// A solid colour swatch: a legend's status colour, a small filled box.
pub fn swatch(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme: &Theme| container::Style {
        background: Some(Background::Color(color)),
        border: Border::default().rounded(2.0),
        snap: true,
        ..container::Style::default()
    }
}

/// A stamp: a chip in `tone` — its soft tint behind the tone's text.
pub fn stamp(tone: Tone) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        text_color: Some(tone.color()),
        background: Some(tone.soft().into()),
        border: Border::default().rounded(skin().radius_sm),
        snap: true,
        ..container::Style::default()
    }
}

/// A plain bounded box: the serial pane, a key/value block.
pub fn boxed(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(palette().ink),
        background: Some(palette().sheet_2.into()),
        border: Border::default()
            .width(1.0)
            .color(palette().rule_faint)
            .rounded(skin().radius),
        snap: true,
        ..container::Style::default()
    }
}

/// The veil behind a dialog: the console dimmed under the sheet that is over it.
pub fn scrim(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(palette().scrim.into()),
        snap: true,
        ..container::Style::default()
    }
}

/// The notice line: the last action's outcome, on the tone's soft tint.
pub fn notice(tone: Tone) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        text_color: Some(palette().ink),
        background: Some(tone.soft().into()),
        border: Border {
            color: Color {
                a: 0.45,
                ..tone.color()
            },
            width: 1.0,
            radius: skin().radius.into(),
        },
        snap: true,
        ..container::Style::default()
    }
}

/// A list row: an entry in the side list, or a run in a table. `selected` presses the row; hover lifts it.
pub fn register_row(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let background = match (selected, status) {
            (true, _) => Some(palette().press.into()),
            (false, button::Status::Hovered | button::Status::Pressed) => {
                Some(palette().hover.into())
            }
            (false, button::Status::Active | button::Status::Disabled) => None,
        };
        button::Style {
            background,
            text_color: palette().ink,
            border: Border::default().rounded(skin().radius_sm),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// The view's one primary action: the skin's action colour (ink in Loom, the signal in Graphite).
pub fn primary_button(_theme: &Theme, status: button::Status) -> button::Style {
    let p = palette();
    let fill = match status {
        button::Status::Hovered | button::Status::Pressed => Color {
            a: 0.88,
            ..p.action
        },
        button::Status::Disabled => Color { a: 0.5, ..p.action },
        button::Status::Active => p.action,
    };
    button::Style {
        background: Some(fill.into()),
        text_color: p.on_action,
        border: Border::default()
            .width(1.0)
            .color(fill)
            .rounded(skin().radius),
        shadow: Shadow::default(),
        snap: true,
    }
}

/// A secondary action: bordered on the sheet.
pub fn secondary_button(_theme: &Theme, status: button::Status) -> button::Style {
    let p = palette();
    let fill = match status {
        button::Status::Hovered | button::Status::Pressed => p.hover,
        _ => p.sheet,
    };
    button::Style {
        background: Some(fill.into()),
        text_color: if status == button::Status::Disabled {
            p.ink_3
        } else {
            p.ink
        },
        border: Border::default()
            .width(1.0)
            .color(p.rule_strong)
            .rounded(skin().radius),
        shadow: Shadow::default(),
        snap: true,
    }
}

/// A committing press in `tone`: it has to look like one, at every step.
///
/// `armed` is the second press's own look — the tone filled — while an unarmed press wears the tone's text on its
/// soft tint. It is the one body behind every arm-then-confirm press: a run's kill, a task's withdraw, an agent's
/// stop, and the owner's ratify and veto.
pub fn act_button(tone: Tone, armed: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let (fill, on) = tone.fill();
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: Some(
                if armed {
                    fill
                } else if hovered {
                    tone.soft()
                } else {
                    palette().sheet
                }
                .into(),
            ),
            text_color: if armed { on } else { tone.color() },
            border: Border::default()
                .width(1.0)
                .color(if armed {
                    fill
                } else {
                    Color {
                        a: 0.55,
                        ..tone.color()
                    }
                })
                .rounded(skin().radius),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// A destructive action: [`act_button`] in the crit tone.
pub fn kill_button(armed: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    act_button(Tone::Crit, armed)
}

/// The corners of the web's `.input`, `.select` and `.textarea` (8px in both skins).
pub const INPUT_RADIUS: f32 = 8.0;

/// A form field as the web's `.input`: on raised, with the signal edge while focused.
pub fn input(_theme: &Theme, status: text_input::Status) -> text_input::Style {
    let p = palette();
    let focused = matches!(status, text_input::Status::Focused { .. });
    text_input::Style {
        background: Background::Color(p.raised),
        border: Border::default()
            .width(1.0)
            .color(if focused {
                p.signal
            } else if status == text_input::Status::Hovered {
                p.ink_3
            } else {
                p.rule_strong
            })
            .rounded(INPUT_RADIUS),
        icon: p.ink_3,
        placeholder: p.ink_3,
        value: p.ink,
        selection: p.signal_soft,
    }
}

/// A multi-line field as the web's `.textarea`: the same ground and edge as `input`.
pub fn editor(_theme: &Theme, status: text_editor::Status) -> text_editor::Style {
    let p = palette();
    let focused = matches!(status, text_editor::Status::Focused { .. });
    let hovered = matches!(status, text_editor::Status::Hovered);
    text_editor::Style {
        background: Background::Color(p.raised),
        border: Border::default()
            .width(1.0)
            .color(if focused {
                p.signal
            } else if hovered {
                p.ink_3
            } else {
                p.rule_strong
            })
            .rounded(INPUT_RADIUS),
        placeholder: p.ink_3,
        value: p.ink,
        selection: p.signal_soft,
    }
}

/// A ruled line.
pub fn rule(_theme: &Theme) -> rule::Style {
    ruled(palette().rule)
}

/// A stronger line: under a heading, above a total.
pub fn rule_ink(_theme: &Theme) -> rule::Style {
    ruled(palette().rule_strong)
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

/// The pick list as the web's select: on the sheet, bordered, the signal ring while open.
pub fn pick_list(
    _theme: &Theme,
    status: iced::widget::pick_list::Status,
) -> iced::widget::pick_list::Style {
    use iced::widget::pick_list::Status;
    let p = palette();
    let edge = match status {
        Status::Opened { .. } => p.signal,
        Status::Hovered => p.ink_3,
        Status::Active => p.rule_strong,
    };
    iced::widget::pick_list::Style {
        text_color: p.ink,
        placeholder_color: p.ink_3,
        handle_color: p.ink_2,
        background: Background::Color(p.raised),
        border: Border::default()
            .width(1.0)
            .color(edge)
            .rounded(INPUT_RADIUS),
    }
}

/// The palette on screen: light only when the owner chose it, or the OS is light and nothing is chosen.
static LIGHT: AtomicBool = AtomicBool::new(false);
/// Whether the owner has chosen a mode; once chosen, the OS appearance no longer decides.
static CHOSEN: AtomicBool = AtomicBool::new(false);
/// The OS appearance as the shell last reported it (dark until it does). `apply(System)` falls back to it.
static SYSTEM_LIGHT: AtomicBool = AtomicBool::new(false);
/// Counts the palette replacements, read through [`generation`].
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Whether the light palette is in use.
pub fn is_light() -> bool {
    LIGHT.load(Ordering::Relaxed)
}

/// The owner's choice of light or dark, from the switch in the top bar. It is kept for the rest of the run.
pub fn choose(light: bool) {
    LIGHT.store(light, Ordering::Relaxed);
    CHOSEN.store(true, Ordering::Relaxed);
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// The mode saved by an older build (`"light"` or `"dark"` in `desktop.json`, key `theme`); nothing when none is saved.
pub fn choose_saved(value: Option<&str>) {
    match value {
        Some("light") => choose(true),
        Some("dark") => choose(false),
        _ => {}
    }
}

/// The OS appearance: it decides only until the owner chooses. An unknown mode stays dark.
pub fn follow_system(mode: iced::theme::Mode) {
    let light = matches!(mode, iced::theme::Mode::Light);
    SYSTEM_LIGHT.store(light, Ordering::Relaxed);
    if !CHOSEN.load(Ordering::Relaxed) {
        LIGHT.store(light, Ordering::Relaxed);
        GENERATION.fetch_add(1, Ordering::Relaxed);
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

    /// The palette has exactly the theme crate's tokens: none missing, none extra.
    #[test]
    fn palette_fields_are_the_token_list() {
        let list: Vec<&str> = apz_theme::tokens::TOKENS.iter().map(|t| t.name).collect();
        assert_eq!(Palette::NAMES, list.as_slice());
    }

    /// Applying a built-in resolves the same colours the web's generated CSS carries.
    #[test]
    fn apply_resolves_the_builtin_tokens() {
        let a = Appearance {
            theme: "graphite-cobalt".into(),
            mode: ModeChoice::Light,
            ..Appearance::default()
        };
        apply(&a, &[]);
        let want = apz_theme::resolve(
            &apz_theme::builtin::builtin("graphite-cobalt").unwrap(),
            &apz_theme::skin::skin("graphite").unwrap(),
            Mode::Light,
        );
        assert!(is_light());
        assert_eq!(palette().paper, color(want.tokens.get("paper")));
        assert_eq!(palette().action, color(want.tokens.get("signal-fill")));
        assert_eq!(skin().id, "graphite");
        apply(&Appearance::default(), &[]);
    }

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
