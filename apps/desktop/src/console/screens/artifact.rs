//! The artifact viewer: one artifact on its own screen, with its versions, its panels on the 12-column grid, and its
//! actions (share, export, delete). The bodies are drawn by [`crate::console::view`] and the Metrics panels; the
//! screen owns the loaded pictures, the parsed documents, the dataset pages, the live HTML frames, and the code panels
//! the owner opened in full.

use std::collections::{HashMap, HashSet};
use std::ops::{Deref, DerefMut};

use apz_view::artifact::Artifact;
use apz_view::{Body, HtmlBody, Panel};
use iced::widget::{column, container, image, pick_list, row, text};
use iced::{Alignment, Element, Fill, Length, Subscription, Task};
use serde::Deserialize;

use super::{Id, library, metrics};
use crate::console::view::dataset::{DatasetMsg, DatasetState, RowsPage};
use crate::console::view::webview::{self, FrameMsg, Frames};
use crate::console::view::{self, ViewAction};
use crate::console::{Ctx, client, share, ui};
use crate::theme;

/// The press handler the artifact bodies take: a body's press becomes this screen's message.
static ON_VIEW: fn(ViewAction) -> Msg = Msg::View;

/// One version of the artifact, as `GET /v1/artifacts/{id}/versions` lists it, oldest first.
#[derive(Debug, Clone, Deserialize)]
pub struct VersionInfo {
    pub version: u32,
    pub created_ms: u64,
    pub title: String,
}

/// The address `GET /v1/artifacts/{id}/frame` answers with: the content origin's capability URL of one HTML panel.
#[derive(Debug, Clone, Deserialize)]
struct FrameReply {
    url: String,
}

/// A version of the artifact, shown as `v<n>` in the version pick list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ver(pub u32);

impl std::fmt::Display for Ver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "v{}", self.0)
    }
}

/// The exports the head offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Csv,
    Json,
    Png,
}

impl Format {
    fn param(self) -> &'static str {
        match self {
            Format::Csv => "csv",
            Format::Json => "json",
            Format::Png => "png",
        }
    }
}

/// The live HTML frames, wrapped so the screen state keeps its derived `Debug`; the wrapper dereferences to
/// [`Frames`].
#[derive(Default)]
pub struct Live(Frames);

impl std::fmt::Debug for Live {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Live")
    }
}

impl Deref for Live {
    type Target = Frames;

    fn deref(&self) -> &Frames {
        &self.0
    }
}

impl DerefMut for Live {
    fn deref_mut(&mut self) -> &mut Frames {
        &mut self.0
    }
}

/// The Share dialog, wrapped so the screen state keeps its derived `Debug`.
pub struct Sharing(pub share::Dialog);

impl std::fmt::Debug for Sharing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sharing")
    }
}

