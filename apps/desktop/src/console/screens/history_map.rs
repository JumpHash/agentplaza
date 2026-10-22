//! The Map, Files and Trace tabs of a repository in History (Weft), as the web's history view draws them (its
//! `mapBody`, `filesBody` and `traceBody`). Map: the components of the repository's map, its connections (Adopt when one
//! moved on its own) and "Where is…". Files: the folder tree of main with the file read in place, its blame in the
//! gutter when on. Trace: which agent wrote each run of lines of a file, in which change and revision, and at which
//! turn. The repository tabs Queue, Keys, Sync and Ops are in history_ops.rs.

use std::collections::{BTreeMap, BTreeSet};

use iced::widget::text::Wrapping;
use iced::widget::{
    Row, Space, button, column, container, pick_list, responsive, row, text, text_input,
};
use iced::{Alignment, Border, Element, Length, Task, Theme};
use serde::Deserialize;
use serde_json::{Value, json};

use super::history::{RepoRow, size_text};
use super::history_ops;
use crate::console::client::{self, Client, enc};
use crate::console::ui::{self, Kind, Tone};
use crate::console::{Ctx, roles};
use crate::theme;

/// The most lines of a file the viewer draws (the web's cap).
const MAX_LINES: usize = 4000;

/// The width of a blame gutter.
const GUTTER: f32 = 240.0;

/// The width of the folder tree beside a file.
const TREE: f32 = 260.0;

/// The components grid: columns of at least this width, with this gap between cards (the web's auto-fill grid).
const CARD_MIN: f32 = 240.0;
const GAP: f32 = 16.0;

// ---- Data (the hub's answers) ----------------------------------------------------------------------------

/// `GET /v1/weft/repos/{repo}/map`: the map's components, whether it is drafted from the tree, and the connections.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MapAnswer {
    pub map: MapBody,
    pub drafted: bool,
    pub connections: Vec<MapConn>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MapBody {
    pub component: Vec<Component>,
}

/// One component of the map: its name, its title, the paths it holds, and its languages.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Component {
    pub name: String,
    pub title: String,
    pub paths: Vec<String>,
    pub languages: Vec<String>,
}

/// A connection to a git remote, as the map lists it (with its platform and branch).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MapConn {
    pub id: String,
    pub remote: String,
    pub platform: String,
    pub branch: String,
    pub state: String,
    pub detail: String,
}

/// `GET /v1/weft/revisions/{rev}/files`: the files a revision holds.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct Listing {
    files: Vec<FileRow>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FileRow {
    pub path: String,
    pub size: u64,
}

/// `GET /v1/weft/repos/{repo}/blame?path=`: each line of a file, with the change, the turn and the actor that wrote it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct Blame {
    lines: Vec<BlameLine>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BlameLine {
    pub line: u64,
    pub by: Written,
}

/// Who wrote a line: the change and its source, the model and the turn when an agent wrote it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Written {
    pub change: String,
    pub source: String,
    pub model: Option<String>,
    pub turn: Option<u64>,
    pub by: Actor,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Actor {
    pub kind: String,
    pub cid: Option<u64>,
}

/// `GET /v1/weft/repos/{repo}/map/where?q=`: the hits of a question.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Hit {
    pub component: Option<String>,
    pub path: String,
    pub symbol: Option<String>,
    pub why: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct Where {
    hits: Vec<Hit>,
}

/// `GET /v1/agents`: an agent's number and role, for its label.
#[derive(Debug, Deserialize)]
struct AgentRole {
    cid: u64,
    #[serde(default)]
    role: String,
}

/// A run of lines one change, source and actor wrote in a row (the blame gutter shows each run once).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub from: u64,
    pub to: u64,
    pub change: String,
    pub source: String,
    pub kind: String,
    pub cid: Option<u64>,
    pub model: Option<String>,
    pub turn: Option<u64>,
}

/// The runs of a file's lines: consecutive lines of one change, source and actor, a model change alone not breaking one.
pub fn runs(lines: &[BlameLine]) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    for line in lines {
        let by = &line.by;
        let cid = by.by.cid;
        match out.last_mut() {
            Some(last)
                if last.change == by.change
                    && last.source == by.source
                    && last.kind == by.by.kind
                    && last.cid == cid =>
            {
                last.to = line.line;
            }
            _ => out.push(Run {
                from: line.line,
                to: line.line,
                change: by.change.clone(),
                source: by.source.clone(),
                kind: by.by.kind.clone(),
                cid,
                model: by.model.clone(),
                turn: by.turn,
            }),
        }
    }
    out
}

