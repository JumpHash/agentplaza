//! A workstation's work folder (docs/design/workstations.md, section 8): the sync's state and conflicts, the hub's
//! mirror of the folder to browse, and the owner's own moves on it. Both ways of the sync run on the hub; this page
//! shows what it holds and lets the owner download, upload, delete, and put a file into the Library.
//!
//!   GET    /v1/stations/{id}/sync                      → state: files, bytes, last_sync_ms, linked, conflicts
//!   GET    /v1/stations/{id}/files?prefix=             → [FileView] of the mirror under the prefix
//!   GET    /v1/stations/{id}/files/{path}              → the file's content
//!   PUT    /v1/stations/{id}/files/{path}              raw bytes → 201 FileView (the next pass pushes it)
//!   DELETE /v1/stations/{id}/files/{path}              → FileView, deleted (the next pass removes it on the station)
//!   POST   /v1/stations/{id}/files/{path}/deliver      {title?, description?, task?} → 201, the file into the Library
//!   POST   /v1/stations/{id}/sync/resume               {apply_deletions} (shown only when the hub reports the sync paused)

use std::path::PathBuf;

use apz_http::client::request;
use iced::widget::{column, row};
use iced::{Alignment, Element, Length, Task};
use serde::Deserialize;
use serde_json::{Value, json};

use super::ide::enc;
use super::lab_common::ago;
use super::workstations::{Load, cells, mono_text, settle_into, table};
use crate::console::{Ctx, client, ui};

/// A file of the mirror (`GET .../files`), or a conflict copy in the sync's state.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct FileView {
    pub path: String,
    pub hash: Option<String>,
    pub size: u64,
    pub mtime_ms: u64,
    pub version: u64,
    pub conflict_of: Option<String>,
    pub deleted: bool,
}

/// The sync's state (`GET .../sync`). The paused fields are the hub's when it has them: a sync that is paused names
/// its reason and the deletions it holds back.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SyncState {
    pub station: String,
    pub linked: bool,
    pub files: u64,
    pub bytes: u64,
    pub last_sync_ms: u64,
    pub conflicts: Vec<FileView>,
    pub paused: bool,
    pub reason: String,
    pub pending_deletions: Value,
}

/// One row of a folder: a folder to open, or a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// The path from the root of the folder; a folder's path ends in `/`.
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime_ms: u64,
}

#[derive(Debug, Default)]
pub struct State {
    station: String,
    pub sync: Load<SyncState>,
    pub files: Load<Vec<FileView>>,
    /// The folder shown: empty for the root, else `a/b/` with its trailing slash.
    pub prefix: String,
    /// The file whose deletion is asked for.
    confirm: Option<String>,
    busy: bool,
    pub notice: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    SyncLoaded(String, Result<SyncState, String>),
    FilesLoaded(String, String, Result<Vec<FileView>, String>),
    Open(String),
    Download(String),
    DownloadTarget(String, Option<PathBuf>),
    Downloaded(Result<String, String>),
    Upload,
    UploadPicked(Option<PathBuf>),
    Uploaded(Result<FileView, String>),
    AskDelete(String),
    CancelDelete,
    Delete,
    Deleted(Result<Value, String>),
    Deliver(String),
    Delivered(Result<Value, String>),
    Resume(bool),
    Resumed(Result<Value, String>),
}

/// Loads the sync's state and the folder shown. A change of station starts the page over.
pub fn load(state: &mut State, ctx: &Ctx, station: &str) -> Task<Msg> {
    if state.station != station {
        *state = State {
            station: station.to_string(),
            ..Default::default()
        };
    }
    let client = ctx.client.clone();
    let path = format!("/v1/stations/{station}/sync");
    let id = station.to_string();
    let sync_task = client::task(
        move || client.get::<SyncState>(&path),
        move |r| Msg::SyncLoaded(id.clone(), r),
    );
    Task::batch([list_task(ctx, station, &state.prefix), sync_task])
}

/// Lists the folder under `prefix` on the mirror.
fn list_task(ctx: &Ctx, station: &str, prefix: &str) -> Task<Msg> {
    let client = ctx.client.clone();
    let path = format!("/v1/stations/{station}/files?prefix={}", enc(prefix));
    let (id, prefix) = (station.to_string(), prefix.to_string());
    client::task(
        move || client.get::<Vec<FileView>>(&path),
        move |r| Msg::FilesLoaded(id.clone(), prefix.clone(), r),
    )
}

