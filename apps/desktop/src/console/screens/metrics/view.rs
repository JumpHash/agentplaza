//! The Metrics screen's layout: the head (title with the pin star, breadcrumb with the sibling switcher, the
//! controls), the tier line (window end, storage tier, live text, zoom and compare chips, level), the scope tree,
//! the grouped tabs with the alerts badge, and the grid of panels by span (twelfths), as the web's `.mx-view`.

use iced::widget::{Space, button, column, container, pick_list, row, stack, text, text_input};
use iced::{Alignment, Border, Color, Element, Length, Padding, Theme};

use apz_metrics_view::{ScopeNode, View};

use super::{Msg, State, overlays, panels};
use crate::console::Ctx;
use crate::console::ui;
use crate::theme::{SANS_MEDIUM, SANS_SEMIBOLD, TYPE_BADGE, TYPE_META, TYPE_PAGE, TYPE_UI};

pub fn screen<'a>(state: &'a State, ctx: &'a Ctx) -> Element<'a, Msg> {
    let _ = ctx;
    let mut layers: Vec<Element<'a, Msg>> = vec![ui::sheet(head(state), body(state))];
    if let Some(overlay) = &state.overlay {
        layers.push(overlays::layer(state, overlay));
    }
    if let Some(toast) = overlays::toast(state) {
        layers.push(toast);
    }
    if layers.len() == 1 {
        layers.remove(0)
    } else {
        stack(layers).into()
    }
}

/// One entry of a pick list: the value sent on a pick, and the label shown.
#[derive(Debug, Clone, PartialEq)]
struct Choice {
    value: String,
    label: &'static str,
}

impl std::fmt::Display for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label)
    }
}

/// The title with the pin star, the breadcrumb, the controls, then the tier line.
fn head<'a>(state: &'a State) -> Element<'a, Msg> {
    let pal = crate::theme::palette();
    let pinned = state.saved.scopes.contains(&state.q.scope);
    let (glyph, glyph_color) = if pinned {
        ("★", pal.signal)
    } else {
        ("☆", pal.ink_3)
    };
    let star = button(text(glyph).size(15.0).color(glyph_color))
        .padding(Padding::from([0.0, 2.0]))
        .style(move |_, _| button::Style {
            background: None,
            text_color: glyph_color,
            border: Border::default(),
            ..Default::default()
        })
        .on_press(Msg::TogglePinScope);
    let title = row![
        text("Metrics")
            .font(SANS_SEMIBOLD)
            .size(TYPE_PAGE)
            .color(pal.ink),
        star,
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    column![
        title,
        crumbs(state),
        controls(state),
        tier(state),
        ui::hairline::<Msg>()
    ]
    .spacing(8)
    .into()
}

/// The breadcrumb of the scope's path (the last one bold), and the sibling switcher when there are siblings.
fn crumbs<'a>(state: &'a State) -> Element<'a, Msg> {
    let Some(view) = &state.view else {
        return Space::new().into();
    };
    let pal = crate::theme::palette();
    let last = view.scope.path.len().saturating_sub(1);
    let mut items: Vec<Element<'a, Msg>> = Vec::new();
    for (index, (label, id)) in view.scope.path.iter().enumerate() {
        if index > 0 {
            items.push(ui::faint("›").into());
        }
        if index == last {
            items.push(
                text(label.clone())
                    .font(SANS_SEMIBOLD)
                    .size(TYPE_UI)
                    .color(pal.ink)
                    .into(),
            );
        } else {
            items.push(ui::link(label, Msg::Scope(id.clone()), false));
        }
    }
    if view.scope.siblings.len() > 1 {
        items.push(ui::btn_sm("▾", None, ui::Kind::Ghost, Some(Msg::Siblings)));
    }
    row(items).spacing(6).align_y(Alignment::Center).into()
}