/// What a load fetched for the view it answers.
#[derive(Debug, Clone, Default)]
pub struct Data {
    map: MapAnswer,
    /// The files on main (Files and Trace).
    files: Vec<FileRow>,
    /// Files: the open file (empty when it is not on main), its text (None when it is not text), and whether its blame
    /// was read, with its runs.
    file: String,
    text: Option<String>,
    blamed: bool,
    /// Trace: the file traced (the first file while none is picked), and its runs.
    trace: String,
    runs: Vec<Run>,
    /// The agents' labels by cid; an agent the list does not have shows as `agent #N`.
    labels: BTreeMap<u64, String>,
}

// ---- Navigation and state --------------------------------------------------------------------------------

/// The views a repository page adds to the history's tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Map,
    Files,
    Trace,
}

/// What a view's load asks for beyond the repository: the open file and its blame, or the traced file. A change to it
/// reloads the view.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Want {
    file: String,
    blame: bool,
    trace: String,
}

/// A question's answer, with the question it answers.
#[derive(Debug, Clone)]
pub struct Found {
    q: String,
    hits: Vec<Hit>,
}

/// The views' own state: the Where… question and its answer, the file open and its blame, the folders the reader
/// toggled (open at the top two levels by default, shut below), and the file traced.
#[derive(Debug, Default)]
pub struct State {
    pub error: Option<String>,
    pub notice: Option<String>,
    draft: String,
    found: Option<Found>,
    file: String,
    blame: bool,
    toggled: BTreeSet<String>,
    trace: String,
}

impl State {
    /// What a load of `view` asks for.
    pub fn want(&self, view: View) -> Want {
        match view {
            View::Map => Want::default(),
            View::Files => Want {
                file: self.file.clone(),
                blame: self.blame,
                trace: String::new(),
            },
            View::Trace => Want {
                file: String::new(),
                blame: false,
                trace: self.trace.clone(),
            },
        }
    }

