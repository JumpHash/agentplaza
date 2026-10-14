//! The IDE's own window, around its panels: the header (the mark and title, the workspace picker, the
//! state badge, the actions, the theme switch and the way back to the console), the held strip under
//! it, the activity rail on the left, the grab strips between the panels, and the status bar. The
//! panels' contents are `ide.rs`'s; this module draws the frame they sit in and keeps the layout they
//! are drawn with (the panel sizes and which panels are folded), which the desktop settings remember.

use apz_ide::state::Base;
use iced::widget::{Space, button, column, container, mouse_area, row, tooltip};
use iced::{Alignment, Border, Element, Length, Theme, mouse};
use serde_json::{Value, json};

use super::ide::{self, Msg, Plaza, Side, State};
use super::ide_editor;
use crate::console::boot;
use crate::console::shell;
use crate::console::ui::{self, Kind};
use crate::theme;

/// The activity rail's width: icons only.
pub const RAIL_W: f32 = 48.0;
/// The grab strip on a panel's edge. Its line is one pixel of it.
pub const HANDLE_W: f32 = 6.0;
/// The setting that keeps the layout between runs.
const KEY: &str = "ide_layout";

/// A panel's width range and its default.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Bounds {
    default: f32,
    min: f32,
    max: f32,
}

impl Bounds {
    fn clamp(self, width: f32) -> f32 {
        width.clamp(self.min, self.max)
    }
}

const LEFT: Bounds = Bounds {
    default: 264.0,
    min: 200.0,
    max: 420.0,
};
const RIGHT: Bounds = Bounds {
    default: 340.0,
    min: 280.0,
    max: 480.0,
};

/// One of the two side panels, by the edge its grab strip sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
}

impl Edge {
    fn bounds(self) -> Bounds {
        match self {
            Edge::Left => LEFT,
            Edge::Right => RIGHT,
        }
    }
}

/// The side panels' sizes and folds. Sizes are kept within their ranges whatever sets them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub left: f32,
    pub right: f32,
    pub left_open: bool,
    pub right_open: bool,
}

impl Layout {
    /// Both panels open at their default widths.
    pub fn fresh() -> Layout {
        Layout {
            left: LEFT.default,
            right: RIGHT.default,
            left_open: true,
            right_open: true,
        }
    }

    pub fn width(&self, edge: Edge) -> f32 {
        match edge {
            Edge::Left => self.left,
            Edge::Right => self.right,
        }
    }

    pub fn is_open(&self, edge: Edge) -> bool {
        match edge {
            Edge::Left => self.left_open,
            Edge::Right => self.right_open,
        }
    }

    pub fn set_width(&mut self, edge: Edge, width: f32) {
        let width = edge.bounds().clamp(width);
        match edge {
            Edge::Left => self.left = width,
            Edge::Right => self.right = width,
        }
    }

    pub fn reset_width(&mut self, edge: Edge) {
        self.set_width(edge, edge.bounds().default);
    }

    pub fn toggle(&mut self, edge: Edge) {
        match edge {
            Edge::Left => self.left_open = !self.left_open,
            Edge::Right => self.right_open = !self.right_open,
        }
    }

    fn to_json(self) -> Value {
        json!({ "left": self.left, "right": self.right, "left_open": self.left_open, "right_open": self.right_open })
    }

    /// The layout a saved value names. Missing fields keep their defaults; widths are clamped.
    fn from_json(value: &Value) -> Option<Layout> {
        value.as_object()?;
        let mut layout = Layout::fresh();
        if let Some(width) = value.get("left").and_then(Value::as_f64) {
            layout.set_width(Edge::Left, width as f32);
        }
        if let Some(width) = value.get("right").and_then(Value::as_f64) {
            layout.set_width(Edge::Right, width as f32);
        }
        layout.left_open = value.get("left_open").and_then(Value::as_bool).unwrap_or(true);
        layout.right_open = value.get("right_open").and_then(Value::as_bool).unwrap_or(true);
        Some(layout)
    }
}

impl Default for Layout {
    /// The layout the last run left, or the defaults. Tests never read the owner's settings.
    fn default() -> Layout {
        if cfg!(test) {
            return Layout::fresh();
        }
        boot::saved_setting(KEY).as_ref().and_then(Layout::from_json).unwrap_or_else(Layout::fresh)
    }
}

/// Keeps the layout for the next run (not from tests).
pub fn save(layout: &Layout) {
    if cfg!(test) {
        return;
    }
    if let Err(err) = boot::save_setting(KEY, layout.to_json()) {
        eprintln!("agentplaza-desktop: the IDE layout was not saved: {err}");
    }
}

