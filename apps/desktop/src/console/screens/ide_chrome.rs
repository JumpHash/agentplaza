//! The IDE's own window, around its panels: the header (the mark and title, the workspace picker, the
//! state badge, the actions, the theme switch and the way back to the console), the held strip under
//! it, the activity rail on the left, the grab strips between the panels, and the status bar. The
//! panels' contents are `ide.rs`'s; this module draws the frame they sit in and keeps the layout they
//! are drawn with (the panel sizes and which panels are folded), which the desktop settings remember.

use apz_ide::state::Base;
use iced::widget::{Space, container, mouse_area, row};
use iced::{Alignment, Border, Element, Length, Theme, mouse};
use serde_json::{Value, json};

use super::ide::{self, Msg, Plaza, State, has_diff};
use super::ide_editor;
use crate::console::boot;
use crate::console::shell;
use crate::console::ui::{self, Kind};
use crate::theme::{self, TYPE_BADGE, TYPE_META, TYPE_UI};
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
    default: 400.0,
    min: 340.0,
    max: 560.0,
};
/// The terminal docked under the editor: its height.
const BOTTOM: Bounds = Bounds {
    default: 260.0,
    min: 120.0,
    max: 560.0,
};

/// A panel's grab strip, by the edge it sits on: the two side panels', and the terminal's under the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Bottom,
}

impl Edge {
    fn bounds(self) -> Bounds {
        match self {
            Edge::Left => LEFT,
            Edge::Right => RIGHT,
            Edge::Bottom => BOTTOM,
        }
    }
}

/// The side panels' sizes and folds, and the terminal's dock and height. Sizes are kept within their ranges
/// whatever sets them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub left: f32,
    pub right: f32,
    pub left_open: bool,
    pub right_open: bool,
    /// The terminal drawer under the centre is open. It is closed at each start: the owner opens it with Ctrl+`
    /// or the Terminal view, and it stays as they left it for the rest of the run (it is not kept between runs).
    pub drawer: bool,
    pub terminal_height: f32,
}

impl Layout {
    /// Both panels open at their default widths; the terminal drawer closed.
    pub fn fresh() -> Layout {
        Layout {
            left: LEFT.default,
            right: RIGHT.default,
            left_open: true,
            right_open: true,
            drawer: false,
            terminal_height: BOTTOM.default,
        }
    }

    /// The panel's size along its edge's axis: a width, or the terminal's height on the bottom edge.
    pub fn width(&self, edge: Edge) -> f32 {
        match edge {
            Edge::Left => self.left,
            Edge::Right => self.right,
            Edge::Bottom => self.terminal_height,
        }
    }

    /// Whether the panel on the edge shows. The bottom edge's panel is the terminal, when it is docked there.
    pub fn is_open(&self, edge: Edge) -> bool {
        match edge {
            Edge::Left => self.left_open,
            Edge::Right => self.right_open,
            Edge::Bottom => self.drawer,
        }
    }

    pub fn set_width(&mut self, edge: Edge, width: f32) {
        let width = edge.bounds().clamp(width);
        match edge {
            Edge::Left => self.left = width,
            Edge::Right => self.right = width,
            Edge::Bottom => self.terminal_height = width,
        }
    }

    pub fn reset_width(&mut self, edge: Edge) {
        self.set_width(edge, edge.bounds().default);
    }

    /// Folds or unfolds a panel: a side panel, or the terminal drawer on the bottom edge.
    pub fn toggle(&mut self, edge: Edge) {
        match edge {
            Edge::Left => self.left_open = !self.left_open,
            Edge::Right => self.right_open = !self.right_open,
            Edge::Bottom => self.drawer = !self.drawer,
        }
    }

    fn to_json(self) -> Value {
        json!({
            "left": self.left,
            "right": self.right,
            "left_open": self.left_open,
            "right_open": self.right_open,
            "terminal_height": self.terminal_height,
        })
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
        if let Some(height) = value.get("terminal_height").and_then(Value::as_f64) {
            layout.set_width(Edge::Bottom, height as f32);
        }
        layout.left_open = value
            .get("left_open")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        layout.right_open = value
            .get("right_open")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        // The drawer is not kept between runs: it starts closed (see `Layout::drawer`).
        Some(layout)
    }
}

