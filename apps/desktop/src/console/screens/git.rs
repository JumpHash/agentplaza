//! The Git screen (parity with the web console's `git` view): the plaza's repositories on its git
//! host, each with its code, commits, branches and pull requests; the owner reviews and merges
//! here. Drill-downs (repository, tab, branch, folder, file, pull request) are the screen's own
//! state, as the web's route parameters are.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{button, column, container, pick_list, row, scrollable, stack, text_editor, text_input, Space};
use iced::{Alignment, Color, Element, Length, Task};
use serde::Deserialize;
use serde_json::{json, Value};

use super::Id;
use crate::console::client::{self, Client};
use crate::console::ui::{self, Kind, Tone, INK, INK_2, INK_3};
use crate::console::Ctx;
use crate::theme::{self, CRIT, OK};

// ---- Data (the shapes of `apps/web/src/api.ts`) ----------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct GitInfo {
    configured: bool,
    org: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Repo {
    name: String,
    description: String,
    default_branch: String,
    updated_at: String,
    open_prs: u64,
    project: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BranchCommit {
    message: String,
    author: String,
    at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Branch {
    name: String,
    commit: BranchCommit,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Commit {
    short: String,
    message: String,
    author: String,
    at: String,
    additions: u64,
    deletions: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Entry {
    name: String,
    path: String,
    kind: String,
    size: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileView {
    size: u64,
    text: Option<String>,
    binary: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Pull {
    number: u64,
    title: String,
    state: String,
    author: String,
    head: String,
    base: String,
    updated_at: String,
    comments: u64,
    review: String,
    mergeable: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Review {
    author: String,
    state: String,
    body: String,
    at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PullFile {
    path: String,
    additions: u64,
    deletions: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PullDetail {
    #[serde(flatten)]
    pull: Pull,
    body: String,
    diff: String,
    reviews: Vec<Review>,
    files: Vec<PullFile>,
}

/// What the screen shows, as the last load fetched it.
#[derive(Debug, Clone, Default)]
pub struct Data {
    /// None until the host answered.
    configured: Option<bool>,
    org: String,
    repos: Vec<Repo>,
    branches: Vec<Branch>,
    commits: Vec<Commit>,
    tree: Vec<Entry>,
    readme: Option<FileView>,
    file: Option<FileView>,
    pulls: Vec<Pull>,
    pull: Option<PullDetail>,
}

impl Data {
    /// Drops what belongs to one repository's tabs (kept while the tab changes).
    fn clear_tab(&mut self) {
        self.commits.clear();
        self.tree.clear();
        self.readme = None;
        self.file = None;
        self.pulls.clear();
        self.pull = None;
    }

    /// Drops everything about the open repository.
    fn clear_repo(&mut self) {
        self.branches.clear();
        self.clear_tab();
    }
}

// ---- Navigation and modal kinds --------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Code,
    Commits,
    Branches,
    Pulls,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PullState {
    #[default]
    Open,
    Closed,
    All,
}

impl PullState {
    fn key(self) -> &'static str {
        match self {
            PullState::Open => "open",
            PullState::Closed => "closed",
            PullState::All => "all",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewEvent {
    Approve,
    RequestChanges,
    Comment,
}

impl ReviewEvent {
    fn event(self) -> &'static str {
        match self {
            ReviewEvent::Approve => "APPROVE",
            ReviewEvent::RequestChanges => "REQUEST_CHANGES",
            ReviewEvent::Comment => "COMMENT",
        }
    }

    fn label(self) -> &'static str {
        match self {
            ReviewEvent::Approve => "Approve",
            ReviewEvent::RequestChanges => "Request changes",
            ReviewEvent::Comment => "Comment",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Modal {
    #[default]
    None,
    NewRepo,
    Review(ReviewEvent),
    Merge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MergeStyle {
    #[default]
    Merge,
    Squash,
    Rebase,
}

impl MergeStyle {
    fn key(self) -> &'static str {
        match self {
            MergeStyle::Merge => "merge",
            MergeStyle::Squash => "squash",
            MergeStyle::Rebase => "rebase",
        }
    }
}

impl fmt::Display for MergeStyle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            MergeStyle::Merge => "Merge commit",
            MergeStyle::Squash => "Squash",
            MergeStyle::Rebase => "Rebase",
        })
    }
}

/// The request a load answers; a response for an older request is dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    repo: String,
    tab: Tab,
    pulls: PullState,
    pr: Option<u64>,
    git_ref: String,
    path: String,
    file: String,
}

#[derive(Debug, Default)]
pub struct State {
    pub error: Option<String>,
    pub notice: Option<String>,
    pub data: Data,
    /// The open repository ("" for the repository list).
    pub repo: String,
    pub tab: Tab,
    pub pulls: PullState,
    /// The open pull request's number.
    pub pr: Option<u64>,
    pub git_ref: String,
    pub path: String,
    pub file_path: String,
    pub modal: Modal,
    pub new_name: String,
    /// The text of the open modal (a description or a review).
    pub body: text_editor::Content,
    pub merge_style: MergeStyle,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Loaded(Query, Result<Data, String>),
    Open(String),
    Home,
    Tab(Tab),
    Pulls(PullState),
    OpenPull(u64),
    Branch(String),
    Dir(String),
    File(String),
    Copy,
    Modal(Modal),
    NewName(String),
    Body(text_editor::Action),
    MergeStyle(MergeStyle),
    CreateRepo,
    Created(Result<String, String>),
    SendReview(ReviewEvent),
    Merge,
    Acted(Result<String, String>),
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

fn query(state: &State) -> Query {
    Query {
        repo: state.repo.clone(),
        tab: state.tab,
        pulls: state.pulls,
        pr: state.pr,
        git_ref: state.git_ref.clone(),
        path: state.path.clone(),
        file: state.file_path.clone(),
    }
}

/// Loads the screen's data (called when it opens and on every poll tick).
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    refresh(state, ctx)
}

fn refresh(state: &State, ctx: &Ctx) -> Task<Msg> {
    let q = query(state);
    let tag = q.clone();
    let client = ctx.client.clone();
    client::task(move || fetch(&client, &q), move |res| Msg::Loaded(tag.clone(), res))
}

/// Fetches what the open view shows: the host, the repositories, and the open repository's tab.
fn fetch(client: &Client, q: &Query) -> Result<Data, String> {
    let git: GitInfo = client.get("/v1/git")?;
    let mut d = Data { configured: Some(git.configured), org: git.org, ..Data::default() };
    if !git.configured {
        return Ok(d);
    }
    d.repos = client.get("/v1/git/repos")?;
    if q.repo.is_empty() {
        return Ok(d);
    }
    let base = format!("/v1/git/repos/{}", q.repo);
    d.branches = client.get(&format!("{base}/branches"))?;
    match q.tab {
        Tab::Code if !q.git_ref.is_empty() => {
            d.tree = client.get(&format!("{base}/tree?ref={}&path={}", enc(&q.git_ref), enc(&q.path)))?;
            if !q.file.is_empty() {
                d.file = Some(client.get(&format!("{base}/file?ref={}&path={}", enc(&q.git_ref), enc(&q.file)))?);
            } else if let Some(readme) = d.tree.iter().find(|e| e.kind == "file" && e.name == "README.md") {
                let f: FileView = client.get(&format!("{base}/file?ref={}&path={}", enc(&q.git_ref), enc(&readme.path)))?;
                d.readme = Some(f);
            }
        }
        Tab::Commits => d.commits = client.get(&format!("{base}/commits?ref=&limit=30"))?,
        Tab::Pulls => match q.pr {
            Some(n) => d.pull = Some(client.get(&format!("{base}/pulls/{n}"))?),
            None => d.pulls = client.get(&format!("{base}/pulls?state={}", q.pulls.key()))?,
        },
        _ => {}
    }
    Ok(d)
}

/// The branch the code tab should show when the chosen one is gone: the default, else the first.
fn fix_ref(state: &State) -> Option<String> {
    let names: Vec<&str> = state.data.branches.iter().map(|b| b.name.as_str()).collect();
    if names.is_empty() || names.contains(&state.git_ref.as_str()) {
        return None;
    }
    let default = state.data.repos.iter().find(|r| r.name == state.repo).map(|r| r.default_branch.as_str());
    let pick = match default {
        Some(d) if names.contains(&d) => d,
        _ => names[0],
    };
    Some(pick.to_string())
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(q, res) => {
            if q != query(state) {
                return Task::none();
            }
            match res {
                Ok(data) => {
                    state.data = data;
                    state.error = None;
                    if let Some(name) = fix_ref(state) {
                        state.git_ref = name;
                        return refresh(state, ctx);
                    }
                }
                Err(e) => state.error = Some(e),
            }
            Task::none()
        }
        Msg::Open(name) => {
            state.repo = name;
            state.tab = Tab::Code;
            state.pr = None;
            state.git_ref.clear();
            state.path.clear();
            state.file_path.clear();
            state.notice = None;
            state.data.clear_repo();
            refresh(state, ctx)
        }
        Msg::Home => {
            state.repo.clear();
            state.tab = Tab::Code;
            state.pr = None;
            state.data.clear_repo();
            refresh(state, ctx)
        }
        Msg::Tab(tab) => {
            state.tab = tab;
            state.pr = None;
            state.data.clear_tab();
            refresh(state, ctx)
        }
        Msg::Pulls(filter) => {
            state.pulls = filter;
            state.data.pulls.clear();
            refresh(state, ctx)
        }
        Msg::OpenPull(n) => {
            state.tab = Tab::Pulls;
            state.pr = Some(n);
            state.data.clear_tab();
            refresh(state, ctx)
        }
        Msg::Branch(name) => {
            state.git_ref = name;
            state.path.clear();
            state.file_path.clear();
            state.data.clear_tab();
            refresh(state, ctx)
        }
        Msg::Dir(path) => {
            state.path = path;
            state.file_path.clear();
            state.data.clear_tab();
            refresh(state, ctx)
        }
        Msg::File(path) => {
            state.file_path = path;
            state.data.clear_tab();
            refresh(state, ctx)
        }
        Msg::Copy => {
            state.notice = Some("Clone URL copied".into());
            iced::clipboard::write(clone_url(state))
        }
        Msg::Modal(modal) => {
            state.modal = modal;
            state.error = None;
            state.notice = None;
            state.body = text_editor::Content::new();
            if modal == Modal::NewRepo {
                state.new_name.clear();
            }
            if modal == Modal::Merge {
                state.merge_style = MergeStyle::Merge;
            }
            Task::none()
        }
        Msg::NewName(name) => {
            state.new_name = name;
            Task::none()
        }
        Msg::Body(action) => {
            state.body.perform(action);
            Task::none()
        }
        Msg::MergeStyle(style) => {
            state.merge_style = style;
            Task::none()
        }
        Msg::CreateRepo => {
            let name = state.new_name.trim().to_string();
            if name.is_empty() {
                state.error = Some("Name the repository".into());
                return Task::none();
            }
            let payload = json!({ "name": name.clone(), "description": state.body.text().trim() });
            let client = ctx.client.clone();
            client::task(move || client.post::<Value>("/v1/git/repos", &payload).map(|_| name), Msg::Created)
        }
        Msg::Created(res) => match res {
            Ok(name) => {
                state.modal = Modal::None;
                state.repo = name;
                state.notice = Some("Repository created".into());
                state.tab = Tab::Code;
                state.pr = None;
                state.git_ref.clear();
                state.path.clear();
                state.file_path.clear();
                state.data.clear_repo();
                refresh(state, ctx)
            }
            Err(e) => {
                state.error = Some(e);
                Task::none()
            }
        },
        Msg::SendReview(event) => {
            let body = state.body.text().trim().to_string();
            if event != ReviewEvent::Approve && body.is_empty() {
                state.error = Some("Write something first".into());
                return Task::none();
            }
            let Some(n) = state.pr else {
                return Task::none();
            };
            let path = format!("/v1/git/repos/{}/pulls/{n}/review", state.repo);
            let payload = json!({ "event": event.event(), "body": body });
            let client = ctx.client.clone();
            let label = event.label();
            client::task(move || client.call("POST", &path, Some(&payload)).map(|_| format!("{label} sent")), Msg::Acted)
        }
        Msg::Merge => {
            let Some(n) = state.pr else {
                return Task::none();
            };
            let path = format!("/v1/git/repos/{}/pulls/{n}/merge", state.repo);
            let payload = json!({ "style": state.merge_style.key() });
            let client = ctx.client.clone();
            client::task(move || client.call("POST", &path, Some(&payload)).map(|_| format!("#{n} merged")), Msg::Acted)
        }
        Msg::Acted(res) => match res {
            Ok(notice) => {
                state.modal = Modal::None;
                state.notice = Some(notice);
                refresh(state, ctx)
            }
            Err(e) => {
                state.error = Some(e);
                Task::none()
            }
        },
    }
}

// ---- View ------------------------------------------------------------------------------------

pub fn view<'a>(state: &'a State, ctx: &'a Ctx) -> Element<'a, Msg> {
    let _ = ctx;
    let page: Element<'a, Msg> = match state.data.configured {
        None => match &state.error {
            Some(e) => ui::sheet(
                ui::view_head("Git", vec![]),
                column![
                    ui::empty("git", "The git host is offline", "Start the Forgejo container to browse repositories."),
                    ui::error_line(e),
                ]
                .into(),
            ),
            None => ui::sheet(ui::view_head("Git", vec![]), ui::loading()),
        },
        Some(false) => ui::sheet(ui::view_head("Git", vec![]), ui::empty("git", "The git host is not configured.", "")),
        Some(true) if state.repo.is_empty() => repos_page(state),
        Some(true) if state.pr.is_some() => pull_page(state),
        Some(true) => repo_page(state),
    };
    match state.modal {
        Modal::None => page,
        modal => stack(vec![page, modal_layer(state, modal)]).into(),
    }
}

/// A sheet with the screen's notes (an error, a notice) above its body.
fn sheet<'a>(state: &'a State, head: Element<'a, Msg>, body: Element<'a, Msg>) -> Element<'a, Msg> {
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

fn repos_page<'a>(state: &'a State) -> Element<'a, Msg> {
    let create = ui::btn("New repository", None, Kind::Primary, Some(Msg::Modal(Modal::NewRepo)));
    let body: Element<'a, Msg> = if state.data.repos.is_empty() {
        ui::empty("git", "No repositories yet", "Create one for the agents to clone and push to.")
    } else {
        let mut rows: Vec<Element<'a, Msg>> = Vec::new();
        for r in &state.data.repos {
            rows.push(repo_row(r));
            rows.push(ui::hairline());
        }
        column(rows).into()
    };
    sheet(state, ui::view_head("Git", vec![create]), body)
}

fn repo_row<'a>(r: &Repo) -> Element<'a, Msg> {
    let description = if r.description.is_empty() { "No description".to_string() } else { r.description.clone() };
    let info = column![ui::mono(r.name.clone(), 14.0, INK), ui::faint(description)].spacing(2);
    let pulls = if r.open_prs == 1 { "pull request" } else { "pull requests" };
    let meta = column![
        ui::mono(r.default_branch.clone(), 11.0, INK_3),
        ui::faint(format!("{} open {pulls}", r.open_prs)),
        ui::faint(format!("updated {}", ago(&r.updated_at))),
    ]
    .spacing(2);
    let mut line = row![
        button(info)
            .on_press(Msg::Open(r.name.clone()))
            .style(theme::register_row(false))
            .width(Length::Fill),
        meta,
    ]
    .spacing(16)
    .align_y(Alignment::Center);
    if r.project.is_some() {
        line = line.push(ui::btn("project", None, Kind::Ghost, Some(Msg::Go(Id::Projects))));
    }
    line.padding([10, 12]).into()
}

fn repo_page<'a>(state: &'a State) -> Element<'a, Msg> {
    let back = ui::btn("Repositories", None, Kind::Ghost, Some(Msg::Home));
    if !state.data.repos.iter().any(|r| r.name == state.repo) {
        return sheet(state, ui::view_head(&state.repo, vec![back]), ui::empty("git", "No such repository", "It may have been removed."));
    }
    let clone = row![
        ui::label("Clone"),
        ui::mono(clone_url(state), 12.0, INK),
        ui::btn("Copy", None, Kind::Ghost, Some(Msg::Copy)),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    let tabs: Vec<Element<'a, Msg>> = [(Tab::Code, "Code"), (Tab::Commits, "Commits"), (Tab::Branches, "Branches"), (Tab::Pulls, "Pull requests")]
        .into_iter()
        .map(|(tab, label)| {
            let kind = if tab == state.tab { Kind::Secondary } else { Kind::Ghost };
            ui::btn(label, None, kind, Some(Msg::Tab(tab)))
        })
        .collect();
    let body = match state.tab {
        Tab::Code => code_view(state),
        Tab::Commits => commits_view(state),
        Tab::Branches => branches_view(state),
        Tab::Pulls => pulls_view(state),
    };
    let content = column![clone, row(tabs).spacing(6), body].spacing(16);
    sheet(state, ui::view_head(&state.repo, vec![back]), content.into())
}