/// A drag of one panel's edge: the width when it began, and the pointer's window x where it was first
/// measured (the press itself carries no position, so the first pointer move sets it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drag {
    pub edge: Edge,
    pub start: f32,
    pub origin: Option<f32>,
}

impl Drag {
    pub fn begin(layout: &Layout, edge: Edge) -> Drag {
        Drag {
            edge,
            start: layout.width(edge),
            origin: None,
        }
    }
}

/// Applies the pointer at window `x` to a drag. A left panel grows as the pointer moves right, a right
/// panel as it moves left; the result is clamped by the layout.
pub fn drag_to(layout: &mut Layout, drag: &mut Drag, x: f32) {
    let origin = *drag.origin.get_or_insert(x);
    let delta = x - origin;
    let width = match drag.edge {
        Edge::Left => drag.start + delta,
        Edge::Right => drag.start - delta,
    };
    layout.set_width(drag.edge, width);
}

// ---- Words -------------------------------------------------------------------------------------

/// The base the Changes panel compares with, as the status bar says it.
pub fn base_label(base: &Base) -> String {
    match base {
        Base::WorkStart => "since work start".to_string(),
        Base::Checkpoint(n) => format!("checkpoint #{n}"),
        Base::Branch(name) => format!("branch {name}"),
    }
}

/// The language a file is in, by its syntax token, as the status bar names it.
pub fn language_of(path: &str) -> &'static str {
    match ide_editor::token_for(path).as_str() {
        "rs" => "Rust",
        "ts" | "tsx" | "mts" | "cts" => "TypeScript",
        "js" | "jsx" | "mjs" | "cjs" => "JavaScript",
        "py" => "Python",
        "go" => "Go",
        "java" => "Java",
        "c" | "h" => "C",
        "cc" | "cpp" | "cxx" | "hpp" => "C++",
        "md" | "markdown" => "Markdown",
        "json" => "JSON",
        "toml" => "TOML",
        "yml" | "yaml" => "YAML",
        "html" | "htm" => "HTML",
        "css" => "CSS",
        "sh" | "bash" | "zsh" => "Shell",
        "sql" => "SQL",
        "xml" => "XML",
        "wgsl" => "WGSL",
        "makefile" => "Makefile",
        _ => "Plain text",
    }
}

// ---- Views -------------------------------------------------------------------------------------

/// The panel's sheet: the sheet colour, no border (the grab strips draw the rules).
pub fn panel_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(theme::palette().sheet.into()),
        ..Default::default()
    }
}