/// The artifact screen's state: the artifact it shows, the version it is at, and what the owner has opened.
#[derive(Debug, Default)]
pub struct State {
    /// The artifact's id, from the screen's id.
    pub id: String,
    /// The version the owner picked; `None` is the newest.
    pub version: Option<u32>,
    pub artifact: Option<Artifact>,
    pub versions: Vec<VersionInfo>,
    /// Pictures and HTML stills the hub has sent, by blob id.
    pub blobs: view::Blobs,
    /// Parsed Markdown by panel id.
    pub docs: view::Documents,
    /// The dataset panels' states by panel id: where each page starts, how it is sorted and filtered, and its rows.
    pub datasets: HashMap<String, DatasetState>,
    /// The Metrics panels' own state, so a chart or table in an artifact draws as it does on Metrics.
    pub metrics: metrics::State,
    /// The code panels the owner opened in full.
    pub expanded: HashSet<String>,
    /// The picture the owner opened to full size, by blob id.
    pub image: Option<String>,
    /// The live HTML frames of the artifact's panels.
    pub frames: Live,
    /// The Share dialog, while it is open over the screen.
    pub share: Option<Sharing>,
    /// Opens the Share dialog once the artifact loads (a rich chip's Share asks for it), then clears.
    pub open_share_on_load: bool,
    /// The HTML panel whose frame address is on its way from the hub.
    pub running: Option<String>,
    pub confirm_delete: bool,
    /// The delete dialog's title, "Delete <title>?", set when the owner asks.
    pub delete_title: String,
    /// The Export menu, open under the head.
    pub export_open: bool,
    pub notice: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Loaded(Result<Artifact, String>),
    Versions(Result<Vec<VersionInfo>, String>),
    Blob(String, Result<Vec<u8>, String>),
    /// A press in a dataset's table: its filter, a sort, or a page; the panel's id comes first.
    Dataset(String, DatasetMsg),
    /// A page of a blob-backed dataset from the hub, by panel id.
    Rows(String, Result<RowsPage, String>),
    /// The frame address of an HTML panel, by panel id, or why the hub did not give one.
    FrameUrl(String, Result<String, String>),
    /// A message from the live HTML frames.
    Frame(FrameMsg),
    /// The app's appearance changed; the live frames take the new palette.
    Theme,
    Version(Ver),
    /// An export of one panel (by its id) in a format.
    Export(Format, String),
    /// The head's Export press: opens or closes its menu.
    ExportMenu,
    Exported(Result<String, String>),
    /// The head's Share press: opens the share dialog for this artifact at the version it shows.
    ShareOpen,
    /// A message from the share dialog.
    Share(share::Msg),
    AskDelete,
    CancelDelete,
    Delete,
    Deleted(Result<(), String>),
    /// A press in an artifact body.
    View(ViewAction),
    CloseImage,
    Metrics(metrics::Msg),
}

/// The state for opening one artifact.
pub fn opened(id: String) -> State {
    State {
        id,
        ..Default::default()
    }
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        Msg::Metrics(m) => metrics::go(m),
        _ => None,
    }
}

/// The screen's subscription: the live frames' messages, and the appearance changes they take their palette from.
pub fn subscription(state: &State) -> Subscription<Msg> {
    Subscription::batch([
        state.frames.subscription().map(Msg::Frame),
        iced::system::theme_changes().map(|_| Msg::Theme),
    ])
}

/// The artifact's path on the hub.
fn base(id: &str) -> String {
    format!("/v1/artifacts/{}", client::enc(id))
}

/// Loads the artifact (at the picked version) and its versions.
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    if state.id.is_empty() {
        return Task::none();
    }
    if crate::console::rich::take_share_asked() {
        state.open_share_on_load = true;
    }
    let base = base(&state.id);
    let path = match state.version {
        Some(v) => format!("{base}?v={v}"),
        None => base.clone(),
    };
    let artifact_client = ctx.client.clone();
    let artifact = client::task(move || artifact_client.get::<Artifact>(&path), Msg::Loaded);
    let versions_client = ctx.client.clone();
    let versions_path = format!("{base}/versions");
    let versions = client::task(
        move || versions_client.get::<Vec<VersionInfo>>(&versions_path),
        Msg::Versions,
    );
    Task::batch([artifact, versions])
}

/// Fetches one blob's bytes from the artifact's store.
fn fetch(client: &client::Client, path: &str) -> Result<Vec<u8>, String> {
    let reply = apz_http::client::request(&client.target, "GET", path, None, client.timeout)
        .map_err(|e| e.to_string())?;
    if (200..300).contains(&reply.status) {
        Ok(reply.body)
    } else {
        Err(format!("HTTP {}", reply.status))
    }
}

/// The blobs a version's panels show as pictures: image panels and HTML stills.
fn blob_ids(a: &Artifact) -> Vec<String> {
    a.panels
        .iter()
        .filter_map(|p| match &p.body {
            Body::Image(i) => Some(vec![i.blob.id.clone()]),
            Body::Html(h) => h.still.as_ref().map(|b| vec![b.id.clone()]),
            _ => None,
        })
        .flatten()
        .collect()
}