    /// Drops the note of the last action (a tab was left).
    pub fn clear_notes(&mut self) {
        self.error = None;
        self.notice = None;
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// The Where… question as typed.
    Draft(String),
    /// Find: asks the question in the box.
    Find,
    Found(Result<Found, String>),
    /// Adopts a connection that moved on its own; its answer is the note or the error.
    Adopt(String),
    Adopted(Result<String, String>),
    /// Files: opens a file (its text is read).
    Open(String),
    /// Files: shows or hides the blame gutter.
    Blame,
    /// Trace: picks the traced file.
    Trace(String),
    /// Files: opens or shuts a folder (the screen does not reload for it).
    Fold(String),
}

impl Msg {
    /// Whether the message changes what a load asks for, so the view reloads.
    pub fn reloads(&self) -> bool {
        matches!(
            self,
            Msg::Open(_) | Msg::Blame | Msg::Trace(_) | Msg::Adopted(Ok(_))
        )
    }
}

// ---- Data loads and actions ------------------------------------------------------------------------------

/// Fetches what a view shows. `head` is main's revision (`root` before anything landed).
pub fn fetch(
    client: &Client,
    repo: &str,
    head: &str,
    view: View,
    want: &Want,
) -> Result<Data, String> {
    let base = format!("/v1/weft/repos/{}", enc(repo));
    let mut data = Data::default();
    match view {
        View::Map => {
            data.map = client.get(&format!("{base}/map"))?;
        }
        View::Files => {
            if head == "root" {
                return Ok(data);
            }
            data.labels = labels(client);
            data.files = listing(client, head)?;
            if !want.file.is_empty() && data.files.iter().any(|f| f.path == want.file) {
                data.file = want.file.clone();
                data.text = read_text(client, head, &want.file);
                if want.blame && data.text.is_some() {
                    let blame: Result<Blame, String> =
                        client.get(&format!("{base}/blame?path={}", enc(&want.file)));
                    if let Ok(blame) = blame {
                        data.runs = runs(&blame.lines);
                        data.blamed = true;
                    }
                }
            }
        }
        View::Trace => {
            data.labels = labels(client);
            if head != "root" {
                data.files = listing(client, head).unwrap_or_default();
            }
            let chosen = data
                .files
                .iter()
                .find(|f| f.path == want.trace)
                .or_else(|| data.files.first())
                .map(|f| f.path.clone())
                .unwrap_or_default();
            if !chosen.is_empty() {
                let blame: Result<Blame, String> =
                    client.get(&format!("{base}/blame?path={}", enc(&chosen)));
                data.runs = blame.map(|b| runs(&b.lines)).unwrap_or_default();
            }
            data.trace = chosen;
        }
    }
    Ok(data)
}

/// The repository's files on a revision.
fn listing(client: &Client, head: &str) -> Result<Vec<FileRow>, String> {
    Ok(client
        .get::<Listing>(&format!("/v1/weft/revisions/{}/files", enc(head)))?
        .files)
}

/// A file's text at a revision: None when the hub refused it or it is not UTF-8 text (the web reads it the same way).
fn read_text(client: &Client, rev: &str, path: &str) -> Option<String> {
    let target = format!("/v1/weft/revisions/{}/file?path={}", enc(rev), enc(path));
    let reply =
        apz_http::client::request(&client.target, "GET", &target, None, client.timeout).ok()?;
    if !(200..300).contains(&reply.status) {
        return None;
    }
    String::from_utf8(reply.body).ok()
}

/// The agents' labels: an agent the list does not have shows as `agent #N`.
fn labels(client: &Client) -> BTreeMap<u64, String> {
    client
        .get::<Vec<AgentRole>>("/v1/agents")
        .unwrap_or_default()
        .into_iter()
        .map(|a| (a.cid, roles::agent_label(&a.role, a.cid)))
        .collect()
}

/// The snapshot hooks (design review and captures): `APZ_WEFT_FILE=<path>` opens that file, `APZ_WEFT_BLAME=1` shows its
/// blame, `APZ_WEFT_TRACE=<path>` picks the traced file, and `APZ_WEFT_WHERE=<question>` asks Where… once. The
/// question's task is returned.
pub fn snapshot_hooks(state: &mut State, ctx: &Ctx, repo: &str) -> Task<Msg> {
    if let Ok(file) = std::env::var("APZ_WEFT_FILE") {
        state.file = file;
    }
    if let Ok(file) = std::env::var("APZ_WEFT_TRACE") {
        state.trace = file;
    }
    state.blame = std::env::var("APZ_WEFT_BLAME").is_ok_and(|v| v == "1");
    match std::env::var("APZ_WEFT_WHERE") {
        Ok(question) => {
            state.draft = question;
            update(state, Msg::Find, ctx, repo)
        }
        Err(_) => Task::none(),
    }
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx, repo: &str) -> Task<Msg> {
    match msg {
        Msg::Draft(text) => state.draft = text,
        Msg::Find => {
            let question = state.draft.trim().to_string();
            if question.is_empty() {
                state.error = Some("Type what you are looking for first".into());
                return Task::none();
            }
            state.error = None;
            let client = ctx.client.clone();
            let path = format!(
                "/v1/weft/repos/{}/map/where?q={}",
                enc(repo),
                enc(&question)
            );
            return client::task(
                move || {
                    client.get::<Where>(&path).map(|w| Found {
                        q: question,
                        hits: w.hits,
                    })
                },
                Msg::Found,
            );
        }
        Msg::Found(res) => match res {
            Ok(found) => {
                state.found = Some(found);
                state.error = None;
            }
            Err(e) => state.error = Some(e),
        },
        Msg::Adopt(connection) => {
            state.error = None;
            state.notice = None;
            let client = ctx.client.clone();
            let path = format!("/v1/weft/repos/{}/adopt", enc(repo));
            let note = format!("Adopting the commits of {connection}");
            return client::task(
                move || {
                    client
                        .post::<Value>(&path, &json!({ "connection": connection }))
                        .map(|_| note)
                },
                Msg::Adopted,
            );
        }
        Msg::Adopted(res) => match res {
            Ok(note) => {
                state.notice = Some(note);
                state.error = None;
            }
            Err(e) => {
                state.error = Some(e);
                state.notice = None;
            }
        },
        Msg::Open(path) => {
            state.file = path;
            state.error = None;
        }
        Msg::Blame => state.blame = !state.blame,
        Msg::Trace(file) => state.trace = file,
        Msg::Fold(path) => {
            if !state.toggled.remove(&path) {
                state.toggled.insert(path);
            }
        }
    }
    Task::none()
}

// ---- View ------------------------------------------------------------------------------------------------

/// The view's body under the repository's tabs.
pub fn body<'a>(
    state: &'a State,
    view: View,
    repo: &'a RepoRow,
    data: &'a Data,
) -> Element<'a, Msg> {
    match view {
        View::Map => map_body(state, data),
        View::Files => files_body(state, repo, data),
        View::Trace => trace_body(data),
    }
}

