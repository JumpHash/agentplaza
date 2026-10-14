//! The command palette (Ctrl+K): the agents, the files, the tasks and the commands of the IDE, filtered as the owner
//! types. Up and Down move the cursor, Enter runs the item under it, Escape closes it. The items say what they
//! do; `update` runs the one picked.

use std::sync::LazyLock;

use iced::widget::{button, column, container, text_input};
use iced::{Element, Length, Padding};

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
                label: format!("Agent {}", agent.name),
                detail: format!("{} · {}", agent.role_label, agent.status),
                run: Msg::Pick(ide::choice(info)),
            });
        }
    }
    for entry in st
        .tree
        .entries
        .iter()
        .filter(|e| e.kind == apz_ide::EntryKind::File)
    {
        out.push(Item {
            label: format!("File {}", entry.path),
            detail: "Open in the editor".into(),
            run: Msg::OpenPath(entry.path.clone()),
        });
    }
    for task in &st.tasks {
        out.push(Item {
            label: format!("Task {} {}", task.id, task.goal),
            detail: format!("{} · in the Tasks tab", task.state),
            run: Msg::Right(RightTab::Tasks),
        });
    }
    let commands: [(&str, &str, Msg); 14] = [
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
        ("New agent", "Command", Msg::NewAgent),
    ];
    for (label, detail, run) in commands {
        out.push(Item {
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

/// The palette over the window: its box at the top, the query, and the items with the cursor's row marked.
pub fn view(st: &State) -> Element<'_, Msg> {
    let Some(palette) = st.palette.as_ref() else {
        return container(column![]).into();
    };
    let p = theme::palette();
    let matches = filtered(st);
    let mut list = column![].spacing(2);
    if matches.is_empty() {
        list = list.push(ui::faint("Nothing matches."));
    }
    for (i, item) in matches.iter().enumerate().take(SHOWN) {
        list = list.push(
            button(
                column![
                    ui::mono(item.label.clone(), 13.0, p.ink),
                    ui::faint(item.detail.clone())
                ]
                .spacing(2),
            )
            .width(Length::Fill)
            .padding([6, 10])
            .style(theme::register_row(i == palette.cursor))
            .on_press(Msg::PaletteRun(i)),
        );
    }
    let input = text_input(
        "Go to an agent, a file, a task or a command",
        &palette.query,
    )
    .id(ID.clone())
    .on_input(Msg::PaletteQuery)
    .on_submit(Msg::PaletteRun(palette.cursor))
    .style(theme::input)
    .size(14.0)
    .padding(10);
    let card = container(column![input, ui::scroll(list).height(Length::Fixed(360.0))].spacing(10))
        .width(Length::Fixed(560.0))
        .padding(12)
        .style(ui::frame(Some(p.sheet), p.rule));
    container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(Padding {
            top: 72.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        })
        .align_x(iced::alignment::Horizontal::Center)
        .style(theme::scrim)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_commands_are_always_offered_and_an_empty_query_keeps_them_all() {
        let st = State::default();
        let all = filtered(&st);
        assert!(all.iter().any(|i| i.label == "Show the diff"));
        assert_eq!(all.len(), items(&st).len());
    }

    #[test]
    fn a_query_keeps_only_the_items_with_each_of_its_words() {
        let mut st = State::default();
        st.palette = Some(Palette {
            query: "terminal drawer".into(),
            cursor: 0,
        });
        let hits: Vec<String> = filtered(&st).into_iter().map(|i| i.label).collect();
        assert_eq!(hits, vec!["Toggle the terminal drawer".to_string()]);
        st.palette = Some(Palette {
            query: "nothing like this".into(),
            cursor: 0,
        });
        assert!(filtered(&st).is_empty());
    }
}
