//! The IDE's own window, around its panels: the header (the mark and title, the workspace picker, the
//! state badge, the actions, the theme switch and the way back to the console), the held strip under
//! it, the activity rail on the left, the grab strips between the panels, and the status bar. The
//! panels' contents are `ide.rs`'s; this module draws the frame they sit in and keeps the layout they
//! are drawn with (the panel sizes and which panels are folded), which the desktop settings remember.

use apz_ide::state::Base;
use iced::widget::{Space, button, column, container, mouse_area, row, tooltip};
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

/// A 32 px icon button, as the web's `.iconbtn`: the icon in ink-2, the hover tint behind it. `on` keeps the tint
/// (a toggle that is shown).
pub fn tool_btn<'a>(icon: &str, msg: Msg, on: bool) -> Element<'a, Msg> {
    let ink = if on {
        theme::palette().ink
    } else {
        theme::palette().ink_2
    };
    let radius = theme::skin().radius;
    button(
        container(ui::icon(icon, 16.0, ink))
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    )
    .width(Length::Fixed(32.0))
    .height(Length::Fixed(32.0))
    .padding(0)
    .on_press(msg)
    .style(move |_: &Theme, status: button::Status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: (on || hovered)
                .then_some(theme::palette().hover)
                .map(Into::into),
            text_color: ink,
            border: Border {
                radius: radius.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    })
    .into()
}