/// Sends one raw request and returns the reply's body, or the status it answered with.
fn send_raw(
    client: &client::Client,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    let reply =
        request(&client.target, method, path, body, client.timeout).map_err(|e| e.to_string())?;
    if (200..300).contains(&reply.status) {
        Ok(reply.body)
    } else {
        Err(format!("HTTP {}", reply.status))
    }
}

/// A path, with each segment percent-encoded: `a b/c.txt` becomes `a%20b/c.txt`.
pub fn enc_path(path: &str) -> String {
    path.split('/').map(enc).collect::<Vec<_>>().join("/")
}

/// Whether a name is one file name: not empty, not `.` or `..`, and with no slash or control character.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|c| c == '/' || c == '\\' || c.is_control())
}

/// The folder's rows under `prefix`, folders first, each name once: from the mirror's flat list of paths.
pub fn children(files: &[FileView], prefix: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for file in files
        .iter()
        .filter(|f| !f.deleted && f.path.starts_with(prefix))
    {
        let rest = &file.path[prefix.len()..];
        if rest.is_empty() {
            continue;
        }
        match rest.split_once('/') {
            Some((folder, _)) => {
                if !out.iter().any(|e| e.is_dir && e.name == folder) {
                    out.push(Entry {
                        name: folder.to_string(),
                        path: format!("{prefix}{folder}/"),
                        is_dir: true,
                        size: 0,
                        mtime_ms: 0,
                    });
                }
            }
            None => out.push(Entry {
                name: rest.to_string(),
                path: file.path.clone(),
                is_dir: false,
                size: file.size,
                mtime_ms: file.mtime_ms,
            }),
        }
    }
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
}

/// The folder above `prefix` (`a/b/` gives `a/`; the root stays the root).
pub fn parent_of(prefix: &str) -> String {
    let trimmed = prefix.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(i) => format!("{}/", &trimmed[..i]),
        None => String::new(),
    }
}

