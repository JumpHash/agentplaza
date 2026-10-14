//! The IDE's file tree. The hub lists a directory at a time, so the tree is lazy: a folder's
//! listing loads when it opens, and a long folder comes in pages. The changed-only view is the
//! same entries filtered by a change set (`apz_ide::changed_filter`), every folder open.

use std::collections::{BTreeMap, BTreeSet};

use apz_ide::{ChangeSet, Entry, EntryKind, TreePage, changed_filter, children};
use iced::widget::{Space, button, column, row};
use iced::{Element, Length};

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
    path != dir && (dir.is_empty() || path.strip_prefix(dir).is_some_and(|rest| rest.starts_with('/')))
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
    tree.entries.extend(page.entries.into_iter().filter(|e| !known.contains(&e.path)));
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
    let mut loaded: BTreeSet<String> = entries.iter().filter(|e| e.kind == EntryKind::Dir).map(|e| e.path.clone()).collect();
    loaded.insert(String::new());
    Tree {
        entries,
        loaded,
        ..Tree::default()
    }
}

/// The rows on screen, depth first, folders before files in each folder. `open_all` opens every
/// folder (the changed-only view).
pub fn rows(tree: &Tree, open_all: bool) -> Vec<Row> {
    let mut out = Vec::new();
    walk(tree, open_all, "", 0, &mut out);
    out
}

fn walk(tree: &Tree, open_all: bool, dir: &str, depth: usize, out: &mut Vec<Row>) {
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
                    walk(tree, open_all, &e.path, depth + 1, out);
                }
            }
            EntryKind::File | EntryKind::Symlink => out.push(Row::File {
                path: e.path.clone(),
                name,
                depth,
                binary: e.binary,
                symlink: e.kind == EntryKind::Symlink,
            }),
        }
    }
    if tree.more.contains_key(dir) {
        out.push(Row::More { dir: dir.to_string(), depth });
    }
}

const INDENT: f32 = 14.0;

