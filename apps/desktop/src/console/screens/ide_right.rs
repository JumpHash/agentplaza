//! The IDE's right panel: five tabs, each with its count. Board (the project's posts, or all of them, with the owner's
//! pause, export and post box), Files (the tree, the changed filter and the checkpoints), Tasks (done of total for the
//! project), Approvals (what is held for the agent), and Activity (thinking against tools, and each tool call with its
//! time).

// The IDE's own helpers and state, for the messages this area takes (moved from `ide.rs`).
use super::ide::*;
use super::{ide_chat, ide_chrome};
use apz_ide::state::Panel;
use crate::console::Ctx;
use iced::Task;
use iced::widget::text::Wrapping;
use iced::widget::{Space, button, column, container, row, text, tooltip};
use iced::{Alignment, Element, Length, Padding};

use super::board;
use super::ide::{self, Msg, RightTab, State};
use super::ide_files;
use super::ide_model::{self, Activity, ProjectRow};
use super::ide_panels;
use crate::console::ui::{self, Tone};
use crate::theme;

/// The tab row's gap between tabs, and the gaps inside one tab: its icon, its label, its badge.
const TAB_GAP: f32 = 8.0;
const ICON: f32 = 13.0;
const ICON_GAP: f32 = 5.0;
const BADGE_GAP: f32 = 5.0;
/// Estimated advances at the tab's sizes: a label character (Inter 13), a badge digit (Inter 11, semibold), and the
/// badge's padding. They are generous, so a row that is judged to fit does not wrap.
const LABEL_CHAR: f32 = 7.6;
const BADGE_CHAR: f32 = 6.8;
const BADGE_PAD: f32 = 10.0;
/// A tab's own padding (the cell's 2px on each side), and the panel's padding around the tab row (left and right).
const TAB_PAD: f32 = 4.0;
const PANEL_PAD: f32 = 22.0;

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

/// One right tab as the row draws it: its icon, its name, and its count (a badge, when it has one).
struct TabInfo {
    tab: RightTab,
    icon: &'static str,
    label: &'static str,
    count: Option<String>,
}

impl TabInfo {
    /// The tab's width in its labelled form: icon, label and badge on one line, with the gaps between them.
    fn labelled_width(&self) -> f32 {
        let badge = self
            .count
            .as_deref()
            .map_or(0.0, |c| BADGE_GAP + badge_width(c));
        ICON + ICON_GAP + self.label.chars().count() as f32 * LABEL_CHAR + badge + TAB_PAD
    }
}

/// How the right tabs draw: each with its label, or each as icon and badge with the label as its tooltip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabForm {
    Labelled,
    Compact,
}

/// Chooses the tab row's form: labelled when every tab fits in `available` px (their widths and the gaps between
/// them), else compact for all of them. One form for the whole row, so the tabs never wrap or change shape apart.
pub fn tab_form(widths: &[f32], gap: f32, available: f32) -> TabForm {
    let gaps = gap * widths.len().saturating_sub(1) as f32;
    if widths.iter().sum::<f32>() + gaps <= available {
        TabForm::Labelled
    } else {
        TabForm::Compact
    }
}

/// A count badge's width: its characters, and its padding.
fn badge_width(count: &str) -> f32 {
    count.chars().count() as f32 * BADGE_CHAR + BADGE_PAD
}

/// The tabs in tab order, each with its icon, its name and its count.
fn tab_infos(st: &State) -> Vec<TabInfo> {
    let (_, tasks) = scope(st);
    let done = tasks.iter().filter(|t| t.state == "done").count();
    let changed = ide_files::changed_count(st.changes.as_ref());
    let posts = board::count(&st.board, in_scope(st));
    vec![
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
    ]
}

/// The right panel: the tabs, then the shown tab. `width` is the panel's width, which decides the tabs' form.
pub fn view(st: &State, width: f32) -> Element<'_, Msg> {
    let tabs = tab_row(st, width);
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
    column![tabs, body].spacing(12).height(Length::Fill).into()
}