/// A line of plain text under a card, in the muted ink of the web's `.hint`.
fn hint<'a>(line: impl Into<String>) -> Element<'a, Msg> {
    ui::faint(line.into()).into()
}

/// A hairline across the page's width, in the rule colour.
fn rule<'a>() -> Element<'a, Msg> {
    container(Space::new().height(1.0))
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(theme::palette().rule.into()),
            ..Default::default()
        })
        .into()
}

/// Rows with a rule above each, as the web's bordered lists are.
fn ruled<'a>(rows: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = Vec::with_capacity(rows.len() * 2);
    for r in rows {
        items.push(rule());
        items.push(r);
    }
    column(items).width(Length::Fill).into()
}

/// A box with the rule's border and the skin's radius: a tree, a code view, a file that is not there.
fn bordered(_: &Theme) -> container::Style {
    container::Style {
        border: Border {
            color: theme::palette().rule,
            width: 1.0,
            radius: theme::skin().radius.into(),
        },
        ..Default::default()
    }
}

/// The map's components, the connections and "Where is…": the three cards of the web's map.
fn map_body<'a>(state: &'a State, data: &'a Data) -> Element<'a, Msg> {
    let parts: Vec<Element<'a, Msg>> = vec![
        ui::card("Components", components(&data.map)),
        ui::card("Connections", connections(&data.map)),
        ui::card("Where is…", where_box(state)),
    ];
    let mut all: Vec<Element<'a, Msg>> = notes(state);
    all.extend(parts);
    column(all).spacing(16).into()
}

/// The error and the note of the last action, above the cards.
fn notes<'a>(state: &'a State) -> Vec<Element<'a, Msg>> {
    let mut out: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(e) = &state.error {
        out.push(ui::error_line(e));
    }
    if let Some(n) = &state.notice {
        out.push(ui::faint(n.clone()).into());
    }
    out
}

fn components<'a>(answer: &'a MapAnswer) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if answer.drafted {
        parts.push(hint(
            "Drafted from the files on main. No map has landed yet.",
        ));
    }
    if answer.map.component.is_empty() {
        parts.push(ui::empty(
            "file",
            "No components yet",
            "Components come from the files the repository holds.",
        ));
    } else {
        parts.push(component_grid(&answer.map.component));
    }
    column(parts).spacing(12).into()
}

/// The components as the web's auto-fill grid: as many columns of at least `CARD_MIN` as the width holds, each row as
/// tall as its tallest card.
fn component_grid<'a>(components: &'a [Component]) -> Element<'a, Msg> {
    responsive(move |size| {
        let cols = ((size.width + GAP) / (CARD_MIN + GAP)).floor().max(1.0) as usize;
        let mut lines: Vec<Element<'a, Msg>> = Vec::new();
        for chunk in components.chunks(cols) {
            let mut cells: Vec<Element<'a, Msg>> = chunk
                .iter()
                .map(|c| container(component(c)).width(Length::FillPortion(1)).into())
                .collect();
            while cells.len() < cols {
                cells.push(Space::new().width(Length::FillPortion(1)).into());
            }
            lines.push(row(cells).spacing(GAP).into());
        }
        column(lines).spacing(GAP).into()
    })
    .height(Length::Shrink)
    .into()
}

/// One component: its title (or name), the name under a title, up to four paths and "+N", and its language badges.
fn component<'a>(c: &'a Component) -> Element<'a, Msg> {
    let p = theme::palette();
    let title = if c.title.is_empty() {
        c.name.clone()
    } else {
        c.title.clone()
    };
    let mut head: Vec<Element<'a, Msg>> = vec![
        text(title)
            .font(ui::SANS_SEMIBOLD)
            .size(14.0)
            .color(p.ink)
            .into(),
    ];
    if !c.title.is_empty() {
        head.push(ui::mono(c.name.clone(), 11.0, p.ink_3).into());
    }
    let mut paths: Vec<Element<'a, Msg>> = c
        .paths
        .iter()
        .take(4)
        .map(|path| ui::mono(path.clone(), 11.0, p.ink_2).into())
        .collect();
    if c.paths.len() > 4 {
        paths.push(hint(format!("+{}", c.paths.len() - 4)));
    }
    let langs: Vec<Element<'a, Msg>> = c
        .languages
        .iter()
        .map(|l| ui::badge(l, Tone::Muted))
        .collect();
    head.push(ui::chip_row(paths));
    head.push(ui::chip_row(langs));
    container(column(head).spacing(8))
        .padding(16)
        .width(Length::Fill)
        .style(bordered)
        .into()
}

