//! The Metrics overlays, drawn over the screen with a scrim: the panel menu, the sibling switcher, the find palette
//! (and the compare picker), the keyboard help, the "what changed" drawer, "About this metric", and the toast.

use apz_metrics_view::{Around, Body, EventRow, MoverRow, Status, Tone, Unit};
use iced::alignment::{Horizontal, Vertical};
use iced::widget::{
    Space, button, column, container, mouse_area, opaque, row, stack, text, text_input,
};
use iced::{Background, Border, Color, Element, Length, Padding, Shadow, Theme};

use super::{Msg, Overlay, RANGES, State, TOAST, dialogs, figures};
use crate::console::ui::{self, Kind};
use crate::theme::{self, TYPE_BADGE, TYPE_META, TYPE_SECTION, TYPE_UI};

/// The width of the drawer column.
const DRAWER_WIDTH: f32 = 420.0;
/// The offset of the ink shadow behind a box.
const SHADOW: f32 = 3.0;
/// The width of the panel and sibling menus.
const MENU_WIDTH: f32 = 220.0;
/// The width of the find palette.
const PALETTE_WIDTH: f32 = 620.0;
/// The width of the dialogs, the keyboard help and "About this metric".
const DIALOG_WIDTH: f32 = 600.0;
/// The most entries the find palette lists.
const PALETTE_MAX: usize = 40;
/// The keys the help lists, in order: (key, what it does).
const KEYS: &[(&str, &str)] = &[
    ("/", "Find a scope, tab or action"),
    ("Ctrl K", "Same as /"),
    ("1 – 9", "Switch tab"),
    ("[ and ]", "Earlier / later period"),
    ("l", "Live on / off"),
    ("z", "Reset zoom"),
    ("f", "Pin this scope"),
    ("a", "Annotations on / off"),
    ("c", "Compare with another scope"),
    ("d", "Compact density"),
    ("t", "Theme"),
    ("n", "New alert rule"),
    ("?", "This help"),
    ("Esc", "Close, exit full screen"),
    ("drag on a chart", "Zoom every chart to that span"),
    ("click a point", "What changed around then"),
];

/// Where a box sits on the screen.
#[derive(Clone, Copy)]
enum Anchor {
    /// Under the toolbar, at the right (the panel menu).
    Menu,
    /// Under the toolbar, beside the scope tree (the sibling switcher).
    Siblings,
    /// Near the top, centred (the find palette).
    Top,
    /// Centred on the screen (the dialogs, the help, About).
    Centre,
    /// Down the right edge, full height (the drawer).
    Drawer,
}

/// The overlay layer (none when no overlay is open), to stack over the screen.
pub fn layer<'a>(state: &'a State, overlay: &'a Overlay) -> Element<'a, Msg> {
    let (content, anchor) = match overlay {
        Overlay::PanelMenu { panel } => (panel_menu(state, panel), Anchor::Menu),
        Overlay::Siblings => (siblings(state), Anchor::Siblings),
        Overlay::Palette {
            query,
            selected,
            compare,
        } => (palette(state, query, *selected, *compare), Anchor::Top),
        Overlay::Help => (help(), Anchor::Centre),
        Overlay::Drawer(around) => (drawer(around), Anchor::Drawer),
        Overlay::About { panel } => (about(state, panel), Anchor::Centre),
        Overlay::Rule(form) => (
            dialog_frame(dialogs::rule(form, &state.catalog, &state.scopes)),
            Anchor::Centre,
        ),
        Overlay::Settings(form) => (
            dialog_frame(dialogs::settings(form, &state.scopes)),
            Anchor::Centre,
        ),
    };
    stack([scrim(), place(boxed(content), anchor)]).into()
}