/// The header: the fold toggles, the mark and the title, the picker, the state badge; then the actions,
/// the theme switch, the right panel's fold and the way back to the console.
pub fn header(st: &State, plaza: Plaza) -> Element<'_, Msg> {
    let p = theme::palette();
    let on_workspace = st.ide.workspace.is_some();
    let (state_label, tone) = ide::state_badge(st, plaza.held);
    let mut start = row![
        ui::btn("", Some("panel-left"), Kind::Ghost, Some(Msg::ToggleLeft)),
        ui::hive(20.0),
        ui::mono("IDE", 13.0, p.ink),
        ide::picker(st),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    if let Some(changes) = &st.changes {
        start = start.push(ui::mono(format!("{} changed", changes.files.len()), 11.5, p.ink_2));
    }
    start = start.push(ui::badge(state_label, tone));

    let dirty = st.ide.active.as_ref().and_then(|path| st.tabs.get(path)).is_some_and(|tab| tab.buffer.dirty);
    let review_kind = if st.ide.review { Kind::Primary } else { Kind::Secondary };
    let save_kind = if dirty { Kind::Primary } else { Kind::Secondary };
    let (theme_label, theme_icon) = if theme::is_light() { ("Dark theme", "moon") } else { ("Light theme", "sun") };
    // Commit and Open PR are the hooks the git flow plugs into; until it does, they are drawn and not pressed.
    let end = row![
        ui::btn("Review", None, review_kind, (on_workspace && ide::serves_changes(st.ide.workspace.as_ref())).then_some(Msg::ToggleReview)),
        ui::btn("Save", Some("check"), save_kind, (dirty && ide::writable(st)).then_some(Msg::Save)),
        ui::btn("Commit", Some("commit"), Kind::Secondary, None),
        ui::btn("Open PR", Some("pull"), Kind::Secondary, None),
        ui::btn(theme_label, Some(theme_icon), Kind::Ghost, Some(Msg::Shell(shell::Msg::ToggleTheme))),
        ui::btn("", Some("panel-open"), Kind::Ghost, Some(Msg::ToggleRight)),
        ui::btn("Console", Some("overview"), Kind::Secondary, Some(Msg::Back)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    container(row![start, Space::new().width(Length::Fill), end].spacing(16).align_y(Alignment::Center))
        .padding([10, 16])
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            border: Border {
                color: theme::palette().rule,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into()
}

/// The strip under the header while the plaza is held (STOP ALL): what is held, and Resume, which asks
/// for the shell's confirmation.
pub fn held_strip<'a>() -> Element<'a, Msg> {
    let p = theme::palette();
    container(
        row![
            ui::icon("alert", 14.0, p.crit),
            ui::mono("The plaza is held: nothing new starts until you resume.", 12.0, p.crit).width(Length::Fill),
            ui::btn("Resume", Some("play"), Kind::Primary, Some(Msg::Shell(shell::Msg::Resume))),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .padding([6, 16])
    .width(Length::Fill)
    .style(theme::notice(theme::Tone::Crit))
    .into()
}

/// The activity rail: Files, Search, Changes and Checkpoints. The one open shows its panel; a press
/// on the open one folds the panel away.
pub fn rail(st: &State) -> Element<'_, Msg> {
    let items = [(Side::Files, "Files", "file"), (Side::Search, "Search", "search"), (Side::Changes, "Changes", "git"), (Side::Checkpoints, "Checkpoints", "clock")];
    let mut col = column![].spacing(4);
    for (side, label, icon) in items.into_iter().filter(|(side, _, _)| ide::side_offered(*side, st.ide.workspace.as_ref())) {
        let active = st.layout.left_open && st.side == side;
        col = col.push(rail_item(label, icon, active, Msg::Side(side)));
    }
    container(col)
        .padding([8, 0])
        .width(Length::Fixed(RAIL_W))
        .height(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(theme::palette().sheet.into()),
            border: Border {
                color: theme::palette().rule,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into()
}

/// One rail item: its icon, a signal bar when it is the open panel, and its name as a tooltip.
fn rail_item(label: &'static str, icon_name: &'static str, active: bool, msg: Msg) -> Element<'static, Msg> {
    let p = theme::palette();
    let glyph = if active { p.signal } else { p.ink_3 };
    let bar = container(Space::new().width(Length::Fixed(3.0)).height(Length::Fill))
        .height(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: if active { Some(p.signal.into()) } else { None },
            ..Default::default()
        });
    let glyph_box = container(ui::icon(icon_name, 18.0, glyph))
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center);
    let item = button(row![bar, glyph_box].height(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fixed(40.0))
        .padding(0)
        .style(move |_: &Theme, status: button::Status| {
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            let background = if active {
                Some(p.hover.into())
            } else if hovered {
                Some(p.rule_faint.into())
            } else {
                None
            };
            button::Style {
                background,
                text_color: p.ink,
                border: Border::default(),
                ..Default::default()
            }
        })
        .on_press(msg);
    tooltip(
        item,
        container(ui::mono(label, 11.0, p.ink)).padding([4, 8]).style(|_: &Theme| container::Style {
            background: Some(theme::palette().sheet.into()),
            border: Border {
                color: theme::palette().ink,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        }),
        tooltip::Position::Right,
    )
    .into()
}

/// The grab strip on a panel's edge: a resize cursor, a press starts the drag, a double press resets the
/// width. The drag itself is followed by the screen's subscription until the button comes up.
pub fn handle<'a>(edge: Edge) -> Element<'a, Msg> {
    let line = container(Space::new().width(Length::Fixed(1.0)).height(Length::Fill))
        .height(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(theme::palette().rule.into()),
            ..Default::default()
        });
    let strip = container(line).width(Length::Fixed(HANDLE_W)).height(Length::Fill).align_x(Alignment::Center);
    mouse_area(strip)
        .interaction(mouse::Interaction::ResizingHorizontally)
        .on_press(Msg::DragStart(edge))
        .on_release(Msg::DragEnd)
        .on_double_click(Msg::ResetWidth(edge))
        .into()
}

/// The status bar: the workspace and its base, the live link, then the active editor's cursor and
/// language, and the count of unsaved files.
pub fn status_bar(st: &State, live: bool) -> Element<'_, Msg> {
    let p = theme::palette();
    let workspace = st.ide.workspace.as_ref().map_or_else(|| "no workspace".to_string(), |id| id.to_string());
    let link_tone = if live { p.ok } else { p.crit };
    let link = row![
        container(Space::new().width(Length::Fixed(8.0)).height(Length::Fixed(8.0))).style(move |_: &Theme| container::Style {
            background: Some(link_tone.into()),
            ..Default::default()
        }),
        ui::mono(shell::live_word(live), 11.0, p.ink_2),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let mut bar = row![
        ui::mono(workspace, 11.0, p.ink_2),
        ui::mono(base_label(&st.ide.base), 11.0, p.ink_3),
        link,
        Space::new().width(Length::Fill),
    ]
    .spacing(18)
    .align_y(Alignment::Center);
    if let Some(path) = st.ide.active.as_deref()
        && let Some(tab) = st.tabs.get(path)
    {
        let at = tab.content.cursor().position;
        bar = bar.push(ui::mono(format!("Ln {}, Col {}", at.line + 1, at.column + 1), 11.0, p.ink_2));
        bar = bar.push(ui::mono(language_of(path), 11.0, p.ink_2));
    }
    let dirty = st.tabs.values().filter(|tab| tab.buffer.dirty).count();
    let (dirty_text, dirty_ink) = if dirty == 0 { ("all saved".to_string(), p.ink_3) } else { (format!("{dirty} unsaved"), p.signal) };
    bar = bar.push(ui::mono(dirty_text, 11.0, dirty_ink));
    container(bar)
        .padding([4, 12])
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(theme::palette().sheet.into()),
            border: Border {
                color: theme::palette().rule,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_layout_opens_both_panels_at_their_defaults() {
        let layout = Layout::fresh();
        assert_eq!((layout.left, layout.right), (264.0, 340.0));
        assert!(layout.is_open(Edge::Left) && layout.is_open(Edge::Right));
    }

    #[test]
    fn a_panel_width_is_clamped_to_its_range() {
        let mut layout = Layout::fresh();
        layout.set_width(Edge::Left, 10.0);
        assert_eq!(layout.left, 200.0, "the left panel's floor");
        layout.set_width(Edge::Left, 9000.0);
        assert_eq!(layout.left, 420.0, "the left panel's ceiling");
        layout.set_width(Edge::Right, 10.0);
        assert_eq!(layout.right, 280.0, "the right panel's floor");
        layout.set_width(Edge::Right, 9000.0);
        assert_eq!(layout.right, 480.0, "the right panel's ceiling");
        layout.reset_width(Edge::Right);
        assert_eq!(layout.right, 340.0);
    }

    #[test]
    fn a_panel_folds_and_unfolds_on_its_own() {
        let mut layout = Layout::fresh();
        layout.toggle(Edge::Left);
        assert!(!layout.is_open(Edge::Left));
        assert!(layout.is_open(Edge::Right), "the other panel stays as it was");
        layout.toggle(Edge::Left);
        assert!(layout.is_open(Edge::Left));
    }

    #[test]
    fn a_drag_grows_the_left_panel_to_the_right_and_the_right_one_to_the_left() {
        let mut layout = Layout::fresh();
        let mut drag = Drag::begin(&layout, Edge::Left);
        drag_to(&mut layout, &mut drag, 500.0);
        assert_eq!(layout.left, 264.0, "the first pointer position is where the drag is measured from");
        drag_to(&mut layout, &mut drag, 540.0);
        assert_eq!(layout.left, 304.0);

        let mut layout = Layout::fresh();
        let mut drag = Drag::begin(&layout, Edge::Right);
        drag_to(&mut layout, &mut drag, 900.0);
        drag_to(&mut layout, &mut drag, 870.0);
        assert_eq!(layout.right, 370.0, "moving left by 30 widens the right panel by 30");
        drag_to(&mut layout, &mut drag, 9000.0);
        assert_eq!(layout.right, 280.0, "a long drag stops at the floor");
    }

    #[test]
    fn the_layout_is_kept_as_json_and_read_back_clamped() {
        let mut layout = Layout::fresh();
        layout.set_width(Edge::Left, 300.0);
        layout.toggle(Edge::Right);
        let back = Layout::from_json(&layout.to_json()).expect("a saved layout reads back");
        assert_eq!(back, layout);
        let wild = json!({ "left": 5.0, "right": 5000.0, "left_open": false });
        let read = Layout::from_json(&wild).expect("an object reads");
        assert_eq!((read.left, read.right), (200.0, 480.0));
        assert!(!read.left_open && read.right_open, "a missing flag keeps its default");
        assert_eq!(Layout::from_json(&json!("not a layout")), None);
    }

    #[test]
    fn the_base_and_the_language_read_as_the_status_bar_says_them() {
        assert_eq!(base_label(&Base::WorkStart), "since work start");
        assert_eq!(base_label(&Base::Checkpoint(4)), "checkpoint #4");
        assert_eq!(language_of("src/main.rs"), "Rust");
        assert_eq!(language_of("Cargo.toml"), "TOML");
        assert_eq!(language_of("Makefile"), "Makefile");
        assert_eq!(language_of("notes"), "Plain text");
    }
}