fn connections<'a>(answer: &'a MapAnswer) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(moved) = answer.connections.iter().find(|c| c.state == "moved") {
        parts.push(hint(format!(
            "{} moved on its own. Adopt its commits to bring them in.",
            moved.id
        )));
    }
    if answer.connections.is_empty() {
        parts.push(hint(
            "No connection. Export goes nowhere until one is added.",
        ));
    } else {
        let rows: Vec<Element<'a, Msg>> = answer.connections.iter().map(conn_row).collect();
        parts.push(ruled(rows));
    }
    column(parts).spacing(12).into()
}

/// One connection: its id and state, its platform and branch, its remote clipped to 90 characters, its detail, and
/// Adopt when it moved on its own.
fn conn_row<'a>(c: &'a MapConn) -> Element<'a, Msg> {
    let p = theme::palette();
    let mut items: Vec<Element<'a, Msg>> = vec![
        row![
            text(c.id.clone())
                .font(ui::SANS_SEMIBOLD)
                .size(13.0)
                .color(p.ink),
            ui::badge(
                history_ops::connection_text(&c.state),
                history_ops::connection_tone(&c.state),
            ),
            ui::faint(format!("{} · {}", c.platform, c.branch)),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
        ui::mono(ui::ellipsize(&c.remote, 90), 12.0, p.ink_2).into(),
    ];
    if !c.detail.is_empty() {
        items.push(ui::faint(c.detail.clone()).into());
    }
    if c.state == "moved" {
        items.push(ui::btn_sm(
            "Adopt",
            None,
            Kind::Primary,
            Some(Msg::Adopt(c.id.clone())),
        ));
    }
    column(items)
        .spacing(8)
        .padding([12, 0])
        .width(Length::Fill)
        .into()
}

/// Where is…: the question box with Find, and the hits of the last question under it.
fn where_box<'a>(state: &'a State) -> Element<'a, Msg> {
    let question = text_input("Where is the queue?", &state.draft)
        .on_input(Msg::Draft)
        .on_submit(Msg::Find)
        .size(13.0)
        .padding(8)
        .width(Length::Fill);
    let find = ui::btn("Find", None, Kind::Primary, Some(Msg::Find));
    let mut parts: Vec<Element<'a, Msg>> = vec![
        row![question, find]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
    ];
    if let Some(found) = &state.found {
        if found.hits.is_empty() {
            parts.push(hint(format!("Nothing for “{}”.", found.q)));
        } else {
            parts.push(ui::faint(found.q.clone()).into());
            let rows: Vec<Element<'a, Msg>> = found.hits.iter().map(hit_row).collect();
            parts.push(ruled(rows));
        }
    }
    column(parts).spacing(12).into()
}

/// One hit: its path, its symbol, why it matched, and its component.
fn hit_row<'a>(h: &'a Hit) -> Element<'a, Msg> {
    let p = theme::palette();
    let mut items: Vec<Element<'a, Msg>> = vec![ui::mono(h.path.clone(), 12.0, p.ink).into()];
    if let Some(symbol) = &h.symbol {
        items.push(ui::sans(symbol.clone(), 13.0, p.ink).into());
    }
    items.push(ui::faint(format!("· {}", h.why)).into());
    if let Some(component) = &h.component {
        items.push(ui::badge(component, Tone::Info));
    }
    container(
        Row::with_children(items)
            .spacing(8)
            .align_y(Alignment::Center)
            .wrap(),
    )
    .padding([8, 0])
    .width(Length::Fill)
    .into()
}

// ---- Files -----------------------------------------------------------------------------------------------

/// The folders of the files, each level by name: folders first is the web's order, applied in `tree_view`.
#[derive(Debug, Default)]
struct Tree {
    dirs: BTreeMap<String, Tree>,
    /// Each file by its name in the folder: its path and size.
    files: BTreeMap<String, (String, u64)>,
}

