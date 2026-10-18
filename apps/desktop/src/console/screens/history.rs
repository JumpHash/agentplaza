//! The History screen (Weft, parity with the console's history view): the plaza's repositories on Weft, each with its
//! changes (a query box with presets over `log?q=`), a change's page (its sign-off chain as dots, its intents, and
//! Expert: its revisions and each record's predicate), and its releases (notes as plain text, artifacts, and the owner's
//! Sign for a proposed one). Drill-downs are the screen's own state, as the Git screen's were. It replaces the Git screen
//! in the sidebar; the Git screen stays in the tree.

use std::collections::BTreeMap;

use iced::widget::{Space, button, column, container, row, text, text_input};
use iced::{Alignment, Border, Color, Element, Length, Task, Theme};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Id, approvals};
use crate::console::client::{self, Client, enc};
use crate::console::ui::{self, INK, INK_2, INK_3, Kind, Tone};
use crate::console::{Ctx, events};
use crate::theme;

/// How many changes one page of a repository's log asks for.
const LOG_LIMIT: u32 = 50;

/// The width of one step of the sign-off chain.
const NODE_WIDTH: f32 = 108.0;

// ---- Data (the hub's Weft answers; the shapes are the samples the console was written against) -----------

/// `GET /v1/weft`: whether Weft runs on the hub.
#[derive(Debug, Deserialize)]
struct Status {
    running: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Repos {
    repos: Vec<RepoRow>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Log {
    changes: Vec<ChangeRow>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Releases {
    releases: Vec<ReleaseRow>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Artifacts {
    artifacts: Vec<Stored>,
}

/// One repository, as `GET /v1/weft/repos` lists it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RepoRow {
    pub name: String,
    pub description: String,
    pub project: Option<String>,
    pub landed: u64,
    pub open_changes: u64,
    /// The head of each ref, e.g. `main` at `w-1.l`.
    pub refs: BTreeMap<String, String>,
    pub connections: Vec<Connection>,
}

/// A repository's connection to a git remote, and its state (`in-step`, `pushed`, `moved`, `dry-run`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Connection {
    pub id: String,
    pub state: String,
}

/// An intent: what a change does, as its kind and title (the summary and covered paths come with the change's page).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Intent {
    pub kind: String,
    pub title: String,
    pub summary: String,
    pub covers: Vec<String>,
}

/// One change in a repository's log.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ChangeRow {
    pub change: String,
    pub title: String,
    pub state: String,
    pub rounds: u64,
    pub at_ms: u64,
    pub intents: Vec<Intent>,
    /// Set for a change imported from git: its commit and its author (`Name <email>`).
    pub imported: Option<ImportedRow>,
}

/// Where an imported change in the log came from.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ImportedRow {
    pub git: String,
    pub author: String,
}

/// Where an imported change came from: its git commit and author.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Import {
    pub git: String,
    pub author_name: String,
}

/// One change, as `GET /v1/weft/changes/{change}` answers it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ChangeDetail {
    pub change: String,
    pub title: String,
    pub state: String,
    pub rounds: Vec<String>,
    /// The intents of each round, keyed by the round's number.
    pub intents: BTreeMap<String, Vec<Intent>>,
    pub revisions: Vec<Revision>,
    pub records: Vec<Record>,
    /// Set for a change imported from git.
    pub import: Option<Import>,
}

/// A revision of a change: a draft, a sealed round, or the landed one.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Revision {
    pub id: String,
    pub kind: String,
    pub files: u64,
    pub actor: String,
    pub at_ms: u64,
}

/// A signed record about a change: its type (`written`, `checked`, `review`, `gate`, `ruling`, `landed`), who signed
/// it, whether it was signed from a console, and its predicate.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Record {
    pub seq: u64,
    #[serde(rename = "type")]
    pub kind: String,
    pub revision: String,
    pub actor: String,
    pub via_console: bool,
    pub at_ms: u64,
    pub predicate: Value,
}

/// One release of a repository, as the releases list answers it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ReleaseRow {
    pub version: String,
    /// The revision the release was cut from, e.g. `w-1.l`.
    pub from: String,
    pub notes: String,
    /// Set once the owner has signed the release.
    pub signed: Option<Value>,
    /// Set for a release imported from git.
    pub imported: Option<Value>,
    pub artifacts: Vec<Stored>,
}

/// An artifact as the hub lists it (`{artifact, seq, at_ms}`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Stored {
    pub artifact: ArtifactRow,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ArtifactRow {
    pub id: String,
    pub repo: String,
    pub name: String,
    pub target: String,
    pub size: u64,
    pub built_from: String,
}

/// What a load fetched, for the request it answered.
#[derive(Debug, Clone, Default)]
pub struct Data {
    running: bool,
    repos: Vec<RepoRow>,
    changes: Vec<ChangeRow>,
    change: Option<ChangeDetail>,
    releases: Vec<ReleaseRow>,
    /// The open repository's artifacts (every one of them, for the releases' digests).
    artifacts: Vec<ArtifactRow>,
}

// ---- Navigation and presets -------------------------------------------------------------------------------

/// The repository's two tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Changes,
    Releases,
}

/// The query presets above the change list: each sends a query to the hub's log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    All,
    Open,
    Landed,
    Needs,
    Conflicts,
}

impl Preset {
    pub const ALL: [Preset; 5] = [
        Preset::All,
        Preset::Open,
        Preset::Landed,
        Preset::Needs,
        Preset::Conflicts,
    ];

    /// The button's label.
    pub fn label(self) -> &'static str {
        match self {
            Preset::All => "All",
            Preset::Open => "Open",
            Preset::Landed => "Landed",
            Preset::Needs => "Needs me",
            Preset::Conflicts => "Conflicts",
        }
    }

    /// The query the preset sends (Weft's query language).
    pub fn query(self) -> &'static str {
        match self {
            Preset::All => "all()",
            Preset::Open => "open()",
            Preset::Landed => "landed()",
            Preset::Needs => "needs(owner)",
            Preset::Conflicts => "conflicts()",
        }
    }
}

