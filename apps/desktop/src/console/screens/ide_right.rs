//! The IDE's right panel: five tabs, each with its count. Board (the project's posts, or all of them, with the owner's
//! pause, export and post box), Files (the tree, the changed filter and the checkpoints), Tasks (done of total for the
//! project), Approvals (what is held for the agent), and Activity (thinking against tools, and each tool call with its
//! time).

// The IDE's own helpers and state, for the messages this area takes (moved from `ide.rs`).
use super::ide::*;
use super::{ide_chat, ide_chrome};
use crate::console::Ctx;
use apz_ide::state::Panel;
use iced::Task;
use iced::widget::{Space, column, container, row, text};
use iced::{Alignment, Element, Length, Padding};

use super::board;
use super::ide::{self, Msg, RightTab, State};
use super::ide_files;
use super::ide_model::{self, Activity, ProjectRow};
use super::ide_panels;
use crate::console::ui::{self, Tone};
use crate::theme;

/// The project the agent on screen belongs to, the tasks that count for the tabs, and whether a post is in scope.
fn scope(st: &State) -> (Option<&ProjectRow>, Vec<&ide_model::TaskRow>) {
    let project = ide::selected_project(st);
    let tasks = match project {
        Some(project) => project
            .tasks
            .iter()
            .filter_map(|id| st.tasks.iter().find(|t| &t.id == id))
            .collect(),
        None => st.tasks.iter().collect(),
    };
    (project, tasks)
}

/// Whether a board post (its author and task) is in the Board tab's scope: the project's, or every post.
fn in_scope(st: &State) -> impl Fn(u64, Option<&str>) -> bool + '_ {
    let project = ide::selected_project(st);
    let scoped = st.board_project && project.is_some();
    move |cid, task| {
        if !scoped {
            return true;
        }
        let Some(project) = project else { return true };
        // Its agents: the ones working on it, and the ones that worked on it before membership.
        st.roster
            .iter()
            .any(|a| a.cid == cid && a.project.as_deref() == Some(project.id.as_str()))
            || project.members.contains(&cid)
            || task.is_some_and(|t| project.tasks.iter().any(|id| id == t))
    }
}

/// One right tab as the row draws it: its icon, its name, and its count (a badge, when it has one).
struct TabInfo {
    tab: RightTab,
    icon: &'static str,
    label: &'static str,
    count: Option<String>,
}

/// The tabs in tab order, each with its icon, its name and its count: the ones the workspace has (see `right_tabs`).
fn tab_infos(st: &State) -> Vec<TabInfo> {
    let (_, tasks) = scope(st);
    let done = tasks.iter().filter(|t| t.state == "done").count();
    let changed = ide_files::changed_count(st.changes.as_ref());
    let posts = board::count(&st.board, in_scope(st));
    let shown = right_tabs(st.ide.workspace.as_ref());
    let all = vec![
        TabInfo {
            tab: RightTab::Board,
            icon: "board",
            label: "Board",
            count: Some(posts.to_string()),
        },
        TabInfo {
            tab: RightTab::Files,
            icon: "file",
            label: "Files",
            count: (changed > 0).then(|| changed.to_string()),
        },
        TabInfo {
            tab: RightTab::Tasks,
            icon: "tasks",
            label: "Tasks",
            count: Some(format!("{done}/{}", tasks.len())),
        },
        TabInfo {
            tab: RightTab::Approvals,
            icon: "approval",
            label: "Approvals",
            count: Some(st.panels.held.len().to_string()),
        },
        TabInfo {
            tab: RightTab::Activity,
            icon: "clock",
            label: "Activity",
            count: Some(
                ide_model::activity(&st.panels.entries)
                    .calls
                    .len()
                    .to_string(),
            ),
        },
    ];
    all.into_iter()
        .filter(|info| shown.contains(&info.tab))
        .collect()
}

/// The right panel (the web's `.ide-dock`): the tabs, then the shown tab in the panel's padding.
pub fn view(st: &State) -> Element<'_, Msg> {
    let tabs = tab_row(st);
    let agent = matches!(st.ide.workspace, Some(apz_ide::WorkspaceId::Agent(_)));
    let body: Element<'_, Msg> = match st.right {
        RightTab::Board => board_tab(st),
        RightTab::Files => ide_files::files_tab(st),
        RightTab::Tasks => tasks_tab(st),
        RightTab::Approvals => ide_panels::approvals_view(&st.panels, agent).map(Msg::Panels),
        RightTab::Activity => activity_tab(st),
    };
    // The board's posts and the tree and checkpoints size themselves; the other tabs scroll.
    let body: Element<'_, Msg> = match st.right {
        RightTab::Board | RightTab::Files => body,
        _ => ui::scroll(body).height(Length::Fill).into(),
    };
    let body = container(body)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(Padding {
            top: 12.0,
            right: 14.0,
            bottom: 12.0,
            left: 14.0,
        });
    column![tabs, body].height(Length::Fill).into()
}

