//! The command palette (Ctrl+K): the agents, the files, the tasks and the commands of the IDE, filtered as the owner
//! types. Up and Down move the cursor, Enter runs the item under it, Escape closes it. The items say what they
//! do; `update` runs the one picked.

// The IDE's own helpers and state, for the messages this area takes (moved from `ide.rs`).
use super::ide_palette;
use crate::console::Ctx;
use iced::Task;
use std::sync::LazyLock;

use super::ide_chrome;
use iced::widget::{button, column, container, row, text_input};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding, Theme};

use super::ide::{self, Centre, Msg, RightTab, State};
use super::ide_model;
use crate::console::ui;
use crate::theme;

/// The box the palette's query is typed in: focused when it opens.
pub static ID: LazyLock<iced::widget::Id> = LazyLock::new(iced::widget::Id::unique);

/// The palette's rows shown at once; the list scrolls past them.
const SHOWN: usize = 40;

/// The palette's state while it is open: the query and the row under the cursor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Palette {
    pub query: String,
    pub cursor: usize,
}

/// One item: its label (what the query is matched against), a line of detail, and the message it runs.
#[derive(Debug, Clone)]
pub struct Item {
    pub label: String,
    pub group: &'static str,
    pub detail: String,
    pub run: Msg,
}

/// Every item the palette offers on this screen: the listed agents, the loaded files, the tasks, and the commands.
pub fn items(st: &State) -> Vec<Item> {
    let mut out = Vec::new();
    for agent in &st.roster {
        let id = apz_ide::WorkspaceId::Agent(agent.cid).to_string();
        if let Some(info) = st.workspaces.iter().find(|w| w.id == id) {
            out.push(Item {
                group: "Agents",
                label: format!("Agent {}", agent.label),
                detail: format!("{} · {}", agent.role_label, agent.status),
                run: Msg::Pick(ide::choice(info, &st.roster)),
            });
        }
    }
    for info in st.workspaces.iter().filter(|w| w.kind == "station") {
        let name = if info.name.is_empty() {
            info.owner.as_str()
        } else {
            info.name.as_str()
        };
        out.push(Item {
            group: "Agents",
            label: format!("Workstation {name}"),
            detail: format!("{} · online", ide_model::os_label(info.os.as_deref())),
            run: Msg::Pick(ide::choice(info, &st.roster)),
        });
    }
    for entry in st
        .tree
        .entries
        .iter()
        .filter(|e| e.kind == apz_ide::EntryKind::File)
    {
        out.push(Item {
            group: "Files",
            label: format!("File {}", entry.path),
            detail: "Open in the editor".into(),
            run: Msg::OpenPath(entry.path.clone()),
        });
    }
    for task in &st.tasks {
        out.push(Item {
            group: "Tasks",
            label: format!("Task {} {}", task.id, task.goal),
            detail: format!("{} · in the Tasks tab", task.state),
            run: Msg::Right(RightTab::Tasks),
        });
    }
    let commands: [(&str, &str, Msg); 13] = [
        ("Show the chat", "Centre", Msg::Centre(Centre::Chat)),
        ("Show the terminal", "Centre", Msg::Centre(Centre::Terminal)),
        ("Show the editor", "Centre", Msg::Centre(Centre::Editor)),
        ("Show the diff", "Centre", Msg::Centre(Centre::Diff)),
        ("Toggle the terminal drawer", "Ctrl+`", Msg::ToggleDrawer),
        ("Toggle the sidebar", "Ctrl+B", Msg::ToggleLeft),
        ("Toggle the right panel", "Ctrl+J", Msg::ToggleRight),
        ("Board tab", "Right panel", Msg::Right(RightTab::Board)),
        ("Files tab", "Right panel", Msg::Right(RightTab::Files)),
        (
            "Approvals tab",
            "Right panel",
            Msg::Right(RightTab::Approvals),
        ),
        (
            "Activity tab",
            "Right panel",
            Msg::Right(RightTab::Activity),
        ),
        ("Refresh the workspace", "Command", Msg::RefreshAll),
        ("Show only changed files", "Command", Msg::ChangedOnly),
    ];
    for (label, detail, run) in commands {
        out.push(Item {
            group: "Commands",
            label: label.into(),
            detail: detail.into(),
            run,
        });
    }
    out
}

/// The items the query keeps, in order.
pub fn filtered(st: &State) -> Vec<Item> {
    let all = items(st);
    let query = st.palette.as_ref().map(|p| p.query.as_str()).unwrap_or("");
    let labels: Vec<String> = all.iter().map(|i| i.label.clone()).collect();
    ide_model::filter(&labels, query)
        .into_iter()
        .map(|i: usize| all[i].clone())
        .collect()
}