fn code_view<'a>(state: &'a State) -> Element<'a, Msg> {
    let names: Vec<String> = state.data.branches.iter().map(|b| b.name.clone()).collect();
    if names.is_empty() {
        return ui::empty("git", "Nothing pushed yet", "This repository has no branches.");
    }
    let picker = pick_list(names, Some(state.git_ref.clone()), Msg::Branch);
    let toolbar = row![ui::label("Branch"), picker].spacing(10).align_y(Alignment::Center);
    let mut items: Vec<Element<'a, Msg>> = vec![toolbar.into(), crumbs(state), dir_table(&state.data.tree)];
    if let Some(file) = &state.data.file {
        items.push(file_block(&state.file_path, file));
    } else if let Some(readme) = &state.data.readme {
        if let Some(text) = &readme.text {
            items.push(ui::card("README.md", ui::markdown(text)));
        }
    }
    column(items).spacing(14).into()
}

fn crumbs<'a>(state: &'a State) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = vec![ui::btn(&state.repo, None, Kind::Ghost, Some(Msg::Dir(String::new())))];
    let parts: Vec<&str> = if state.path.is_empty() { Vec::new() } else { state.path.split('/').collect() };
    for (i, part) in parts.iter().enumerate() {
        let to = parts[..=i].join("/");
        items.push(ui::faint("/").into());
        items.push(ui::btn(part, None, Kind::Ghost, Some(Msg::Dir(to))));
    }
    row(items).spacing(4).align_y(Alignment::Center).into()
}