/// What the find palette lists for a query: (group, label, the message it sends).
pub fn palette_items(state: &State, query: &str, compare: bool) -> Vec<(String, String, Msg)> {
    let mut items: Vec<(String, String, Msg)> = Vec::new();
    if compare {
        items.extend(state.scopes.iter().map(|scope| {
            (
                scope.kind.clone(),
                scope.label.clone(),
                Msg::Compare(format!("scope:{}", scope.id)),
            )
        }));
    } else {
        if let Some(view) = &state.view {
            items.extend(view.tabs.iter().map(|tab| {
                let group = if tab.group.is_empty() {
                    "Tab".to_owned()
                } else {
                    tab.group.clone()
                };
                (group, tab.label.clone(), Msg::Tab(tab.key.clone()))
            }));
        }
        items.extend(state.scopes.iter().map(|scope| {
            (
                scope.kind.clone(),
                scope.label.clone(),
                Msg::Scope(scope.id.clone()),
            )
        }));
        items.extend(RANGES.iter().map(|range| {
            (
                "Range".to_owned(),
                range.to_string(),
                Msg::Range(range.to_string()),
            )
        }));
        items.extend(actions(state));
    }
    let needle = query.trim().to_lowercase();
    items
        .into_iter()
        .filter(|(group, label, _)| {
            needle.is_empty() || format!("{label} {group}").to_lowercase().contains(&needle)
        })
        .take(PALETTE_MAX)
        .collect()
}

/// The toast, when one is showing: an ink box at the bottom right, until it is older than `TOAST`.
pub fn toast<'a>(state: &'a State) -> Option<Element<'a, Msg>> {
    let (message, shown_at) = state.toast.as_ref()?;
    if shown_at.elapsed() >= TOAST {
        return None;
    }
    let p = theme::palette();
    let (ink, paper) = (p.ink, p.paper);
    let body = container(ui::sans(message.clone(), TYPE_META, paper))
        .padding(pad(8.0, 12.0, 8.0, 12.0))
        .style(move |_: &Theme| container::Style {
            background: Some(Background::Color(ink)),
            ..Default::default()
        });
    Some(
        container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Horizontal::Right)
            .align_y(Vertical::Bottom)
            .padding(pad(0.0, 18.0, 18.0, 0.0))
            .into(),
    )
}

/// Padding from its four sides, in CSS order.
fn pad(top: f32, right: f32, bottom: f32, left: f32) -> Padding {
    Padding {
        top,
        right,
        bottom,
        left,
    }
}

/// No padding at all.
const NONE: Padding = Padding {
    top: 0.0,
    right: 0.0,
    bottom: 0.0,
    left: 0.0,
};

/// The actions the find palette offers, in the order the web lists them.
fn actions(state: &State) -> Vec<(String, String, Msg)> {
    let action = |label: &str, msg: Msg| ("Action".to_owned(), label.to_owned(), msg);
    let pinned = state.saved.scopes.contains(&state.q.scope);
    vec![
        action(
            if state.live { "Live off" } else { "Live on" },
            Msg::ToggleLive,
        ),
        action("New alert rule", Msg::NewRule),
        action("Compare with another scope", Msg::OpenPalette(true)),
        action(
            if pinned {
                "Unpin this scope"
            } else {
                "Pin this scope"
            },
            Msg::TogglePinScope,
        ),
        action("Reset zoom", Msg::ResetZoom),
        action(
            if state.annotations {
                "Annotations off"
            } else {
                "Annotations on"
            },
            Msg::ToggleAnnotations,
        ),
        action(
            if state.compact {
                "Comfortable density"
            } else {
                "Compact density"
            },
            Msg::ToggleCompact,
        ),
        action("Keyboard shortcuts", Msg::Help),
    ]
}

/// The scrim: the paper at 60 per cent, which closes the overlay when pressed.
fn scrim<'a>() -> Element<'a, Msg> {
    let paper = theme::palette().paper;
    let veil = container(Space::new().width(Length::Fill).height(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(Background::Color(Color { a: 0.6, ..paper })),
            ..Default::default()
        });
    mouse_area(veil).on_press(Msg::CloseOverlay).into()
}

/// The box: a sheet with a 1 px ink-3 border, and a 3 px ink shadow drawn as a second container behind it. The
/// sheet is opaque, so a press inside it does not reach the scrim.
fn boxed<'a>(content: Element<'a, Msg>) -> Element<'a, Msg> {
    let p = theme::palette();
    let ink = p.ink;
    let sheet = container(content).style(ui::frame(Some(p.sheet), p.ink_3));
    container(opaque(sheet))
        .padding(pad(0.0, SHADOW, SHADOW, 0.0))
        .style(move |_: &Theme| container::Style {
            background: Some(Background::Color(ink)),
            ..Default::default()
        })
        .into()
}

