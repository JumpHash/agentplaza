//! The Hugging Face panel on the Models screen (contract: docs/design/foundry.md, "Hugging Face"):
//! the owner's token (its state only; the field is masked and its value is never shown back), a
//! search with filters and sorting, a detail drawer per model (files, sizes against the cap, what it
//! can be imported as), and the import queue with live progress frames, cancel and resume. Every
//! route is owner-only. Terms are never accepted here: a gated model shows its page address.
//! The web's `lab-hf.ts` is the twin.

use std::collections::BTreeMap;

use iced::widget::{Space, column, container, row, text, text_input};
use iced::{Alignment, Element, Length, Subscription, Task, Theme};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Id;
use super::lab_common;
use crate::console::Ctx;
use crate::console::client::{self, Client};
use crate::console::events;
use crate::console::ui::{self, Kind, Tone};
use crate::theme::{MONO, SANS};

const GIB: u64 = 1024 * 1024 * 1024;
const LIBRARIES: [&str; 5] = ["any", "onnx", "safetensors", "transformers", "gguf"];
const SORTS: [(&str, &str); 4] = [
    ("downloads", "Downloads"),
    ("likes", "Likes"),
    ("lastModified", "Updated"),
    ("trending", "Trending"),
];
/// The default file sets, as the contract names them (the hub checks the real files at import).
const BASE_FILES: [&str; 10] = [
    "config.json",
    "tokenizer.json",
    "tokenizer_config.json",
    "vocab.json",
    "vocab.txt",
    "merges.txt",
    "special_tokens_map.json",
    "sentencepiece.bpe.model",
    "generation_config.json",
    "pytorch_model.bin",
];
const BASE_SUFFIXES: [&str; 2] = [".safetensors", ".safetensors.index.json"];
const SERVABLE_FILES: [&str; 5] = [
    "config.json",
    "model_card.json",
    "onnx/model.onnx",
    "model.onnx",
    "tokenizer.json",
];

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct HfStatus {
    pub hf_token_set: bool,
    pub hf_max_gb: f64,
    pub free_bytes: Option<u64>,
    pub downloads_dir: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct HfItem {
    pub id: String,
    pub author: String,
    pub task: Option<String>,
    pub library: Option<String>,
    pub downloads: u64,
    pub likes: u64,
    pub last_modified: Option<String>,
    pub gated: Value,
    pub license: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct HfPage {
    items: Vec<HfItem>,
    next_cursor: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct HfFile {
    pub path: String,
    pub size: u64,
    pub lfs: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Importable {
    pub servable: bool,
    pub servable_reason: Option<String>,
    pub base: bool,
    pub base_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct HfDetail {
    pub id: String,
    pub author: String,
    pub sha: String,
    pub url: String,
    pub task: Option<String>,
    pub library: Option<String>,
    pub license: Option<String>,
    pub gated: Value,
    pub downloads: u64,
    pub likes: u64,
    pub last_modified: Option<String>,
    pub summary: String,
    pub files: Vec<HfFile>,
    pub importable: Importable,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ImportFile {
    pub path: String,
    pub size: u64,
    pub bytes: u64,
    pub state: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ImportRecord {
    pub n: u64,
    pub id: String,
    pub revision: String,
    #[serde(rename = "as")]
    pub as_kind: String,
    pub state: String,
    pub files: Vec<ImportFile>,
    pub bytes: u64,
    pub total: u64,
    pub model_id: Option<String>,
    pub reason: Option<String>,
    pub error: Option<String>,
    pub updated_ms: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct ImportList {
    imports: Vec<ImportRecord>,
}

/// A `foundry` frame of kind `hf_import`: byte progress for one import (and file), or a state change.
#[derive(Debug, Clone)]
pub struct Frame {
    pub import: u64,
    pub file: String,
    pub bytes: u64,
    pub total: u64,
    pub state: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Downloads,
    Likes,
    Updated,
    License,
    Access,
}

#[derive(Debug, Clone, Default)]
pub struct State {
    pub status: Option<HfStatus>,
    pub status_error: Option<String>,
    /// The token as typed, until it is saved. Never shown back once saved.
    pub token_input: String,
    pub token_error: Option<String>,
    pub search: String,
    pub task: String,
    pub library: usize,
    pub sort: usize,
    pub items: Vec<HfItem>,
    pub next: Option<String>,
    pub searched: bool,
    pub search_error: Option<String>,
    pub sort_key: Option<SortKey>,
    pub sort_asc: bool,
    pub detail: Option<HfDetail>,
    pub detail_error: Option<String>,
    pub import_error: Option<String>,
    pub imports: Vec<ImportRecord>,
    pub imports_error: Option<String>,
    /// The state each import was last seen in, to tell when a registration may have changed the registry.
    known: BTreeMap<u64, String>,
    /// Set when an import just finished, so the Models screen reloads the registry.
    pub models_changed: bool,
    /// Why the last external link did not open.
    pub link_error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    TokenInput(String),
    SaveToken,
    RemoveToken,
    Token(Result<Value, String>),
    Status(Result<HfStatus, String>),
    SearchText(String),
    TaskText(String),
    Library(usize),
    Sort(usize),
    Submit,
    More,
    Page(Result<HfPage, String>),
    SortBy(SortKey),
    Open(String),
    Opened(Result<HfDetail, String>),
    CloseDetail,
    Import(String, &'static str),
    Imported(Result<Value, String>),
    Imports(Result<Vec<ImportRecord>, String>),
    Cancel(u64),
    Resume(ImportRecord),
    Acted(Result<Value, String>),
    Frame(Frame),
    /// Open an external `https://` link in the system's browser.
    OpenUrl(String),
}

/// The navigation a message asks for.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// The `foundry` frames of kind `hf_import`, as the panel's messages.
pub fn subscription(ctx: &Ctx) -> Subscription<Msg> {
    lab_common::live(&ctx.client).filter_map(|e| frame(&e))
}

fn frame(event: &events::Event) -> Option<Msg> {
    if event.kind != "foundry"
        || event.data.get("kind").and_then(Value::as_str) != Some("hf_import")
    {
        return None;
    }
    Some(Msg::Frame(Frame {
        import: event.data.get("import")?.as_u64()?,
        file: event
            .data
            .get("file")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        bytes: event.data.get("bytes").and_then(Value::as_u64).unwrap_or(0),
        total: event.data.get("total").and_then(Value::as_u64).unwrap_or(0),
        state: event
            .data
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    }))
}

/// A model id as a path: its slash stays a slash, every segment encoded.
fn model_path(id: &str) -> String {
    id.split('/').map(enc).collect::<Vec<_>>().join("/")
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Whether a `gated` field is set: the hub says `false`, or names a gate kind such as `auto` or `manual`.
pub fn is_gated(gated: &Value) -> bool {
    !matches!(gated, Value::Bool(false) | Value::Null) && gated.as_str() != Some("")
}

/// The files an import of this kind takes by default.
pub fn default_files<'a>(files: &'a [HfFile], as_kind: &str) -> Vec<&'a HfFile> {
    files
        .iter()
        .filter(|f| {
            if as_kind == "servable" {
                SERVABLE_FILES.contains(&f.path.as_str())
            } else {
                BASE_FILES.contains(&f.path.as_str())
                    || BASE_SUFFIXES.iter().any(|s| f.path.ends_with(s))
            }
        })
        .collect()
}

fn token_status_get(c: &Client) -> Result<HfStatus, String> {
    c.get("/v1/foundry/hf/status")
}

fn imports_get(c: &Client) -> Result<Vec<ImportRecord>, String> {
    c.get::<ImportList>("/v1/foundry/hf/imports")
        .map(|l| l.imports)
}

/// Loads the token state and the import queue; the search and the drawer load on demand.
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let a = ctx.client.clone();
    let b = ctx.client.clone();
    let _ = state;
    Task::batch([
        client::task(move || token_status_get(&a), Msg::Status),
        client::task(move || imports_get(&b), Msg::Imports),
    ])
}

fn search_task(c: &Client, state: &State, cursor: Option<String>) -> Task<Msg> {
    let c = c.clone();
    let mut query = vec![
        format!("sort={}", SORTS[state.sort].0),
        "limit=30".to_string(),
    ];
    if !state.search.trim().is_empty() {
        query.push(format!("search={}", enc(state.search.trim())));
    }
    if !state.task.trim().is_empty() {
        query.push(format!("task={}", enc(state.task.trim())));
    }
    if state.library != 0 {
        query.push(format!("library={}", LIBRARIES[state.library]));
    }
    if let Some(cursor) = cursor {
        query.push(format!("cursor={}", enc(&cursor)));
    }
    let path = format!("/v1/foundry/hf/models?{}", query.join("&"));
    client::task(move || c.get::<HfPage>(&path), Msg::Page)
}

/// Applies a message; the registry reload runs from the Models screen when `models_changed` is set.
pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::TokenInput(value) => {
            state.token_input = value;
            Task::none()
        }
        Msg::SaveToken => {
            let value = state.token_input.trim().to_string();
            if value.is_empty() {
                state.token_error = Some("Paste the token first.".to_string());
                return Task::none();
            }
            state.token_error = None;
            client::task(
                move || {
                    c.call(
                        "PUT",
                        "/v1/foundry/hf/token",
                        Some(&json!({ "value": value })),
                    )
                },
                Msg::Token,
            )
        }
        Msg::RemoveToken => {
            state.token_error = None;
            client::task(move || c.delete("/v1/foundry/hf/token"), Msg::Token)
        }
        Msg::Token(Ok(_)) => {
            state.token_input.clear();
            state.token_error = None;
            load(state, ctx)
        }
        Msg::Token(Err(err)) => {
            state.token_error = Some(err);
            Task::none()
        }
        Msg::Status(Ok(status)) => {
            state.status = Some(status);
            state.status_error = None;
            Task::none()
        }
        Msg::Status(Err(err)) => {
            state.status_error = Some(err);
            Task::none()
        }
        Msg::SearchText(v) => {
            state.search = v;
            Task::none()
        }
        Msg::TaskText(v) => {
            state.task = v;
            Task::none()
        }
        Msg::Library(i) => {
            state.library = i;
            Task::none()
        }
        Msg::Sort(i) => {
            state.sort = i;
            Task::none()
        }
        Msg::Submit => {
            state.items.clear();
            state.next = None;
            search_task(&c, state, None)
        }
        Msg::More => match state.next.clone() {
            Some(cursor) => search_task(&c, state, Some(cursor)),
            None => Task::none(),
        },
        Msg::Page(Ok(page)) => {
            state.items.extend(page.items);
            state.next = page.next_cursor;
            state.search_error = None;
            state.searched = true;
            Task::none()
        }
        Msg::Page(Err(err)) => {
            state.search_error = Some(err);
            state.searched = true;
            Task::none()
        }
        Msg::SortBy(key) => {
            state.sort_asc = state.sort_key == Some(key) && !state.sort_asc;
            state.sort_key = Some(key);
            Task::none()
        }
        Msg::Open(id) => {
            state.detail = None;
            state.detail_error = None;
            state.import_error = None;
            let path = format!("/v1/foundry/hf/models/{}", model_path(&id));
            client::task(move || c.get::<HfDetail>(&path), Msg::Opened)
        }
        Msg::Opened(Ok(detail)) => {
            state.detail = Some(detail);
            Task::none()
        }
        Msg::Opened(Err(err)) => {
            state.detail_error = Some(err);
            Task::none()
        }
        Msg::CloseDetail => {
            state.detail = None;
            Task::none()
        }
        Msg::Import(id, as_kind) => {
            state.import_error = None;
            let revision = state
                .detail
                .as_ref()
                .map(|d| d.sha.clone())
                .unwrap_or_else(|| "main".to_string());
            let body = json!({ "id": id, "revision": revision, "as": as_kind });
            client::task(
                move || c.post::<Value>("/v1/foundry/hf/import", &body),
                Msg::Imported,
            )
        }
        Msg::Imported(Ok(_)) => load(state, ctx),
        Msg::Imported(Err(err)) => {
            state.import_error = Some(err);
            Task::none()
        }
        Msg::Imports(Ok(list)) => {
            let mut finished = false;
            for r in &list {
                if let Some(before) = state.known.get(&r.n) {
                    if before != &r.state && (r.state == "done" || r.state == "failed") {
                        finished = true;
                    }
                }
                state.known.insert(r.n, r.state.clone());
            }
            state.models_changed |= finished;
            state.imports = list;
            state.imports_error = None;
            Task::none()
        }
        Msg::Imports(Err(err)) => {
            state.imports_error = Some(err);
            Task::none()
        }
        Msg::Cancel(n) => client::task(
            move || c.delete(&format!("/v1/foundry/hf/imports/{n}")),
            Msg::Acted,
        ),
        Msg::Resume(r) => {
            let body = json!({ "id": r.id, "revision": r.revision, "as": r.as_kind });
            client::task(
                move || c.post::<Value>("/v1/foundry/hf/import", &body),
                Msg::Acted,
            )
        }
        Msg::Acted(Ok(_)) => load(state, ctx),
        Msg::Acted(Err(err)) => {
            state.imports_error = Some(err);
            Task::none()
        }
        Msg::OpenUrl(url) => {
            state.link_error = ui::open_url(&url).err();
            Task::none()
        }
        Msg::Frame(frame) => {
            let index = state.imports.iter().position(|r| r.n == frame.import);
            match index {
                Some(i) => {
                    let r = &mut state.imports[i];
                    let before = r.state.clone();
                    r.state = frame.state.clone();
                    r.bytes = frame.bytes;
                    r.total = frame.total;
                    if let Some(file) = r.files.iter_mut().find(|f| f.path == frame.file) {
                        file.bytes = frame.bytes;
                    }
                    if before != r.state && (r.state == "done" || r.state == "failed") {
                        state.models_changed = true;
                    }
                    Task::none()
                }
                // An import we have not listed yet: fetch the queue.
                None => load(state, ctx),
            }
        }
    }
}

/// The panel: the token, the search, the drawer (when a model is open) and the import queue.
pub fn view<'a>(state: &'a State) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = vec![token_block(state), search_block(state)];
    if let Some(err) = &state.link_error {
        parts.push(ui::error_line(err));
    }
    if let Some(detail) = &state.detail {
        parts.push(drawer(state, detail));
    } else if let Some(err) = &state.detail_error {
        parts.push(ui::error_line(err));
    }
    parts.push(imports_block(state));
    column(parts).spacing(18).into()
}

fn token_block<'a>(state: &'a State) -> Element<'a, Msg> {
    let set = state.status.as_ref().is_some_and(|s| s.hf_token_set);
    let facts: Element<'a, Msg> = match &state.status {
        Some(s) => text(format!(
            "Max import {} GB · free disk {}",
            lab_common::number(s.hf_max_gb, 0),
            s.free_bytes.map_or("unknown".to_string(), lab_common::size),
        ))
        .font(SANS)
        .size(12.0)
        .color(crate::theme::palette().ink_2)
        .into(),
        None => ui::faint(
            state
                .status_error
                .clone()
                .unwrap_or_else(|| "Reading the token state".to_string()),
        )
        .into(),
    };
    let field = text_input("Token value", &state.token_input)
        .on_input(Msg::TokenInput)
        .secure(true)
        .on_submit(Msg::SaveToken)
        .padding(8);
    let save = ui::btn(
        "Save token",
        None,
        Kind::Primary,
        (!state.token_input.trim().is_empty()).then_some(Msg::SaveToken),
    );
    let remove = set.then(|| ui::btn("Remove token", None, Kind::Danger, Some(Msg::RemoveToken)));
    let mut actions = row![field, save].spacing(8).align_y(Alignment::Center);
    if let Some(remove) = remove {
        actions = actions.push(remove);
    }
    let mut body: Vec<Element<'a, Msg>> = vec![
        row![
            ui::badge(
                if state.status.is_none() {
                    "Token state unknown"
                } else if set {
                    "Token set"
                } else {
                    "Token not set"
                },
                if set { Tone::Ok } else { Tone::Muted }
            ),
            facts
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into(),
        actions.into(),
        row![
            ui::faint("The token is stored on this machine and never shown back. Create one at"),
            ui::link(
                "huggingface.co/settings/tokens",
                Msg::OpenUrl("https://huggingface.co/settings/tokens".to_string()),
                false
            ),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into(),
    ];
    if let Some(err) = &state.token_error {
        body.push(ui::error_line(err));
    }
    section("Hugging Face token", column(body).spacing(12).into())
}

fn search_block<'a>(state: &'a State) -> Element<'a, Msg> {
    let search = text_input("Search models", &state.search)
        .on_input(Msg::SearchText)
        .on_submit(Msg::Submit)
        .padding(8);
    let task = text_input("Task, e.g. text-classification", &state.task)
        .on_input(Msg::TaskText)
        .on_submit(Msg::Submit)
        .padding(8);
    let libs: Vec<(usize, &str)> = LIBRARIES
        .iter()
        .enumerate()
        .map(|(i, l)| (i, if i == 0 { "Any library" } else { *l }))
        .collect();
    let sorts: Vec<(usize, &str)> = SORTS
        .iter()
        .enumerate()
        .map(|(i, (_, label))| (i, *label))
        .collect();
    let mut body: Vec<Element<'a, Msg>> = vec![
        row![search, task].spacing(8).into(),
        ui::seg(&libs, state.library, Msg::Library),
        row![
            ui::seg(&sorts, state.sort, Msg::Sort),
            ui::btn("Search", Some("search"), Kind::Primary, Some(Msg::Submit))
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into(),
    ];
    body.push(results(state));
    if let Some(err) = &state.search_error {
        body.push(ui::error_line(err));
    }
    section("Search", column(body).spacing(12).into())
}