/// The dock's tab order (the web's): files, tasks, board, approvals, activity.
fn dock_rank(tab: RightTab) -> u8 {
    match tab {
        RightTab::Files => 0,
        RightTab::Tasks => 1,
        RightTab::Board => 2,
        RightTab::Approvals => 3,
        RightTab::Activity => 4,
    }
}

/// The tab row (the web's `.ide-dtabs`): icon and count on each tab, the label on the one shown, a rule under the row.
fn tab_row(st: &State) -> Element<'_, Msg> {
    let mut infos = tab_infos(st);
    infos.sort_by_key(|info| dock_rank(info.tab));
    let mut strip = row![].spacing(2).align_y(Alignment::Center);
    for info in infos {
        let on = info.tab == st.right;
        strip = strip.push(ide_chrome::dock_tab(
            info.icon,
            info.label,
            info.count,
            on,
            Msg::Right(info.tab),
        ));
    }
    let bar = container(strip)
        .width(Length::Fill)
        .padding([0, 8])
        .center_y(Length::Fixed(44.0));
    column![bar, ide_chrome::rule_strip()].into()
}

/// The Board tab: the scope, and the board's pause and export, on top; the project's posts (or all of them) in the
/// middle; the owner's post box at the foot, as the board screen has it.
fn board_tab(st: &State) -> Element<'_, Msg> {
    let scope = ui::seg(
        &[(true, "Project"), (false, "All")],
        st.board_project,
        Msg::BoardScope,
    );
    let head = row![
        scope,
        Space::new().width(Length::Fill),
        board::pause_bar(st.board.paused()).map(Msg::Board)
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let mut posts = column![].spacing(10).width(Length::Fill);
    if st.board.paused() {
        posts = posts.push(board::paused_banner().map(Msg::Board));
    }
    posts = posts.push(board::panel(&st.board, in_scope(st)).map(Msg::Board));
    let composer =
        board::composer_view(st.board.composer(), st.board.mentionable()).map(Msg::Board);
    column![
        head,
        ui::scroll(posts).height(Length::Fill),
        container(composer).width(Length::Fill).padding(Padding {
            top: 8.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        }),
    ]
    .spacing(10)
    .height(Length::Fill)
    .into()
}

/// The Tasks tab: the project's tasks (or all of them, with no project), done of total in the tab.
fn tasks_tab(st: &State) -> Element<'_, Msg> {
    let p = theme::palette();
    let (project, tasks) = scope(st);
    let done = tasks.iter().filter(|t| t.state == "done").count();
    let mut col = column![ui::mono(
        match project {
            Some(project) => format!("{} · {done} of {} done", project.title, tasks.len()),
            None => format!("All tasks · {done} of {} done", tasks.len()),
        },
        11.5,
        p.ink_2
    )]
    .spacing(8);
    if tasks.is_empty() {
        col = col.push(ui::faint("No tasks here yet."));
    }
    for task in tasks {
        let tone = match task.state.as_str() {
            "done" => Tone::Ok,
            "failed" | "error" => Tone::Danger,
            _ => Tone::Muted,
        };
        col = col.push(
            column![
                row![
                    ui::badge(&task.state, tone),
                    text(ui::ellipsize(&task.id, 14))
                        .font(theme::MONO)
                        .size(11.0)
                        .color(p.ink_3)
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                text(ui::ellipsize(&task.goal, 160))
                    .font(theme::SANS)
                    .size(14.0)
                    .color(p.ink),
            ]
            .spacing(4),
        );
    }
    col.into()
}