/// The preset a query is, when it is one of the presets' queries.
pub fn preset_of(query: &str) -> Option<Preset> {
    Preset::ALL.into_iter().find(|p| p.query() == query.trim())
}

/// The position a stage of the sign-off chain stands at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Written,
    Checked,
    Review,
    Gate,
    Ruling,
    Landed,
}

impl Stage {
    pub const ALL: [Stage; 6] = [
        Stage::Written,
        Stage::Checked,
        Stage::Review,
        Stage::Gate,
        Stage::Ruling,
        Stage::Landed,
    ];

    /// The stage a record's type names.
    fn of(kind: &str) -> Option<Stage> {
        match kind {
            "written" => Some(Stage::Written),
            "checked" => Some(Stage::Checked),
            "review" => Some(Stage::Review),
            "gate" => Some(Stage::Gate),
            "ruling" => Some(Stage::Ruling),
            "landed" => Some(Stage::Landed),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Stage::Written => "Written",
            Stage::Checked => "Checked",
            Stage::Review => "Review",
            Stage::Gate => "Gate",
            Stage::Ruling => "Ruling",
            Stage::Landed => "Landed",
        }
    }
}

/// One step of the chain: a run of records of one stage, in the order they were signed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub stage: Stage,
    /// How many records the run merges (one review per reviewer).
    pub count: usize,
    /// The step's colour: done, watch (sent back), stop, or the owner's hand.
    pub tone: Tone,
    /// Whether one of the run's records was signed from a console.
    pub console: bool,
    /// The gate's outcome or the ruling's decision (`pass`, `rework`, `owner`, `fail`); empty otherwise.
    pub outcome: String,
}

/// The sign-off chain of a change: its records in the order they were signed, runs of one stage merged. Records the
/// hub listed out of order are sorted by their sequence number first.
pub fn chain(records: &[Record]) -> Vec<Step> {
    let mut sorted: Vec<&Record> = records.iter().collect();
    sorted.sort_by_key(|r| r.seq);
    let mut steps: Vec<Step> = Vec::new();
    for record in sorted {
        let Some(stage) = Stage::of(&record.kind) else {
            continue;
        };
        let (tone, outcome) = mark(stage, &record.predicate);
        match steps.last_mut().filter(|step| step.stage == stage) {
            Some(step) => {
                step.count += 1;
                step.tone = worse(step.tone, tone);
                step.console |= record.via_console;
                step.outcome = outcome;
            }
            None => steps.push(Step {
                stage,
                count: 1,
                tone,
                console: record.via_console,
                outcome,
            }),
        }
    }
    steps
}

/// How a record reads on the chain, and its outcome word: a review passes or is a watch; the gate passes, sends back
/// (rework), asks the owner, or fails; the owner's ruling is always the owner's hand.
fn mark(stage: Stage, predicate: &Value) -> (Tone, String) {
    let word = |key: &str| {
        predicate
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    match stage {
        Stage::Written | Stage::Checked | Stage::Landed => (Tone::Ok, String::new()),
        Stage::Review => {
            let pass = predicate
                .get("pass")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            (if pass { Tone::Ok } else { Tone::Warn }, String::new())
        }
        Stage::Gate => {
            let outcome = word("outcome");
            let tone = match outcome.as_str() {
                "pass" => Tone::Ok,
                "fail" => Tone::Danger,
                _ => Tone::Warn,
            };
            (tone, outcome)
        }
        Stage::Ruling => (Tone::Signal, word("decision")),
    }
}

/// The worse of two tones on a chain step (a run with one failed review is a watch).
fn worse(a: Tone, b: Tone) -> Tone {
    fn rank(t: Tone) -> u8 {
        match t {
            Tone::Danger => 2,
            Tone::Warn => 1,
            _ => 0,
        }
    }
    if rank(b) > rank(a) { b } else { a }
}

/// The stages still to come after the chain's last step, for a change in `state`: the ruling when the gate asked the
/// owner, the landing after a pass, the worker again after a send-back. Nothing for a landed or dropped change.
pub fn to_come(steps: &[Step], state: &str) -> Vec<Stage> {
    if matches!(state, "landed" | "abandoned") {
        return Vec::new();
    }
    let Some(last) = steps.last() else {
        return Stage::ALL.to_vec();
    };
    match (last.stage, last.outcome.as_str()) {
        (Stage::Gate, "pass") | (Stage::Ruling, "pass") => vec![Stage::Landed],
        (Stage::Gate, "owner") => vec![Stage::Ruling, Stage::Landed],
        (Stage::Gate, "rework") | (Stage::Ruling, "rework") => vec![Stage::Written],
        (Stage::Gate, "fail") | (Stage::Ruling, _) | (Stage::Landed, _) => Vec::new(),
        _ => vec![Stage::Gate, Stage::Landed],
    }
}

/// A change state in plain words.
pub fn change_state_label(state: &str) -> &'static str {
    match state {
        "open" => "open",
        "sealed" => "in review",
        "queued" => "in queue",
        "conflict" => "conflict",
        "landed" => "landed",
        "abandoned" => "abandoned",
        _ => "unknown",
    }
}

fn change_tone(state: &str) -> Tone {
    match state {
        "landed" => Tone::Ok,
        "conflict" => Tone::Danger,
        "abandoned" => Tone::Muted,
        _ => Tone::Info,
    }
}

/// A release's state: signed by the owner, imported from git, or still proposed (waiting for the owner).
pub fn release_state(signed: bool, imported: bool) -> &'static str {
    if signed {
        "signed"
    } else if imported {
        "imported"
    } else {
        "proposed"
    }
}

fn release_tone(label: &str) -> Tone {
    match label {
        "signed" => Tone::Ok,
        "imported" => Tone::Info,
        _ => Tone::Warn,
    }
}

