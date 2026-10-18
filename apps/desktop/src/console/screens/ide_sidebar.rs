//! The IDE's left sidebar: a search field, the owner's conversations with the orchestrator, the projects as
//! collapsible groups of agents and repos (Coral's grouping), each agent's status dot, name, current task and unread
//! count, a hover card with what the agent is, and "+ New conversation" and "+ New agent" at the foot. In project
//! mode (the IDE opened under a project) it is that project's: its head with the way to its tasks, board, work and
//! settings, its conversation, and its agents and repo only. Rows come from the roster `ide.rs` loads (`ide_model`
//! groups and reads them).

// The IDE's own helpers and state, for the messages this area takes (moved from `ide.rs`).
use super::ide::*;
use crate::console::Ctx;
use apz_ide::WorkspaceId;
use iced::Task;
use iced::font::{Font, Style as FontStyle, Weight};
use iced::widget::{Space, Stack, button, column, container, row, text, text_input, tooltip};
use iced::{Alignment, Element, Length};
use std::str::FromStr;

use super::Id;
use super::ide::{self, Msg, State};
use super::ide_model::{self, AgentRow, Dot, Group};
use crate::console::{scope, shell, ui};
use crate::theme;
use serde_json::Value;

/// How many conversations the sidebar lists.
const CONVERSATIONS: usize = 6;

/// The status dot's diameter, and the rule drawn through a finished agent's name.
const DOT: f32 = 9.0;

/// The agents the search field keeps: each word must appear in the name, the role or the current task.
fn matching(st: &State) -> Vec<AgentRow> {
    if st.sidebar_query.trim().is_empty() {
        return st.roster.clone();
    }
    let labels: Vec<String> = st
        .roster
        .iter()
        .map(|a| {
            format!(
                "{} {} {}",
                a.name,
                a.role_label,
                a.task.as_deref().unwrap_or("")
            )
        })
        .collect();
    ide_model::filter(&labels, &st.sidebar_query)
        .into_iter()
        .map(|i| st.roster[i].clone())
        .collect()
}

/// The sidebar: search, the groups, and the footer.
pub fn view(st: &State) -> Element<'_, Msg> {
    let search = text_input("Search agents and repos", &st.sidebar_query)
        .on_input(Msg::SidebarQuery)
        .style(theme::input)
        .size(13.0)
        .padding(6);
    let agents = matching(st);
    let repos: Vec<(String, String)> = st
        .workspaces
        .iter()
        .filter(|w| w.kind == "repo")
        .map(|w| (w.id.clone(), w.name.clone()))
        .collect();
    let stations: Vec<String> = st
        .workspaces
        .iter()
        .filter(|w| w.kind == "station")
        .map(|w| w.id.clone())
        .collect();
    // In project mode, the project's group alone.
    let groups = match &st.project {
        Some(project) => ide_model::project_group(&agents, &st.projects, &repos, project),
        None => ide_model::groups(&agents, &st.projects, &repos, &stations),
    };
    let unread = super::board::unread(&st.board, st.board_seen_ms);

    let mut list = column![].spacing(6);
    for row in conversation_rows(st) {
        list = list.push(row);
    }
    if let Some(error) = &st.roster_error {
        list = list.push(ui::error_line(error));
    } else if !st.roster_loaded {
        list = list.push(ui::faint("Loading agents…"));
    } else if groups.is_empty() {
        list = list.push(ui::faint(if st.project.is_some() {
            "Nobody works on this project yet: add agents from the bench in its Team tab."
        } else {
            "Nothing yet. Start with a conversation: + New conversation, below."
        }));
    }
    let selected_cid = match st.ide.workspace {
        Some(apz_ide::WorkspaceId::Agent(cid)) => Some(cid),
        _ => None,
    };
    for group in &groups {
        list = list.push(group_head(st, group));
        if st.collapsed.contains(&group.key) {
            continue;
        }
        for cid in &group.agents {
            if let Some(agent) = agents.iter().find(|a| a.cid == *cid) {
                let last = last_of(st, *cid, selected_cid);
                list = list.push(agent_row(
                    st,
                    agent,
                    last.as_deref(),
                    unread.get(cid).copied().unwrap_or(0),
                ));
            }
        }
        for id in &group.repos {
            if let Some(w) = st.workspaces.iter().find(|w| &w.id == id) {
                list = list.push(repo_row(st, w));
            }
        }
        for id in &group.stations {
            if let Some(w) = st.workspaces.iter().find(|w| &w.id == id) {
                list = list.push(station_row(st, w));
            }
        }
    }

    let foot = |label: &'static str, msg: Msg| {
        button(
            row![
                ui::icon("plus", 14.0, theme::palette().ink_2),
                ui::sans(label, 12.5, theme::palette().ink)
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .on_press(msg)
        .width(Length::Fill)
        .padding([7, 10])
        .style(theme::register_row(false))
    };
    let footer = column![
        foot("New conversation", Msg::NewProject),
        foot("New agent", Msg::NewAgent)
    ]
    .spacing(4);

    let mut col = column![search].spacing(10).height(Length::Fill);
    if let Some(project) = &st.project {
        col = col.push(project_head(project));
    }
    col.push(ui::scroll(list).height(Length::Fill))
        .push(footer)
        .into()
}