/// The palette over the window (the web's `.idem-pal`): the box at the top, its query under a rule, and the items under
/// their group's heading, the cursor's row marked.
pub fn view(st: &State) -> Element<'_, Msg> {
    let Some(palette) = st.palette.as_ref() else {
        return container(column![]).into();
    };
    let p = theme::palette();
    let matches = filtered(st);
    let mut list = column![].spacing(0);
    let mut height = 0.0_f32;
    if matches.is_empty() {
        list = list.push(
            container(ui::faint("No match"))
                .padding(Padding {
                    top: 16.0,
                    right: 16.0,
                    bottom: 16.0,
                    left: 16.0,
                })
                .width(Length::Fill),
        );
        height += 52.0;
    }
    let mut group = "";
    for (i, item) in matches.iter().enumerate().take(SHOWN) {
        if item.group != group {
            group = item.group;
            list = list.push(
                container(ui::faint(group))
                    .padding(Padding {
                        top: 8.0,
                        right: 16.0,
                        bottom: 4.0,
                        left: 16.0,
                    })
                    .width(Length::Fill),
            );
            height += 30.0;
        }
        list = list.push(option_row(item, i, i == palette.cursor));
        height += 36.0;
    }
    let input = text_input("Go to an agent, file, task or command", &palette.query)
        .id(ID.clone())
        .on_input(Msg::PaletteQuery)
        .on_submit(Msg::PaletteRun(palette.cursor))
        .style(input_style)
        .size(14.0)
        .padding(16.0);
    let list = container(ui::scroll(list).height(Length::Fixed(height.min(PALETTE_MAX))))
        .padding(Padding {
            top: 4.0,
            right: 0.0,
            bottom: 4.0,
            left: 0.0,
        })
        .width(Length::Fill);
    let card = container(column![input, ide_chrome::rule_strip(), list])
        .width(Length::Fixed(600.0))
        .style(ui::frame(Some(p.raised), p.rule));
    container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(Padding {
            top: 96.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        })
        .align_x(iced::alignment::Horizontal::Center)
        .style(theme::scrim)
        .into()
}

/// The height the palette's list grows to before it scrolls (about 70 % of a laptop window).
const PALETTE_MAX: f32 = 420.0;

/// One row of the palette: its label, and its detail at the right in the faint ink. The cursor's row takes the hover tint.
fn option_row<'a>(item: &Item, i: usize, on: bool) -> Element<'a, Msg> {
    let p = theme::palette();
    let ink = if on { p.ink } else { p.ink_2 };
    let line = row![
        ui::sans(item.label.clone(), theme::TYPE_UI, ink).width(Length::Fill),
        ui::sans(item.detail.clone(), theme::TYPE_META, p.ink_3),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    button(line)
        .width(Length::Fill)
        .padding(Padding {
            top: 8.0,
            right: 16.0,
            bottom: 8.0,
            left: 16.0,
        })
        .on_press(Msg::PaletteRun(i))
        .style(move |_: &Theme, status: button::Status| {
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: (on || hovered)
                    .then_some(theme::palette().hover)
                    .map(Into::into),
                text_color: ink,
                ..Default::default()
            }
        })
        .into()
}

/// The query box: no box of its own (the card's rule is its bottom edge), the placeholder in the faint ink.
fn input_style(_: &Theme, _status: text_input::Status) -> text_input::Style {
    let p = theme::palette();
    text_input::Style {
        background: Background::Color(Color::TRANSPARENT),
        border: Border::default(),
        icon: p.ink_3,
        placeholder: p.ink_3,
        value: p.ink,
        selection: p.signal_soft,
    }
}

// ---- The messages the IDE hands to this area (moved from `ide.rs`'s `update`) ----------------------

/// Whether a message is this area's: `update` below takes it, and `ide.rs` hands it over.
pub(super) fn handles(msg: &Msg) -> bool {
    matches!(
        msg,
        Msg::PaletteOpen
            | Msg::PaletteQuery(..)
            | Msg::PaletteMove(..)
            | Msg::PaletteRun(..)
            | Msg::PaletteClose
    )
}

/// The area's messages: the state they change, and the tasks they start. See `handles`.
pub(super) fn update(st: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::PaletteOpen => {
            st.palette = Some(ide_palette::Palette::default());
            iced::widget::operation::focus(ide_palette::ID.clone())
        }
        Msg::PaletteQuery(query) => {
            if let Some(palette) = st.palette.as_mut() {
                palette.query = query;
                palette.cursor = 0;
            }
            Task::none()
        }
        Msg::PaletteMove(step) => {
            let count = ide_palette::filtered(st).len();
            if let Some(palette) = st.palette.as_mut() {
                let next = palette.cursor as i64 + i64::from(step);
                palette.cursor = next.clamp(0, count.saturating_sub(1) as i64) as usize;
            }
            Task::none()
        }
        Msg::PaletteRun(index) => {
            let picked = ide_palette::filtered(st).into_iter().nth(index);
            st.palette = None;
            match picked {
                Some(item) => super::ide::update(st, item.run, ctx),
                None => Task::none(),
            }
        }
        Msg::PaletteClose => {
            st.palette = None;
            Task::none()
        }
        _ => Task::none(),
    }
}