/// A git connection's state in plain words.
pub fn connection_label(state: &str) -> &str {
    match state {
        "in-step" => "in step",
        "moved" => "remote moved",
        "dry-run" => "dry run",
        other => other,
    }
}

/// A change's name from a revision's id: `w-1.l` is `w-1`.
fn change_of(revision: &str) -> &str {
    revision.split('.').next().unwrap_or(revision)
}

/// The line at the top of a change's page: where it stands, and what it needs from the owner, in plain words.
fn summary(detail: &ChangeDetail, steps: &[Step]) -> String {
    if let Some(i) = &detail.import
        && steps.is_empty()
    {
        let short: String = i.git.chars().take(7).collect();
        return format!(
            "Imported from git ({short}), by {}: written before Weft, so it has no sign-off.",
            i.author_name
        );
    }
    let last = steps.last();
    let waiting_owner = matches!(last, Some(s) if s.stage == Stage::Gate && s.outcome == "owner");
    let sent_back = matches!(last, Some(s) if s.outcome == "rework");
    match detail.state.as_str() {
        "landed" => "Landed on main.".to_string(),
        "abandoned" => "Dropped; it did not land.".to_string(),
        "conflict" => "Conflicts with main; the worker has to rebase it.".to_string(),
        "queued" => "In the queue for main.".to_string(),
        _ if waiting_owner => "Waiting for your ruling.".to_string(),
        _ if sent_back => "Sent back to the worker.".to_string(),
        _ if steps.is_empty() => "Being written.".to_string(),
        _ => format!("In review, round {}.", detail.rounds.len().max(1)),
    }
}

/// A git author's name, without the `<email>`.
fn author_name(author: &str) -> &str {
    author
        .split_once(" <")
        .map_or(author, |(name, _)| name)
        .trim()
}

/// The intents of a change's latest round.
fn latest_intents(detail: &ChangeDetail) -> &[Intent] {
    detail
        .intents
        .iter()
        .max_by_key(|(round, _)| round.parse::<u64>().unwrap_or(0))
        .map(|(_, intents)| intents.as_slice())
        .unwrap_or(&[])
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A byte count in KB or MB, for an artifact's line.
fn size_text(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// The repository's main line: the change its main ref sits on.
fn main_of(repo: &RepoRow) -> String {
    repo.refs
        .get("main")
        .map(|rev| change_of(rev).to_string())
        .unwrap_or_else(|| "nothing yet".to_string())
}

fn connections_text(repo: &RepoRow) -> String {
    if repo.connections.is_empty() {
        return "No remote connected.".to_string();
    }
    repo.connections
        .iter()
        .map(|c| format!("{}: {}", c.id, connection_label(&c.state)))
        .collect::<Vec<_>>()
        .join(" · ")
}

// ---- State and messages -----------------------------------------------------------------------------------

/// The request a load answers; a response for an older request is dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    repo: String,
    tab: Tab,
    query: String,
    change: Option<String>,
}

#[derive(Debug, Default)]
pub struct State {
    pub error: Option<String>,
    pub notice: Option<String>,
    /// The open repository ("" for the list of repositories).
    pub repo: String,
    pub tab: Tab,
    /// The query box's text, and the query the change list shows (sent on Enter or by a preset).
    pub query: String,
    pub applied: String,
    /// The open change's name, when its page is open.
    pub change: Option<String>,
    /// The Expert view: a change's revisions and each record's predicate.
    pub expert: bool,
    /// The version being signed.
    pub signing: Option<String>,
    /// The request the data answers; the screen shows loading until it is the current request.
    loaded: Option<Request>,
    data: Data,
}

#[derive(Debug, Clone)]
pub enum Msg {
    OpenRepo(String),
    Home,
    Tab(Tab),
    Query(String),
    Search,
    Preset(Preset),
    OpenChange(String),
    BackToChanges,
    Expert(bool),
    /// Signs a proposed release by its version.
    Sign(String),
    Signed(Result<String, String>),
    Loaded(Request, Result<Data, String>),
    /// A hub event: the Weft topics reload the screen.
    Event(events::Event),
}

/// The navigation this message asks for: none, the screen keeps its own drill-downs.
pub fn go(_msg: &Msg) -> Option<Id> {
    None
}

fn request(state: &State) -> Request {
    Request {
        repo: state.repo.clone(),
        tab: state.tab,
        query: if state.applied.is_empty() {
            Preset::All.query().to_string()
        } else {
            state.applied.clone()
        },
        change: state.change.clone(),
    }
}

/// Whether the data on screen answers the current request.
fn ready(state: &State) -> bool {
    state.loaded.as_ref() == Some(&request(state))
}

/// Loads the screen's data (called when it opens and on every poll tick).
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    refresh(state, ctx)
}

fn refresh(state: &State, ctx: &Ctx) -> Task<Msg> {
    let req = request(state);
    let tag = req.clone();
    let client = ctx.client.clone();
    client::task(
        move || fetch(&client, &req),
        move |res| Msg::Loaded(tag.clone(), res),
    )
}

/// Fetches what the open view shows: the hub's repositories, and the open repository's change list, change, or releases.
fn fetch(client: &Client, req: &Request) -> Result<Data, String> {
    let status: Status = client.get("/v1/weft")?;
    let mut data = Data {
        running: status.running,
        ..Data::default()
    };
    if !status.running {
        return Ok(data);
    }
    data.repos = client.get::<Repos>("/v1/weft/repos")?.repos;
    if req.repo.is_empty() {
        return Ok(data);
    }
    let base = format!("/v1/weft/repos/{}", enc(&req.repo));
    match (req.tab, &req.change) {
        (_, Some(name)) => {
            data.change = Some(client.get(&format!("/v1/weft/changes/{}", enc(name)))?);
        }
        (Tab::Changes, None) => {
            let path = format!("{base}/log?q={}&limit={LOG_LIMIT}", enc(&req.query));
            data.changes = client.get::<Log>(&path)?.changes;
        }
        (Tab::Releases, None) => {
            data.releases = client
                .get::<Releases>(&format!("{base}/releases"))?
                .releases;
            let all = client.get::<Artifacts>("/v1/weft/artifacts")?;
            data.artifacts = all
                .artifacts
                .into_iter()
                .map(|s| s.artifact)
                .filter(|a| a.repo == req.repo)
                .collect();
        }
    }
    Ok(data)
}