/// Places a box on the screen, full size, so the scrim shows around it.
fn place<'a>(content: Element<'a, Msg>, anchor: Anchor) -> Element<'a, Msg> {
    let (horizontal, vertical, top, right, left) = match anchor {
        Anchor::Menu => (Horizontal::Right, Vertical::Top, 110.0, 24.0, 0.0),
        Anchor::Siblings => (Horizontal::Left, Vertical::Top, 110.0, 0.0, 240.0),
        Anchor::Top => (Horizontal::Center, Vertical::Top, 96.0, 0.0, 0.0),
        Anchor::Centre => (Horizontal::Center, Vertical::Center, 0.0, 0.0, 0.0),
        Anchor::Drawer => (Horizontal::Right, Vertical::Top, 0.0, 0.0, 0.0),
    };
    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(horizontal)
        .align_y(vertical)
        .padding(pad(top, right, 0.0, left))
        .into()
}

/// A dialog's frame: its content with room around it, at the dialog width.
fn dialog_frame<'a>(content: Element<'a, Msg>) -> Element<'a, Msg> {
    container(content)
        .padding(pad(16.0, 16.0, 16.0, 16.0))
        .width(Length::Fixed(DIALOG_WIDTH))
        .into()
}

/// A title row: the title at the left, a Close button at the right.
fn head_row<'a>(title: String) -> Element<'a, Msg> {
    let p = theme::palette();
    let parts: Vec<Element<'a, Msg>> = vec![
        ui::sans(title, TYPE_SECTION, p.ink)
            .width(Length::Fill)
            .into(),
        ui::btn("Close", None, Kind::Ghost, Some(Msg::CloseOverlay)),
    ];
    row(parts).align_y(Vertical::Center).into()
}

/// A column of menu rows, at the menu width.
fn menu<'a>(items: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    container(column(items))
        .padding(pad(4.0, 0.0, 4.0, 0.0))
        .width(Length::Fixed(MENU_WIDTH))
        .into()
}

/// A button with no frame that takes the hover tone when pointed at. Its content sets the padding.
fn plain_button<'a>(content: Element<'a, Msg>, msg: Msg) -> Element<'a, Msg> {
    let p = theme::palette();
    let (hover, ink, ink_2) = (p.hover, p.ink, p.ink_2);
    button(content)
        .on_press(msg)
        .width(Length::Fill)
        .padding(NONE)
        .style(move |_: &Theme, status: button::Status| {
            let hot = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: if hot {
                    Some(Background::Color(hover))
                } else {
                    None
                },
                text_color: if hot { ink } else { ink_2 },
                border: Border::default(),
                shadow: Shadow::default(),
                snap: true,
            }
        })
        .into()
}

/// A menu row: its label, and an optional key hint at the right.
fn menu_item<'a>(label: &str, key: Option<&str>, msg: Msg) -> Element<'a, Msg> {
    let p = theme::palette();
    let mut parts: Vec<Element<'a, Msg>> = vec![
        ui::sans(label.to_owned(), TYPE_META, p.ink_2)
            .width(Length::Fill)
            .into(),
    ];
    if let Some(key) = key {
        parts.push(ui::mono(key.to_owned(), TYPE_BADGE, p.ink_3).into());
    }
    let line = row(parts).spacing(8.0).align_y(Vertical::Center);
    plain_button(
        container(line)
            .width(Length::Fill)
            .padding(pad(6.0, 12.0, 6.0, 12.0))
            .into(),
        msg,
    )
}