fn results<'a>(state: &'a State) -> Element<'a, Msg> {
    if !state.searched {
        return ui::faint("Search the Hub, or leave the box empty for the most downloaded models.")
            .into();
    }
    if state.items.is_empty() {
        return ui::faint("No models match.").into();
    }
    let mut items: Vec<&HfItem> = state.items.iter().collect();
    if let Some(key) = state.sort_key {
        items.sort_by(|a, b| {
            let ord = match key {
                SortKey::Downloads => a.downloads.cmp(&b.downloads),
                SortKey::Likes => a.likes.cmp(&b.likes),
                SortKey::Updated => a.last_modified.cmp(&b.last_modified),
                SortKey::License => a.license.cmp(&b.license),
                SortKey::Access => is_gated(&a.gated).cmp(&is_gated(&b.gated)),
            };
            if state.sort_asc { ord } else { ord.reverse() }
        });
    }
    let head = row![
        cell(4, false, ui::faint("Model".to_string())),
        cell(2, false, ui::faint("Task".to_string())),
        cell(2, false, ui::faint("Library".to_string())),
        sort_head("Downloads", SortKey::Downloads, state, 2),
        sort_head("Likes", SortKey::Likes, state, 1),
        sort_head("Updated", SortKey::Updated, state, 2),
        sort_head("License", SortKey::License, state, 2),
        sort_head("Access", SortKey::Access, state, 2),
    ]
    .spacing(10);
    let mut lines = column![head, ui::hairline()].spacing(8);
    for m in items {
        let access: Element<'a, Msg> = if is_gated(&m.gated) {
            ui::badge("gated", Tone::Warn)
        } else {
            value("open")
        };
        lines = lines.push(
            row![
                cell(
                    4,
                    false,
                    column![
                        ui::link(&m.id, Msg::Open(m.id.clone()), true),
                        value(&m.author)
                    ]
                    .spacing(2)
                ),
                cell(2, false, value(m.task.as_deref().unwrap_or("—"))),
                cell(2, false, value(m.library.as_deref().unwrap_or("—"))),
                cell(2, true, value(&lab_common::count(m.downloads))),
                cell(1, true, value(&lab_common::count(m.likes))),
                cell(
                    2,
                    false,
                    value(
                        m.last_modified
                            .as_deref()
                            .map_or("—", |d| d.get(..10).unwrap_or(d))
                    )
                ),
                cell(2, false, value(m.license.as_deref().unwrap_or("—"))),
                cell(2, false, access),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        );
        lines = lines.push(ui::hairline());
    }
    let mut out = column![lines].spacing(10);
    if state.next.is_some() {
        out = out.push(ui::btn("Load more", None, Kind::Secondary, Some(Msg::More)));
    }
    out.into()
}