/// Signs a proposed release: the digest over its artifacts, then the owner's `release.sign` through the console.
fn sign(
    client: &Client,
    repo: &str,
    version: &str,
    artifacts: &[String],
) -> Result<String, String> {
    let base = format!("/v1/weft/repos/{}/releases/{}", enc(repo), enc(version));
    let answer: Value = client.post(
        &format!("{base}/digest"),
        &json!({ "artifacts": artifacts }),
    )?;
    let digest = answer.get("digest").cloned().unwrap_or(Value::Null);
    let body = json!({
        "repo": repo,
        "version": version,
        "artifacts": artifacts,
        "digest": digest,
    });
    client.post::<Value>(
        "/v1/weft/console",
        &json!({ "kind": "release.sign", "body": body }),
    )?;
    Ok(format!("{version} signed"))
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::OpenRepo(repo) => {
            state.repo = repo;
            state.tab = Tab::Changes;
            state.change = None;
            state.query = Preset::All.query().to_string();
            state.applied = state.query.clone();
            state.notice = None;
            refresh(state, ctx)
        }
        Msg::Home => {
            state.repo.clear();
            state.change = None;
            state.notice = None;
            refresh(state, ctx)
        }
        Msg::Tab(tab) => {
            state.tab = tab;
            state.change = None;
            refresh(state, ctx)
        }
        Msg::Query(query) => {
            state.query = query;
            Task::none()
        }
        Msg::Search => {
            let query = state.query.trim();
            state.applied = if query.is_empty() {
                Preset::All.query().to_string()
            } else {
                query.to_string()
            };
            state.change = None;
            refresh(state, ctx)
        }
        Msg::Preset(preset) => {
            state.query = preset.query().to_string();
            state.applied = state.query.clone();
            state.change = None;
            refresh(state, ctx)
        }
        Msg::OpenChange(name) => {
            state.change = Some(name);
            refresh(state, ctx)
        }
        Msg::BackToChanges => {
            state.change = None;
            refresh(state, ctx)
        }
        Msg::Expert(on) => {
            state.expert = on;
            Task::none()
        }
        Msg::Sign(version) => {
            let Some(release) = state.data.releases.iter().find(|r| r.version == version) else {
                return Task::none();
            };
            let ids: Vec<String> = state
                .data
                .artifacts
                .iter()
                .filter(|a| a.built_from == release.from)
                .map(|a| a.id.clone())
                .collect();
            let repo = state.repo.clone();
            let client = ctx.client.clone();
            state.signing = Some(version.clone());
            state.error = None;
            state.notice = None;
            client::task(move || sign(&client, &repo, &version, &ids), Msg::Signed)
        }
        Msg::Signed(res) => {
            state.signing = None;
            match res {
                Ok(notice) => {
                    state.notice = Some(notice);
                    state.error = None;
                    refresh(state, ctx)
                }
                Err(e) => {
                    state.error = Some(e);
                    Task::none()
                }
            }
        }
        Msg::Loaded(req, res) => {
            if req != request(state) {
                return Task::none();
            }
            match res {
                Ok(data) => {
                    state.data = data;
                    state.loaded = Some(req);
                    state.error = None;
                }
                Err(e) => state.error = Some(e),
            }
            Task::none()
        }
        Msg::Event(event) => match event.kind.as_str() {
            "weft_change" | "weft_queue" | "weft_land" | "weft_release" | "weft_export"
            | "weft_op" => refresh(state, ctx),
            _ => Task::none(),
        },
    }
}

pub fn subscription(ctx: &Ctx) -> iced::Subscription<Msg> {
    events::subscription(&ctx.client).map(Msg::Event)
}

// ---- View -------------------------------------------------------------------------------------------------

/// A dot of `size` px: filled in `color`, or a ring when it is not filled (a step still to come).
pub(super) fn dot<'a, M: 'a>(color: Color, filled: bool, size: f32) -> Element<'a, M> {
    container(Space::new())
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(move |_: &Theme| container::Style {
            background: if filled { Some(color.into()) } else { None },
            border: Border {
                color,
                width: 1.0,
                radius: (size / 2.0).into(),
            },
            ..Default::default()
        })
        .into()
}

/// A hairline rule, full width.
fn rule<'a>() -> Element<'a, Msg> {
    container(Space::new().height(1.0))
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(theme::palette().rule.into()),
            ..Default::default()
        })
        .into()
}

/// Rows with a hairline under each.
fn hairlines<'a>(rows: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = Vec::with_capacity(rows.len() * 2);
    for r in rows {
        items.push(r);
        items.push(rule());
    }
    column(items).into()
}

/// The line under a page's head: the state, in one sentence.
fn lede<'a>(line: String) -> Element<'a, Msg> {
    ui::prose(line).size(15.0).color(INK_2).into()
}

/// A sheet with the screen's notes (an error, a notice) above its body.
fn page<'a>(state: &'a State, head: Element<'a, Msg>, body: Element<'a, Msg>) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(e) = &state.error {
        items.push(ui::error_line(e));
    }
    if let Some(n) = &state.notice {
        items.push(ui::faint(n.clone()).into());
    }
    items.push(body);
    ui::sheet(head, column(items).spacing(14).into())
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    if state.loaded.is_some() && !state.data.running {
        return page(
            state,
            ui::view_head("History", vec![]),
            ui::empty(
                "git",
                "Weft is not running",
                "Start Weft on the hub to see the history.",
            ),
        );
    }
    if state.repo.is_empty() {
        repos_page(state)
    } else if state.change.is_some() {
        change_page(state)
    } else {
        repo_page(state)
    }
}