/// The panel menu: the panel's view, pin, export, link, About and source actions.
fn panel_menu<'a>(state: &'a State, id: &'a str) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = Vec::new();
    let panel = state
        .view
        .as_ref()
        .and_then(|view| view.panels.iter().find(|panel| panel.id == id));
    if let Some(panel) = panel {
        let chart = matches!(panel.body, Body::Chart(_));
        if chart {
            let table = state.as_table.contains(id);
            let label = if table {
                "View as chart"
            } else {
                "View as table"
            };
            items.push(menu_item(label, None, Msg::AsTable(id.to_owned())));
        }
        let full = state.full.as_deref() == Some(id);
        let label = if full {
            "Exit full screen"
        } else {
            "Full screen"
        };
        items.push(menu_item(
            label,
            Some("Esc"),
            Msg::Full(if full { None } else { Some(id.to_owned()) }),
        ));
        let tab = panel
            .pinned
            .as_ref()
            .map_or(state.q.tab.as_str(), |pin| pin.tab.as_str());
        let pinned = state
            .saved
            .pins
            .iter()
            .any(|pin| pin.tab == tab && pin.scope == state.q.scope && pin.panel == id);
        let label = if pinned {
            "Unpin"
        } else {
            "Pin to My dashboard"
        };
        items.push(menu_item(label, None, Msg::Pin(id.to_owned())));
        if chart {
            items.push(menu_item("Export CSV", None, Msg::ExportCsv(id.to_owned())));
        }
        items.push(menu_item(
            "Copy link to this view",
            None,
            Msg::CopyLink(id.to_owned()),
        ));
        items.push(menu_item(
            "About this metric",
            None,
            Msg::About(id.to_owned()),
        ));
        if let Some(pin) = &panel.pinned {
            items.push(menu_item(
                "Open at its source",
                None,
                Msg::Open(Some(pin.scope.clone()), Some(pin.tab.clone()), None),
            ));
        }
    }
    menu(items)
}

/// The sibling switcher: the scopes of the same kind under the same parent, the current one marked.
fn siblings<'a>(state: &'a State) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(view) = &state.view {
        for (label, id) in &view.scope.siblings {
            let here = if *id == view.scope.id {
                Some("here")
            } else {
                None
            };
            items.push(menu_item(label, here, Msg::Scope(id.clone())));
        }
    }
    menu(items)
}

/// The find palette: a filter box, the matching entries (the selected one marked), and the key hints.
fn palette<'a>(
    state: &'a State,
    query: &'a str,
    selected: usize,
    compare: bool,
) -> Element<'a, Msg> {
    let p = theme::palette();
    let placeholder = if compare {
        "Compare with which scope?"
    } else {
        "Find a scope, tab or action…"
    };
    let input = text_input(placeholder, query)
        .on_input(Msg::PaletteInput)
        .on_submit(Msg::PaletteRun(selected))
        .padding(pad(12.0, 14.0, 12.0, 14.0))
        .size(15.0);

    let items = palette_items(state, query, compare);
    let rows: Vec<Element<'a, Msg>> = if items.is_empty() {
        vec![ui::sans("Nothing matches.", TYPE_UI, p.ink_3).into()]
    } else {
        items
            .into_iter()
            .enumerate()
            .map(|(index, (group, label, _))| palette_row(index, index == selected, group, label))
            .collect()
    };

    let foot: Vec<Element<'a, Msg>> = vec![
        ui::mono("↑ ↓", TYPE_BADGE, p.ink_2).into(),
        ui::sans(" move", TYPE_BADGE, p.ink_3).into(),
        ui::mono("Enter", TYPE_BADGE, p.ink_2).into(),
        ui::sans(" open", TYPE_BADGE, p.ink_3).into(),
        ui::mono("Esc", TYPE_BADGE, p.ink_2).into(),
        ui::sans(" close", TYPE_BADGE, p.ink_3).into(),
    ];
    let body: Vec<Element<'a, Msg>> = vec![
        input.into(),
        ui::hairline(),
        ui::scroll(column(rows)).height(Length::Fixed(340.0)).into(),
        ui::hairline(),
        row(foot)
            .spacing(4.0)
            .padding(pad(6.0, 14.0, 6.0, 14.0))
            .into(),
    ];
    container(column(body))
        .width(Length::Fixed(PALETTE_WIDTH))
        .into()
}