/// A dock tab (the web's `.ide-dtab`): its icon, and its label only while it is the one shown; the others name
/// themselves on hover. The shown tab takes the hover tint.
pub fn dock_tab<'a, M: Clone + 'a>(
    icon: &str,
    label: &'static str,
    count: Option<String>,
    on: bool,
    msg: M,
) -> Element<'a, M> {
    let ink = if on {
        theme::palette().ink
    } else {
        theme::palette().ink_3
    };
    let mut line = row![ui::icon(icon, 15.0, ink)]
        .spacing(6)
        .align_y(Alignment::Center);
    if on {
        line = line.push(
            iced::widget::text(label)
                .font(theme::SANS_MEDIUM)
                .size(TYPE_UI)
                .color(ink),
        );
    }
    if let Some(count) = count.filter(|c| !c.is_empty()) {
        line = line.push(ui::mono(count, TYPE_BADGE, theme::palette().ink_3));
    }
    let radius = theme::skin().radius;
    let tab = button(container(line).center_y(Length::Fill))
        .height(Length::Fixed(30.0))
        .padding([0, 10])
        .on_press(msg)
        .style(move |_: &Theme, status: button::Status| {
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            let p = theme::palette();
            button::Style {
                background: (on || hovered).then_some(p.hover).map(Into::into),
                text_color: if on || hovered { p.ink } else { p.ink_3 },
                border: Border {
                    radius: radius.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        });
    if on {
        return tab.into();
    }
    tooltip(
        tab,
        container(iced::widget::text(label).size(TYPE_BADGE))
            .padding([3, 6])
            .style(|_: &Theme| container::Style {
                background: Some(theme::palette().raised.into()),
                border: Border {
                    color: theme::palette().rule,
                    width: 1.0,
                    radius: theme::skin().radius.into(),
                },
                ..Default::default()
            }),
        tooltip::Position::Bottom,
    )
    .into()
}

/// The centre's breadcrumb: the project, the agent or repo, and the base, the last one bold (the web's `.ide-crumb`).
fn crumb<'a>(st: &State) -> Element<'a, Msg> {
    let p = theme::palette();
    let parts: Vec<String> = if st.ide.workspace.is_some() {
        let (project, name, base) = ide::crumbs(st);
        vec![project, name, base]
    } else {
        vec!["IDE".to_string()]
    };
    let last = parts.len().saturating_sub(1);
    let mut line = row![].spacing(4).align_y(Alignment::Center);
    for (i, part) in parts.into_iter().enumerate() {
        if i > 0 {
            line = line.push(ui::sans("/", TYPE_META, p.ink_3));
        }
        let text = ui::ellipsize(&part, 40);
        line = line.push(if i == last {
            ui::sans(text, TYPE_UI, p.ink).font(theme::SANS_SEMIBOLD)
        } else {
            ui::sans(text, TYPE_META, p.ink_2)
        });
    }
    line.into()
}

/// The top bar of the centre (the web's `.ide-top`): the navigator's toggle while the navigator is hidden, the crumb,
/// the state, then the theme switch, the way back to the console, and the right panel's toggle. The git actions and the
/// Save sit with the views that use them (the editor's and the diff's), not here.
pub fn header(st: &State, plaza: Plaza) -> Element<'_, Msg> {
    let (state_label, tone) = ide::state_badge(st, plaza.held);
    let side_docked = st.layout.left_open && !st.left_is_drawer();
    let right_docked = st.layout.right_open && !st.right_is_sheet();
    let (theme_label, theme_icon) = if theme::is_light() {
        ("Dark theme", "moon")
    } else {
        ("Light theme", "sun")
    };
    let mut start = row![].spacing(8).align_y(Alignment::Center);
    if !side_docked {
        start = start.push(tool_btn("panel-left", Msg::ToggleLeft, false));
    }
    start = start.push(crumb(st));
    start = start.push(ui::badge(state_label, tone));
    let end = row![
        ui::btn(
            theme_label,
            Some(theme_icon),
            Kind::Ghost,
            Some(Msg::Shell(shell::Msg::ToggleTheme))
        ),
        ui::btn("Console", None, Kind::Secondary, Some(Msg::Back)),
        tool_btn("panel-open", Msg::ToggleRight, right_docked),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let bar = container(
        row![start, Space::new().width(Length::Fill), end]
            .spacing(8)
            .align_y(Alignment::Center),
    )
    .padding([8, 12])
    .height(Length::Fixed(48.0))
    .width(Length::Fill)
    .style(|_: &Theme| container::Style {
        background: Some(theme::palette().sheet.into()),
        ..Default::default()
    });
    column![bar, rule_strip()].into()
}

/// A one-pixel rule across the full width (the web's `border-bottom` or `border-top` of a bar).
pub fn rule_strip<'a>() -> Element<'a, Msg> {
    container(Space::new().width(Length::Fill).height(Length::Fixed(1.0)))
        .width(Length::Fill)
        .style(rule_line)
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

/// The status bar under the centre (the web's `.ide-status`, in mono): the live link, the workspace and its base on the
/// left; the cursor, the language and the unsaved count on the right.
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
                .width(Length::Fixed(6.0))
                .height(Length::Fixed(6.0))
        )
        .style(move |_: &Theme| container::Style {
            background: Some(link_tone.into()),
            border: Border {
                radius: 3.0.into(),
                ..Default::default()
            },
            ..Default::default()
        }),
        ui::mono(shell::live_word(live), TYPE_BADGE, p.ink_3),
    ]
    .spacing(4)
    .align_y(Alignment::Center);
    // A workstation has no diff, so no base to compare with.
    let base = if has_diff(st.ide.workspace.as_ref()) {
        base_label(&st.ide.base)
    } else {
        String::new()
    };
    let mut group = row![].spacing(16).align_y(Alignment::Center);
    if let Some(path) = st.ide.active.as_deref()
        && let Some(tab) = st.tabs.get(path)
    {
        let at = tab.content.cursor().position;
        group = group.push(ui::mono(
            format!("Ln {}, Col {}", at.line + 1, at.column + 1),
            TYPE_BADGE,
            p.ink_3,
        ));
        group = group.push(ui::mono(language_of(path), TYPE_BADGE, p.ink_3));
    }
    let dirty = st.tabs.values().filter(|tab| tab.buffer.dirty).count();
    if dirty > 0 {
        group = group.push(ui::mono(format!("{dirty} unsaved"), TYPE_BADGE, p.ink_3));
    }
    let bar = row![
        ui::mono(workspace, TYPE_BADGE, p.ink_3),
        ui::mono(base, TYPE_BADGE, p.ink_3),
        link,
        Space::new().width(Length::Fill),
        group,
    ]
    .spacing(16)
    .align_y(Alignment::Center);
    let strip = container(bar)
        .padding([4, 12])
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(theme::palette().sheet.into()),
            ..Default::default()
        });
    column![rule_strip(), strip].into()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