fn repos_page<'a>(state: &'a State) -> Element<'a, Msg> {
    let head = ui::view_head("History", vec![]);
    if !ready(state) {
        return page(state, head, ui::loading());
    }
    let repos = &state.data.repos;
    if repos.is_empty() {
        return page(
            state,
            head,
            ui::empty(
                "git",
                "No repositories on Weft yet",
                "A project's repository appears here once it has a change.",
            ),
        );
    }
    let open: u64 = repos.iter().map(|r| r.open_changes).sum();
    let landed: u64 = repos.iter().map(|r| r.landed).sum();
    let rows: Vec<Element<'a, Msg>> = repos.iter().map(repo_row).collect();
    let body = column![
        lede(format!(
            "{} · {open} open · {landed} landed",
            plural(repos.len() as u64, "repository", "repositories")
        )),
        hairlines(rows),
    ]
    .spacing(16);
    page(state, head, body.into())
}

/// A repository in the list: its name and description, its main line, landed and open counts, and its connection.
fn repo_row<'a>(r: &RepoRow) -> Element<'a, Msg> {
    let description = if r.description.is_empty() {
        "No description".to_string()
    } else {
        r.description.clone()
    };
    let info = column![
        text(r.name.clone())
            .font(ui::SANS_SEMIBOLD)
            .size(14.0)
            .color(INK),
        ui::prose(description).size(13.0).color(INK_2),
    ]
    .spacing(2);
    let mut facts: Vec<Element<'a, Msg>> = vec![
        ui::sans(format!("main at {}", main_of(r)), 11.0, INK_3).into(),
        ui::faint(format!("{} landed · {} open", r.landed, r.open_changes)).into(),
        ui::faint(connections_text(r)).into(),
    ];
    if let Some(project) = &r.project {
        facts.push(ui::faint(format!("project {project}")).into());
    }
    let open = button(info)
        .on_press(Msg::OpenRepo(r.name.clone()))
        .style(theme::register_row(false))
        .width(Length::Fill);
    row![open, column(facts).spacing(2)]
        .spacing(16)
        .align_y(Alignment::Center)
        .padding([10, 12])
        .into()
}

fn repo_page<'a>(state: &'a State) -> Element<'a, Msg> {
    let back = ui::btn("Repositories", None, Kind::Ghost, Some(Msg::Home));
    let head = ui::view_head(&state.repo, vec![back]);
    if !ready(state) {
        return page(state, head, ui::loading());
    }
    let Some(repo) = state.data.repos.iter().find(|r| r.name == state.repo) else {
        return page(
            state,
            head,
            ui::empty("git", "No such repository", "It may have been removed."),
        );
    };
    let tabs = ui::tabs(
        &[(Tab::Changes, "Changes"), (Tab::Releases, "Releases")],
        state.tab,
        Msg::Tab,
    );
    let (line, body) = match state.tab {
        Tab::Changes => (
            format!(
                "{} open · {} landed · main at {}",
                repo.open_changes,
                repo.landed,
                main_of(repo)
            ),
            changes_view(state),
        ),
        Tab::Releases => (releases_lede(state), releases_view(state)),
    };
    let content = column![lede(line), ui::faint(connections_text(repo)), tabs, body,].spacing(16);
    page(state, head, content.into())
}

fn releases_lede(state: &State) -> String {
    let proposed = state
        .data
        .releases
        .iter()
        .filter(|r| release_state(r.signed.is_some(), r.imported.is_some()) == "proposed")
        .count() as u64;
    format!(
        "{} · {} waiting for your signature",
        plural(state.data.releases.len() as u64, "release", "releases"),
        proposed
    )
}

/// The change list: the query box, the presets, and the changes the query matches.
fn changes_view<'a>(state: &'a State) -> Element<'a, Msg> {
    if !ready(state) {
        return ui::loading();
    }
    let search: Element<'a, Msg> = text_input("Query, e.g. open() & touches(src/**)", &state.query)
        .on_input(Msg::Query)
        .on_submit(Msg::Search)
        .size(13.0)
        .width(Length::Fill)
        .into();
    let presets: Vec<Element<'a, Msg>> = Preset::ALL
        .into_iter()
        .map(|p| {
            let kind = if preset_of(&state.applied) == Some(p) {
                Kind::Secondary
            } else {
                Kind::Ghost
            };
            ui::btn(p.label(), None, kind, Some(Msg::Preset(p)))
        })
        .collect();
    let list: Element<'a, Msg> = if state.data.changes.is_empty() {
        ui::empty(
            "git",
            "No changes here",
            "Nothing in this repository matches the query.",
        )
    } else {
        hairlines(state.data.changes.iter().map(change_row).collect())
    };
    column![search, row(presets).spacing(6), list]
        .spacing(14)
        .into()
}

