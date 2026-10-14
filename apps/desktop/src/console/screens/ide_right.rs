//! The IDE's right panel: five tabs, each with its count. Board (the project's posts, or all of them), Files (the
//! tree, the changed filter and the checkpoints), Tasks (done of total for the project), Approvals (what is held
//! for the agent), and Activity (thinking against tools, and each tool call with its time).

use iced::widget::{Space, button, column, container, row, text};
use iced::{Alignment, Element, Font, Length};

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
        project.members.contains(&cid)
            || task.is_some_and(|t| project.tasks.iter().any(|id| id == t))
    }
}

/// The tab labels with their counts, in tab order.
fn labels(st: &State) -> Vec<(RightTab, String)> {
    let (_, tasks) = scope(st);
    let done = tasks.iter().filter(|t| t.state == "done").count();
    let changed = ide_files::changed_count(st.changes.as_ref());
    let posts = board::count(&st.board, in_scope(st));
    vec![
        (RightTab::Board, format!("Board {posts}")),
        (
            RightTab::Files,
            if changed > 0 {
                format!("Files {changed}")
            } else {
                "Files".to_string()
            },
        ),
        (RightTab::Tasks, format!("Tasks {done}/{}", tasks.len())),
        (
            RightTab::Approvals,
            format!("Approvals {}", st.panels.held.len()),
        ),
        (
            RightTab::Activity,
            format!(
                "Activity {}",
                ide_model::activity(&st.panels.entries).calls.len()
            ),
        ),
    ]
}

/// The right panel: the tabs, then the shown tab.
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
    // The tree and the checkpoints size themselves; the other tabs scroll.
    let body: Element<'_, Msg> = match st.right {
        RightTab::Files => body,
        _ => ui::scroll(body).height(Length::Fill).into(),
    };
    column![tabs, body].spacing(12).height(Length::Fill).into()
}

/// The tab row: text tabs with an accent underline on the one shown, each with its count. Tighter than the shared
/// tab strip, so five tabs with counts fit a panel of the default width.
fn tab_row(st: &State) -> Element<'_, Msg> {
    let p = theme::palette();
    let mut strip = row![].spacing(12).align_y(Alignment::Center);
    for (tab, label) in labels(st) {
        let on = tab == st.right;
        let ink = if on { p.ink } else { p.ink_3 };
        let underline = container(Space::new().width(Length::Fill).height(Length::Fixed(2.0)))
            .style(move |_| container::Style {
                background: if on { Some(p.signal.into()) } else { None },
                ..Default::default()
            });
        let cell = column![ui::mono(label, 11.5, ink), underline].spacing(4);
        strip = strip.push(
            button(cell)
                .padding([4, 2])
                .style(theme::register_row(false))
                .on_press(Msg::Right(tab)),
        );
    }
    strip.into()
}

/// The Board tab: the project's posts or all of them, with the author colour bar, mentions and the reply composer.
fn board_tab(st: &State) -> Element<'_, Msg> {
    let scope = ui::seg(
        &[(true, "Project"), (false, "All")],
        st.board_project,
        Msg::BoardScope,
    );
    let posts = board::panel(&st.board, in_scope(st)).map(Msg::Board);
    column![
        row![Space::new().width(Length::Fill), scope].align_y(Alignment::Center),
        posts
    ]
    .spacing(10)
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
                    .font(Font { ..theme::SANS })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_duration_reads_in_milliseconds_then_seconds() {
        assert_eq!(fmt_ms(420), "420 ms");
        assert_eq!(fmt_ms(2_500), "2.5 s");
    }

    #[test]
    fn a_share_is_at_least_one_portion_and_never_more_than_the_whole() {
        assert_eq!(portion(0, 100), 1);
        assert_eq!(portion(100, 100), 1000);
        assert_eq!(portion(50, 100), 500);
    }
}