/// The request for one page of a blob-backed dataset: its offset, sort, filter and the version it is at.
fn rows_task(ctx: &Ctx, id: &str, version: u32, panel: &str, ds: &DatasetState) -> Task<Msg> {
    let mut query = vec![
        format!("panel={}", client::enc(panel)),
        format!("offset={}", ds.offset),
        format!("limit={}", view::dataset::PAGE),
    ];
    if let Some((key, ascending)) = &ds.sort {
        query.push(format!("sort={}", client::enc(key)));
        query.push(format!("desc={}", u8::from(!*ascending)));
    }
    query.push(format!("filter={}", client::enc(&ds.filter)));
    query.push(format!("v={version}"));
    let path = format!("{}/rows?{}", base(id), query.join("&"));
    let rows_client = ctx.client.clone();
    let key = panel.to_string();
    client::task(
        move || rows_client.get::<RowsPage>(&path),
        move |r| Msg::Rows(key.clone(), r),
    )
}

/// Downloads an export of the artifact into the Downloads folder.
fn export(state: &State, format: Format, panel: &str, ctx: &Ctx) -> Task<Msg> {
    let Some(a) = state.artifact.as_ref() else {
        return Task::none();
    };
    let mut query = format!("format={}&panel={}", format.param(), client::enc(panel));
    let name = format!("{}-{}.{}", a.title, panel, format.param());
    if let Some(v) = state.version {
        query.push_str(&format!("&v={v}"));
    }
    let path = format!("{}/export?{query}", base(&state.id));
    let export_client = ctx.client.clone();
    client::task(
        move || {
            let bytes = fetch(&export_client, &path)?;
            library::save_download(&name, &bytes)
        },
        Msg::Exported,
    )
}