/// A change in the list: its name, state, title and intents; a click opens its page.
fn change_row<'a>(c: &ChangeRow) -> Element<'a, Msg> {
    let intents: Vec<Element<'a, Msg>> = c
        .intents
        .iter()
        .map(|i| {
            row![
                ui::mono(i.kind.clone(), 11.0, INK_3),
                ui::prose(i.title.clone()).size(13.0).color(INK_2),
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into()
        })
        .collect();
    let title = column![
        row![
            ui::mono(c.change.clone(), 12.0, INK_2),
            ui::badge(change_state_label(&c.state), change_tone(&c.state)),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
        text(c.title.clone())
            .font(ui::SANS_SEMIBOLD)
            .size(14.0)
            .color(INK),
        column(intents).spacing(3),
    ]
    .spacing(6);
    let open = button(title)
        .on_press(Msg::OpenChange(c.change.clone()))
        .style(theme::register_row(false))
        .width(Length::Fill);
    // An imported change has no rounds: it says whose commit it was instead.
    let origin = match &c.imported {
        Some(i) => format!("imported · {}", author_name(&i.author)),
        None => plural(c.rounds, "round", "rounds"),
    };
    let meta = ui::faint(format!("{origin} · {}", approvals::ago(c.at_ms as i64)));
    row![open, meta]
        .spacing(16)
        .align_y(Alignment::Center)
        .padding([10, 12])
        .into()
}

/// A change's page: its line, its title and state, the sign-off chain, its intents, and under Expert its revisions and
/// each record's predicate.
fn change_page<'a>(state: &'a State) -> Element<'a, Msg> {
    let name = state.change.clone().unwrap_or_default();
    let back = ui::btn("Changes", None, Kind::Ghost, Some(Msg::BackToChanges));
    let expert = ui::seg(
        &[(false, "Plain"), (true, "Expert")],
        state.expert,
        Msg::Expert,
    );
    let head = ui::view_head(&format!("{name} · {}", state.repo), vec![back, expert]);
    let detail = match state.data.change.as_ref() {
        Some(d) if ready(state) && d.change == name => d,
        _ => return page(state, head, ui::loading()),
    };
    let steps = chain(&detail.records);
    let pending = to_come(&steps, &detail.state);
    let mut body: Vec<Element<'a, Msg>> = vec![
        lede(summary(detail, &steps)),
        text(detail.title.clone())
            .font(ui::SANS_SEMIBOLD)
            .size(16.0)
            .color(INK)
            .into(),
        row![
            ui::badge(
                change_state_label(&detail.state),
                change_tone(&detail.state)
            ),
            ui::faint(plural(detail.rounds.len() as u64, "round", "rounds")),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into(),
        container(chain_view(&steps, &pending))
            .padding([8, 0])
            .width(Length::Fill)
            .into(),
        ui::label("Intents").into(),
        intents_view(detail, state.expert),
    ];
    if state.expert {
        body.push(expert_view(detail));
    }
    page(state, head, column(body).spacing(18).into())
}

/// The sign-off chain: a row of dots joined by a rule, one per step, then the stages still to come as rings. A ruling
/// signed from a console is captioned so; a run of records shows its count.
fn chain_view<'a>(steps: &[Step], pending: &[Stage]) -> Element<'a, Msg> {
    let total = steps.len() + pending.len();
    let mut nodes: Vec<Element<'a, Msg>> = Vec::with_capacity(total);
    for (i, step) in steps.iter().enumerate() {
        let caption = if step.console {
            "from the console".to_string()
        } else if step.count > 1 {
            format!("×{}", step.count)
        } else {
            String::new()
        };
        nodes.push(node(
            dot::<Msg>(step.tone.color(), true, 12.0),
            step.stage.label(),
            caption,
            i == 0,
            i + 1 == total,
        ));
    }
    for (j, stage) in pending.iter().enumerate() {
        let i = steps.len() + j;
        nodes.push(node(
            dot::<Msg>(theme::palette().ink_3, false, 12.0),
            stage.label(),
            "to come".to_string(),
            i == 0,
            i + 1 == total,
        ));
    }
    ui::scroll_x(row(nodes).width(Length::Shrink)).into()
}

/// One step of the chain: its dot on a rule that joins it to the steps either side, its stage, and a caption.
fn node<'a>(
    dot: Element<'a, Msg>,
    stage: &str,
    caption: String,
    first: bool,
    last: bool,
) -> Element<'a, Msg> {
    let left: Element<'a, Msg> = if first {
        Space::new().width(Length::Fill).into()
    } else {
        rule()
    };
    let right: Element<'a, Msg> = if last {
        Space::new().width(Length::Fill).into()
    } else {
        rule()
    };
    column![
        row![left, dot, right]
            .width(Length::Fill)
            .align_y(Alignment::Center),
        ui::sans(stage.to_string(), 13.0, INK),
        ui::faint(caption),
    ]
    .spacing(6)
    .align_x(Alignment::Center)
    .width(Length::Fixed(NODE_WIDTH))
    .into()
}

/// The change's intents of its latest round: the kind and the title, the summary under it, and (Expert) what it covers.
fn intents_view<'a>(detail: &'a ChangeDetail, expert: bool) -> Element<'a, Msg> {
    let intents = latest_intents(detail);
    if intents.is_empty() {
        return ui::faint("No intents yet.").into();
    }
    let rows: Vec<Element<'a, Msg>> = intents
        .iter()
        .map(|i| -> Element<'a, Msg> {
            let mut lines: Vec<Element<'a, Msg>> = vec![
                row![
                    ui::mono(i.kind.clone(), 11.0, INK_3),
                    ui::sans(i.title.clone(), 14.0, INK),
                ]
                .spacing(10)
                .align_y(Alignment::Center)
                .into(),
            ];
            if !i.summary.is_empty() {
                lines.push(ui::prose(i.summary.clone()).size(13.0).color(INK_2).into());
            }
            if expert && !i.covers.is_empty() {
                lines.push(ui::mono(i.covers.join(", "), 11.0, INK_3).into());
            }
            column(lines).spacing(4).into()
        })
        .collect();
    hairlines(rows)
}

/// Expert: a change's revisions, and each record in the order it was signed, with its predicate.
fn expert_view<'a>(detail: &'a ChangeDetail) -> Element<'a, Msg> {
    let revisions: Vec<Element<'a, Msg>> = detail
        .revisions
        .iter()
        .map(|r| {
            row![
                ui::mono(r.id.clone(), 12.0, INK),
                ui::faint(r.kind.clone()),
                ui::faint(r.actor.clone()),
                ui::faint(plural(r.files, "file", "files")),
                ui::faint(approvals::ago(r.at_ms as i64)),
            ]
            .spacing(14)
            .align_y(Alignment::Center)
            .into()
        })
        .collect();
    let mut records: Vec<&Record> = detail.records.iter().collect();
    records.sort_by_key(|r| r.seq);
    let records: Vec<Element<'a, Msg>> = records.into_iter().map(record_row).collect();
    column![
        ui::label("Revisions"),
        hairlines(revisions),
        ui::label("Records"),
        hairlines(records),
    ]
    .spacing(10)
    .into()
}