/// The Activity tab: how the time split between the model's turns and the tools, then each tool call and its time.
fn activity_tab(st: &State) -> Element<'_, Msg> {
    let p = theme::palette();
    let activity: Activity = ide_model::activity(&st.panels.entries);
    let total = (activity.thinking_ms + activity.tools_ms).max(1);
    let think = portion(activity.thinking_ms, total);
    let tools = portion(activity.tools_ms, total);
    let split = row![
        container(Space::new().height(Length::Fixed(8.0)))
            .width(Length::FillPortion(think))
            .style(move |_| container::Style {
                background: Some(p.info.into()),
                ..Default::default()
            }),
        container(Space::new().height(Length::Fixed(8.0)))
            .width(Length::FillPortion(tools))
            .style(move |_| container::Style {
                background: Some(p.signal.into()),
                ..Default::default()
            }),
    ]
    .spacing(2);
    let mut col = column![
        split,
        row![
            ui::mono(
                format!("Thinking {}", fmt_ms(activity.thinking_ms)),
                11.5,
                p.info
            ),
            Space::new().width(Length::Fill),
            ui::mono(
                format!("Tools {}", fmt_ms(activity.tools_ms)),
                11.5,
                p.signal
            ),
        ],
    ]
    .spacing(8);
    if activity.calls.is_empty() {
        col = col.push(ui::faint("No tool calls yet."));
    }
    for call in activity.calls.iter().rev().take(200) {
        col = col.push(
            row![
                ui::mono(call.name.clone(), 12.0, p.ink),
                Space::new().width(Length::Fill),
                ui::mono(fmt_ms(call.ms), 11.5, p.ink_2),
            ]
            .align_y(Alignment::Center),
        );
    }
    col.into()
}

/// A share of the total as a flex portion (at least one, so an empty side still shows its edge).
fn portion(part: u64, total: u64) -> u16 {
    ((part * 1000 / total).max(1)).min(u64::from(u16::MAX)) as u16
}

/// A duration as the tab reads it: milliseconds under a second, else seconds.
pub fn fmt_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else {
        format!("{:.1} s", ms as f64 / 1000.0)
    }
}

// ---- The messages the IDE hands to this area (moved from `ide.rs`'s `update`) ----------------------

/// Whether a message is this area's: `update` below takes it, and `ide.rs` hands it over.
pub(super) fn handles(msg: &Msg) -> bool {
    matches!(
        msg,
        Msg::Right(..) | Msg::Board(..) | Msg::BoardScope(..) | Msg::Panels(..)
    )
}

/// The area's messages: the state they change, and the tasks they start. See `handles`.
pub(super) fn update(st: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        // A tab the workspace does not have (a workstation's board, approvals or activity) is not shown.
        Msg::Right(tab) if !right_tabs(st.ide.workspace.as_ref()).contains(&tab) => Task::none(),
        Msg::Right(tab) => {
            st.right = tab;
            st.layout.right_open = true;
            ide_chrome::save(&st.layout);
            let Some(ws) = st.ide.workspace.clone() else {
                return if tab == RightTab::Board {
                    load_board(st, ctx)
                } else {
                    Task::none()
                };
            };
            match tab {
                RightTab::Board => load_board(st, ctx),
                RightTab::Approvals => {
                    ide_panels::show(&mut st.panels, Panel::Approvals, &ctx.client, &ws)
                        .map(Msg::Panels)
                }
                RightTab::Files => {
                    if st.tree.loaded.is_empty() {
                        list_dir(st, ctx, String::new(), None)
                    } else {
                        Task::none()
                    }
                }
                RightTab::Tasks | RightTab::Activity => Task::none(),
            }
        }
        Msg::Board(msg) => board::update(&mut st.board, msg, ctx).map(Msg::Board),
        Msg::BoardScope(project) => {
            st.board_project = project;
            Task::none()
        }
        Msg::Panels(ide_panels::Msg::Polled { .. }) if st.away => {
            st.poll_inflight = false;
            Task::none()
        }
        Msg::Panels(msg) => {
            st.panels.agent_working =
                ide::selected_agent(st).is_some_and(|a| a.status == "working");
            let answers_run = matches!(
                msg,
                ide_panels::Msg::Started { .. } | ide_panels::Msg::Polled { .. }
            );
            let transcript = matches!(msg, ide_panels::Msg::Transcript { .. });
            let task = match st.ide.workspace.clone() {
                Some(ws) => {
                    ide_panels::update(&mut st.panels, msg, &ctx.client, &ws).map(Msg::Panels)
                }
                None => Task::none(),
            };
            if answers_run {
                // The run's chain goes on from here: ide_panels asks for the next poll while the run is active.
                st.poll_inflight = st.panels.run.active() && st.panels.run.run.is_some();
            }
            if transcript {
                // A transcript that landed names its references: ask the hub for the ones it does not hold yet. The
                // chat follows its newest entry, or counts what came for the pill when the reader is above them.
                let follow = ide_chat::transcript_landed(st);
                return Task::batch([task, prefetch_transcript(st, ctx), follow]);
            }
            task
        }
        _ => Task::none(),
    }
}