/// One palette entry: its group, its label, a signal bar at the left when it is selected. A click runs it.
fn palette_row<'a>(index: usize, selected: bool, group: String, label: String) -> Element<'a, Msg> {
    let p = theme::palette();
    let (hover, signal, ink, ink_3) = (p.hover, p.signal, p.ink, p.ink_3);
    let marker = container(Space::new())
        .width(Length::Fixed(3.0))
        .height(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: if selected {
                Some(Background::Color(signal))
            } else {
                None
            },
            ..Default::default()
        });
    let parts: Vec<Element<'a, Msg>> = vec![
        marker.into(),
        ui::sans(group, TYPE_BADGE, ink_3)
            .width(Length::Fixed(54.0))
            .into(),
        ui::sans(label, TYPE_UI, ink).width(Length::Fill).into(),
    ];
    let line = container(row(parts).spacing(10.0).align_y(Vertical::Center))
        .width(Length::Fill)
        .padding(pad(6.0, 14.0, 6.0, 0.0))
        .style(move |_: &Theme| container::Style {
            background: if selected {
                Some(Background::Color(hover))
            } else {
                None
            },
            ..Default::default()
        });
    mouse_area(line).on_press(Msg::PaletteRun(index)).into()
}

/// The keyboard and mouse help, in two columns.
fn help<'a>() -> Element<'a, Msg> {
    let (left, right) = KEYS.split_at(KEYS.len() / 2);
    let columns: Vec<Element<'a, Msg>> = vec![help_column(left), help_column(right)];
    let body: Vec<Element<'a, Msg>> = vec![
        head_row("Keyboard and mouse".to_owned()),
        row(columns).spacing(24.0).into(),
    ];
    container(column(body).spacing(12.0))
        .padding(pad(16.0, 16.0, 16.0, 16.0))
        .width(Length::Fixed(DIALOG_WIDTH))
        .into()
}

/// One column of the help: a row per key.
fn help_column<'a>(items: &[(&str, &str)]) -> Element<'a, Msg> {
    let rows: Vec<Element<'a, Msg>> = items
        .iter()
        .map(|(key, text)| help_row(key, text))
        .collect();
    column(rows).width(Length::Fill).into()
}

/// One help row: what the key does, and the key at the right.
fn help_row<'a>(key: &str, text: &str) -> Element<'a, Msg> {
    let p = theme::palette();
    let parts: Vec<Element<'a, Msg>> = vec![
        ui::sans(text.to_owned(), TYPE_UI, p.ink_2)
            .width(Length::Fill)
            .into(),
        ui::mono(key.to_owned(), TYPE_BADGE, p.ink).into(),
    ];
    row(parts)
        .spacing(10.0)
        .align_y(Vertical::Center)
        .padding(pad(3.0, 0.0, 3.0, 0.0))
        .into()
}

/// A small heading inside the drawer.
fn heading<'a>(label: &str) -> Element<'a, Msg> {
    ui::sans(label.to_owned(), TYPE_META, theme::palette().ink_3).into()
}

/// The "what changed around then" drawer: the events, the metrics that moved most, the traces kept, and the zoom.
fn drawer<'a>(around: &'a Around) -> Element<'a, Msg> {
    let p = theme::palette();
    let traces: Vec<Element<'a, Msg>> = vec![
        ui::sans(
            format!(
                "{} turns · {} errors · {} slow",
                around.traces.total, around.traces.errors, around.traces.slow
            ),
            TYPE_META,
            p.ink_2,
        )
        .width(Length::Fill)
        .into(),
        ui::btn(
            "Open traces",
            None,
            Kind::Secondary,
            Some(Msg::Tab("traces".to_owned())),
        ),
    ];

    let mut body: Vec<Element<'a, Msg>> = vec![
        head_row(format!("Around {}", around.label)),
        ui::sans(around.sub.clone(), TYPE_META, p.ink_2).into(),
        heading("Events"),
    ];
    body.extend(around.events.iter().map(event_row));
    body.push(heading("Moved the most"));
    body.extend(around.movers.iter().map(mover_row));
    body.push(heading("Traces kept in this window"));
    body.push(row(traces).spacing(8.0).align_y(Vertical::Center).into());
    body.push(ui::btn(
        "Zoom to ± 30 min",
        None,
        Kind::Primary,
        Some(Msg::Zoom(around.zoom.0, around.zoom.1)),
    ));

    container(ui::scroll(column(body).spacing(6.0)).height(Length::Fill))
        .padding(pad(16.0, 18.0, 16.0, 18.0))
        .width(Length::Fixed(DRAWER_WIDTH))
        .height(Length::Fill)
        .into()
}