fn sort_head<'a>(label: &str, key: SortKey, state: &State, portion: u16) -> Element<'a, Msg> {
    let mark = if state.sort_key == Some(key) {
        if state.sort_asc { " ^" } else { " v" }
    } else {
        ""
    };
    cell(
        portion,
        false,
        ui::link(&format!("{label}{mark}"), Msg::SortBy(key), true),
    )
}

/// The drawer: a model's identity, its gate, what it can be imported as, its files against the cap, and the imports.
fn drawer<'a>(state: &'a State, d: &'a HfDetail) -> Element<'a, Msg> {
    let cap = state
        .status
        .as_ref()
        .map_or(0, |s| (s.hf_max_gb * GIB as f64) as u64);
    let base: u64 = default_files(&d.files, "base").iter().map(|f| f.size).sum();
    let servable: u64 = default_files(&d.files, "servable")
        .iter()
        .map(|f| f.size)
        .sum();
    let size_line = |label: &str, size: u64| -> Element<'a, Msg> {
        let over = size > cap;
        let line = format!(
            "{label}: {}{}",
            lab_common::size(size),
            if over { " (over the cap)" } else { "" }
        );
        text(line)
            .font(MONO)
            .size(12.0)
            .color(if over {
                crate::theme::palette().crit
            } else {
                crate::theme::palette().ink_2
            })
            .into()
    };
    let mut body: Vec<Element<'a, Msg>> = Vec::new();
    if is_gated(&d.gated) {
        body.push(
            column![
                text("Gated: this model needs its terms accepted on its Hub page. Accept them there yourself; this console never accepts terms for you.")
                    .font(SANS)
                    .size(12.0)
                    .color(crate::theme::palette().crit),
                ui::link("Open its page on huggingface.co", Msg::OpenUrl(d.url.clone()), false),
            ]
            .spacing(6)
            .into(),
        );
    }
    body.push(
        column![
            pair("Author", value(&d.author)),
            pair("Revision", value(d.sha.get(..12).unwrap_or(d.sha.as_str()))),
            pair("Task", value(d.task.as_deref().unwrap_or("—"))),
            pair("Library", value(d.library.as_deref().unwrap_or("—"))),
            pair("License", value(d.license.as_deref().unwrap_or("—"))),
            pair("Downloads", value(&lab_common::count(d.downloads))),
            pair("Likes", value(&lab_common::count(d.likes))),
            pair(
                "Servable",
                value(&if d.importable.servable {
                    "yes".to_string()
                } else {
                    format!(
                        "no: {}",
                        d.importable
                            .servable_reason
                            .as_deref()
                            .unwrap_or("not a servable model")
                    )
                })
            ),
            pair(
                "Base",
                value(&if d.importable.base {
                    "yes".to_string()
                } else {
                    format!(
                        "no: {}",
                        d.importable
                            .base_reason
                            .as_deref()
                            .unwrap_or("not a base model")
                    )
                })
            ),
        ]
        .spacing(8)
        .into(),
    );
    if !d.summary.is_empty() {
        body.push(ui::faint(d.summary.clone()).into());
    }
    body.push(section_title("Default selections against the cap"));
    body.push(
        row![
            size_line("Base set", base),
            size_line("Servable set", servable),
            value(&format!(
                "Cap: {} GB",
                lab_common::number(cap as f64 / GIB as f64, 0)
            ))
        ]
        .spacing(18)
        .into(),
    );
    body.push(section_title(&format!(
        "Files ({})",
        lab_common::count(d.files.len() as u64)
    )));
    body.push(file_table(&d.files));
    body.push(
        row![
            ui::btn(
                "Import as base",
                Some("plus"),
                Kind::Primary,
                Some(Msg::Import(d.id.clone(), "base"))
            ),
            ui::btn(
                "Import as servable",
                Some("plus"),
                Kind::Secondary,
                Some(Msg::Import(d.id.clone(), "servable"))
            ),
        ]
        .spacing(8)
        .into(),
    );
    if let Some(err) = &state.import_error {
        body.push(ui::error_line(err));
    }
    let head = row![
        ui::link(&d.id, Msg::OpenUrl(d.url.clone()), true),
        Space::new().width(Length::Fill),
        ui::btn("Close", None, Kind::Ghost, Some(Msg::CloseDetail))
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    section_head(head.into(), column(body).spacing(14).into())
}