/// A byte count in words: bytes, KB, MB or GB, one decimal from 10 units up to none.
pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 10.0 {
        format!("{:.0} {}", value, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

/// A pending deletion count from the hub's value: a list's length, or a number as it is.
pub fn deletions_of(value: &Value) -> u64 {
    match value {
        Value::Array(list) => list.len() as u64,
        Value::Number(n) => n.as_u64().unwrap_or(0),
        _ => 0,
    }
}

/// The folder's page: the sync, then the mirror under the folder shown.
pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    let station = state.station.clone();
    match msg {
        Msg::SyncLoaded(id, result) => {
            if id == station {
                settle_into(&mut state.sync, result);
            }
            Task::none()
        }
        Msg::FilesLoaded(id, prefix, result) => {
            if id == station && prefix == state.prefix {
                settle_into(&mut state.files, result);
            }
            Task::none()
        }
        Msg::Open(prefix) => {
            state.prefix = prefix.clone();
            state.files = Load::Pending;
            state.confirm = None;
            list_task(ctx, &station, &prefix)
        }
        Msg::Download(path) => {
            let name = path.rsplit('/').next().unwrap_or("file").to_string();
            Task::perform(
                async move {
                    rfd::AsyncFileDialog::new()
                        .set_file_name(name)
                        .save_file()
                        .await
                        .map(|file| file.path().to_path_buf())
                },
                move |target| Msg::DownloadTarget(path.clone(), target),
            )
        }
        Msg::DownloadTarget(path, Some(target)) => {
            let client = ctx.client.clone();
            let url = format!("/v1/stations/{station}/files/{}", enc_path(&path));
            client::task(
                move || {
                    let bytes = send_raw(&client, "GET", &url, None)?;
                    std::fs::write(&target, &bytes)
                        .map_err(|e| format!("The file could not be saved: {e}"))?;
                    Ok(target.display().to_string())
                },
                Msg::Downloaded,
            )
        }
        Msg::DownloadTarget(_, None) => Task::none(),
        Msg::Downloaded(Ok(saved)) => {
            state.error = None;
            state.notice = Some(format!("Saved to {saved}"));
            Task::none()
        }
        Msg::Downloaded(Err(error)) => {
            state.error = Some(error);
            Task::none()
        }
        Msg::Upload => Task::perform(
            async {
                rfd::AsyncFileDialog::new()
                    .set_title("Choose a file for the work folder")
                    .pick_file()
                    .await
                    .map(|file| file.path().to_path_buf())
            },
            Msg::UploadPicked,
        ),
        Msg::UploadPicked(None) => Task::none(),
        Msg::UploadPicked(Some(file)) => {
            let Some(name) = file
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
            else {
                state.error = Some("That file has no name the work folder can keep.".into());
                return Task::none();
            };
            if !valid_name(&name) {
                state.error = Some("That file has no name the work folder can keep.".into());
                return Task::none();
            }
            state.busy = true;
            let url = format!(
                "/v1/stations/{station}/files/{}",
                enc_path(&format!("{}{name}", state.prefix))
            );
            let client = ctx.client.clone();
            client::task(
                move || {
                    let bytes = std::fs::read(&file)
                        .map_err(|e| format!("The file could not be read: {e}"))?;
                    let reply = send_raw(&client, "PUT", &url, Some(&bytes))?;
                    serde_json::from_slice::<FileView>(&reply).map_err(|e| e.to_string())
                },
                Msg::Uploaded,
            )
        }
        Msg::Uploaded(Ok(file)) => {
            state.busy = false;
            state.error = None;
            state.notice = Some(format!(
                "{} is in the work folder; the workstation gets it at its next sync",
                file.path
            ));
            list_task(ctx, &station, &state.prefix)
        }
        Msg::Uploaded(Err(error)) => {
            state.busy = false;
            state.error = Some(error);
            Task::none()
        }
        Msg::AskDelete(path) => {
            state.confirm = Some(path);
            Task::none()
        }
        Msg::CancelDelete => {
            state.confirm = None;
            Task::none()
        }
        Msg::Delete => {
            let Some(path) = state.confirm.take() else {
                return Task::none();
            };
            let client = ctx.client.clone();
            let url = format!("/v1/stations/{station}/files/{}", enc_path(&path));
            client::task(move || client.delete(&url), Msg::Deleted)
        }
        Msg::Deleted(Ok(_)) => {
            state.error = None;
            state.notice = Some(
                "Deleted from the work folder; the workstation removes it at its next sync".into(),
            );
            list_task(ctx, &station, &state.prefix)
        }
        Msg::Deleted(Err(error)) => {
            state.error = Some(error);
            Task::none()
        }
        Msg::Deliver(path) => {
            let client = ctx.client.clone();
            let url = format!("/v1/stations/{station}/files/{}/deliver", enc_path(&path));
            client::task(
                move || client.call("POST", &url, Some(&json!({}))),
                Msg::Delivered,
            )
        }
        Msg::Delivered(Ok(item)) => {
            state.error = None;
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("the file");
            state.notice = Some(format!("{name} is in the Library"));
            Task::none()
        }
        Msg::Delivered(Err(error)) => {
            state.error = Some(error);
            Task::none()
        }
        Msg::Resume(apply_deletions) => {
            let client = ctx.client.clone();
            let url = format!("/v1/stations/{station}/sync/resume");
            let body = json!({ "apply_deletions": apply_deletions });
            client::task(move || client.call("POST", &url, Some(&body)), Msg::Resumed)
        }
        Msg::Resumed(Ok(_)) => {
            state.error = None;
            state.notice = Some("The sync is running again".into());
            load(state, ctx, &station)
        }
        Msg::Resumed(Err(error)) => {
            state.error = Some(error);
            Task::none()
        }
    }
}

/// The work-folder card: the sync's state and conflicts, the folder's path, and its rows.
pub fn view(state: &State) -> Element<'_, Msg> {
    let mut parts: Vec<Element<'_, Msg>> = Vec::new();
    if let Some(error) = &state.error {
        parts.push(ui::error_line(error.as_str()));
    }
    if let Some(notice) = &state.notice {
        parts.push(ui::faint(notice.as_str()).into());
    }
    match &state.sync {
        Load::Pending => parts.push(ui::loading()),
        Load::Failed => parts.push(ui::empty("alert", "Could not load the sync.", "")),
        Load::Ready(sync) => {
            parts.push(sync_view(sync));
        }
    }
    parts.push(breadcrumb(&state.prefix));
    parts.push(
        row![ui::btn(
            "Upload a file here",
            None,
            ui::Kind::Secondary,
            (!state.busy).then_some(Msg::Upload),
        )]
        .into(),
    );
    if let Some(path) = &state.confirm {
        parts.push(
            column![
                ui::faint(format!("Delete {path} from the work folder? The workstation removes its copy at its next sync.")),
                row![
                    ui::btn("Cancel", None, ui::Kind::Secondary, Some(Msg::CancelDelete)),
                    ui::btn("Delete", None, ui::Kind::Danger, Some(Msg::Delete)),
                ]
                .spacing(8),
            ]
            .spacing(10)
            .into(),
        );
    }
    parts.push(listing(state));
    column(parts).spacing(14).width(Length::Fill).into()
}