/// The tab row: icon, label and count on one line, with an accent underline under the tab shown. Labelled when the
/// row fits the panel (see `tab_form`); else icon and count only, with the label as the tooltip.
fn tab_row(st: &State, width: f32) -> Element<'_, Msg> {
    let p = theme::palette();
    let infos = tab_infos(st);
    let widths: Vec<f32> = infos.iter().map(TabInfo::labelled_width).collect();
    let form = tab_form(&widths, TAB_GAP, width - PANEL_PAD);
    let mut strip = row![].spacing(TAB_GAP).align_y(Alignment::Center);
    for info in infos {
        let on = info.tab == st.right;
        let ink = if on { p.ink } else { p.ink_3 };
        let mut line = row![ui::icon(info.icon, ICON, ink)]
            .spacing(ICON_GAP)
            .align_y(Alignment::Center);
        if form == TabForm::Labelled {
            line = line.push(
                text(info.label)
                    .font(if on {
                        theme::SANS_SEMIBOLD
                    } else {
                        theme::SANS_MEDIUM
                    })
                    .size(theme::TYPE_UI)
                    .wrapping(Wrapping::None)
                    .color(ink),
            );
        }
        if let Some(count) = info.count.clone() {
            line = line.push(count_badge(count, on));
        }
        let underline = container(Space::new().width(Length::Fill).height(Length::Fixed(2.0)))
            .style(move |_| container::Style {
                background: if on { Some(p.signal.into()) } else { None },
                ..Default::default()
            });
        let cell = button(column![line, underline].spacing(4))
            .padding([4, 2])
            .style(theme::register_row(false))
            .on_press(Msg::Right(info.tab));
        let item: Element<'_, Msg> = match form {
            TabForm::Labelled => cell.into(),
            TabForm::Compact => {
                tooltip(cell, tab_tooltip(info.label), tooltip::Position::Bottom).into()
            }
        };
        strip = strip.push(item);
    }
    strip.into()
}

/// A tab's count, in a small bordered badge that does not wrap.
fn count_badge(count: String, on: bool) -> Element<'static, Msg> {
    let p = theme::palette();
    container(
        text(count)
            .font(theme::SANS_SEMIBOLD)
            .size(theme::TYPE_BADGE)
            .wrapping(Wrapping::None)
            .color(if on { p.ink } else { p.ink_2 }),
    )
    .padding([0, 4])
    .style(move |_| container::Style {
        border: iced::Border {
            color: p.rule,
            width: 1.0,
            radius: 3.0.into(),
        },
        ..Default::default()
    })
    .into()
}

/// The tooltip of a compact tab: its name.
fn tab_tooltip(label: &'static str) -> Element<'static, Msg> {
    let p = theme::palette();
    container(
        text(label)
            .font(theme::SANS_MEDIUM)
            .size(theme::TYPE_META)
            .wrapping(Wrapping::None)
            .color(p.ink),
    )
    .padding([4, 8])
    .style(ui::frame(Some(p.sheet), p.rule))
    .into()
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
        container(composer)
            .width(Length::Fill)
            .padding(Padding {
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
    matches!(msg, Msg::Right(..) | Msg::Board(..) | Msg::BoardScope(..) | Msg::Panels(..))
}

/// The area's messages: the state they change, and the tasks they start. See `handles`.
pub(super) fn update(st: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
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

    #[test]
    fn the_tabs_are_labelled_when_the_row_fits_and_compact_when_it_does_not() {
        let labelled = [90.0, 60.0, 100.0];
        assert_eq!(tab_form(&labelled, 8.0, 900.0), TabForm::Labelled);
        assert_eq!(
            tab_form(&labelled, 8.0, 40.0),
            TabForm::Compact,
            "one form for the whole row"
        );
        // The gaps count: 250 of tabs plus two gaps of 8 is 266.
        assert_eq!(tab_form(&labelled, 8.0, 266.0), TabForm::Labelled);
        assert_eq!(tab_form(&labelled, 8.0, 265.0), TabForm::Compact);
        assert_eq!(tab_form(&[], 8.0, 0.0), TabForm::Labelled, "no tabs fit anywhere");
    }

    #[test]
    fn a_tab_is_as_wide_as_its_icon_label_and_badge_on_one_line() {
        let with = TabInfo {
            tab: RightTab::Approvals,
            icon: "approval",
            label: "Approvals",
            count: Some("0".into()),
        };
        let without = TabInfo {
            count: None,
            ..with
        };
        assert!(with.labelled_width() > without.labelled_width());
        assert!(with.labelled_width() > 9.0 * LABEL_CHAR, "the label is counted in full");
        assert!(badge_width("2/4") > badge_width("2"), "a longer count takes more room");
    }
}