/// In project mode: the project's title, and the way to its tasks, board, work and settings in the console.
fn project_head(project: &str) -> Element<'static, Msg> {
    let go = |to: Id| Msg::Shell(shell::Msg::Scope(Some(project.to_string()), Some(to)));
    let links = row![
        ui::text_link("Tasks", go(Id::Tasks)),
        ui::text_link("Board", go(Id::Board)),
        ui::text_link("Work", go(Id::WorkOn(project.to_string()))),
        ui::text_link(
            "Settings",
            go(Id::ProjectTab(
                project.to_string(),
                super::projects::Tab::Settings
            ))
        ),
    ]
    .spacing(12);
    container(
        column![
            ui::sans(scope::title(project), 14.0, theme::palette().ink),
            links
        ]
        .spacing(6),
    )
    .padding([4, 6])
    .width(Length::Fill)
    .into()
}

/// The conversations: the newest few (in project mode, the ones of the project), under their own heading.
fn conversation_rows(st: &State) -> Vec<Element<'static, Msg>> {
    let q = st.sidebar_query.trim().to_lowercase();
    let shown: Vec<&Value> = st
        .conversations
        .iter()
        .filter(|c| {
            st.project
                .as_deref()
                .is_none_or(|p| c["project"].as_str() == Some(p))
        })
        .filter(|c| {
            q.is_empty()
                || c["title"]
                    .as_str()
                    .is_some_and(|t| t.to_lowercase().contains(&q))
        })
        .take(CONVERSATIONS)
        .collect();
    if shown.is_empty() {
        return Vec::new();
    }
    let open = st.conversation.as_ref().map(|c| c.id.as_str());
    let mut rows: Vec<Element<'static, Msg>> = vec![ui::label("Conversations").into()];
    for c in shown {
        let id = c["id"].as_str().unwrap_or("").to_string();
        let pending = c["pending"].as_u64().unwrap_or(0) > 0;
        let meta = if pending {
            "answering…".to_string()
        } else if let Some(p) = c["project"].as_str() {
            scope::title(p)
        } else if let Some(r) = c["draft"]["revision"].as_u64() {
            format!("draft · revision {r}")
        } else {
            ide_model::ago(ide::now_ms(), c["updated_ms"].as_u64().unwrap_or(0))
        };
        let dot = if pending { Dot::Working } else { Dot::Idle };
        let body = column![
            row![
                dot_mark(dot),
                text(ui::ellipsize(c["title"].as_str().unwrap_or(&id), 34))
                    .font(Font {
                        weight: Weight::Medium,
                        ..theme::SANS
                    })
                    .size(13.0)
                    .color(theme::palette().ink)
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            ui::faint(meta).size(11.5),
        ]
        .spacing(2);
        let selected = open == Some(id.as_str());
        rows.push(
            button(body)
                .on_press(Msg::OpenConversation(id))
                .width(Length::Fill)
                .padding([5, 8])
                .style(theme::register_row(selected))
                .into(),
        );
    }
    rows
}

/// An agent's last finished work, for its status dot: the transcript's last outcome for the agent on screen (the
/// fresher of the two), else the outcome of its last finished task on the roster. Every agent gets its dot this way.
fn last_of(st: &State, cid: u64, selected: Option<u64>) -> Option<String> {
    let on_screen = (selected == Some(cid))
        .then(|| ide_model::last_outcome(&st.panels.entries))
        .flatten();
    on_screen.or_else(|| ide_model::last_task_outcome(&st.tasks, cid))
}