/// One event: its time, a status dot, the subject and the detail.
fn event_row<'a>(event: &'a EventRow) -> Element<'a, Msg> {
    let p = theme::palette();
    let mut lines: Vec<Element<'a, Msg>> =
        vec![ui::sans(event.subject.clone(), TYPE_UI, p.ink).into()];
    if !event.detail.is_empty() {
        lines.push(ui::sans(event.detail.clone(), TYPE_BADGE, p.ink_3).into());
    }
    let parts: Vec<Element<'a, Msg>> = vec![
        ui::mono(event.time.clone(), TYPE_BADGE, p.ink_3)
            .width(Length::Fixed(48.0))
            .into(),
        text("●")
            .size(TYPE_BADGE)
            .color(super::status(event.status))
            .into(),
        column(lines).spacing(2.0).width(Length::Fill).into(),
    ];
    row(parts)
        .spacing(8.0)
        .padding(pad(5.0, 0.0, 5.0, 0.0))
        .into()
}

/// One mover: a button that opens its tab, with its change and its sparkline.
fn mover_row<'a>(mover: &'a MoverRow) -> Element<'a, Msg> {
    let p = theme::palette();
    let tone = match mover.status {
        Status::Ok => Tone::Ok,
        Status::Warn => Tone::Warn,
        Status::Crit => Tone::Crit,
        Status::Info => Tone::Info,
        Status::Mute => Tone::Ink3,
    };
    let parts: Vec<Element<'a, Msg>> = vec![
        ui::sans(mover.name.clone(), TYPE_UI, p.ink)
            .width(Length::Fill)
            .into(),
        ui::mono(mover.change.clone(), TYPE_UI, super::status(mover.status)).into(),
        figures::spark(&mover.spark, tone, 90.0, 20.0),
    ];
    let line = row(parts).spacing(8.0).align_y(Vertical::Center);
    plain_button(
        container(line)
            .width(Length::Fill)
            .padding(pad(4.0, 0.0, 4.0, 0.0))
            .into(),
        Msg::Open(mover.scope.clone(), Some(mover.tab.clone()), None),
    )
}

/// "About this metric": the catalog entry of the panel's metric, or a note when there is none.
fn about<'a>(state: &'a State, id: &'a str) -> Element<'a, Msg> {
    let p = theme::palette();
    let metric = state
        .view
        .as_ref()
        .and_then(|view| view.panels.iter().find(|panel| panel.id == id))
        .and_then(|panel| panel.metric.as_deref());
    let info = metric.and_then(|metric| state.catalog.iter().find(|info| info.id == metric));

    let title = info.map_or("About this metric".to_owned(), |info| info.title.clone());
    let mut body: Vec<Element<'a, Msg>> = vec![head_row(title)];
    match info {
        Some(info) => {
            body.push(detail_row("What", info.description.clone()));
            body.push(detail_row("Unit", unit_name(&info.unit).to_owned()));
            body.push(detail_row("Source", info.source.clone()));
            body.push(detail_row("Level", info.level.clone()));
            body.push(detail_row("Cost", info.cost.clone()));
        }
        None => body.push(ui::sans("This panel has no catalog entry.", TYPE_UI, p.ink_2).into()),
    }
    container(column(body).spacing(8.0))
        .padding(pad(16.0, 16.0, 16.0, 16.0))
        .width(Length::Fixed(DIALOG_WIDTH))
        .into()
}

/// One labelled line of About: the label at the left, its value after it.
fn detail_row<'a>(label: &str, value: String) -> Element<'a, Msg> {
    let p = theme::palette();
    let parts: Vec<Element<'a, Msg>> = vec![
        ui::sans(label.to_owned(), TYPE_META, p.ink_3)
            .width(Length::Fixed(90.0))
            .into(),
        ui::sans(value, TYPE_UI, p.ink).width(Length::Fill).into(),
    ];
    row(parts).spacing(12.0).into()
}

/// The words for a unit, as About shows them.
fn unit_name(unit: &Unit) -> &'static str {
    match unit {
        Unit::Count => "count",
        Unit::Percent => "percent",
        Unit::Ms => "milliseconds",
        Unit::Bytes => "bytes",
        Unit::BytesPerSec => "bytes per second",
        Unit::Mib => "mebibytes",
        Unit::Usd => "US dollars",
        Unit::Tokens => "tokens",
        Unit::TokensPerSec => "tokens per second",
        Unit::Ratio => "ratio",
    }
}