/// The right side of the head: find, the time shift and range, live, group, compare, annotations, export, settings.
fn controls<'a>(state: &'a State) -> Element<'a, Msg> {
    let pal = crate::theme::palette();
    let find = ui::btn_sm(
        "Find… /",
        None,
        ui::Kind::Ghost,
        Some(Msg::OpenPalette(false)),
    );

    let shift_later = if state.q.shift < 0 {
        Some(Msg::Shift(state.q.shift + 1))
    } else {
        None
    };
    let shift = strip(vec![
        ghost(
            text("‹").font(SANS_MEDIUM).size(TYPE_META),
            Some(Msg::Shift(state.q.shift - 1)),
        ),
        ghost(text("›").font(SANS_MEDIUM).size(TYPE_META), shift_later),
    ]);

    let ranges: Vec<(&str, &str)> = super::RANGES.iter().map(|range| (*range, *range)).collect();
    let range = ui::seg(&ranges, state.q.range.as_str(), |value: &str| {
        Msg::Range(value.to_string())
    });

    let dot_color = if state.live { pal.ok } else { pal.ink_3 };
    let dot = container(Space::new())
        .width(Length::Fixed(8.0))
        .height(Length::Fixed(8.0))
        .style(move |_: &Theme| container::Style {
            background: Some(dot_color.into()),
            border: Border {
                radius: 4.0.into(),
                ..Default::default()
            },
            ..Default::default()
        });
    let live = ghost(
        row![dot, ui::sans("Live", TYPE_UI, pal.ink_2)]
            .spacing(6)
            .align_y(Alignment::Center),
        Some(Msg::ToggleLive),
    );

    let group_options: Vec<Choice> = super::GROUPS
        .iter()
        .map(|&(value, label)| Choice {
            value: value.to_string(),
            label,
        })
        .collect();
    let group_selected = group_options
        .iter()
        .find(|choice| choice.value == state.q.group)
        .cloned();
    let group = pick_list(group_options, group_selected, |choice: Choice| {
        Msg::Group(choice.value)
    })
    .text_size(TYPE_UI)
    .style(crate::theme::pick_list);

    let compare_key = if state.q.compare.starts_with("scope:") {
        "scope"
    } else {
        state.q.compare.as_str()
    };
    let compare_options = vec![
        Choice {
            value: "prev".to_string(),
            label: "vs previous period",
        },
        Choice {
            value: "week".to_string(),
            label: "vs same time last week",
        },
        Choice {
            value: "scope".to_string(),
            label: "vs another scope…",
        },
        Choice {
            value: "none".to_string(),
            label: "no comparison",
        },
    ];
    let compare_selected = compare_options
        .iter()
        .find(|choice| choice.value == compare_key)
        .cloned();
    let compare = pick_list(compare_options, compare_selected, |choice: Choice| {
        if choice.value == "scope" {
            Msg::OpenPalette(true)
        } else {
            Msg::Compare(choice.value)
        }
    })
    .text_size(TYPE_UI)
    .style(crate::theme::pick_list);

    let annotations = ui::btn_sm(
        "Annotations",
        None,
        ui::Kind::Ghost,
        Some(Msg::ToggleAnnotations),
    );
    let export = ui::btn_sm(
        "Export",
        None,
        ui::Kind::Ghost,
        Some(Msg::ExportCsv(String::new())),
    );
    let settings = ui::btn_sm("Settings", None, ui::Kind::Ghost, Some(Msg::OpenSettings));

    let items: Vec<Element<'a, Msg>> = vec![
        find.into(),
        shift.into(),
        range.into(),
        live.into(),
        group.into(),
        compare.into(),
        annotations.into(),
        export.into(),
        settings.into(),
    ];
    row(items).spacing(8).wrap().vertical_spacing(8).into()
}

/// The tier line: the window's end, the storage tier the hub read, the live text, then the zoom and compare
/// chips and the collection level.
fn tier<'a>(state: &'a State) -> Element<'a, Msg> {
    let pal = crate::theme::palette();
    let meta = |content: String| ui::sans(content, TYPE_META, pal.ink_3);
    let mut items: Vec<Element<'a, Msg>> = Vec::new();
    let end = match &state.view {
        Some(view) if state.q.shift != 0 => format!("ending {}", clock(view.to_ms)),
        _ => "ending now".to_string(),
    };
    items.push(meta(end).into());
    if let Some(view) = &state.view {
        items.push(meta("· reading".to_string()).into());
        items.push(
            ui::sans(view.tier.clone(), TYPE_META, pal.ink_2)
                .font(SANS_MEDIUM)
                .into(),
        );
        items.push(
            meta(format!(
                ", {} points per series · {}",
                view.points,
                live_text(state)
            ))
            .into(),
        );
        if let Some((from, to)) = state.q.zoom {
            items.push(chip(
                row![
                    ui::sans(
                        format!("zoomed {:.0}–{:.0}% of the range", from * 100.0, to * 100.0),
                        TYPE_BADGE,
                        pal.signal_ink,
                    ),
                    ui::btn_sm("reset (z)", None, ui::Kind::Ghost, Some(Msg::ResetZoom)),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            ));
        }
        if state.q.compare.starts_with("scope:") {
            items.push(chip(
                row![
                    ui::sans(
                        format!("comparing with {}", view.compare_label),
                        TYPE_BADGE,
                        pal.signal_ink,
                    ),
                    ui::btn_sm(
                        "stop",
                        None,
                        ui::Kind::Ghost,
                        Some(Msg::Compare("prev".to_string())),
                    ),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            ));
        }
        items.push(chip(ui::sans(
            format!("level: {}", view.level),
            TYPE_BADGE,
            pal.signal_ink,
        )));
        if view.stepped_down {
            items.push(chip(ui::sans("stepped down", TYPE_BADGE, pal.signal_ink)));
        }
    }
    row(items).spacing(6).align_y(Alignment::Center).into()
}

/// The live text of the tier line, as the web's.
fn live_text(state: &State) -> &'static str {
    if !state.live {
        "live off"
    } else if state.q.shift != 0 {
        "live pauses while looking back"
    } else if !matches!(state.q.range.as_str(), "15m" | "1h") {
        "live applies to the 15m and 1h ranges"
    } else {
        "live: the scope on screen, drawn at most once a second"
    }
}