/// A group's header: its disclosure, its title and its count. A press folds the group.
fn group_head(st: &State, group: &Group) -> Element<'static, Msg> {
    let open = !st.collapsed.contains(&group.key);
    let marker = if open { "▾" } else { "▸" };
    let title = text(group.title.clone())
        .font(Font {
            weight: Weight::Semibold,
            ..theme::SANS
        })
        .size(14.0)
        .color(theme::palette().ink);
    button(
        row![
            ui::mono(marker, 11.0, theme::palette().ink_3),
            title,
            Space::new().width(Length::Fill),
            ui::mono(group.count().to_string(), 11.0, theme::palette().ink_2),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .on_press(Msg::ToggleGroup(group.key.clone()))
    .width(Length::Fill)
    .padding([4, 6])
    .style(theme::register_row(false))
    .into()
}

/// The status dot: a filled accent circle while working, a hollow one when idle, and a glyph for the other states.
pub fn dot_mark<'a>(dot: Dot) -> Element<'a, Msg> {
    let p = theme::palette();
    match dot {
        Dot::Sleeping => ui::icon("moon", 11.0, p.ink_3),
        Dot::Done => ui::icon("check", 11.0, p.ok),
        Dot::Failed => ui::icon("alert", 11.0, p.crit),
        Dot::Working | Dot::Idle => {
            let (fill, edge) = if dot == Dot::Working {
                (Some(p.signal), p.signal)
            } else {
                (None, p.ink_2)
            };
            container(Space::new())
                .width(Length::Fixed(DOT))
                .height(Length::Fixed(DOT))
                .style(move |_| container::Style {
                    background: fill.map(Into::into),
                    border: iced::Border {
                        color: edge,
                        width: 1.5,
                        radius: (DOT / 2.0).into(),
                    },
                    ..Default::default()
                })
                .into()
        }
    }
}

/// A name with a rule through its middle (iced has no strikethrough): a finished agent's name.
fn struck(name: String) -> Element<'static, Msg> {
    // The rule is one pixel tall, centred on the name: only the line takes the colour, not the whole stacked row.
    let line = container(Space::new().width(Length::Fill).height(Length::Fixed(1.0))).style(|_| {
        container::Style {
            background: Some(theme::palette().ink_3.into()),
            ..Default::default()
        }
    });
    let rule = container(line)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_y(Alignment::Center);
    Stack::new()
        .push(
            text(name)
                .font(Font {
                    weight: Weight::Medium,
                    ..theme::SANS
                })
                .size(14.0)
                .color(theme::palette().ink_3),
        )
        .push(rule)
        .into()
}