fn dir_table<'a>(tree: &[Entry]) -> Element<'a, Msg> {
    if tree.is_empty() {
        return ui::empty("git", "Empty directory", "");
    }
    let mut sorted: Vec<&Entry> = tree.iter().collect();
    sorted.sort_by(|a, b| (a.kind != "dir").cmp(&(b.kind != "dir")).then_with(|| a.name.cmp(&b.name)));
    let mut rows: Vec<Element<'a, Msg>> = vec![
        row![cell(Length::Fill, ui::label("Name")), cell(Length::Fixed(120.0), ui::label("Size"))].padding([8, 0]).into(),
    ];
    for e in sorted {
        let dir = e.kind == "dir";
        let label = if dir { format!("{}/", e.name) } else { e.name.clone() };
        let on_press = if dir { Msg::Dir(e.path.clone()) } else { Msg::File(e.path.clone()) };
        let size = if dir { "—".to_string() } else { fmt_size(e.size) };
        rows.push(
            row![
                cell(Length::Fill, button(ui::mono(label, 12.0, INK)).on_press(on_press).style(theme::register_row(false)).width(Length::Fill)),
                cell(Length::Fixed(120.0), ui::mono(size, 12.0, INK_3)),
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into(),
        );
    }
    list(rows)
}

fn file_block<'a>(path: &str, file: &'a FileView) -> Element<'a, Msg> {
    let body: Element<'a, Msg> = if file.binary {
        ui::faint(format!("Binary file ({} bytes)", file.size)).into()
    } else {
        match &file.text {
            None => ui::faint("No preview for this file.").into(),
            Some(text) => {
                let mut lines: Vec<&str> = text.split('\n').collect();
                if lines.last() == Some(&"") {
                    lines.pop();
                }
                let rows: Vec<Element<'a, Msg>> = lines
                    .iter()
                    .enumerate()
                    .map(|(i, line)| {
                        row![
                            cell(Length::Fixed(44.0), ui::mono((i + 1).to_string(), 11.0, INK_3)),
                            ui::mono(line.to_string(), 12.0, INK),
                        ]
                        .spacing(12)
                        .into()
                    })
                    .collect();
                scrollable(column(rows)).width(Length::Fill).into()
            }
        }
    };
    let head = row![ui::mono(path.to_string(), 12.0, INK), Space::new().width(Length::Fill), ui::faint(fmt_size(file.size))]
        .align_y(Alignment::Center);
    column![head, ui::hairline(), body].spacing(8).into()
}