/// One record: its sequence number, type, revision, signer, a console mark, and its predicate as JSON.
fn record_row<'a>(r: &Record) -> Element<'a, Msg> {
    let predicate =
        serde_json::to_string_pretty(&r.predicate).unwrap_or_else(|_| "null".to_string());
    let mut head = row![
        ui::mono(format!("#{}", r.seq), 12.0, INK),
        ui::mono(r.kind.clone(), 12.0, INK_2),
        ui::faint(r.revision.clone()),
        ui::faint(r.actor.clone()),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    if r.via_console {
        head = head.push(ui::badge("from the console", Tone::Signal));
    }
    head = head.push(Space::new().width(Length::Fill));
    head = head.push(ui::faint(approvals::ago(r.at_ms as i64)));
    column![head, code_box(predicate)].spacing(8).into()
}

/// The predicate's JSON, on a soft surface.
fn code_box<'a>(source: String) -> Element<'a, Msg> {
    container(ui::mono(source, 12.0, INK_2))
        .padding([10, 12])
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(theme::palette().hover.into()),
            border: Border {
                color: theme::palette().rule,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into()
}

fn releases_view<'a>(state: &'a State) -> Element<'a, Msg> {
    if !ready(state) {
        return ui::loading();
    }
    if state.data.releases.is_empty() {
        return ui::empty(
            "git",
            "No releases yet",
            "A release is cut from a landed revision, with notes and artifacts, and the owner signs it.",
        );
    }
    hairlines(
        state
            .data
            .releases
            .iter()
            .map(|r| release_row(state, r))
            .collect(),
    )
}