fn build_tree(files: &[FileRow]) -> Tree {
    let mut root = Tree::default();
    for f in files {
        let parts: Vec<&str> = f.path.split('/').filter(|p| !p.is_empty()).collect();
        let Some((name, dirs)) = parts.split_last() else {
            continue;
        };
        let mut node = &mut root;
        for dir in dirs {
            node = node.dirs.entry((*dir).to_string()).or_default();
        }
        node.files
            .insert((*name).to_string(), (f.path.clone(), f.size));
    }
    root
}

fn files_body<'a>(state: &'a State, repo: &'a RepoRow, data: &'a Data) -> Element<'a, Msg> {
    if !repo.refs.contains_key("main") {
        return ui::empty(
            "file",
            "Nothing landed yet",
            "Files appear here once a change lands on main.",
        );
    }
    let tree = container(tree_view(state, &build_tree(&data.files), "", 0))
        .width(Length::Fixed(TREE))
        .padding(12)
        .clip(true)
        .style(bordered);
    row![tree, container(viewer(state, data)).width(Length::Fill),]
        .spacing(16)
        .align_y(Alignment::Start)
        .into()
}

/// The folder tree: a folder opens on its own at the top two levels, and the reader's toggle flips it.
fn tree_view<'a>(state: &State, node: &Tree, prefix: &str, depth: usize) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = Vec::new();
    for (name, child) in &node.dirs {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let open = (depth < 2) != state.toggled.contains(&path);
        items.push(ui::disclosure(
            name.clone(),
            theme::palette().ink_2,
            open,
            Msg::Fold(path.clone()),
        ));
        if open {
            items.push(
                container(tree_view(state, child, &path, depth + 1))
                    .padding([0, 12])
                    .into(),
            );
        }
    }
    for (name, (path, size)) in &node.files {
        items.push(file_row(name, path, *size, state.file == *path));
    }
    column(items).into()
}