fn commits_view<'a>(state: &'a State) -> Element<'a, Msg> {
    if state.data.commits.is_empty() {
        return ui::empty("git", "No commits yet", "");
    }
    list(state.data.commits.iter().map(commit_row).collect())
}

fn commit_row<'a>(k: &Commit) -> Element<'a, Msg> {
    row![
        cell(Length::Fixed(80.0), ui::mono(k.short.clone(), 12.0, INK)),
        cell(Length::Fill, ui::prose(first_line(&k.message).to_string())),
        cell(Length::Fixed(140.0), ui::faint(k.author.clone())),
        cell(Length::Fixed(90.0), ui::faint(ago(&k.at))),
        cell(Length::Fixed(60.0), ui::mono(format!("+{}", k.additions), 12.0, OK)),
        cell(Length::Fixed(60.0), ui::mono(format!("−{}", k.deletions), 12.0, CRIT)),
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .padding([8, 0])
    .into()
}

fn branches_view<'a>(state: &'a State) -> Element<'a, Msg> {
    if state.data.branches.is_empty() {
        return ui::empty("git", "No branches yet", "");
    }
    let mut rows: Vec<Element<'a, Msg>> = vec![
        row![
            cell(Length::Fixed(200.0), ui::label("Branch")),
            cell(Length::Fill, ui::label("Last commit")),
            cell(Length::Fixed(140.0), ui::label("Author")),
            cell(Length::Fixed(90.0), ui::label("Committed")),
        ]
        .spacing(12)
        .padding([8, 0])
        .into(),
    ];
    for b in &state.data.branches {
        rows.push(
            row![
                cell(Length::Fixed(200.0), ui::mono(b.name.clone(), 12.0, INK)),
                cell(Length::Fill, ui::prose(first_line(&b.commit.message).to_string())),
                cell(Length::Fixed(140.0), ui::faint(b.commit.author.clone())),
                cell(Length::Fixed(90.0), ui::faint(ago(&b.commit.at))),
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .padding([8, 0])
            .into(),
        );
    }
    list(rows)
}