/// Opens the Share dialog for the artifact at one version, and hides the live frames under it.
fn open_share(state: &mut State, hub: &client::Client, title: &str, version: u32) -> Task<Msg> {
    let (dialog, task) = share::open(hub, "artifact", &state.id, title, Some(version));
    state.share = Some(Sharing(dialog));
    state.frames.hide_all(true);
    task.map(Msg::Share)
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(Ok(a)) => {
            state.error = None;
            state.docs.clear();
            for p in &a.panels {
                if let Body::Document { markdown } = &p.body {
                    state
                        .docs
                        .insert(p.id.clone(), view::document::parse(markdown));
                }
            }
            let path = base(&state.id);
            let mut tasks = Vec::new();
            for id in blob_ids(&a) {
                if state.blobs.contains_key(&id) {
                    continue;
                }
                let blob_client = ctx.client.clone();
                let url = format!("{path}/blobs/{}", client::enc(&id));
                let key = id.clone();
                tasks.push(client::task(
                    move || fetch(&blob_client, &url),
                    move |r| Msg::Blob(key.clone(), r),
                ));
            }
            for p in &a.panels {
                let Body::Dataset(body) = &p.body else {
                    continue;
                };
                let ds = state.datasets.entry(p.id.clone()).or_default();
                if body.blob.is_some() {
                    ds.loading = true;
                    ds.error = None;
                    tasks.push(rows_task(ctx, &state.id, a.version, &p.id, ds));
                } else {
                    ds.page = Some(view::dataset::local_page(body, ds));
                }
            }
            if std::mem::take(&mut state.open_share_on_load) {
                tasks.push(open_share(state, &ctx.client, &a.title, a.version));
            }
            state.artifact = Some(a);
            Task::batch(tasks)
        }
        Msg::Loaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::Versions(Ok(versions)) => {
            state.versions = versions;
            Task::none()
        }
        Msg::Versions(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::Blob(id, Ok(bytes)) => {
            state.blobs.insert(id, image::Handle::from_bytes(bytes));
            Task::none()
        }
        Msg::Blob(_, Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::Dataset(panel, msg) => {
            let Some(a) = state.artifact.as_ref() else {
                return Task::none();
            };
            let Some(Body::Dataset(body)) =
                a.panels.iter().find(|p| p.id == panel).map(|p| &p.body)
            else {
                return Task::none();
            };
            let Some(ds) = state.datasets.get_mut(&panel) else {
                return Task::none();
            };
            if view::dataset::update(body, ds, msg) {
                return rows_task(ctx, &state.id, a.version, &panel, ds);
            }
            let page = view::dataset::local_page(body, ds);
            ds.page = Some(page);
            Task::none()
        }
        Msg::Rows(panel, Ok(page)) => {
            if let Some(ds) = state.datasets.get_mut(&panel) {
                ds.page = Some(page);
                ds.loading = false;
                ds.error = None;
            }
            Task::none()
        }
        Msg::Rows(panel, Err(e)) => {
            if let Some(ds) = state.datasets.get_mut(&panel) {
                ds.loading = false;
                ds.error = Some(e);
            }
            Task::none()
        }
        Msg::FrameUrl(panel, Ok(url)) => {
            state.running = None;
            let Some(version) = state.artifact.as_ref().map(|a| a.version) else {
                return Task::none();
            };
            if webview::LIVE {
                let run = FrameMsg::Run {
                    artifact: state.id.clone(),
                    version,
                    panel,
                    url,
                };
                return state.frames.update(run).map(Msg::Frame);
            }
            // Without a live web view the frame opens in the browser, as the still card says.
            if ui::openable(&url) {
                if let Err(e) = ui::open_url(&url) {
                    state.error = Some(e);
                }
            } else {
                state.notice = Some("This frame cannot be opened here.".to_string());
            }
            Task::none()
        }
        Msg::FrameUrl(_, Err(e)) => {
            state.running = None;
            state.error = Some(e);
            Task::none()
        }
        Msg::Frame(frame) => state.frames.update(frame).map(Msg::Frame),
        Msg::Theme => {
            state.frames.send_theme();
            Task::none()
        }
        Msg::Version(Ver(n)) => {
            state.version = Some(n);
            load(state, ctx)
        }
        Msg::Export(format, panel) => {
            state.export_open = false;
            export(state, format, &panel, ctx)
        }
        Msg::ExportMenu => {
            state.export_open = !state.export_open;
            Task::none()
        }
        Msg::Exported(Ok(path)) => {
            state.notice = Some(format!("Saved to {path}"));
            Task::none()
        }
        Msg::Exported(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::ShareOpen => {
            let Some((title, version)) = state
                .artifact
                .as_ref()
                .map(|a| (a.title.clone(), a.version))
            else {
                return Task::none();
            };
            open_share(state, &ctx.client, &title, version)
        }
        Msg::Share(msg) => {
            let Some(sharing) = state.share.as_mut() else {
                return Task::none();
            };
            let (task, closed) = share::update(&mut sharing.0, msg);
            if closed {
                state.share = None;
                state.frames.hide_all(false);
            }
            task.map(Msg::Share)
        }
        Msg::AskDelete => {
            state.delete_title = state
                .artifact
                .as_ref()
                .map(|a| format!("Delete {}?", a.title))
                .unwrap_or_default();
            state.confirm_delete = true;
            state.frames.hide_all(true);
            Task::none()
        }
        Msg::CancelDelete => {
            state.confirm_delete = false;
            state.frames.hide_all(false);
            Task::none()
        }
        Msg::Delete => {
            state.confirm_delete = false;
            state.frames.hide_all(false);
            let delete_client = ctx.client.clone();
            let path = base(&state.id);
            client::task(
                move || delete_client.delete(&path).map(|_| ()),
                Msg::Deleted,
            )
        }
        Msg::Deleted(Ok(())) => Task::done(Msg::Go(Id::Library)),
        Msg::Deleted(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::View(action) => match action {
            ViewAction::OpenImage(blob) => {
                state.frames.hide_all(true);
                state.image = Some(blob);
                Task::none()
            }
            ViewAction::CopyCode(code) => {
                state.notice = Some("Copied the code.".to_string());
                iced::clipboard::write::<Msg>(code)
            }
            ViewAction::RunHtml(panel) => {
                let Some(version) = state.artifact.as_ref().map(|a| a.version) else {
                    return Task::none();
                };
                let path = format!(
                    "{}/frame?v={version}&panel={}",
                    base(&state.id),
                    client::enc(&panel)
                );
                state.running = Some(panel.clone());
                let frame_client = ctx.client.clone();
                let key = panel;
                client::task(
                    move || frame_client.get::<FrameReply>(&path).map(|f| f.url),
                    move |r| Msg::FrameUrl(key.clone(), r),
                )
            }
            ViewAction::LoadMore(panel) => {
                state.expanded.insert(panel);
                Task::none()
            }
            ViewAction::OpenLink(url) => {
                if ui::openable(&url) {
                    if let Err(e) = ui::open_url(&url) {
                        state.error = Some(e);
                    }
                } else {
                    state.notice = Some("This link cannot be opened here.".to_string());
                }
                Task::none()
            }
        },
        Msg::CloseImage => {
            state.image = None;
            state.frames.hide_all(false);
            Task::none()
        }
        Msg::Metrics(m) => metrics::update(&mut state.metrics, m, ctx).map(Msg::Metrics),
    }
}

/// The screen: the artifact's page, with the Share dialog stacked over it while it is open.
pub fn view<'a>(state: &'a State, ctx: &'a Ctx) -> Element<'a, Msg> {
    let page = ui::with_modal(page(state, ctx), delete_dialog(state));
    match &state.share {
        Some(sharing) => {
            iced::widget::stack([page, share::view(&sharing.0).map(Msg::Share)]).into()
        }
        None => page,
    }
}

/// The artifact's page: its head, its notices, and its panels.
fn page<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let Some(a) = state.artifact.as_ref() else {
        let body: Element<'a, Msg> = match &state.error {
            Some(e) => ui::error_line(e),
            None => ui::loading(),
        };
        return ui::sheet(ui::view_head("Artifact", vec![]), ui::scroll(body).into());
    };
    let mut items: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(e) = &state.error {
        items.push(ui::error_line(e));
    }
    if let Some(n) = &state.notice {
        items.push(ui::faint(n.as_str()).into());
    }
    if let Some(question) = state.frames.prompt() {
        items.push(
            row![
                ui::faint(question),
                ui::btn(
                    "Yes",
                    None,
                    ui::Kind::Primary,
                    Some(Msg::Frame(FrameMsg::Confirm(true)))
                ),
                ui::btn(
                    "No",
                    None,
                    ui::Kind::Ghost,
                    Some(Msg::Frame(FrameMsg::Confirm(false)))
                ),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
        );
    }
    if let Some(blob) = &state.image {
        items.push(full_image(state, blob.as_str()));
    }
    items.push(grid(state, a));
    ui::sheet(
        head(state, a),
        ui::scroll(column(items).spacing(16).width(Fill)).into(),
    )
}

/// The head, as the web's artifact page: the kicker back to the Library, the title with Share, Export and Delete beside
/// it and the version picker, the artifact's sub line, and its kind, version, author and age. The Export menu opens
/// under the head.
fn head<'a>(state: &'a State, a: &'a Artifact) -> Element<'a, Msg> {
    let pal = theme::palette();
    let back = iced::widget::button(ui::faint("Library"))
        .on_press(Msg::Go(Id::Library))
        .padding(0)
        .style(iced::widget::button::text);
    let kicker = row![back, ui::faint("/"), ui::faint("Artifact")]
        .spacing(8)
        .align_y(Alignment::Center);
    let versions: Vec<Ver> = if state.versions.is_empty() {
        (1..=a.latest.max(a.version)).map(Ver).collect()
    } else {
        state.versions.iter().map(|v| Ver(v.version)).collect()
    };
    let controls = row![
        pick_list(versions, Some(Ver(a.version)), Msg::Version).style(theme::pick_list),
        ui::btn("Share", None, ui::Kind::Ghost, Some(Msg::ShareOpen)),
        ui::btn_sm("Export", None, ui::Kind::Ghost, Some(Msg::ExportMenu)),
        ui::btn("Delete", None, ui::Kind::Ghost, Some(Msg::AskDelete)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let title_row = row![
        text(a.title.as_str())
            .font(theme::SANS_SEMIBOLD)
            .size(theme::TYPE_PAGE)
            .color(pal.ink)
            .width(Fill),
        controls,
    ]
    .spacing(16)
    .align_y(Alignment::Center);
    let mut items: Vec<Element<'a, Msg>> = vec![kicker.into(), title_row.into()];
    if !a.sub.is_empty() {
        items.push(ui::sans(a.sub.as_str(), theme::TYPE_PROSE, pal.ink_2).into());
    }
    items.push(
        row![
            ui::badge(a.kind.as_str(), ui::Tone::Signal),
            ui::badge(&format!("v{}", a.version), ui::Tone::Muted),
            ui::faint(format!("by {}", a.author)),
            ui::faint(format!("updated {}", library::ago(a.updated_ms as i64))),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
    );
    if state.export_open {
        items.push(
            container(export_menu(a))
                .width(Fill)
                .align_x(Alignment::End)
                .into(),
        );
    }
    column(items).spacing(8).width(Fill).into()
}

/// The Export menu, as the web's: CSV and JSON for each table or dataset panel, PNG for each chart, bars and tiles
/// panel. Each item exports that panel.
fn export_menu<'a>(a: &'a Artifact) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = Vec::new();
    for (n, p) in a.panels.iter().enumerate() {
        let label = if p.title.is_empty() {
            format!("Panel {}", n + 1)
        } else {
            p.title.clone()
        };
        if matches!(p.body, Body::Table(_) | Body::Dataset(_)) {
            for (format, name) in [(Format::Csv, "CSV"), (Format::Json, "JSON")] {
                items.push(ui::btn_sm(
                    &format!("{name} · {label}"),
                    None,
                    ui::Kind::Ghost,
                    Some(Msg::Export(format, p.id.clone())),
                ));
            }
        } else if matches!(
            p.body,
            Body::Chart(_) | Body::Bars { .. } | Body::Tiles { .. }
        ) {
            items.push(ui::btn_sm(
                &format!("PNG · {label}"),
                None,
                ui::Kind::Ghost,
                Some(Msg::Export(Format::Png, p.id.clone())),
            ));
        }
    }
    if items.is_empty() {
        items.push(ui::faint("Nothing to export: no table, chart or tile panels.").into());
    }
    container(column(items).spacing(4).width(Fill))
        .padding(8)
        .width(Length::Fixed(320.0))
        .style(theme::pop)
        .into()
}

/// The delete dialog over the page, while the owner has asked to delete the artifact (the web's confirm): what it
/// removes, and Cancel and Delete in the foot. A deleted artifact leaves the screen for the Library.
fn delete_dialog(state: &State) -> Option<Element<'_, Msg>> {
    if !state.confirm_delete {
        return None;
    }
    let body = ui::faint(
        "Removes the artifact and all its versions. Its share links stop working. This cannot be undone.",
    );
    Some(ui::modal(
        state.delete_title.as_str(),
        body.into(),
        vec![
            ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CancelDelete)),
            ui::btn("Delete", None, ui::Kind::Danger, Some(Msg::Delete)),
        ],
        Msg::CancelDelete,
        440.0,
    ))
}