/// A local date and time, for a view that looks back.
fn clock(ms: u64) -> String {
    let offset_min = chrono::Local::now().offset().local_minus_utc() / 60;
    apz_metrics_view::format::time_label(ms, 3_600_000, offset_min)
}

/// A bordered box around its content, for the tier line's chips.
fn chip<'a>(content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    let signal = crate::theme::palette().signal;
    container(content)
        .padding(Padding {
            top: 1.0,
            right: 6.0,
            bottom: 1.0,
            left: 6.0,
        })
        .style(move |_: &Theme| container::Style {
            border: Border {
                color: signal,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into()
}

/// Cells in one bordered strip, as the segmented controls.
fn strip<'a>(cells: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let rule = crate::theme::palette().rule;
    container(row(cells).align_y(Alignment::Center))
        .style(move |_: &Theme| container::Style {
            border: Border {
                color: rule,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into()
}

/// A text button that shows a hover fill; with no message it is disabled.
fn ghost<'a>(content: impl Into<Element<'a, Msg>>, msg: Option<Msg>) -> Element<'a, Msg> {
    let pal = crate::theme::palette();
    let (hover, ink_2) = (pal.hover, pal.ink_2);
    button(content)
        .padding(Padding::from([4.0, 8.0]))
        .style(move |_, status: button::Status| {
            let lit = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: if lit { Some(hover.into()) } else { None },
                text_color: ink_2,
                border: Border::default(),
                ..Default::default()
            }
        })
        .on_press_maybe(msg)
        .into()
}

/// The scope tree: the pinned scopes, the recent ones, then every scope by its group; the filter narrows all.
fn tree<'a>(state: &'a State) -> Element<'a, Msg> {
    let pal = crate::theme::palette();
    let (sheet, rule) = (pal.sheet, pal.rule);
    let query = state.tree_filter.trim().to_lowercase();
    let current = state.q.scope.as_str();

    let mut sections: Vec<(String, Vec<&'a ScopeNode>)> = Vec::new();
    let pinned: Vec<&'a ScopeNode> = state
        .saved
        .scopes
        .iter()
        .filter_map(|id| find_scope(&state.scopes, id))
        .collect();
    if !pinned.is_empty() {
        sections.push(("★ Pinned scopes".to_string(), pinned));
    }
    let recent: Vec<&'a ScopeNode> = state
        .saved
        .recent
        .iter()
        .filter(|id| !state.saved.scopes.contains(*id) && id.as_str() != current)
        .filter_map(|id| find_scope(&state.scopes, id))
        .collect();
    if !recent.is_empty() {
        sections.push(("Recent".to_string(), recent));
    }
    let mut groups: Vec<(String, Vec<&'a ScopeNode>)> = Vec::new();
    for node in &state.scopes {
        match groups.iter_mut().find(|(label, _)| *label == node.group) {
            Some((_, nodes)) => nodes.push(node),
            None => groups.push((node.group.clone(), vec![node])),
        }
    }
    sections.extend(groups);

    let mut rows: Vec<Element<'a, Msg>> = Vec::new();
    for (label, nodes) in sections {
        let matching: Vec<&'a ScopeNode> = nodes
            .into_iter()
            .filter(|node| query.is_empty() || node.label.to_lowercase().contains(&query))
            .collect();
        if matching.is_empty() {
            continue;
        }
        if !label.is_empty() {
            rows.push(
                container(ui::sans(label, TYPE_BADGE, pal.ink_3).font(SANS_MEDIUM))
                    .padding(Padding {
                        top: 8.0,
                        right: 12.0,
                        bottom: 2.0,
                        left: 12.0,
                    })
                    .into(),
            );
        }
        for node in matching {
            rows.push(tree_row(node, node.id == current));
        }
    }

    let filter = container(
        text_input("Find a node, VM, agent, model…", &state.tree_filter)
            .on_input(Msg::TreeFilter)
            .size(TYPE_META)
            .padding(Padding::from([5.0, 8.0]))
            .style(crate::theme::input),
    )
    .padding(Padding::from([0.0, 8.0]));
    container(column![filter, ui::scroll(column(rows))].spacing(8))
        .width(Length::Fixed(220.0))
        .padding(Padding {
            top: 8.0,
            right: 0.0,
            bottom: 8.0,
            left: 0.0,
        })
        .style(move |_: &Theme| container::Style {
            background: Some(sheet.into()),
            border: Border {
                color: rule,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into()
}

/// The scope with this id, from the tree's list.
fn find_scope<'s>(scopes: &'s [ScopeNode], id: &str) -> Option<&'s ScopeNode> {
    scopes.iter().find(|node| node.id == id)
}

/// One row of the tree: a 3 px signal bar when current, the indent by depth, a status dot, the label and the tag.
fn tree_row<'a>(node: &'a ScopeNode, current: bool) -> Element<'a, Msg> {
    let pal = crate::theme::palette();
    let (bar, hover, rule) = if current {
        (pal.signal, pal.hover, pal.rule)
    } else {
        (Color::TRANSPARENT, pal.hover, pal.rule)
    };
    let ink = if current { pal.ink } else { pal.ink_2 };
    let indent = f32::from(node.depth) * 10.0;
    let dot: Element<'a, Msg> = match node.status {
        Some(status) => {
            let color = super::status(status);
            container(Space::new())
                .width(Length::Fixed(6.0))
                .height(Length::Fixed(6.0))
                .style(move |_: &Theme| container::Style {
                    background: Some(color.into()),
                    border: Border {
                        radius: 3.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .into()
        }
        None => Space::new().width(Length::Fixed(6.0)).into(),
    };
    let tag: Element<'a, Msg> = if node.tag.is_empty() {
        Space::new().into()
    } else {
        container(ui::sans(node.tag.clone(), TYPE_BADGE, pal.ink_3))
            .padding(Padding::from([0.0, 4.0]))
            .style(move |_: &Theme| container::Style {
                border: Border {
                    color: rule,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .into()
    };
    let content = row![
        container(Space::new())
            .width(Length::Fixed(3.0))
            .height(Length::Fixed(20.0))
            .style(move |_: &Theme| container::Style {
                background: Some(bar.into()),
                ..Default::default()
            }),
        Space::new().width(Length::Fixed(9.0 + indent)),
        dot,
        ui::sans(node.label.trim().to_string(), TYPE_UI, ink),
        Space::new().width(Length::Fill),
        tag,
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    button(content)
        .width(Length::Fill)
        .padding(Padding {
            top: 4.0,
            right: 12.0,
            bottom: 4.0,
            left: 0.0,
        })
        .style(move |_, status: button::Status| {
            let lit =
                current || matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: if lit { Some(hover.into()) } else { None },
                text_color: ink,
                border: Border::default(),
                ..Default::default()
            }
        })
        .on_press(Msg::Scope(node.id.clone()))
        .into()
}

/// The body: the tree at the left, the tabs and the grid (or the loading and error lines) at the right.
fn body<'a>(state: &'a State) -> Element<'a, Msg> {
    let gap = if state.compact { 10.0 } else { 14.0 };
    let main: Element<'a, Msg> = match &state.view {
        Some(view) => main_column(view, state, gap),
        None => match &state.error {
            Some(error) => ui::error_line(error),
            None => ui::loading(),
        },
    };
    row![tree(state), container(main).width(Length::Fill)]
        .spacing(gap)
        .into()
}

/// The content at the right of the tree: the error line, the tabs (not in full screen) and the grid, in a scroll.
fn main_column<'a>(view: &'a View, state: &'a State, gap: f32) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(error) = &state.error {
        parts.push(ui::error_line(error));
    }
    if state.full.is_none() {
        parts.push(tabs(view, state));
    }
    parts.push(grid(view, state, gap));
    ui::scroll(column(parts).spacing(gap)).into()
}