fn file_table<'a>(files: &'a [HfFile]) -> Element<'a, Msg> {
    if files.is_empty() {
        return ui::faint("No files listed.").into();
    }
    let mut lines = column![
        row![
            cell(6, false, ui::faint("File".to_string())),
            cell(2, true, ui::faint("Size".to_string())),
            cell(2, false, ui::faint("Storage".to_string()))
        ]
        .spacing(10)
    ]
    .spacing(6);
    for f in files {
        lines = lines.push(
            row![
                cell(6, false, value(&f.path)),
                cell(2, true, value(&lab_common::size(f.size))),
                cell(2, false, value(if f.lfs { "LFS" } else { "git" })),
            ]
            .spacing(10),
        );
    }
    lines.into()
}

fn imports_block<'a>(state: &'a State) -> Element<'a, Msg> {
    let mut body: Vec<Element<'a, Msg>> = Vec::new();
    if state.imports.is_empty() {
        body.push(ui::faint("No imports yet.").into());
    }
    let mut lines = column(Vec::<Element<'a, Msg>>::new()).spacing(10);
    for r in &state.imports {
        let percent = if r.total > 0 {
            (r.bytes as f64 / r.total as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let active = matches!(
            r.state.as_str(),
            "queued" | "downloading" | "verifying" | "registering"
        );
        let model: Element<'a, Msg> = match &r.model_id {
            Some(model) => ui::link(model, Msg::Go(Id::LabModelDetail(model.clone())), true),
            None => value(&r.id),
        };
        let mut actions = row![].spacing(8);
        if r.state == "interrupted" {
            actions = actions.push(ui::btn_sm(
                "Resume",
                None,
                Kind::Primary,
                Some(Msg::Resume(r.clone())),
            ));
        }
        let (label, kind) = if active {
            ("Cancel", Kind::Danger)
        } else {
            ("Remove", Kind::Secondary)
        };
        actions = actions.push(ui::btn_sm(label, None, kind, Some(Msg::Cancel(r.n))));
        let mut state_cell: Vec<Element<'a, Msg>> = vec![ui::badge(&r.state, state_tone(&r.state))];
        if let Some(reason) = &r.reason {
            state_cell.push(ui::faint(reason.clone()).into());
        }
        if let Some(error) = &r.error {
            state_cell.push(
                text(error.clone())
                    .font(SANS)
                    .size(12.0)
                    .color(crate::theme::palette().crit)
                    .into(),
            );
        }
        lines = lines.push(
            row![
                cell(1, false, value(&format!("#{}", r.n))),
                cell(4, false, column![model, value(&r.revision)].spacing(2)),
                cell(1, false, value(&r.as_kind)),
                cell(2, false, column(state_cell).spacing(4)),
                cell(
                    3,
                    false,
                    column![
                        bar(percent),
                        value(&format!(
                            "{} of {}",
                            lab_common::size(r.bytes),
                            lab_common::size(r.total)
                        ))
                    ]
                    .spacing(4)
                ),
                cell(2, false, value(&lab_common::time(r.updated_ms))),
                cell(3, true, actions),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        );
    }
    if !state.imports.is_empty() {
        body.push(lines.into());
    }
    if let Some(err) = &state.imports_error {
        body.push(ui::error_line(err));
    }
    section("Imports", column(body).spacing(12).into())
}

fn state_tone(state: &str) -> Tone {
    match state {
        "downloading" | "verifying" | "registering" => Tone::Info,
        "done" => Tone::Ok,
        "failed" => Tone::Danger,
        "interrupted" => Tone::Warn,
        _ => Tone::Muted,
    }
}

/// A labelled figure: the label in caps beside its value.
fn pair<'a>(label: &str, value: Element<'a, Msg>) -> Element<'a, Msg> {
    row![
        container(ui::label(label)).width(Length::Fixed(130.0)),
        value
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into()
}

fn value<'a>(s: &str) -> Element<'a, Msg> {
    text(s.to_string())
        .font(SANS)
        .size(12.0)
        .color(crate::theme::palette().ink_2)
        .width(Length::Fill)
        .into()
}

fn bar<'a>(ratio: f64) -> Element<'a, Msg> {
    let pct = (ratio * 100.0).round() as u16;
    row![
        container(Space::new().width(Length::Fill).height(6.0))
            .width(Length::FillPortion(pct.max(1)))
            .style(|_: &Theme| container::Style {
                background: Some(crate::theme::palette().signal.into()),
                ..Default::default()
            }),
        container(Space::new().width(Length::Fill).height(6.0))
            .width(Length::FillPortion((100 - pct).max(1)))
            .style(|_: &Theme| container::Style {
                background: Some(crate::theme::palette().rule_faint.into()),
                ..Default::default()
            }),
    ]
    .into()
}

fn cell<'a>(portion: u16, right: bool, content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    let c = container(content).width(Length::FillPortion(portion));
    if right {
        c.align_x(iced::alignment::Horizontal::Right).into()
    } else {
        c.into()
    }
}