/// The panels on the 12-column grid: a row takes panels while their spans add up to at most 12.
fn grid<'a>(state: &'a State, a: &'a Artifact) -> Element<'a, Msg> {
    let mut rows: Vec<Element<'a, Msg>> = Vec::new();
    let mut current: Vec<Element<'a, Msg>> = Vec::new();
    let mut used: u16 = 0;
    for p in &a.panels {
        let span = u16::from(p.span.clamp(1, 12));
        if used + span > 12 && !current.is_empty() {
            rows.push(line(std::mem::take(&mut current)));
            used = 0;
        }
        used += span;
        current.push(
            container(panel_el(state, p))
                .width(Length::FillPortion(span))
                .into(),
        );
    }
    if !current.is_empty() {
        rows.push(line(current));
    }
    column(rows).spacing(16).width(Fill).into()
}

/// One row of panels.
fn line<'a>(items: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    row(items).spacing(16).width(Fill).into()
}

/// One panel: a picture, Markdown, code, HTML, a dataset or a graph is framed here; the Metrics bodies draw their own
/// frame.
fn panel_el<'a>(state: &'a State, p: &'a Panel) -> Element<'a, Msg> {
    match &p.body {
        Body::Image(_) | Body::Document { .. } => framed(
            p,
            view::artifact_body(&p.id, &p.body, &state.blobs, &state.docs, &ON_VIEW),
        ),
        Body::Code(c) => framed(
            p,
            view::code::code_view(&p.id, c, state.expanded.contains(&p.id), &ON_VIEW),
        ),
        Body::Html(h) => framed(p, html_body(state, p, h)),
        Body::Dataset(d) => match state.datasets.get(&p.id) {
            Some(ds) => {
                let id = p.id.clone();
                framed(
                    p,
                    view::dataset::dataset(d, ds, move |m| Msg::Dataset(id.clone(), m)),
                )
            }
            None => framed(p, view::body(&p.body)),
        },
        Body::Graph(g) => framed(p, view::graph::graph(g)),
        _ => metrics::panels::panel(p, &state.metrics).map(Msg::Metrics),
    }
}