/// The tree's rows as widgets. `selected` is the open file's path.
pub fn view(rows: Vec<Row>, selected: Option<String>) -> Element<'static, Msg> {
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
                let chevron = if open { "chevronDown" } else { "chevronRight" };
                let mut label = row![
                    Space::new().width(Length::Fixed(depth as f32 * INDENT)),
                    ui::icon(chevron, 12.0, theme::palette().ink_3),
                    ui::mono(name, 12.0, theme::palette().ink),
                ]
                .spacing(6)
                .align_y(iced::Alignment::Center);
                if loading {
                    label = label.push(ui::faint("loading"));
                }
                button(label)
                    .on_press(Msg::ToggleDir(path))
                    .padding([3, 6])
                    .width(Length::Fill)
                    .style(theme::register_row(false))
                    .into()
            }
            Row::File {
                path,
                name,
                depth,
                binary,
                symlink,
            } => {
                let on = selected.as_deref() == Some(path.as_str());
                let ink = if binary || symlink {
                    theme::palette().ink_3
                } else {
                    theme::palette().ink
                };
                let label = row![Space::new().width(Length::Fixed(depth as f32 * INDENT + 18.0)), ui::mono(name, 12.0, ink),]
                    .align_y(iced::Alignment::Center);
                button(label)
                    .on_press(Msg::OpenPath(path))
                    .padding([3, 6])
                    .width(Length::Fill)
                    .style(theme::register_row(on))
                    .into()
            }
            Row::More { dir, depth } => row![
                Space::new().width(Length::Fixed(depth as f32 * INDENT + 18.0)),
                ui::btn_sm("Load more", None, Kind::Ghost, Some(Msg::MoreDir(dir))),
            ]
            .into(),
        };
        col = col.push(el);
    }
    col.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_ide::{FileChange, Status};

    fn entry(path: &str, kind: EntryKind) -> Entry {
        Entry {
            path: path.to_string(),
            kind,
            size: 1,
            hash: (kind == EntryKind::File).then(|| format!("h-{path}")),
            binary: false,
        }
    }

    fn page(entries: Vec<Entry>, next: Option<&str>) -> TreePage {
        TreePage {
            entries,
            next: next.map(str::to_string),
        }
    }

    fn names(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Row::Dir { name, .. } | Row::File { name, .. } => name.clone(),
                Row::More { dir, .. } => format!("more:{dir}"),
            })
            .collect()
    }

    /// The root listed, and `src` listed beneath it (both pages as the hub gives them).
    fn root_tree() -> Tree {
        let mut t = Tree::default();
        apply_page(
            &mut t,
            "",
            page(vec![entry("README.md", EntryKind::File), entry("src", EntryKind::Dir)], None),
            true,
        );
        apply_page(&mut t, "src", page(vec![entry("src/main.rs", EntryKind::File)], None), true);
        t
    }

    #[test]
    fn a_closed_folder_hides_its_children_until_it_opens() {
        let mut t = root_tree();
        assert_eq!(names(&rows(&t, false)), ["src", "README.md"], "folders first, then files");
        assert!(toggle(&mut t, "src"), "opening reports the open state");
        assert_eq!(names(&rows(&t, false)), ["src", "main.rs", "README.md"]);
        assert!(!toggle(&mut t, "src"), "a second press closes it");
        assert_eq!(names(&rows(&t, false)), ["src", "README.md"]);
    }

    #[test]
    fn an_open_folder_without_its_listing_shows_as_loading_and_is_asked_for() {
        let mut t = Tree::default();
        apply_page(&mut t, "", page(vec![entry("docs", EntryKind::Dir)], None), true);
        toggle(&mut t, "docs");
        let shown = rows(&t, false);
        assert!(matches!(
            &shown[0],
            Row::Dir {
                open: true,
                loading: true,
                ..
            }
        ));
        assert_eq!(missing(&t), ["docs"]);
        t.inflight.insert("docs".into());
        assert!(missing(&t).is_empty(), "a listing on its way is not asked for twice");
        apply_page(&mut t, "docs", page(vec![entry("docs/a.md", EntryKind::File)], None), true);
        assert_eq!(names(&rows(&t, false)), ["docs", "a.md"]);
        assert!(missing(&t).is_empty());
    }

    #[test]
    fn a_folder_under_a_closed_one_is_not_fetched() {
        let mut t = root_tree();
        t.expanded.insert("a/b".into());
        assert!(missing(&t).is_empty(), "a/b sits under a folder that is not open");
        t.expanded.insert("src".into());
        assert!(missing(&t).is_empty(), "src is listed already, so opening it asks for nothing");
        apply_page(&mut t, "src", page(vec![entry("src/x", EntryKind::Dir)], None), false);
        t.expanded.insert("src/x".into());
        assert_eq!(missing(&t), ["src/x"], "the next folder down is fetched once its parent lists it");
    }

    #[test]
    fn a_first_page_replaces_a_folders_listing_and_a_later_page_adds_to_it() {
        let mut t = root_tree();
        apply_page(&mut t, "src", page(vec![entry("src/a.rs", EntryKind::File)], Some("c1")), true);
        assert!(names(&rows(&t, true)).contains(&"a.rs".to_string()));
        assert!(
            !names(&rows(&t, true)).contains(&"main.rs".to_string()),
            "a first page drops the old listing"
        );
        assert_eq!(t.more.get("src").map(String::as_str), Some("c1"));
        apply_page(&mut t, "src", page(vec![entry("src/b.rs", EntryKind::File)], None), false);
        let got = names(&rows(&t, true));
        assert!(got.contains(&"a.rs".to_string()) && got.contains(&"b.rs".to_string()));
        assert!(!t.more.contains_key("src"), "the last page clears the cursor");
    }

    #[test]
    fn a_root_with_more_pages_ends_with_a_load_more_row() {
        let mut t = Tree::default();
        apply_page(&mut t, "", page(vec![entry("a.txt", EntryKind::File)], Some("next")), true);
        assert_eq!(names(&rows(&t, false)), ["a.txt", "more:"]);
    }

    #[test]
    fn the_changed_view_shows_touched_paths_with_their_folders_open() {
        let mut t = root_tree();
        apply_page(&mut t, "src", page(vec![entry("src/lib.rs", EntryKind::File)], None), true);
        let cs = ChangeSet {
            files: vec![FileChange {
                path: "src/main.rs".into(),
                old_path: None,
                status: Status::Modified,
                binary: false,
                hunks: vec![],
            }],
        };
        let view = changed(&t, &cs);
        assert_eq!(names(&rows(&view, true)), ["src", "main.rs"], "only the changed file and its folder");
    }

    #[test]
    fn parent_and_under_follow_the_slash_separated_paths() {
        assert_eq!(parent("a/b/c.txt"), "a/b");
        assert_eq!(parent("top.txt"), "");
        assert!(under("a/b", "a"));
        assert!(!under("ab", "a"), "a sibling with the same prefix is not under it");
        assert!(!under("a", "a"));
    }
}