fn sync_view(sync: &SyncState) -> Element<'_, Msg> {
    let link = if sync.linked {
        ui::badge("Connected", ui::Tone::Ok)
    } else {
        ui::badge("Waiting for the workstation", ui::Tone::Muted)
    };
    let mut parts: Vec<Element<'_, Msg>> = vec![
        row![
            link,
            mono_text(format!(
                "{} files · {} · synced {}",
                sync.files,
                size_text(sync.bytes),
                ago(sync.last_sync_ms)
            )),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into(),
    ];
    if sync.paused {
        let pending = deletions_of(&sync.pending_deletions);
        let reason = if sync.reason.is_empty() {
            "The sync is paused.".to_string()
        } else {
            format!("The sync is paused: {}", sync.reason)
        };
        parts.push(ui::faint(reason).into());
        let mut actions: Vec<Element<'_, Msg>> = vec![ui::btn(
            "Resume sync",
            None,
            ui::Kind::Primary,
            Some(Msg::Resume(false)),
        )];
        if pending > 0 {
            actions.push(ui::btn(
                &format!("Resume and apply {pending} deletions"),
                None,
                ui::Kind::Danger,
                Some(Msg::Resume(true)),
            ));
        }
        parts.push(row(actions).spacing(8).into());
    }
    for conflict in &sync.conflicts {
        parts.push(
            ui::faint(format!(
                "Conflict: {} is a copy of {}, kept on both sides.",
                conflict.path,
                conflict
                    .conflict_of
                    .clone()
                    .unwrap_or_else(|| "a file".into())
            ))
            .color(crate::theme::palette().warn)
            .into(),
        );
    }
    column(parts).spacing(10).width(Length::Fill).into()
}

/// The folder's path: the root, then each folder, each one a link back to it.
fn breadcrumb(prefix: &str) -> Element<'_, Msg> {
    let mut crumbs: Vec<Element<'_, Msg>> =
        vec![ui::link("Work folder", Msg::Open(String::new()), false)];
    let mut so_far = String::new();
    for folder in prefix
        .trim_end_matches('/')
        .split('/')
        .filter(|f| !f.is_empty())
    {
        so_far.push_str(folder);
        so_far.push('/');
        crumbs.push(ui::faint(" / ").into());
        crumbs.push(ui::link(folder, Msg::Open(so_far.clone()), false));
    }
    row(crumbs).spacing(4).align_y(Alignment::Center).into()
}

fn listing(state: &State) -> Element<'_, Msg> {
    match &state.files {
        Load::Pending => ui::loading(),
        Load::Failed => ui::empty("alert", "Could not load this folder.", ""),
        Load::Ready(files) => {
            let rows = children(files, &state.prefix);
            if rows.is_empty() {
                return ui::empty(
                    "machine",
                    "Nothing here yet",
                    "Files the workstation or the agents make in the work folder show up here.",
                );
            }
            table(
                &[(4, "Name"), (2, "Size"), (2, "Modified"), (4, "")],
                rows.into_iter().map(entry_row).collect(),
            )
        }
    }
}

