//! The IDE's files, placed in the new layout: the Files tab of the right panel (the tree with its changed-only
//! filter and refresh, the search when one is open, and the checkpoints), the editor and the diff in the centre.
//! The tree, search, checkpoint and diff pieces are the ones the IDE has always had; this module places them.

use apz_ide::state::Base;
use apz_ide::{ChangeSet, FileChange};
use iced::widget::{Space, button, column, container, row, text_input};
use iced::{Alignment, Element, Length};

use super::ide::{self, BaseChoice, Checkpoint, Msg, QUICK_ID, SEARCH_ID, Search, State};
use super::ide_diff;
use super::ide_editor;
use super::ide_tree;
use crate::console::ui::{self, Kind};
use crate::theme;

/// The Files tab: the tree (or the search, when one is open) above the checkpoints. The tree takes most of the
/// height; the checkpoints keep the rest.
pub fn files_tab(st: &State) -> Element<'_, Msg> {
    let on_workspace = st.ide.workspace.is_some();
    let filter = st.ide.filter_changed;
    let head = row![
        ui::mono("Files", 13.0, theme::palette().ink),
        Space::new().width(Length::Fill),
        ui::btn_sm(
            "Changed",
            None,
            if filter {
                Kind::Primary
            } else {
                Kind::Secondary
            },
            on_workspace.then_some(Msg::ChangedOnly)
        ),
        ui::btn_sm(
            "",
            Some("search"),
            Kind::Ghost,
            on_workspace.then_some(Msg::ToggleSearch)
        ),
        ui::btn_sm(
            "",
            Some("refresh"),
            Kind::Ghost,
            on_workspace.then_some(Msg::RefreshAll)
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let main: Element<'_, Msg> = match (&st.search, filter, &st.changes) {
        (Some(search), _, _) => search_panel(search),
        (None, true, Some(changes)) => ui::scroll(ide_tree::view(
            ide_tree::rows(&ide_tree::changed(&st.tree, changes), true),
            st.ide.active.clone(),
        ))
        .into(),
        (None, _, _) => ui::scroll(ide_tree::view(
            ide_tree::rows(&st.tree, false),
            st.ide.active.clone(),
        ))
        .into(),
    };
    let tree = container(main).height(Length::FillPortion(3));
    let checkpoints = column![
        ui::label("Checkpoints"),
        ui::scroll(checkpoints_panel(st)).height(Length::Fill)
    ]
    .spacing(8)
    .height(Length::FillPortion(2));
    column![head, tree, checkpoints]
        .spacing(10)
        .height(Length::Fill)
        .into()
}

/// The editor in the centre: the open tabs and the active file, or a note when no file is open. A quick open
/// (Ctrl+P) takes the centre while it is up.
pub fn editor_centre(st: &State) -> Element<'_, Msg> {
    if let Some(query) = st.quick.as_deref() {
        return ide_editor::quick_open(query, ide::quick_hits(st), QUICK_ID.clone());
    }
    if let Some(path) = st.ide.active.as_deref()
        && let Some(tab) = st.tabs.get(path)
    {
        return column![
            ide_editor::tab_strip(&st.ide.open, st.ide.active.as_deref(), &st.tabs),
            ide_editor::editor(path, tab, ide::writable(st)),
        ]
        .spacing(8)
        .height(Length::Fill)
        .into();
    }
    ui::empty(
        "file",
        "No file open",
        "Pick a file in the Files tab on the right, or press Ctrl+P to find one by name.",
    )
}

/// The diff in the centre: the changed files since the base, with the base choice, on the left, and the file
/// on the right (or the whole review when no file is picked).
pub fn diff_centre(st: &State) -> Element<'_, Msg> {
    let base = match st.ide.base {
        Base::Checkpoint(_) => BaseChoice::Checkpoint,
        _ => BaseChoice::WorkStart,
    };
    let seg = ui::seg(
        &[
            (BaseChoice::WorkStart, "Work start"),
            (BaseChoice::Checkpoint, "Checkpoint"),
        ],
        base,
        Msg::PickBase,
    );
    let mut side = column![ui::label("Changed since"), seg].spacing(10);
    if let Some(error) = &st.changes_error {
        side = side.push(ui::error_line(error));
    }
    let Some(changes) = st.changes.as_ref() else {
        if st.changes_error.is_none() {
            side = side.push(ui::faint("Loading changes…"));
        }
        return row![
            container(side).width(Length::Fixed(260.0)),
            ui::empty(
                "git",
                "Nothing to review yet",
                "The changes are still loading."
            )
        ]
        .spacing(12)
        .height(Length::Fill)
        .into();
    };
    side = side.push(ui::mono(
        ide_diff::summary(changes),
        11.0,
        theme::palette().ink_2,
    ));
    side = side.push(
        ui::scroll(ide_diff::file_list(changes, st.diff_file.as_deref())).height(Length::Fill),
    );
    let picked = st
        .diff_file
        .as_deref()
        .and_then(|path| changes.files.iter().find(|f| f.path == path));
    let body: Element<'_, Msg> = match picked {
        Some(file) => diff_view(file),
        None => ide_diff::review(changes),
    };
    row![
        container(side)
            .width(Length::Fixed(260.0))
            .height(Length::Fill),
        container(body).width(Length::Fill).height(Length::Fill),
    ]
    .spacing(12)
    .height(Length::Fill)
    .into()
}