/// One agent: its dot, name, unread count, and its current task in italics. The row opens the agent's workspace
/// when it has one; an agent with no VM has no workspace to open. The hover card says what it is.
fn agent_row(
    st: &State,
    agent: &AgentRow,
    last: Option<&str>,
    unread: usize,
) -> Element<'static, Msg> {
    let dot = ide_model::dot(&agent.status, last);
    let id = apz_ide::WorkspaceId::Agent(agent.cid).to_string();
    let listed = st.workspaces.iter().find(|w| w.id == id).map(ide::choice);
    let selected = st
        .ide
        .workspace
        .as_ref()
        .is_some_and(|ws| ws.to_string() == id);
    let name: Element<'static, Msg> = if dot == Dot::Done {
        struck(agent.name.clone())
    } else {
        text(agent.name.clone())
            .font(Font {
                weight: Weight::Medium,
                ..theme::SANS
            })
            .size(14.0)
            .color(theme::palette().ink)
            .into()
    };
    let mut top = row![dot_mark(dot), name]
        .spacing(8)
        .align_y(Alignment::Center);
    top = top.push(Space::new().width(Length::Fill));
    if unread > 0 {
        top = top.push(
            container(ui::mono(
                unread.to_string(),
                11.0,
                theme::palette().on_signal,
            ))
            .padding([0, 5])
            .style(|_| container::Style {
                background: Some(theme::palette().signal.into()),
                border: iced::Border {
                    radius: 6.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
        );
    }
    let mut col = column![top].spacing(2);
    if let Some(task) = agent.task.as_deref() {
        let italic = Font {
            style: FontStyle::Italic,
            ..theme::SANS
        };
        col = col.push(
            text(ui::ellipsize(task, 44))
                .font(italic)
                .size(12.0)
                .color(theme::palette().ink_2),
        );
    }
    let body = button(col)
        .width(Length::Fill)
        .padding([6, 8])
        .style(theme::register_row(selected))
        .on_press_maybe(listed.map(Msg::Pick));
    tooltip(body, hover_card(agent, last), tooltip::Position::Right).into()
}

/// A repo workspace's row: its name and branch, opened on press.
fn repo_row(st: &State, w: &super::ide::WorkspaceInfo) -> Element<'static, Msg> {
    let selected = st
        .ide
        .workspace
        .as_ref()
        .is_some_and(|ws| ws.to_string() == w.id);
    let label = ide::choice(w).label;
    let short = label
        .split(" · ")
        .nth(1)
        .unwrap_or(label.as_str())
        .to_string();
    button(
        row![
            ui::icon("git", 13.0, theme::palette().ink_2),
            ui::mono(short, 12.5, theme::palette().ink)
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([6, 8])
    .style(theme::register_row(selected))
    .on_press(Msg::Pick(ide::choice(w)))
    .into()
}

/// A workstation's row: a machine glyph and its name, its OS, and a warning when its agents act as the owner. Opened on
/// press. The hub lists only online workstations, so a row is always online.
fn station_row(st: &State, w: &super::ide::WorkspaceInfo) -> Element<'static, Msg> {
    let p = theme::palette();
    let selected = st
        .ide
        .workspace
        .as_ref()
        .is_some_and(|ws| ws.to_string() == w.id);
    let name = if w.name.is_empty() {
        w.owner.clone()
    } else {
        w.name.clone()
    };
    let warn = st
        .stations
        .iter()
        .find(|s| s.id == w.owner)
        .and_then(|s| s.owner_account.clone());
    let mut lines = column![
        row![
            ui::icon("machine", 13.0, p.ink_2),
            ui::mono(name, 12.5, p.ink)
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        ui::faint(ide_model::os_label(w.os.as_deref())),
    ]
    .spacing(2);
    if let Some(account) = warn {
        lines = lines.push(ui::mono(format!("agents act as {account}"), 11.0, p.warn));
    }
    button(lines)
        .width(Length::Fill)
        .padding([6, 8])
        .style(theme::register_row(selected))
        .on_press(Msg::Pick(ide::choice(w)))
        .into()
}

/// The hover card: the agent's role, model, what it is doing, when it was last seen, and what it has spent.
fn hover_card(agent: &AgentRow, last: Option<&str>) -> Element<'static, Msg> {
    let p = theme::palette();
    let model = if agent.model.is_empty() {
        "default"
    } else {
        agent.model.as_str()
    };
    let now = match agent.task.as_deref() {
        Some(task) => ui::ellipsize(task, 90),
        None => "nothing right now".to_string(),
    };
    let outcome = last.map(|o| format!("Last work: {o}")).unwrap_or_default();
    let body = column![
        ui::mono(agent.name.clone(), 12.5, p.ink),
        ui::faint(format!("Role: {}", agent.role_label)),
        ui::faint(format!("Model: {model}")),
        ui::faint(format!("Now: {now}")),
        ui::faint(format!(
            "Last activity: {}",
            ide_model::ago(ide::now_ms(), agent.last_seen_ms)
        )),
        ui::faint(format!("Spent: {} tokens", agent.tokens)),
        ui::faint(outcome),
    ]
    .spacing(4)
    .width(Length::Fixed(260.0));
    container(body)
        .padding(10)
        .style(ui::frame(Some(p.sheet), p.rule))
        .into()
}

// ---- The messages the IDE hands to this area (moved from `ide.rs`'s `update`) ----------------------

/// Whether a message is this area's: `update` below takes it, and `ide.rs` hands it over.
pub(super) fn handles(msg: &Msg) -> bool {
    matches!(
        msg,
        Msg::Pick(..)
            | Msg::SidebarQuery(..)
            | Msg::ToggleGroup(..)
            | Msg::NewAgent
            | Msg::Roster(..)
    )
}

/// The area's messages: the state they change, and the tasks they start. See `handles`.
pub(super) fn update(st: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Pick(pick) => match WorkspaceId::from_str(&pick.id) {
            Ok(id) => select(st, id, ctx),
            Err(e) => {
                st.notice = Some(e.to_string());
                Task::none()
            }
        },
        Msg::SidebarQuery(query) => {
            st.sidebar_query = query;
            Task::none()
        }
        Msg::ToggleGroup(key) => {
            if !st.collapsed.remove(&key) {
                st.collapsed.insert(key);
            }
            Task::none()
        }
        Msg::NewAgent => Task::none(),
        Msg::Roster(res) => {
            match res {
                Ok(roster) => {
                    st.roster = roster.agents;
                    st.projects = roster.projects;
                    st.tasks = roster.tasks;
                    st.stations = roster.stations;
                    st.roster_loaded = true;
                    st.roster_error = None;
                    // The board names its authors from this roster, so a post reads by name, not by cid.
                    st.board.adopt_names(st.roster.iter().map(|a| {
                        let name = a.name.trim();
                        (
                            a.cid,
                            if name.is_empty() {
                                "Unnamed".to_string()
                            } else {
                                name.to_string()
                            },
                        )
                    }));
                }
                Err(e) => st.roster_error = Some(e),
            }
            Task::none()
        }
        _ => Task::none(),
    }
}