fn pulls_view<'a>(state: &'a State) -> Element<'a, Msg> {
    let filters: Vec<Element<'a, Msg>> = [(PullState::Open, "Open"), (PullState::Closed, "Closed"), (PullState::All, "All")]
        .into_iter()
        .map(|(filter, label)| {
            let kind = if filter == state.pulls { Kind::Secondary } else { Kind::Ghost };
            ui::btn(label, None, kind, Some(Msg::Pulls(filter)))
        })
        .collect();
    let body = if state.data.pulls.is_empty() {
        ui::empty("git", "No pull requests here", "")
    } else {
        list(state.data.pulls.iter().map(pull_row).collect())
    };
    column![row(filters).spacing(6), body].spacing(12).into()
}

fn pull_row<'a>(p: &Pull) -> Element<'a, Msg> {
    button(
        row![
            cell(Length::Fixed(60.0), ui::mono(format!("#{}", p.number), 12.0, INK_3)),
            cell(Length::Fill, ui::mono(p.title.clone(), 13.0, INK)),
            cell(Length::Fixed(220.0), ui::mono(format!("{} → {}", p.head, p.base), 11.0, INK_2)),
            cell(Length::Fixed(120.0), ui::faint(p.author.clone())),
            review_badge(&p.review),
            ui::faint(format!("{} comments", p.comments)),
            ui::faint(format!("updated {}", ago(&p.updated_at))),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .on_press(Msg::OpenPull(p.number))
    .style(theme::register_row(false))
    .width(Length::Fill)
    .padding([8, 4])
    .into()
}

fn pull_page<'a>(state: &'a State) -> Element<'a, Msg> {
    let Some(d) = state.data.pull.as_ref() else {
        return sheet(state, ui::view_head(&state.repo, vec![]), ui::loading());
    };
    let p = &d.pull;
    let open = p.state == "open";
    let mergeable = open && p.review == "approved" && p.mergeable;
    let trail = row![
        ui::btn(&state.repo, None, Kind::Ghost, Some(Msg::Tab(Tab::Code))),
        ui::faint("/"),
        ui::btn("Pull requests", None, Kind::Ghost, Some(Msg::Tab(Tab::Pulls))),
        ui::faint("/"),
        ui::mono(format!("#{}", p.number), 12.0, INK_2),
    ]
    .spacing(4)
    .align_y(Alignment::Center);
    let meta = row![
        ui::mono(format!("#{}", p.number), 12.0, INK_3),
        ui::badge(&p.state, if open { Tone::Ok } else { Tone::Muted }),
        review_badge(&p.review),
        ui::faint(p.author.clone()),
        ui::mono(format!("{} → {}", p.head, p.base), 12.0, INK_2),
        ui::faint(format!("updated {}", ago(&p.updated_at))),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    let actions = row![
        ui::btn("Approve", None, Kind::Primary, Some(Msg::Modal(Modal::Review(ReviewEvent::Approve)))),
        ui::btn("Request changes", None, Kind::Danger, Some(Msg::Modal(Modal::Review(ReviewEvent::RequestChanges)))),
        ui::btn("Comment", None, Kind::Ghost, Some(Msg::Modal(Modal::Review(ReviewEvent::Comment)))),
        ui::btn("Merge", None, Kind::Primary, mergeable.then_some(Msg::Modal(Modal::Merge))),
    ]
    .spacing(8);
    let description: Element<'a, Msg> = if d.body.is_empty() {
        ui::faint("No description provided.").into()
    } else {
        container(ui::prose(d.body.clone())).max_width(620).into()
    };
    let files: Element<'a, Msg> = if d.files.is_empty() {
        ui::faint("No files.").into()
    } else {
        list(d.files.iter().map(file_row).collect())
    };
    let reviews: Element<'a, Msg> = if d.reviews.is_empty() {
        ui::faint("No reviews yet.").into()
    } else {
        column(d.reviews.iter().map(review_row).collect::<Vec<Element<'a, Msg>>>()).spacing(12).into()
    };
    let body = column![
        trail,
        meta,
        actions,
        ui::card("Description", description),
        ui::card(&format!("Files changed ({})", d.files.len()), files),
        ui::card("Diff", diff_view(&d.diff)),
        ui::card(&format!("Reviews ({})", d.reviews.len()), reviews),
    ]
    .spacing(16);
    sheet(state, ui::view_head(&p.title, vec![]), body.into())
}