fn section_title<'a>(label: &str) -> Element<'a, Msg> {
    text(label.to_string())
        .font(SANS)
        .size(11.0)
        .color(crate::theme::palette().ink_3)
        .into()
}

/// A panel section: a ruled box with its title in caps and its body.
fn section<'a>(title: &str, body: Element<'a, Msg>) -> Element<'a, Msg> {
    container(
        column![
            text(title.to_string())
                .font(SANS)
                .size(11.5)
                .color(crate::theme::palette().ink),
            container(Space::new().height(1.0))
                .width(Length::Fill)
                .style(|_: &Theme| container::Style {
                    background: Some(crate::theme::palette().rule.into()),
                    ..Default::default()
                }),
            body,
        ]
        .spacing(12),
    )
    .padding(16)
    .width(Length::Fill)
    .style(|_: &Theme| container::Style {
        border: iced::Border {
            color: crate::theme::palette().rule,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    })
    .into()
}

fn section_head<'a>(head: Element<'a, Msg>, body: Element<'a, Msg>) -> Element<'a, Msg> {
    container(
        column![
            head,
            container(Space::new().height(1.0))
                .width(Length::Fill)
                .style(|_: &Theme| container::Style {
                    background: Some(crate::theme::palette().rule.into()),
                    ..Default::default()
                }),
            body
        ]
        .spacing(12),
    )
    .padding(16)
    .width(Length::Fill)
    .style(|_: &Theme| container::Style {
        border: iced::Border {
            color: crate::theme::palette().rule,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    })
    .into()
}
