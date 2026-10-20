//! The IDE's file tree. The hub lists a directory at a time, so the tree is lazy: a folder's
//! listing loads when it opens, and a long folder comes in pages. The changed-only view is the
//! same entries filtered by a change set (`apz_ide::changed_filter`), every folder open.

use std::collections::{BTreeMap, BTreeSet};

use apz_ide::{ChangeSet, Entry, EntryKind, TreePage, changed_filter, children};
use iced::widget::{Space, button, column, container, row};
use iced::{Border, Element, Length, Padding, Theme};

use super::ide::Msg;
use crate::console::ui::{self, Kind};
use crate::theme;

/// What the tree knows: the entries listed so far, which folders are listed, which have a further
/// page, which are open, and which listings are on their way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tree {
    pub entries: Vec<Entry>,
    /// Folders whose listing is in `entries` (`""` is the root).
    pub loaded: BTreeSet<String>,
    /// Folders with a further page: the cursor for it.
    pub more: BTreeMap<String, String>,
    pub expanded: BTreeSet<String>,
    /// Folders being listed now.
    pub inflight: BTreeSet<String>,
}

/// One row of the tree, top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Dir {
        path: String,
        name: String,
        depth: usize,
        open: bool,
        loading: bool,
    },
    File {
        path: String,
        name: String,
        depth: usize,
        binary: bool,
        symlink: bool,
        /// Changed since the base: the row gets a dot.
        changed: bool,
    },
    /// A folder with a further page of its listing.
    More { dir: String, depth: usize },
}

/// The folder a path sits in (`""` for a top-level entry).
pub fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// Whether `path` lies strictly under `dir` (every path is under the root).
fn under(path: &str, dir: &str) -> bool {
    path != dir
        && (dir.is_empty()
            || path
                .strip_prefix(dir)
                .is_some_and(|rest| rest.starts_with('/')))
}

/// Takes one page of `dir`'s listing. The first page replaces what was listed under `dir` (so a
/// folder that vanished goes), and later pages add to it.
pub fn apply_page(tree: &mut Tree, dir: &str, page: TreePage, first: bool) {
    tree.inflight.remove(dir);
    if first {
        tree.entries.retain(|e| !under(&e.path, dir));
        tree.loaded.retain(|d| !under(d, dir));
        tree.more.retain(|d, _| !under(d, dir));
    }
    let known: BTreeSet<String> = tree.entries.iter().map(|e| e.path.clone()).collect();
    tree.entries.extend(
        page.entries
            .into_iter()
            .filter(|e| !known.contains(&e.path)),
    );
    match page.next {
        Some(cursor) => tree.more.insert(dir.to_string(), cursor),
        None => tree.more.remove(dir),
    };
    tree.loaded.insert(dir.to_string());
}

/// Opens or closes a folder. Returns true when it is now open.
pub fn toggle(tree: &mut Tree, dir: &str) -> bool {
    if tree.expanded.remove(dir) {
        false
    } else {
        tree.expanded.insert(dir.to_string());
        true
    }
}

/// Whether a folder is on screen: every folder above it is open.
fn visible(tree: &Tree, dir: &str) -> bool {
    let mut cur = dir;
    loop {
        if cur.is_empty() {
            return true;
        }
        let up = parent(cur);
        if !up.is_empty() && !tree.expanded.contains(up) {
            return false;
        }
        cur = up;
    }
}

/// The open folders on screen whose listing is missing and not on its way: the listings to fetch.
pub fn missing(tree: &Tree) -> Vec<String> {
    tree.expanded
        .iter()
        .filter(|d| !tree.loaded.contains(*d) && !tree.inflight.contains(*d) && visible(tree, d))
        .cloned()
        .collect()
}

/// The changed-only view: the entries the change set touches and their folders, every folder open.
pub fn changed(tree: &Tree, changes: &ChangeSet) -> Tree {
    let entries = changed_filter(&tree.entries, changes);
    let mut loaded: BTreeSet<String> = entries
        .iter()
        .filter(|e| e.kind == EntryKind::Dir)
        .map(|e| e.path.clone())
        .collect();
    loaded.insert(String::new());
    Tree {
        entries,
        loaded,
        ..Tree::default()
    }
}

/// The rows on screen, depth first, folders before files in each folder. `open_all` opens every
/// folder (the changed-only view).
pub fn rows(tree: &Tree, open_all: bool, changed: &BTreeSet<String>) -> Vec<Row> {
    let mut out = Vec::new();
    walk(tree, open_all, changed, "", 0, &mut out);
    out
}