fn file_row<'a>(f: &PullFile) -> Element<'a, Msg> {
    row![
        cell(Length::Fill, ui::mono(f.path.clone(), 12.0, INK)),
        cell(Length::Fixed(60.0), ui::mono(format!("+{}", f.additions), 12.0, OK)),
        cell(Length::Fixed(60.0), ui::mono(format!("−{}", f.deletions), 12.0, CRIT)),
    ]
    .spacing(12)
    .padding([6, 0])
    .into()
}

fn review_row<'a>(rv: &Review) -> Element<'a, Msg> {
    let s = rv.state.to_lowercase();
    let tone = if s.contains("approv") {
        Tone::Ok
    } else if s.contains("change") {
        Tone::Danger
    } else {
        Tone::Muted
    };
    let head = row![
        ui::mono(rv.author.clone(), 13.0, INK),
        ui::badge(&s.replace('_', " "), tone),
        ui::faint(ago(&rv.at)),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let mut col = column![head].spacing(6);
    if !rv.body.is_empty() {
        col = col.push(ui::prose(rv.body.clone()));
    }
    col.into()
}

/// The diff as mono lines, coloured by their marker.
fn diff_view<'a>(diff: &str) -> Element<'a, Msg> {
    let lines: Vec<Element<'a, Msg>> = diff.split('\n').map(|line| ui::mono(line.to_string(), 12.0, diff_colour(line)).into()).collect();
    scrollable(column(lines)).width(Length::Fill).into()
}

fn diff_colour(line: &str) -> Color {
    if line.starts_with("@@") {
        INK_2
    } else if line.starts_with("+++") || line.starts_with("---") {
        INK_3
    } else if line.starts_with('+') {
        OK
    } else if line.starts_with('-') {
        CRIT
    } else {
        INK
    }
}

fn review_badge<'a>(review: &str) -> Element<'a, Msg> {
    match review {
        "approved" => ui::badge("approved", Tone::Ok),
        "changes_requested" => ui::badge("changes requested", Tone::Danger),
        _ => ui::badge("pending", Tone::Muted),
    }
}

/// The modal over the screen: a boxed card on a scrim.
fn modal_layer<'a>(state: &'a State, modal: Modal) -> Element<'a, Msg> {
    let n = state.pr.unwrap_or(0);
    let (title, body): (String, Element<'a, Msg>) = match modal {
        Modal::None => (String::new(), column![].into()),
        Modal::NewRepo => ("New repository".to_string(), new_repo_form(state)),
        Modal::Review(event) => (format!("{} on #{n}", event.label()), review_form(state, event)),
        Modal::Merge => (format!("Merge #{n}"), merge_form(state)),
    };
    let boxed = container(ui::card(&title, body)).width(Length::Fixed(520.0)).style(theme::boxed);
    container(boxed)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(theme::scrim)
        .into()
}

