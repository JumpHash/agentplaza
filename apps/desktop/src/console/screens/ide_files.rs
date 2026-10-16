//! The IDE's files, placed in the new layout: the Files tab of the right panel (the tree with its changed-only
//! filter and refresh, the search when one is open, and the checkpoints), the editor and the diff in the centre.
//! The tree, search, checkpoint and diff pieces are the ones the IDE has always had; this module places them.

// The IDE's own helpers and state, for the messages this area takes (moved from `ide.rs`).
use super::ide::*;
use super::ide_chrome;
use crate::console::{Ctx, client};
use apz_ide::state::Action;
use apz_ide::state::Base;
use apz_ide::{ChangeSet, FileChange};
use iced::Task;
use iced::widget::{Space, button, column, container, row, text_input};
use iced::{Alignment, Element, Length};
use serde_json::{Value, json};

use super::ide::{self, BaseChoice, Checkpoint, Msg, QUICK_ID, SEARCH_ID, Search, State};
use super::ide_diff;
use super::ide_editor;
use super::ide_tree;
use crate::console::ui::{self, Kind};
use crate::theme;

/// The Files tab: the tree (or the search, when one is open) above the checkpoints. The tree takes most of the
/// height; the checkpoints keep the rest. A workstation has no diff, so no Changed filter and no checkpoints: the
/// tree takes the whole tab.
pub fn files_tab(st: &State) -> Element<'_, Msg> {
    let on_workspace = st.ide.workspace.is_some();
    let diff = has_diff(st.ide.workspace.as_ref());
    let filter = diff && st.ide.filter_changed;
    let mut head = row![
        ui::mono("Files", 13.0, theme::palette().ink),
        Space::new().width(Length::Fill)
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    if diff {
        head = head.push(ui::btn_sm(
            "Changed",
            None,
            if filter {
                Kind::Primary
            } else {
                Kind::Secondary
            },
            on_workspace.then_some(Msg::ChangedOnly),
        ));
    }
    let head = head
        .push(ui::btn_sm(
            "",
            Some("search"),
            Kind::Ghost,
            on_workspace.then_some(Msg::ToggleSearch),
        ))
        .push(ui::btn_sm(
            "",
            Some("refresh"),
            Kind::Ghost,
            on_workspace.then_some(Msg::RefreshAll),
        ));
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
    if !diff {
        let tree = container(main).height(Length::Fill);
        return column![head, tree].spacing(10).height(Length::Fill).into();
    }
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
    points.sort_by_key(|a| std::cmp::Reverse(a.n));
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

// ---- The messages the IDE hands to this area (moved from `ide.rs`'s `update`) ----------------------

/// Whether a message is this area's: `update` below takes it, and `ide.rs` hands it over.
pub(super) fn handles(msg: &Msg) -> bool {
    matches!(
        msg,
        Msg::Dir { .. }
            | Msg::ToggleDir(..)
            | Msg::MoreDir(..)
            | Msg::RefreshAll
            | Msg::ChangedOnly
            | Msg::OpenPath(..)
            | Msg::ShowDiff(..)
            | Msg::CloseDiff
            | Msg::Opened { .. }
            | Msg::Activate(..)
            | Msg::Close(..)
            | Msg::Edit(..)
            | Msg::Save
            | Msg::Saved { .. }
            | Msg::Reload(..)
            | Msg::Reloaded { .. }
            | Msg::KeepMine(..)
            | Msg::Remote { .. }
            | Msg::Changed { .. }
            | Msg::PickBase(..)
            | Msg::CompareWith(..)
            | Msg::Checkpoints { .. }
            | Msg::AskRestore(..)
            | Msg::CancelRestore
            | Msg::Restore(..)
            | Msg::Restored { .. }
            | Msg::ToggleQuick
            | Msg::QuickQuery(..)
            | Msg::QuickFirst
            | Msg::ToggleSearch
            | Msg::SearchQuery(..)
            | Msg::SearchGlob(..)
            | Msg::RunSearch
            | Msg::Searched { .. }
            | Msg::OpenMatch(..)
    )
}

/// The area's messages: the state they change, and the tasks they start. See `handles`.
pub(super) fn update(st: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Dir {
            ws,
            dir,
            first,
            res,
        } => {
            if !is_current(st, &ws) {
                return Task::none();
            }
            match res {
                Ok(page) => {
                    ide_tree::apply_page(&mut st.tree, &dir, page, first);
                    load_missing(st, ctx)
                }
                Err(e) => {
                    st.tree.inflight.remove(&dir);
                    st.notice = Some(format!(
                        "Could not list {}: {e}",
                        if dir.is_empty() { "the root" } else { &dir }
                    ));
                    Task::none()
                }
            }
        }
        Msg::ToggleDir(dir) => {
            ide_tree::toggle(&mut st.tree, &dir);
            load_missing(st, ctx)
        }
        Msg::MoreDir(dir) => {
            let Some(cursor) = st.tree.more.get(&dir).cloned() else {
                return Task::none();
            };
            list_dir(st, ctx, dir, Some(cursor))
        }
        Msg::RefreshAll => reload_all(st, ctx),
        // A workstation has no diff, so nothing to filter to.
        Msg::ChangedOnly if !has_diff(st.ide.workspace.as_ref()) => Task::none(),
        Msg::ChangedOnly => {
            dispatch(st, Action::ToggleChangedFilter);
            load_missing(st, ctx)
        }
        Msg::OpenPath(path) => open_path(st, path, ctx),
        Msg::ShowDiff(path) => {
            // A changed file's diff is shown in the centre's Diff view, beside the list of changes.
            st.quick = None;
            st.centre = Centre::Diff;
            st.diff_file = Some(path);
            Task::none()
        }
        Msg::CloseDiff => {
            st.diff_file = None;
            Task::none()
        }
        Msg::Opened { ws, path, res } => {
            if !is_current(st, &ws) {
                return Task::none();
            }
            match res {
                Ok(file) => match file.text {
                    Some(text) => {
                        st.tabs
                            .insert(path.clone(), Tab::new(&path, file.hash, &text));
                        dispatch(st, Action::Open(path.clone()));
                        st.notice = None;
                        apply_goto(st, &path);
                    }
                    None => st.notice = Some(format!("{path} is not a text file.")),
                },
                Err(e) => st.notice = Some(format!("{path}: {e}")),
            }
            Task::none()
        }
        Msg::Activate(path) => {
            st.diff_file = None;
            dispatch(st, Action::Activate(path));
            Task::none()
        }
        Msg::Close(path) => {
            if st.tabs.get(&path).is_some_and(|t| t.buffer.dirty) {
                st.notice = Some(format!("Save {path} before closing it."));
                return Task::none();
            }
            st.tabs.remove(&path);
            dispatch(st, Action::Close(path));
            Task::none()
        }
        Msg::Edit(path, action) => {
            if writable(st)
                && let Some(tab) = st.tabs.get_mut(&path)
                && tab.edit(action)
            {
                st.saved = false;
            }
            Task::none()
        }
        Msg::Save => {
            let Some(path) = st.ide.active.clone() else {
                return Task::none();
            };
            let Some(ws) = st.ide.workspace.clone() else {
                return Task::none();
            };
            let Some(tab) = st.tabs.get(&path) else {
                return Task::none();
            };
            if !tab.buffer.dirty {
                return Task::none();
            }
            if !writable(st) {
                st.notice = Some("This workspace is read-only.".into());
                return Task::none();
            }
            let sent = tab.buffer.text.clone();
            let expected = tab.buffer.base_hash.clone();
            st.notice = Some(format!("Saving {path}…"));
            let c = ctx.client.clone();
            let key = ws.to_string();
            let p = path.clone();
            let text = sent.clone();
            // `client::task` is for `String` errors; a save carries its own (`SaveError`), so it runs as a plain task.
            Task::perform(
                async move { put_file(&c, &ws, &p, &text, expected.as_deref()) },
                move |res| Msg::Saved {
                    ws: key.clone(),
                    path: path.clone(),
                    sent: sent.clone(),
                    res,
                },
            )
        }
        Msg::Saved {
            ws,
            path,
            sent,
            res,
        } => {
            if !is_current(st, &ws) {
                return Task::none();
            }
            match res {
                Ok(saved) => {
                    if let Some(tab) = st.tabs.get_mut(&path) {
                        tab.saved(&sent, saved.hash);
                    }
                    st.saved = true;
                    st.notice = Some(format!("Saved {path}"));
                    Task::batch([changes_throttled(st, ctx), checkpoints_task(st, ctx)])
                }
                Err(SaveError::Conflict(current)) => {
                    if let Some(tab) = st.tabs.get_mut(&path) {
                        tab.conflicted(current);
                    }
                    st.notice = Some(format!(
                        "{path} changed on the hub since you opened it. Reload it or keep yours."
                    ));
                    Task::none()
                }
                Err(SaveError::Failed(e)) => {
                    st.notice = Some(format!("Save failed: {e}"));
                    Task::none()
                }
            }
        }
        Msg::Reload(path) => {
            let Some(ws) = st.ide.workspace.clone() else {
                return Task::none();
            };
            let c = ctx.client.clone();
            let key = ws.to_string();
            let p = path.clone();
            client::task(
                move || fetch_file(&c, &ws, &p),
                move |res| Msg::Reloaded {
                    ws: key.clone(),
                    path: path.clone(),
                    res,
                },
            )
        }
        Msg::Reloaded { ws, path, res } => {
            if !is_current(st, &ws) {
                return Task::none();
            }
            match res {
                Ok(file) => match (file.text, st.tabs.get_mut(&path)) {
                    (Some(text), Some(tab)) => {
                        tab.reload(&path, file.hash, &text);
                        st.notice = Some(format!("Reloaded {path}"));
                    }
                    (None, _) => st.notice = Some(format!("{path} is not a text file.")),
                    (Some(_), None) => {}
                },
                Err(e) => st.notice = Some(format!("Reload failed: {e}")),
            }
            Task::none()
        }
        Msg::KeepMine(path) => {
            if let Some(tab) = st.tabs.get_mut(&path) {
                tab.keep_mine();
                st.notice = Some(format!(
                    "Kept your version of {path}. Saving replaces the hub's file."
                ));
            }
            Task::none()
        }
        Msg::Remote { ws, path, res } => {
            if is_current(st, &ws)
                && let Ok(file) = res
                && let Some(tab) = st.tabs.get_mut(&path)
            {
                match tab.remote(file.hash.as_deref()) {
                    Remote::Reload => {
                        if let Some(text) = &file.text {
                            tab.reload(&path, file.hash.clone(), text);
                        }
                    }
                    Remote::Same | Remote::Conflict => {}
                }
            }
            Task::none()
        }
        Msg::Changed { ws, base, res } => {
            st.changes_busy = false;
            if is_current(st, &ws) && base == st.ide.base {
                match res {
                    Ok(set) => {
                        st.changes = Some(set);
                        st.changes_error = None;
                    }
                    Err(e) => st.changes_error = Some(e),
                }
            }
            Task::none()
        }
        Msg::PickBase(choice) => match choice {
            BaseChoice::WorkStart => {
                dispatch(st, Action::SetBase(Base::WorkStart));
                changes_task(st, ctx)
            }
            BaseChoice::Checkpoint => match st.checkpoints.iter().map(|c| c.n).max() {
                Some(n) => {
                    dispatch(st, Action::SetBase(Base::Checkpoint(n)));
                    changes_task(st, ctx)
                }
                None => {
                    st.notice = Some("There is no checkpoint to compare with yet.".into());
                    Task::none()
                }
            },
        },
        Msg::CompareWith(n) => {
            dispatch(st, Action::SetBase(Base::Checkpoint(n)));
            changes_task(st, ctx)
        }
        Msg::Checkpoints { ws, res } => {
            if is_current(st, &ws) {
                match res {
                    Ok(list) => st.checkpoints = list,
                    Err(e) => st.notice = Some(format!("Checkpoints: {e}")),
                }
            }
            Task::none()
        }
        Msg::AskRestore(n) => {
            st.restore = Some(n);
            Task::none()
        }
        Msg::CancelRestore => {
            st.restore = None;
            Task::none()
        }
        Msg::Restore(n) => {
            let Some(ws) = st.ide.workspace.clone() else {
                return Task::none();
            };
            st.notice = Some(format!("Restoring checkpoint #{n}…"));
            let c = ctx.client.clone();
            let key = ws.to_string();
            client::task(
                move || {
                    c.post::<Value>(
                        &route(&ws, &format!("/checkpoints/{n}/restore")),
                        &json!({}),
                    )
                    .map(|_| ())
                },
                move |res| Msg::Restored {
                    ws: key.clone(),
                    res,
                },
            )
        }
        Msg::Restored { ws, res } => {
            if !is_current(st, &ws) {
                return Task::none();
            }
            st.restore = None;
            match res {
                Ok(()) => {
                    st.notice = Some("Restored the checkpoint.".into());
                    reload_all(st, ctx)
                }
                Err(e) => {
                    st.notice = Some(format!("Restore failed: {e}"));
                    Task::none()
                }
            }
        }
        Msg::ToggleQuick => {
            if st.quick.is_some() {
                st.quick = None;
                Task::none()
            } else {
                st.quick = Some(String::new());
                st.search = None;
                st.centre = Centre::Editor;
                iced::widget::operation::focus(QUICK_ID.clone())
            }
        }
        Msg::QuickQuery(query) => {
            st.quick = Some(query);
            Task::none()
        }
        Msg::QuickFirst => match quick_hits(st).into_iter().next() {
            Some(first) => open_path(st, first, ctx),
            None => Task::none(),
        },
        Msg::ToggleSearch => {
            if st.search.is_some() {
                st.search = None;
                Task::none()
            } else {
                // The search opens in the Files tab, in place of the tree.
                st.right = RightTab::Files;
                st.layout.right_open = true;
                ide_chrome::save(&st.layout);
                open_search(st)
            }
        }
        Msg::SearchQuery(query) => {
            if let Some(search) = st.search.as_mut() {
                search.query = query;
            }
            Task::none()
        }
        Msg::SearchGlob(glob) => {
            if let Some(search) = st.search.as_mut() {
                search.glob = glob;
            }
            Task::none()
        }
        Msg::RunSearch => {
            let Some(ws) = st.ide.workspace.clone() else {
                return Task::none();
            };
            let Some(search) = st.search.as_mut() else {
                return Task::none();
            };
            let query = search.query.trim().to_string();
            if query.is_empty() {
                return Task::none();
            }
            search.busy = true;
            search.error = None;
            let glob = search.glob.trim().to_string();
            let c = ctx.client.clone();
            let key = ws.to_string();
            client::task(
                move || fetch_search(&c, &ws, &query, &glob),
                move |res| Msg::Searched {
                    ws: key.clone(),
                    res,
                },
            )
        }
        Msg::Searched { ws, res } => {
            if is_current(st, &ws)
                && let Some(search) = st.search.as_mut()
            {
                search.busy = false;
                match res {
                    Ok(reply) => {
                        search.results = Some(reply);
                        search.error = None;
                    }
                    Err(e) => search.error = Some(e),
                }
            }
            Task::none()
        }
        Msg::OpenMatch(path, line) => {
            st.diff_file = None;
            if st.tabs.contains_key(&path) {
                dispatch(st, Action::Activate(path.clone()));
                if let Some(tab) = st.tabs.get_mut(&path) {
                    goto_line(tab, line);
                }
                Task::none()
            } else {
                st.goto = Some((path.clone(), line));
                open_path(st, path, ctx)
            }
        }
        _ => Task::none(),
    }
}