fn walk(
    tree: &Tree,
    open_all: bool,
    changed: &BTreeSet<String>,
    dir: &str,
    depth: usize,
    out: &mut Vec<Row>,
) {
    for e in children(&tree.entries, dir) {
        let name = e.name().to_string();
        match e.kind {
            EntryKind::Dir => {
                let open = open_all || tree.expanded.contains(&e.path);
                let loaded = tree.loaded.contains(&e.path);
                out.push(Row::Dir {
                    path: e.path.clone(),
                    name,
                    depth,
                    open,
                    loading: open && !loaded,
                });
                if open && loaded {
                    walk(tree, open_all, changed, &e.path, depth + 1, out);
                }
            }
            EntryKind::File | EntryKind::Symlink => out.push(Row::File {
                path: e.path.clone(),
                name,
                depth,
                binary: e.binary,
                symlink: e.kind == EntryKind::Symlink,
                changed: changed.contains(&e.path),
            }),
        }
    }
    if tree.more.contains_key(dir) {
        out.push(Row::More {
            dir: dir.to_string(),
            depth,
        });
    }
}

const INDENT: f32 = 14.0;

/// A tree row's ground, the web's `.ided-row`: the open file on the signal-soft ground, hover on the hover ground.
fn row_style(on: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let p = theme::palette();
        let hot = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let background = if on {
            Some(p.signal_soft.into())
        } else if hot {
            Some(p.hover.into())
        } else {
            None
        };
        button::Style {
            background,
            text_color: p.ink,
            border: Border::default(),
            ..Default::default()
        }
    }
}

/// The tree's rows as widgets, the web's `.ided-row`: a full-width row with a 16 px caret, the name in the sans face
/// (bold when changed), a bin tag for a binary file and a dot for a changed one. `selected` is the open file's path.
pub fn view(rows: Vec<Row>, selected: Option<String>) -> Element<'static, Msg> {
    let p = theme::palette();
    let pad = Padding {
        top: 4.0,
        right: 16.0,
        bottom: 4.0,
        left: 16.0,
    };
    let mut col = column![].spacing(0);
    for r in rows {
        let el: Element<'static, Msg> = match r {
            Row::Dir {
                path,
                name,
                depth,
                open,
                loading,
            } => {
                let caret = if open { "▾" } else { "▸" };
                let mut label = row![
                    Space::new().width(Length::Fixed(depth as f32 * INDENT)),
                    ui::sans(caret, 12.0, p.ink_3).width(Length::Fixed(16.0)),
                    ui::sans(name, 13.0, p.ink).width(Length::Fill),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center);
                if loading {
                    label = label.push(ui::faint("loading"));
                }
                button(label)
                    .on_press(Msg::ToggleDir(path))
                    .padding(pad)
                    .width(Length::Fill)
                    .style(row_style(false))
                    .into()
            }
            Row::File {
                path,
                name,
                depth,
                binary,
                symlink,
                changed,
            } => {
                let on = selected.as_deref() == Some(path.as_str());
                let ink = if binary || symlink { p.ink_3 } else { p.ink };
                let name_text = if changed {
                    iced::widget::text(name)
                        .font(theme::SANS_SEMIBOLD)
                        .size(13.0)
                        .color(ink)
                } else {
                    ui::sans(name, 13.0, ink)
                };
                let mut label = row![
                    Space::new().width(Length::Fixed(depth as f32 * INDENT + 24.0)),
                    name_text.width(Length::Fill),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center);
                if binary {
                    label = label.push(ui::badge::<Msg>("bin", ui::Tone::Muted));
                }
                if changed {
                    label = label.push(
                        container(
                            Space::new()
                                .width(Length::Fixed(6.0))
                                .height(Length::Fixed(6.0)),
                        )
                        .style(move |_| container::Style {
                            background: Some(p.signal.into()),
                            border: Border {
                                radius: 3.0.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }),
                    );
                }
                button(label)
                    .on_press(Msg::OpenPath(path))
                    .padding(pad)
                    .width(Length::Fill)
                    .style(row_style(on))
                    .into()
            }
            Row::More { dir, depth } => row![
                Space::new().width(Length::Fixed(depth as f32 * INDENT + 24.0)),
                ui::btn_sm("Load more", None, Kind::Ghost, Some(Msg::MoreDir(dir))),
            ]
            .into(),
        };
        col = col.push(el);
    }
    col.into()
}