fn new_repo_form<'a>(state: &'a State) -> Element<'a, Msg> {
    column![
        field(
            "Name",
            text_input("repo-name", &state.new_name).on_input(Msg::NewName).style(theme::input).into(),
        ),
        field(
            "Description",
            text_editor(&state.body)
                .placeholder("What this repository is for")
                .on_action(Msg::Body)
                .height(Length::Fixed(96.0))
                .into(),
        ),
        form_actions(Msg::CreateRepo, "Create", Kind::Primary),
    ]
    .spacing(14)
    .into()
}

fn review_form<'a>(state: &'a State, event: ReviewEvent) -> Element<'a, Msg> {
    let placeholder = if event == ReviewEvent::Approve { "Optional note" } else { "Write the review" };
    let kind = if event == ReviewEvent::RequestChanges { Kind::Danger } else { Kind::Primary };
    column![
        field(
            "Review",
            text_editor(&state.body)
                .placeholder(placeholder)
                .on_action(Msg::Body)
                .height(Length::Fixed(140.0))
                .into(),
        ),
        form_actions(Msg::SendReview(event), event.label(), kind),
    ]
    .spacing(14)
    .into()
}

fn merge_form<'a>(state: &'a State) -> Element<'a, Msg> {
    let text = match state.data.pull.as_ref() {
        Some(d) => format!("Merge {} into {}?", d.pull.head, d.pull.base),
        None => String::new(),
    };
    let style = pick_list(
        vec![MergeStyle::Merge, MergeStyle::Squash, MergeStyle::Rebase],
        Some(state.merge_style),
        Msg::MergeStyle,
    );
    column![ui::prose(text), field("Style", style.into()), form_actions(Msg::Merge, "Merge", Kind::Primary)]
        .spacing(14)
        .into()
}