/// Frames a body with the panel's title and subtitle.
fn framed<'a>(p: &'a Panel, body: Element<'a, Msg>) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = Vec::new();
    if !p.sub.is_empty() {
        items.push(ui::faint(p.sub.as_str()).into());
    }
    items.push(body);
    let inner: Element<'a, Msg> = column(items).spacing(8).width(Fill).into();
    if p.title.is_empty() {
        inner
    } else {
        ui::card(p.title.as_str(), inner)
    }
}

/// An HTML panel: its live frame when one runs, else its still picture, if the hub sent one, and a Run action that
/// opens it in a sandboxed frame.
fn html_body<'a>(state: &'a State, p: &'a Panel, h: &'a HtmlBody) -> Element<'a, Msg> {
    let live = state.artifact.as_ref().and_then(|a| {
        state
            .frames
            .view_of(&state.id, a.version, &p.id, h.height as f32)
    });
    if let Some(frame) = live {
        return frame.map(Msg::Frame);
    }
    let still: Element<'a, Msg> = match h.still.as_ref().and_then(|b| state.blobs.get(&b.id)) {
        Some(handle) => image(handle.clone()).width(Fill).into(),
        None => ui::faint(format!("{} · {} px high", h.entry, h.height)).into(),
    };
    let note: Element<'a, Msg> = if state.running.as_deref() == Some(p.id.as_str()) {
        ui::faint("Opening the frame.").into()
    } else if webview::LIVE {
        ui::faint("Runs in a sandboxed frame.").into()
    } else {
        ui::faint("Opens in your browser.").into()
    };
    let run = ui::btn(
        "Run",
        None,
        ui::Kind::Primary,
        Some(Msg::View(ViewAction::RunHtml(p.id.clone()))),
    );
    column![still, note, run].spacing(8).width(Fill).into()
}

/// A picture opened to full size, with a Close action.
fn full_image<'a>(state: &'a State, blob: &'a str) -> Element<'a, Msg> {
    let shown: Element<'a, Msg> = match state.blobs.get(blob) {
        Some(handle) => image(handle.clone()).width(Fill).into(),
        None => ui::faint("Loading the picture.").into(),
    };
    column![
        ui::btn("Close", None, ui::Kind::Ghost, Some(Msg::CloseImage)),
        shown,
    ]
    .spacing(8)
    .width(Fill)
    .into()
}