/// A file in the tree: its name, its size, and the open file's selection.
fn file_row<'a>(name: &str, path: &str, size: u64, on: bool) -> Element<'a, Msg> {
    let p = theme::palette();
    let radius = theme::skin().radius;
    button(
        row![
            text(name.to_string()).size(13.0).color(p.ink),
            Space::new().width(Length::Fill),
            ui::faint(size_text(size)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .on_press(Msg::Open(path.to_string()))
    .padding([4, 8])
    .width(Length::Fill)
    .style(move |_: &Theme, status: button::Status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: if on {
                Some(p.signal_soft.into())
            } else if hovered {
                Some(p.hover.into())
            } else {
                None
            },
            text_color: p.ink,
            border: Border::default().rounded(radius),
            ..Default::default()
        }
    })
    .into()
}

/// The viewer: the path and the Blame toggle, then the text (4,000 lines at most) with the blame gutter when on.
fn viewer<'a>(state: &'a State, data: &'a Data) -> Element<'a, Msg> {
    let p = theme::palette();
    if state.file.is_empty() {
        return container(
            column![
                ui::icon("file-text", 28.0, p.ink_2),
                text("Choose a file to read it.").size(13.0).color(p.ink_3),
            ]
            .spacing(12)
            .align_x(Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fixed(420.0))
        .padding(24)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(bordered)
        .into();
    }
    if data.file.is_empty() {
        return hint("No such file on main.");
    }
    let blame = if state.blame {
        ui::btn_sm("Hide blame", None, Kind::Secondary, Some(Msg::Blame))
    } else {
        ui::btn_sm("Blame", None, Kind::Ghost, Some(Msg::Blame))
    };
    let head: Element<'a, Msg> = row![
        ui::mono(data.file.clone(), 12.0, p.ink_2),
        Space::new().width(Length::Fill),
        blame,
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into();
    let Some(source) = data.text.as_deref() else {
        return column![
            head,
            hint("This file is not text, or it could not be read."),
        ]
        .spacing(8)
        .into();
    };
    let mut lines: Vec<&str> = source.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let total = lines.len();
    let shown = total.min(MAX_LINES);

    // Each run's label sits on its first line; the runs alternate shade so the reader sees the seams.
    let mut label_at: BTreeMap<u64, String> = BTreeMap::new();
    let mut run_at: Vec<Option<usize>> = vec![None; shown];
    for (r, run) in data.runs.iter().enumerate() {
        let model = run
            .model
            .as_ref()
            .map_or(String::new(), |m| format!(" · {m}"));
        label_at.insert(
            run.from,
            format!(
                "{} · {}{model}",
                run.change,
                author_of(&data.labels, run.cid, &run.kind)
            ),
        );
        for n in run.from..=run.to {
            if let Some(slot) = (n as usize).checked_sub(1).and_then(|i| run_at.get_mut(i)) {
                *slot = Some(r);
            }
        }
    }

    let mut rows: Vec<Element<'a, Msg>> = Vec::with_capacity(shown);
    for (i, line) in lines.iter().take(shown).enumerate() {
        let n = i as u64 + 1;
        let shaded = matches!(run_at[i], Some(r) if r % 2 == 1);
        let mut cells: Vec<Element<'a, Msg>> = Vec::new();
        if data.blamed {
            let label = label_at.get(&n).cloned().unwrap_or_default();
            cells.push(
                container(ui::mono(label, 11.0, p.ink_3).wrapping(Wrapping::None))
                    .width(Length::Fixed(GUTTER))
                    .padding([4, 8])
                    .clip(true)
                    .into(),
            );
        }
        cells.push(
            container(ui::mono(n.to_string(), 12.0, p.ink_3))
                .width(Length::Fixed(48.0))
                .padding([0, 8])
                .align_x(Alignment::End)
                .into(),
        );
        let shown_text = if line.is_empty() { " " } else { line };
        cells.push(
            container(ui::mono(shown_text, 12.0, p.ink).wrapping(Wrapping::None))
                .padding([0, 8])
                .into(),
        );
        rows.push(
            container(row(cells).align_y(Alignment::Center))
                .width(Length::Shrink)
                .style(move |_: &Theme| container::Style {
                    background: if shaded {
                        Some(theme::palette().sheet_2.into())
                    } else {
                        None
                    },
                    ..Default::default()
                })
                .into(),
        );
    }
    let code = container(ui::scroll(ui::scroll_x(column(rows))))
        .width(Length::Fill)
        .max_height(560.0)
        .style(|_: &Theme| container::Style {
            background: Some(theme::palette().sheet.into()),
            border: Border {
                color: theme::palette().rule,
                width: 1.0,
                radius: theme::skin().radius.into(),
            },
            ..Default::default()
        });
    let mut out: Vec<Element<'a, Msg>> = vec![head, code.into()];
    if total > MAX_LINES {
        out.push(hint(format!(
            "Showing the first {MAX_LINES} of {total} lines."
        )));
    }
    column(out).spacing(8).into()
}

/// A line's author as the web names it: the agent's label, or the kind when no agent wrote it.
fn author_of(labels: &BTreeMap<u64, String>, cid: Option<u64>, kind: &str) -> String {
    match cid {
        None => kind.to_string(),
        Some(c) => labels
            .get(&c)
            .cloned()
            .unwrap_or_else(|| roles::agent_label("", c)),
    }
}

// ---- Trace -----------------------------------------------------------------------------------------------

fn trace_body<'a>(data: &'a Data) -> Element<'a, Msg> {
    if data.files.is_empty() {
        return ui::card(
            "Agent trace",
            ui::empty(
                "git",
                "No traced files",
                "Agent trace shows once a repository has agent-written lines.",
            ),
        );
    }
    let paths: Vec<String> = data.files.iter().map(|f| f.path.clone()).collect();
    let picker = pick_list(paths, Some(data.trace.clone()), Msg::Trace).width(Length::Fill);
    let top = field(
        "File",
        picker.into(),
        &format!(
            "{} in this file, in line order.",
            super::history::plural(data.runs.len() as u64, "run", "runs")
        ),
    );
    let body: Element<'a, Msg> = if data.runs.is_empty() {
        hint("No lines in this file.")
    } else {
        runs_table(&data.labels, &data.runs)
    };
    ui::card("Agent trace", column![top, body].spacing(12).into())
}

/// A labelled control with its hint under it.
fn field<'a>(label: &str, control: Element<'a, Msg>, hint_text: &str) -> Element<'a, Msg> {
    column![
        ui::sans(label.to_string(), 12.0, theme::palette().ink_3),
        control,
        ui::faint(hint_text.to_string()),
    ]
    .spacing(6)
    .width(Length::Fill)
    .into()
}