/// The changed files of a set, counted for the tabs.
pub fn changed_count(changes: Option<&ChangeSet>) -> usize {
    changes.map_or(0, |c| c.files.len())
}

fn search_panel(search: &Search) -> Element<'_, Msg> {
    let header = row![
        ui::label("Search"),
        Space::new().width(Length::Fill),
        ui::btn_sm("Close", Some("x"), Kind::Ghost, Some(Msg::ToggleSearch)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let query = text_input("Text to find", &search.query)
        .id(SEARCH_ID.clone())
        .on_input(Msg::SearchQuery)
        .on_submit(Msg::RunSearch)
        .style(theme::input)
        .padding(8);
    let glob = text_input("Glob, e.g. *.rs (optional)", &search.glob)
        .on_input(Msg::SearchGlob)
        .on_submit(Msg::RunSearch)
        .style(theme::input)
        .padding(8);
    let mut list = column![].spacing(2);
    if search.busy {
        list = list.push(ui::faint("Searching…"));
    }
    if let Some(error) = &search.error {
        list = list.push(ui::error_line(error));
    }
    if let Some(reply) = &search.results {
        if reply.matches.is_empty() {
            list = list.push(ui::faint("No matches."));
        }
        for m in &reply.matches {
            let hit = column![
                ui::mono(
                    format!("{}:{}", m.path, m.line),
                    10.5,
                    theme::palette().ink_2
                ),
                ui::mono(m.text.trim().to_string(), 11.5, theme::palette().ink),
            ];
            list = list.push(
                button(hit)
                    .on_press(Msg::OpenMatch(m.path.clone(), m.line))
                    .padding([4, 6])
                    .width(Length::Fill)
                    .style(theme::register_row(false)),
            );
        }
        if reply.truncated {
            list = list.push(ui::faint(
                "Results are cut short. Narrow the search or the glob.",
            ));
        }
    }
    column![header, query, glob, ui::scroll(list).height(Length::Fill)]
        .spacing(8)
        .height(Length::Fill)
        .into()
}
/// One changed file's diff in the centre: its path, Open in editor and Close, then its rows.
fn diff_view(file: &FileChange) -> Element<'static, Msg> {
    let open = (file.status != apz_ide::Status::Deleted).then(|| Msg::OpenPath(file.path.clone()));
    let head = row![
        ui::mono(file.path.clone(), 12.0, theme::palette().ink),
        Space::new().width(Length::Fill),
        ui::btn_sm("Open in editor", None, Kind::Ghost, open),
        ui::btn_sm("Close", Some("x"), Kind::Ghost, Some(Msg::CloseDiff)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    column![
        head,
        ui::scroll(ide_diff::file_block(file)).height(Length::Fill)
    ]
    .spacing(10)
    .height(Length::Fill)
    .into()
}
fn checkpoints_panel(st: &State) -> Element<'_, Msg> {
    let mut list = column![].spacing(10);
    let mut points: Vec<&Checkpoint> = st.checkpoints.iter().collect();
    points.sort_by(|a, b| b.n.cmp(&a.n));
    if points.is_empty() {
        list = list.push(ui::faint("No checkpoints yet."));
    }
    let can_restore = ide::writable(st);
    for point in points {
        let title = if point.label.is_empty() {
            format!("#{}", point.n)
        } else {
            format!("#{}  {}", point.n, point.label)
        };
        let actions = row![
            ui::btn_sm(
                "Compare",
                None,
                Kind::Ghost,
                Some(Msg::CompareWith(point.n))
            ),
            ui::btn_sm(
                "Restore",
                None,
                Kind::Secondary,
                can_restore.then_some(Msg::AskRestore(point.n))
            ),
        ]
        .spacing(6);
        list = list.push(
            column![
                ui::mono(title, 11.5, theme::palette().ink),
                ui::faint(format!("{} files", point.files)),
                actions
            ]
            .spacing(4),
        );
    }
    if let Some(n) = st.restore {
        let confirm = column![
            ui::mono(
                format!("Restore checkpoint #{n}?"),
                11.5,
                theme::palette().ink
            ),
            ui::faint("The workspace files go back to that point. Open editors reload."),
            row![
                ui::btn_sm("Restore", None, Kind::Danger, Some(Msg::Restore(n))),
                ui::btn_sm("Cancel", None, Kind::Ghost, Some(Msg::CancelRestore)),
            ]
            .spacing(6),
        ]
        .spacing(8);
        list = list.push(
            container(confirm)
                .padding([10, 12])
                .width(Length::Fill)
                .style(theme::notice(theme::Tone::Warn)),
        );
    }
    list.into()
}