/// The tabs, grouped: a small uppercase label before each group, the current tab underlined in signal, and the
/// alerts count in crit.
fn tabs<'a>(view: &'a View, state: &'a State) -> Element<'a, Msg> {
    let pal = crate::theme::palette();
    let (ink, ink_2, ink_3, signal, crit, sheet_ink) = (
        pal.ink, pal.ink_2, pal.ink_3, pal.signal, pal.crit, pal.sheet,
    );
    let mut cells: Vec<Element<'a, Msg>> = Vec::new();
    let mut group: Option<&str> = None;
    for tab in &view.tabs {
        if group != Some(tab.group.as_str()) {
            group = Some(tab.group.as_str());
            let label = if tab.group.is_empty() {
                "·".to_string()
            } else {
                tab.group.to_uppercase()
            };
            // The label sits on the tabs' text line: the tab's top padding (8) plus half the difference in line height.
            cells.push(
                container(ui::sans(label, TYPE_BADGE, ink_3).font(SANS_MEDIUM))
                    .padding(Padding {
                        top: 9.0,
                        right: 4.0,
                        bottom: 0.0,
                        left: 12.0,
                    })
                    .into(),
            );
        }
        let selected = tab.key == state.q.tab;
        let count = if tab.key == "alerts" { view.firing } else { 0 };
        let mut label = row![
            ui::sans(
                tab.label.clone(),
                TYPE_UI,
                if selected { ink } else { ink_2 },
            )
            .font(SANS_MEDIUM)
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        if count > 0 {
            label = label.push(
                container(
                    text(count.to_string())
                        .font(SANS_SEMIBOLD)
                        .size(10.0)
                        .color(sheet_ink),
                )
                .padding(Padding::from([0.0, 5.0]))
                .style(move |_: &Theme| container::Style {
                    background: Some(crit.into()),
                    ..Default::default()
                }),
            );
        }
        let underline_color = if selected { signal } else { Color::TRANSPARENT };
        let underline = container(Space::new())
            .width(Length::Fill)
            .height(Length::Fixed(2.0))
            .style(move |_: &Theme| container::Style {
                background: Some(underline_color.into()),
                ..Default::default()
            });
        cells.push(
            // Shrink: the underline fills the label's width, not the row's, so the tabs wrap in rows.
            button(column![label, underline].spacing(8).width(Length::Shrink))
                .padding(Padding {
                    top: 8.0,
                    right: 10.0,
                    bottom: 0.0,
                    left: 10.0,
                })
                .style(move |_, status: button::Status| {
                    let hovered =
                        matches!(status, button::Status::Hovered | button::Status::Pressed);
                    button::Style {
                        background: None,
                        text_color: if hovered { ink } else { ink_2 },
                        border: Border::default(),
                        ..Default::default()
                    }
                })
                .on_press(Msg::Tab(tab.key.clone()))
                .into(),
        );
    }
    let tab_row: Element<'a, Msg> = row(cells).wrap().vertical_spacing(4).into();
    column![tab_row, ui::hairline::<Msg>()].into()
}

/// The panels in rows whose spans add to twelve, each panel a portion of its row. In full screen, the one panel at
/// full size with a button to leave.
fn grid<'a>(view: &'a View, state: &'a State, gap: f32) -> Element<'a, Msg> {
    if let Some(id) = &state.full {
        if let Some(panel) = view.panels.iter().find(|panel| &panel.id == id) {
            return column![
                ui::btn_sm(
                    "Exit full screen",
                    None,
                    ui::Kind::Ghost,
                    Some(Msg::Full(None))
                ),
                panels::panel(panel, state),
            ]
            .spacing(gap)
            .into();
        }
    }
    let mut rows: Vec<Element<'a, Msg>> = Vec::new();
    let mut cells: Vec<Element<'a, Msg>> = Vec::new();
    let mut used: u32 = 0;
    for panel in &view.panels {
        let span = u32::from(panel.span).clamp(1, 12);
        if used + span > 12 && !cells.is_empty() {
            rows.push(row(std::mem::take(&mut cells)).spacing(gap).into());
            used = 0;
        }
        used += span;
        cells.push(
            container(panels::panel(panel, state))
                .width(Length::FillPortion(span as u16))
                .into(),
        );
    }
    if !cells.is_empty() {
        rows.push(row(cells).spacing(gap).into());
    }
    column(rows).spacing(gap).into()
}