/// The runs as the web's table: the lines, who wrote them, the change, the revision and the turn.
fn runs_table<'a>(labels: &BTreeMap<u64, String>, runs: &[Run]) -> Element<'a, Msg> {
    let p = theme::palette();
    let widths = [
        Length::Shrink,
        Length::FillPortion(3),
        Length::FillPortion(3),
        Length::FillPortion(3),
        Length::Shrink,
    ];
    let heads: Vec<Element<'a, Msg>> = ["Lines", "Written by", "Change", "Revision", "Turn"]
        .into_iter()
        .map(|h| ui::label(h).into())
        .collect();
    let mut lines: Vec<Element<'a, Msg>> = vec![
        cells(&widths, heads),
        container(Space::new().height(1.0))
            .width(Length::Fill)
            .style(|_: &Theme| container::Style {
                background: Some(theme::palette().rule_strong.into()),
                ..Default::default()
            })
            .into(),
    ];
    for r in runs {
        let items: Vec<Element<'a, Msg>> = vec![
            ui::mono(format!("L{}–{}", r.from, r.to), 12.0, p.ink).into(),
            written_by(labels, r),
            ui::mono(r.change.clone(), 12.0, p.ink_2).into(),
            ui::mono(r.source.clone(), 12.0, p.ink_2).into(),
            ui::faint(r.turn.map_or("—".to_string(), |t| format!("turn {t}"))).into(),
        ];
        lines.push(cells(&widths, items));
        lines.push(rule());
    }
    column(lines).width(Length::Fill).into()
}

/// One table row: its cells in their columns, padded 9 by 12 as the web's cells are.
fn cells<'a>(widths: &[Length; 5], items: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let laid: Vec<Element<'a, Msg>> = widths
        .iter()
        .zip(items)
        .map(|(w, item)| container(item).width(*w).into())
        .collect();
    container(row(laid).spacing(12).align_y(Alignment::Center))
        .padding([9, 12])
        .width(Length::Fill)
        .into()
}

/// Who wrote a run: its owner, its agent with the model, or its kind.
fn written_by<'a>(labels: &BTreeMap<u64, String>, r: &Run) -> Element<'a, Msg> {
    let (name, badge, tone) = match r.kind.as_str() {
        "owner" => ("Owner".to_string(), "human".to_string(), Tone::Muted),
        "agent" => (
            author_of(labels, r.cid, "agent"),
            r.model
                .as_ref()
                .map_or("ai".to_string(), |m| format!("ai · {m}")),
            Tone::Signal,
        ),
        other => (other.to_string(), other.to_string(), Tone::Muted),
    };
    row![
        text(name)
            .font(ui::SANS_SEMIBOLD)
            .size(13.0)
            .color(theme::palette().ink),
        ui::badge(&badge, tone),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(n: u64, change: &str, kind: &str, cid: Value) -> BlameLine {
        serde_json::from_value(json!({
            "line": n,
            "by": { "change": change, "source": "agent", "model": "m", "turn": 1,
                    "by": { "kind": kind, "cid": cid } }
        }))
        .expect("a blame line of the hub's shape")
    }

    #[test]
    fn a_run_is_one_change_source_and_actor_in_a_row() {
        let lines = vec![
            line(1, "w-1", "agent", json!(211)),
            line(2, "w-1", "agent", json!(211)),
            line(3, "w-2", "agent", json!(211)),
            line(4, "w-2", "owner", Value::Null),
        ];
        let got: Vec<(u64, u64)> = runs(&lines).iter().map(|r| (r.from, r.to)).collect();
        assert_eq!(got, vec![(1, 2), (3, 3), (4, 4)]);
    }

    #[test]
    fn folders_come_first_and_each_level_is_by_name() {
        let files = vec![
            FileRow {
                path: "b.txt".into(),
                size: 1,
            },
            FileRow {
                path: "src/z.ts".into(),
                size: 2,
            },
            FileRow {
                path: "docs/a.md".into(),
                size: 3,
            },
            FileRow {
                path: "src/lib/x.ts".into(),
                size: 4,
            },
        ];
        let tree = build_tree(&files);
        let dirs: Vec<&String> = tree.dirs.keys().collect();
        assert_eq!(dirs, vec!["docs", "src"]);
        assert_eq!(tree.files.keys().collect::<Vec<_>>(), vec!["b.txt"]);
        let src = &tree.dirs["src"];
        assert_eq!(src.files.keys().collect::<Vec<_>>(), vec!["z.ts"]);
        assert_eq!(src.dirs.keys().collect::<Vec<_>>(), vec!["lib"]);
    }
}