fn field<'a>(title: &str, body: Element<'a, Msg>) -> Element<'a, Msg> {
    column![ui::label(title), body].spacing(6).into()
}

fn form_actions<'a>(confirm: Msg, label: &str, kind: Kind) -> Element<'a, Msg> {
    row![
        Space::new().width(Length::Fill),
        ui::btn("Cancel", None, Kind::Ghost, Some(Msg::Modal(Modal::None))),
        ui::btn(label, None, kind, Some(confirm)),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

// ---- Helpers ---------------------------------------------------------------------------------

/// A table cell at a fixed or fill width.
fn cell<'a>(width: Length, content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    container(content).width(width).into()
}

/// Rows with a hairline under each.
fn list<'a>(rows: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = Vec::with_capacity(rows.len() * 2);
    for r in rows {
        items.push(r);
        items.push(ui::hairline());
    }
    column(items).into()
}

fn clone_url(state: &State) -> String {
    format!("http://plaza.gateway/git/{}/{}.git", state.data.org, state.repo)
}

/// Percent-encodes a query value.
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn first_line(message: &str) -> &str {
    message.split('\n').next().unwrap_or("")
}

fn fmt_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}

/// A relative time ("5m ago") for an RFC 3339 timestamp; the raw text when it does not parse.
fn ago(at: &str) -> String {
    let Some(t) = parse_time(at) else {
        return at.to_string();
    };
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(t);
    let secs = (now - t).max(0);
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}

/// Unix seconds of an RFC 3339 timestamp.
fn parse_time(s: &str) -> Option<i64> {
    let num = |a: usize, b: usize| -> Option<i64> { s.get(a..b)?.parse::<i64>().ok() };
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, se) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let rest = s.get(19..)?.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match rest.as_bytes().first() {
        Some(b'+') | Some(b'-') => {
            let sign = if rest.starts_with('-') { -1 } else { 1 };
            let hh = rest.get(1..3)?.parse::<i64>().ok()?;
            let mm = rest.get(4..6)?.parse::<i64>().ok()?;
            sign * (hh * 3600 + mm * 60)
        }
        _ => 0,
    };
    Some(days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + se - offset)
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}