fn entry_row(entry: Entry) -> Element<'static, Msg> {
    if entry.is_dir {
        return cells(vec![
            (
                4,
                ui::link(
                    &format!("{}/", entry.name),
                    Msg::Open(entry.path.clone()),
                    false,
                ),
            ),
            (2, ui::faint("—").into()),
            (2, ui::faint("").into()),
            (4, ui::faint("folder").into()),
        ]);
    }
    let actions = row![
        ui::btn_sm(
            "Download",
            None,
            ui::Kind::Secondary,
            Some(Msg::Download(entry.path.clone()))
        ),
        ui::btn_sm(
            "Deliver to Library",
            None,
            ui::Kind::Secondary,
            Some(Msg::Deliver(entry.path.clone()))
        ),
        ui::btn_sm(
            "Delete",
            None,
            ui::Kind::Danger,
            Some(Msg::AskDelete(entry.path.clone()))
        ),
    ]
    .spacing(6);
    cells(vec![
        (4, mono_text(entry.name.clone())),
        (2, mono_text(size_text(entry.size))),
        (2, ui::faint(ago(entry.mtime_ms)).into()),
        (4, actions.into()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, size: u64) -> FileView {
        FileView {
            path: path.into(),
            size,
            ..Default::default()
        }
    }

    #[test]
    fn a_folder_lists_its_files_and_each_subfolder_once() {
        let files = vec![
            file("notes.md", 10),
            file("renders/a.png", 20),
            file("renders/b.png", 30),
            file("renders/deep/c.png", 40),
            FileView {
                path: "gone.txt".into(),
                deleted: true,
                ..Default::default()
            },
        ];
        let root = children(&files, "");
        let names: Vec<(&str, bool)> = root.iter().map(|e| (e.name.as_str(), e.is_dir)).collect();
        assert_eq!(
            names,
            vec![("renders", true), ("notes.md", false)],
            "folders first, a deleted file is gone"
        );
        assert_eq!(root[0].path, "renders/");

        let inside = children(&files, "renders/");
        let inner: Vec<&str> = inside.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(inner, vec!["deep", "a.png", "b.png"]);
        assert_eq!(inside[1].path, "renders/a.png");
        assert_eq!(inside[1].size, 20);
    }

    #[test]
    fn a_folder_path_goes_up_one_level_and_the_root_stays() {
        assert_eq!(parent_of("a/b/"), "a/");
        assert_eq!(parent_of("a/"), "");
        assert_eq!(parent_of(""), "");
    }

    #[test]
    fn paths_are_encoded_per_segment_and_names_are_one_segment() {
        assert_eq!(enc_path("a b/c.txt"), "a%20b/c.txt");
        assert!(valid_name("report v2.pdf"));
        assert!(!valid_name(""));
        assert!(!valid_name(".."));
        assert!(!valid_name("a/b"));
        assert!(!valid_name("a\\b"));
        assert!(!valid_name("tab\tname"));
    }

    #[test]
    fn sizes_read_in_bytes_then_units() {
        assert_eq!(size_text(0), "0 B");
        assert_eq!(size_text(1023), "1023 B");
        assert_eq!(size_text(1536), "1.5 KB");
        assert_eq!(size_text(20 * 1024 * 1024), "20 MB");
        assert_eq!(size_text(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn the_hub_pending_deletions_count_as_a_list_or_a_number() {
        assert_eq!(deletions_of(&json!(["a", "b"])), 2);
        assert_eq!(deletions_of(&json!(4)), 4);
        assert_eq!(deletions_of(&Value::Null), 0);
    }

    #[test]
    fn a_sync_state_parses_with_and_without_the_paused_fields() {
        let plain: SyncState = serde_json::from_value(json!({
            "station": "w1", "linked": true, "files": 3, "bytes": 99, "last_sync_ms": 5, "conflicts": []
        }))
        .expect("the hub's state parses");
        assert!(!plain.paused);
        assert_eq!(plain.files, 3);
        let paused: SyncState = serde_json::from_value(json!({
            "paused": true, "reason": "the disk is full", "pending_deletions": ["x"]
        }))
        .expect("the paused fields parse");
        assert!(paused.paused);
        assert_eq!(deletions_of(&paused.pending_deletions), 1);
        let conflict: FileView = serde_json::from_value(json!({
            "path": "a (conflict w1 2026-10-14 1200).md", "conflict_of": "a.md"
        }))
        .expect("a conflict copy parses");
        assert_eq!(conflict.conflict_of.as_deref(), Some("a.md"));
    }
}