impl Default for Layout {
    /// The layout the last run left, or the defaults. Tests never read the owner's settings.
    fn default() -> Layout {
        if cfg!(test) {
            return Layout::fresh();
        }
        boot::saved_setting(KEY)
            .as_ref()
            .and_then(Layout::from_json)
            .unwrap_or_else(Layout::fresh)
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

/// Applies the pointer at window `(x, y)` to a drag. A left panel grows as the pointer moves right, a right
/// panel as it moves left, and the terminal as it moves up; the result is clamped by the layout.
pub fn drag_to(layout: &mut Layout, drag: &mut Drag, x: f32, y: f32) {
    let pointer = if drag.edge == Edge::Bottom { y } else { x };
    let origin = *drag.origin.get_or_insert(pointer);
    let delta = pointer - origin;
    let size = match drag.edge {
        Edge::Left => drag.start + delta,
        Edge::Right | Edge::Bottom => drag.start - delta,
    };
    layout.set_width(drag.edge, size);
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

/// A panel laid over the centre in a narrow window (a drawer on the left, a sheet on the right): the panel at its edge,
/// ruled off from the centre. The rest of the layer is clear, so the centre under it still takes its clicks.
pub fn overlay<'a>(panel: Element<'a, Msg>, edge: Edge) -> Element<'a, Msg> {
    let align = match edge {
        Edge::Right => Alignment::End,
        Edge::Left | Edge::Bottom => Alignment::Start,
    };
    container(
        container(panel)
            .height(Length::Fill)
            .style(|_: &Theme| container::Style {
                border: Border {
                    color: theme::palette().rule,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            }),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_x(align)
    .into()
}

/// The top bar: the mark, the breadcrumb (project / agent or repo / branch), the state badge and the changed count;
/// then the git actions (Save; Commit and Open PR, drawn until the git flow is wired), the command palette's key,
/// the right panel's fold, the theme switch and the way back to the console.
pub fn header(st: &State, plaza: Plaza) -> Element<'_, Msg> {
    let p = theme::palette();
    let on_workspace = st.ide.workspace.is_some();
    let (state_label, tone) = ide::state_badge(st, plaza.held);
    let mut start = row![
        ui::btn("", Some("panel-left"), Kind::Ghost, Some(Msg::ToggleLeft)),
        ui::hive(20.0)
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    // The chrome is Inter in sentence case; only the branch name is code, so it alone is Martian Mono.
    if on_workspace {
        let (project, name, branch) = ide::crumbs(st);
        start = start.push(ui::sans(project, TYPE_META, p.ink_2));
        start = start.push(ui::sans("/", TYPE_META, p.ink_3));
        start = start.push(ui::sans(name, TYPE_UI, p.ink));
        start = start.push(ui::sans("/", TYPE_META, p.ink_3));
        start = start.push(ui::mono(branch, TYPE_META, p.ink_2));
    } else {
        start = start.push(ui::sans("IDE", TYPE_UI, p.ink));
    }
    start = start.push(ui::badge(state_label, tone));
    if let Some(changes) = &st.changes {
        start = start.push(ui::sans(
            format!("{} changed", changes.files.len()),
            TYPE_BADGE,
            p.ink_2,
        ));
    }

    let dirty = st
        .ide
        .active
        .as_ref()
        .and_then(|path| st.tabs.get(path))
        .is_some_and(|tab| tab.buffer.dirty);
    let save_kind = if dirty {
        Kind::Primary
    } else {
        Kind::Secondary
    };
    let (theme_label, theme_icon) = if theme::is_light() {
        ("Dark theme", "moon")
    } else {
        ("Light theme", "sun")
    };
    // Commit and Open PR are the hooks the git flow plugs into; until it does, they are drawn and not pressed. In a
    // narrow window they leave the bar, which has no room for them beside the Save and the panel toggles.
    let narrow = st.width > 0.0 && st.width < ide::RIGHT_SHEET_BELOW;
    let commit: Element<'_, Msg> = if narrow {
        Space::new().width(Length::Fixed(0.0)).into()
    } else {
        ui::btn("Commit", Some("commit"), Kind::Secondary, None)
    };
    let open_pr: Element<'_, Msg> = if narrow {
        Space::new().width(Length::Fixed(0.0)).into()
    } else {
        ui::btn("Open PR", Some("pull"), Kind::Secondary, None)
    };
    let end = row![
        ui::btn(
            "Save",
            Some("check"),
            save_kind,
            (dirty && ide::writable(st)).then_some(Msg::Save)
        ),
        commit,
        open_pr,
        ui::btn(
            "Go to Ctrl+K",
            Some("search"),
            Kind::Ghost,
            Some(Msg::PaletteOpen)
        ),
        ui::btn("", Some("panel-open"), Kind::Ghost, Some(Msg::ToggleRight)),
        ui::btn(
            theme_label,
            Some(theme_icon),
            Kind::Ghost,
            Some(Msg::Shell(shell::Msg::ToggleTheme))
        ),
        ui::btn(
            "Console",
            Some("overview"),
            Kind::Secondary,
            Some(Msg::Back)
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    container(
        row![start, Space::new().width(Length::Fill), end]
            .spacing(16)
            .align_y(Alignment::Center),
    )
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
            ui::mono(
                "The plaza is held: nothing new starts until you resume.",
                12.0,
                p.crit
            )
            .width(Length::Fill),
            ui::btn(
                "Resume",
                Some("play"),
                Kind::Primary,
                Some(Msg::Shell(shell::Msg::Resume))
            ),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .padding([6, 16])
    .width(Length::Fill)
    .style(theme::notice(theme::Tone::Crit))
    .into()
}

/// The grab strip on a panel's edge: a resize cursor, a press starts the drag, a double press resets the
/// size. The side panels' strips run down their edge; the terminal's runs across the top of its dock.
/// The drag itself is followed by the screen's subscription until the button comes up.
pub fn handle<'a>(edge: Edge) -> Element<'a, Msg> {
    let (strip, cursor): (Element<'a, Msg>, mouse::Interaction) = match edge {
        Edge::Bottom => {
            let line = container(Space::new().width(Length::Fill).height(Length::Fixed(1.0)))
                .width(Length::Fill)
                .style(rule_line);
            let strip = container(line)
                .width(Length::Fill)
                .height(Length::Fixed(HANDLE_W))
                .align_y(Alignment::Center);
            (strip.into(), mouse::Interaction::ResizingVertically)
        }
        Edge::Left | Edge::Right => {
            let line = container(Space::new().width(Length::Fixed(1.0)).height(Length::Fill))
                .height(Length::Fill)
                .style(rule_line);
            let strip = container(line)
                .width(Length::Fixed(HANDLE_W))
                .height(Length::Fill)
                .align_x(Alignment::Center);
            (strip.into(), mouse::Interaction::ResizingHorizontally)
        }
    };
    mouse_area(strip)
        .interaction(cursor)
        .on_press(Msg::DragStart(edge))
        .on_release(Msg::DragEnd)
        .on_double_click(Msg::ResetWidth(edge))
        .into()
}

/// A grab strip's one-pixel rule.
fn rule_line(_: &Theme) -> container::Style {
    container::Style {
        background: Some(theme::palette().rule.into()),
        ..Default::default()
    }
}

/// The status bar: the workspace and its base, the live link, then the active editor's cursor and
/// language, and the count of unsaved files.
pub fn status_bar(st: &State, live: bool) -> Element<'_, Msg> {
    let p = theme::palette();
    let workspace = st
        .ide
        .workspace
        .as_ref()
        .map_or_else(|| "no workspace".to_string(), |id| id.to_string());
    let link_tone = if live { p.ok } else { p.crit };
    let link = row![
        container(
            Space::new()
                .width(Length::Fixed(8.0))
                .height(Length::Fixed(8.0))
        )
        .style(move |_: &Theme| container::Style {
            background: Some(link_tone.into()),
            ..Default::default()
        }),
        ui::sans(shell::live_word(live), TYPE_BADGE, p.ink_2),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    // A workstation has no diff, so no base to compare with.
    let base = if has_diff(st.ide.workspace.as_ref()) {
        base_label(&st.ide.base)
    } else {
        String::new()
    };
    let mut bar = row![
        ui::mono(workspace, TYPE_BADGE, p.ink_2),
        ui::sans(base, TYPE_BADGE, p.ink_3),
        link,
        Space::new().width(Length::Fill),
    ]
    .spacing(18)
    .align_y(Alignment::Center);
    if let Some(path) = st.ide.active.as_deref()
        && let Some(tab) = st.tabs.get(path)
    {
        let at = tab.content.cursor().position;
        bar = bar.push(ui::mono(
            format!("Ln {}, Col {}", at.line + 1, at.column + 1),
            TYPE_BADGE,
            p.ink_2,
        ));
        bar = bar.push(ui::sans(language_of(path), TYPE_BADGE, p.ink_2));
    }
    let dirty = st.tabs.values().filter(|tab| tab.buffer.dirty).count();
    let (dirty_text, dirty_ink) = if dirty == 0 {
        ("all saved".to_string(), p.ink_3)
    } else {
        (format!("{dirty} unsaved"), p.signal)
    };
    bar = bar.push(ui::sans(dirty_text, TYPE_BADGE, dirty_ink));
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
        assert_eq!((layout.left, layout.right), (264.0, 400.0));
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
        assert_eq!(layout.right, 340.0, "the right panel's floor");
        layout.set_width(Edge::Right, 9000.0);
        assert_eq!(layout.right, 560.0, "the right panel's ceiling");
        layout.reset_width(Edge::Right);
        assert_eq!(layout.right, 400.0);
        layout.set_width(Edge::Bottom, 9000.0);
        assert_eq!(layout.terminal_height, 560.0, "the terminal's ceiling");
        layout.reset_width(Edge::Bottom);
        assert_eq!(layout.terminal_height, 260.0);
    }

    #[test]
    fn a_panel_folds_and_unfolds_on_its_own() {
        let mut layout = Layout::fresh();
        layout.toggle(Edge::Left);
        assert!(!layout.is_open(Edge::Left));
        assert!(
            layout.is_open(Edge::Right),
            "the other panel stays as it was"
        );
        layout.toggle(Edge::Left);
        assert!(layout.is_open(Edge::Left));
    }

    #[test]
    fn a_drag_grows_the_left_panel_to_the_right_and_the_right_one_to_the_left() {
        let mut layout = Layout::fresh();
        let mut drag = Drag::begin(&layout, Edge::Left);
        drag_to(&mut layout, &mut drag, 500.0, 0.0);
        assert_eq!(
            layout.left, 264.0,
            "the first pointer position is where the drag is measured from"
        );
        drag_to(&mut layout, &mut drag, 540.0, 0.0);
        assert_eq!(layout.left, 304.0);

        let mut layout = Layout::fresh();
        let mut drag = Drag::begin(&layout, Edge::Right);
        drag_to(&mut layout, &mut drag, 900.0, 0.0);
        drag_to(&mut layout, &mut drag, 870.0, 0.0);
        assert_eq!(
            layout.right, 430.0,
            "moving left by 30 widens the right panel by 30"
        );
        drag_to(&mut layout, &mut drag, 9000.0, 0.0);
        assert_eq!(layout.right, 340.0, "a long drag stops at the floor");
    }

    #[test]
    fn the_terminal_strip_drags_up_to_grow_the_dock() {
        let mut layout = Layout::fresh();
        assert!(!layout.drawer, "the drawer is closed at the start");
        let mut drag = Drag::begin(&layout, Edge::Bottom);
        drag_to(&mut layout, &mut drag, 700.0, 400.0);
        drag_to(&mut layout, &mut drag, 9.0, 350.0);
        assert_eq!(
            layout.terminal_height, 310.0,
            "moving up by 50 grows the dock by 50, whatever x does"
        );
        drag_to(&mut layout, &mut drag, 9.0, 9000.0);
        assert_eq!(
            layout.terminal_height, 120.0,
            "a long drag stops at the floor"
        );
        layout.toggle(Edge::Bottom);
        assert!(layout.drawer, "the bottom edge's toggle opens the drawer");
        layout.toggle(Edge::Bottom);
        assert!(!layout.drawer, "and a second toggle folds it");
    }

    #[test]
    fn the_layout_is_kept_as_json_and_read_back_clamped() {
        let mut layout = Layout::fresh();
        layout.set_width(Edge::Left, 300.0);
        layout.toggle(Edge::Right);
        layout.toggle(Edge::Bottom);
        layout.set_width(Edge::Bottom, 400.0);
        let back = Layout::from_json(&layout.to_json()).expect("a saved layout reads back");
        assert_eq!(
            (back.drawer, back.terminal_height),
            (false, 400.0),
            "the drawer starts closed in a new run; its height is kept"
        );
        layout.drawer = false;
        assert_eq!(back, layout, "everything else reads back as it was saved");
        let wild =
            json!({ "left": 5.0, "right": 5000.0, "left_open": false, "terminal_height": 9.0 });
        let read = Layout::from_json(&wild).expect("an object reads");
        assert_eq!((read.left, read.right), (200.0, 560.0));
        assert_eq!(read.terminal_height, 120.0);
        assert!(
            !read.drawer,
            "the drawer is closed at a start, whatever was kept"
        );
        assert!(
            !read.left_open && read.right_open,
            "a missing flag keeps its default"
        );
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