/// A release: its version and state, its notes as plain text, its artifacts, and the owner's Sign when it is proposed.
fn release_row<'a>(state: &'a State, r: &'a ReleaseRow) -> Element<'a, Msg> {
    let label = release_state(r.signed.is_some(), r.imported.is_some());
    let proposed = label == "proposed";
    let busy = state.signing.as_deref() == Some(r.version.as_str());
    let mut head = row![
        text(r.version.clone())
            .font(ui::MONO_SEMIBOLD)
            .size(15.0)
            .color(INK),
        ui::badge(label, release_tone(label)),
        ui::faint(format!("from {}", change_of(&r.from))),
        Space::new().width(Length::Fill),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    if proposed {
        let call = if busy { "Signing…" } else { "Sign" };
        let press = (!busy && state.signing.is_none()).then(|| Msg::Sign(r.version.clone()));
        head = head.push(ui::btn(call, None, Kind::Primary, press));
    }
    let artifacts: Vec<&ArtifactRow> = if proposed {
        state
            .data
            .artifacts
            .iter()
            .filter(|a| a.built_from == r.from)
            .collect()
    } else {
        r.artifacts.iter().map(|s| &s.artifact).collect()
    };
    let mut items: Vec<Element<'a, Msg>> = vec![head.into(), notes_view(&r.notes)];
    if artifacts.is_empty() {
        items.push(ui::faint("No artifacts built from this revision.").into());
    }
    for a in artifacts {
        items.push(
            row![
                ui::mono(a.name.clone(), 12.0, INK),
                ui::faint(a.target.clone()),
                ui::faint(size_text(a.size)),
                ui::faint(format!("built from {}", change_of(&a.built_from))),
            ]
            .spacing(14)
            .align_y(Alignment::Center)
            .into(),
        );
    }
    column(items).spacing(10).padding([12, 0]).into()
}

/// The release notes as plain text: headings in semibold, list items with a dot, the rest as they are.
fn notes_view<'a>(notes: &str) -> Element<'a, Msg> {
    let lines: Vec<Element<'a, Msg>> = notes
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|line| -> Element<'a, Msg> {
            if let Some(heading) = line.strip_prefix('#') {
                text(heading.trim_start_matches('#').trim().to_string())
                    .font(ui::SANS_SEMIBOLD)
                    .size(13.0)
                    .color(INK)
                    .into()
            } else if let Some(item) = line.strip_prefix("- ") {
                ui::prose(format!("•  {item}"))
                    .size(14.0)
                    .color(INK_2)
                    .into()
            } else {
                ui::prose(line.to_string()).size(14.0).color(INK_2).into()
            }
        })
        .collect();
    column(lines).spacing(4).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(v: Value) -> Record {
        serde_json::from_value(v).expect("a record of the hub's shape")
    }

    #[test]
    fn an_imported_change_says_where_it_came_from_and_whose_it_was() {
        // As the hub answers a change imported from git: no records, its commit and author kept.
        let detail: ChangeDetail = serde_json::from_value(json!({
            "change": "w-3", "title": "Priority queues", "state": "landed", "rounds": [], "records": [],
            "import": { "git": "18db19d7cfc09a1947dbd25d6692408d7288ecf9", "author_name": "Ada Owner", "author_email": "ada@example.test" }
        }))
        .expect("a change of the hub's shape");
        assert_eq!(
            summary(&detail, &chain(&detail.records)),
            "Imported from git (18db19d), by Ada Owner: written before Weft, so it has no sign-off."
        );
        let row: ChangeRow = serde_json::from_value(json!({
            "change": "w-3", "title": "Priority queues", "state": "landed",
            "imported": { "git": "18db19d", "author": "Ada Owner <ada@example.test>" }
        }))
        .expect("a log row of the hub's shape");
        assert_eq!(author_name(&row.imported.unwrap().author), "Ada Owner");
        assert_eq!(author_name("plain"), "plain");
    }

    #[test]
    fn the_chain_orders_records_by_sequence_and_merges_each_run() {
        // Listed out of order on purpose: the chain follows the sequence numbers the hub signed them with.
        let records: Vec<Record> = [
            json!({"seq": 39, "type": "ruling", "revision": "w-1.r2", "actor": "owner", "via_console": true, "at_ms": 3, "predicate": {"decision": "pass", "run": "rv2"}}),
            json!({"seq": 13, "type": "written", "revision": "w-1.r1", "actor": "agent:300", "via_console": false, "at_ms": 1, "predicate": {"summary": "next() now takes the oldest item"}}),
            json!({"seq": 25, "type": "gate", "revision": "w-1.r1", "actor": "hub", "via_console": false, "at_ms": 2, "predicate": {"outcome": "rework", "recipe": "code@3"}}),
            json!({"seq": 17, "type": "review", "revision": "w-1.r1", "actor": "agent:301", "via_console": false, "at_ms": 2, "predicate": {"pass": true}}),
            json!({"seq": 15, "type": "checked", "revision": "w-1.r1", "actor": "hub", "via_console": false, "at_ms": 2, "predicate": {"tool": "divergence"}}),
            json!({"seq": 14, "type": "checked", "revision": "w-1.r1", "actor": "hub", "via_console": false, "at_ms": 2, "predicate": {"tool": "tree"}}),
            json!({"seq": 19, "type": "review", "revision": "w-1.r1", "actor": "agent:302", "via_console": false, "at_ms": 2, "predicate": {"pass": false}}),
            json!({"seq": 23, "type": "review", "revision": "w-1.r1", "actor": "agent:303", "via_console": false, "at_ms": 2, "predicate": {"pass": true}}),
            json!({"seq": 29, "type": "written", "revision": "w-1.r2", "actor": "agent:300", "via_console": false, "at_ms": 4, "predicate": {"summary": "tested"}}),
            json!({"seq": 38, "type": "gate", "revision": "w-1.r2", "actor": "hub", "via_console": false, "at_ms": 5, "predicate": {"outcome": "owner"}}),
            json!({"seq": 44, "type": "unknown", "revision": "w-1.r2", "actor": "hub", "via_console": false, "at_ms": 6, "predicate": {}}),
        ]
        .into_iter()
        .map(record)
        .collect();

        let steps = chain(&records);
        let shape: Vec<(Stage, usize)> = steps.iter().map(|s| (s.stage, s.count)).collect();
        assert_eq!(
            shape,
            vec![
                (Stage::Written, 1),
                (Stage::Checked, 2),
                (Stage::Review, 3),
                (Stage::Gate, 1),
                (Stage::Written, 1),
                (Stage::Gate, 1),
                (Stage::Ruling, 1),
            ]
        );
        // A run with one failed review is a watch; the rework and the owner's ask are watches too.
        assert_eq!(steps[2].tone, Tone::Warn);
        assert_eq!(steps[3].tone, Tone::Warn);
        assert_eq!(steps[3].outcome, "rework");
        assert_eq!(steps[5].outcome, "owner");
        // The owner's ruling is the owner's hand, and it was signed from a console.
        assert_eq!(steps[6].tone, Tone::Signal);
        assert!(steps[6].console);
        assert_eq!(steps[6].outcome, "pass");
        assert!(!steps[0].console);
    }

    #[test]
    fn the_chain_says_what_is_still_to_come() {
        let step = |stage, outcome: &str| Step {
            stage,
            count: 1,
            tone: Tone::Ok,
            console: false,
            outcome: outcome.to_string(),
        };
        // The gate asked the owner: the ruling, then the landing.
        let asked = [step(Stage::Written, ""), step(Stage::Gate, "owner")];
        assert_eq!(to_come(&asked, "open"), vec![Stage::Ruling, Stage::Landed]);
        // The gate sent it back: the worker writes again.
        let back = [step(Stage::Gate, "rework")];
        assert_eq!(to_come(&back, "open"), vec![Stage::Written]);
        // The owner passed it: it lands.
        let passed = [step(Stage::Gate, "pass"), step(Stage::Ruling, "pass")];
        assert_eq!(to_come(&passed, "queued"), vec![Stage::Landed]);
        // A landed or dropped change has nothing to come; an empty change has the whole chain.
        assert!(to_come(&passed, "landed").is_empty());
        assert!(to_come(&[], "abandoned").is_empty());
        assert_eq!(to_come(&[], "open"), Stage::ALL.to_vec());
    }

    #[test]
    fn the_states_read_in_plain_words() {
        assert_eq!(change_state_label("open"), "open");
        assert_eq!(change_state_label("sealed"), "in review");
        assert_eq!(change_state_label("queued"), "in queue");
        assert_eq!(change_state_label("conflict"), "conflict");
        assert_eq!(change_state_label("landed"), "landed");
        assert_eq!(change_state_label("abandoned"), "abandoned");
        assert_eq!(change_state_label("???"), "unknown");

        assert_eq!(release_state(false, false), "proposed");
        assert_eq!(release_state(true, false), "signed");
        assert_eq!(release_state(false, true), "imported");

        assert_eq!(connection_label("in-step"), "in step");
        assert_eq!(connection_label("moved"), "remote moved");
        assert_eq!(change_of("w-1.l"), "w-1");
        assert_eq!(change_of("root"), "root");
    }

    #[test]
    fn the_query_presets_send_the_hub_queries() {
        let queries: Vec<&str> = Preset::ALL.into_iter().map(Preset::query).collect();
        assert_eq!(
            queries,
            vec!["all()", "open()", "landed()", "needs(owner)", "conflicts()"]
        );
        let labels: Vec<&str> = Preset::ALL.into_iter().map(Preset::label).collect();
        assert_eq!(
            labels,
            vec!["All", "Open", "Landed", "Needs me", "Conflicts"]
        );
        // A preset is recognised by its query, whatever the spacing around it.
        assert_eq!(preset_of("needs(owner)"), Some(Preset::Needs));
        assert_eq!(preset_of("  open()  "), Some(Preset::Open));
        assert_eq!(preset_of("touches(src/**)"), None);
    }
}
