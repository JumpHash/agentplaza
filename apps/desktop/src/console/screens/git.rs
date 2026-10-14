//! The Git screen (parity with the web console's `git` view): the plaza's repositories on its git
//! host, each with its code, commits, branches and pull requests; the owner reviews and merges
//! here. Drill-downs (repository, tab, branch, folder, file, pull request) are the screen's own
//! state, as the web's route parameters are.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{
    Space, button, column, container, pick_list, row, stack, text, text_editor, text_input,
};
use iced::{Alignment, Border, Color, Element, Font, Length, Padding, Task, Theme, font, keyboard};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Id, ide};
use crate::console::client::{self, Client};
use crate::console::ui::{self, INK, INK_2, INK_3, Kind, Tone};
use crate::console::{Ctx, rich};
use crate::theme::{self};

/// Serif semibold: a pull request's title.
const SANS_SEMIBOLD: Font = Font {
    weight: font::Weight::Semibold,
    ..theme::SANS
};
/// Serif bold: a review's author.
const SANS_BOLD: Font = Font {
    weight: font::Weight::Bold,
    ..theme::SANS
};

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
    /// A merged pull request is closed in Forgejo, with `merged` true. Older hubs omit both fields.
    #[serde(default)]
    merged: bool,
    #[serde(default)]
    merged_at: Option<String>,
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

/// One comment in a pull request's discussion (an issue comment on it).
#[derive(Debug, Clone, Deserialize)]
pub struct Comment {
    author: String,
    body: String,
    at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PullDetail {
    /// The row: `comments` is the discussion's count; the comments themselves are `discussion`.
    #[serde(flatten)]
    pull: Pull,
    body: String,
    diff: String,
    reviews: Vec<Review>,
    #[serde(default)]
    discussion: Vec<Comment>,
    files: Vec<PullFile>,
}

/// How many pull requests one page of the list asks for (the hub's limit maximum).
const PULL_PAGE: u64 = 50;

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
    /// True when the list has a further page (the last one loaded was full).
    pulls_more: bool,
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
        self.pulls_more = false;
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
    NewPull,
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
    /// How many pages of the pull request list are loaded (each page is fetched again on a poll).
    pages: u64,
}

#[derive(Debug, Default)]
pub struct State {
    pub error: Option<String>,
    pub notice: Option<String>,
    pub data: Data,
    /// The rich content of the open pull request's description and discussion: the hub's cards and images.
    pub cards: rich::Cards,
    /// The open repository ("" for the repository list).
    pub repo: String,
    pub tab: Tab,
    pub pulls: PullState,
    /// The pages of the pull request list the screen has asked for (0 counts as 1).
    pub pull_pages: u64,
    /// The open pull request's number.
    pub pr: Option<u64>,
    pub git_ref: String,
    pub path: String,
    pub file_path: String,
    pub modal: Modal,
    pub new_name: String,
    /// The text of the open modal (a description or a review), and a new pull request's body.
    pub body: text_editor::Content,
    /// The new pull request's title, head branch and base branch.
    pub pr_title: String,
    pub pr_head: Option<String>,
    pub pr_base: Option<String>,
    /// The discussion's comment box.
    pub comment: text_editor::Content,
    pub merge_style: MergeStyle,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    /// Opens the IDE on a target (a file of a branch, or a pull request's head in review mode).
    OpenIde(String),
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
    PrTitle(String),
    PrHead(String),
    PrBase(String),
    CreatePull,
    PullCreated(Result<Pull, String>),
    MorePulls,
    Comment(text_editor::Action),
    SendComment,
    Commented(Result<String, String>),
    /// Closes ("closed") or reopens ("open") the open pull request.
    SetState(&'static str),
    SendReview(ReviewEvent),
    Merge,
    Acted(Result<String, String>),
    /// A rich-content click (a chip, a link, an Open button) or a load of its cards and images.
    Rich(rich::Ev),
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        Msg::OpenIde(target) => Some(Id::Ide(Some(target.clone()))),
        Msg::Rich(rich::Ev::Action(action)) => action.route(),
        _ => None,
    }
}

/// A rich-content click or load, as this screen's message.
fn on_rich(action: rich::RichAction) -> Msg {
    Msg::Rich(rich::Ev::Action(action))
}

/// The rich context of the open repository: its name is "owner/repo" for a plaza repository, which makes `PR #N` a
/// pull of it.
fn rich_ctx(state: &State) -> rich::RichCtx {
    rich::context(
        &state.cards,
        None,
        state.repo.contains('/').then_some(state.repo.as_str()),
        None,
    )
}

/// Asks the hub for the cards of the open pull request's description and discussion that it does not hold yet.
fn prefetch(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let texts: Vec<&str> = match state.data.pull.as_ref() {
        Some(d) => std::iter::once(d.body.as_str())
            .chain(d.discussion.iter().map(|c| c.body.as_str()))
            .collect(),
        None => Vec::new(),
    };
    let rctx = rich_ctx(state);
    rich::prefetch(&mut state.cards, &ctx.client, &rctx, &texts).map(Msg::Rich)
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
        pages: state.pull_pages.max(1),
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
    client::task(
        move || fetch(&client, &q),
        move |res| Msg::Loaded(tag.clone(), res),
    )
}

/// Fetches what the open view shows: the host, the repositories, and the open repository's tab.
fn fetch(client: &Client, q: &Query) -> Result<Data, String> {
    let git: GitInfo = client.get("/v1/git")?;
    let mut d = Data {
        configured: Some(git.configured),
        org: git.org,
        ..Data::default()
    };
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
            d.tree = client.get(&format!(
                "{base}/tree?ref={}&path={}",
                enc(&q.git_ref),
                enc(&q.path)
            ))?;
            if !q.file.is_empty() {
                d.file = Some(client.get(&format!(
                    "{base}/file?ref={}&path={}",
                    enc(&q.git_ref),
                    enc(&q.file)
                ))?);
            } else if let Some(readme) = d
                .tree
                .iter()
                .find(|e| e.kind == "file" && e.name == "README.md")
            {
                let f: FileView = client.get(&format!(
                    "{base}/file?ref={}&path={}",
                    enc(&q.git_ref),
                    enc(&readme.path)
                ))?;
                d.readme = Some(f);
            }
        }
        Tab::Commits => {
            d.commits = client.get(&format!("{base}/commits?ref={}&limit=30", enc(&q.git_ref)))?
        }
        Tab::Pulls => match q.pr {
            Some(n) => d.pull = Some(client.get(&format!("{base}/pulls/{n}"))?),
            None => {
                // Every page loaded so far, in order: a poll keeps the list as the reader paged it.
                for page in 1..=q.pages {
                    let rows: Vec<Pull> = client.get(&format!(
                        "{base}/pulls?state={}&page={page}&limit={PULL_PAGE}",
                        q.pulls.key()
                    ))?;
                    let full = rows.len() as u64 == PULL_PAGE;
                    d.pulls.extend(rows);
                    d.pulls_more = full;
                    if !full {
                        break;
                    }
                }
            }
        },
        _ => {}
    }
    Ok(d)
}

/// The branch the code tab should show when the chosen one is gone: the default, else the first.
fn fix_ref(state: &State) -> Option<String> {
    let names: Vec<&str> = state
        .data
        .branches
        .iter()
        .map(|b| b.name.as_str())
        .collect();
    if names.is_empty() || names.contains(&state.git_ref.as_str()) {
        return None;
    }
    let default = state
        .data
        .repos
        .iter()
        .find(|r| r.name == state.repo)
        .map(|r| r.default_branch.as_str());
    let pick = match default {
        Some(d) if names.contains(&d) => d,
        _ => names[0],
    };
    Some(pick.to_string())
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    if let Msg::Rich(ev) = msg {
        rich::apply(&mut state.cards, ev);
        return Task::none();
    }
    let loaded = matches!(msg, Msg::Loaded(_, Ok(_)));
    let task = apply(state, msg, ctx);
    if loaded {
        Task::batch([task, prefetch(state, ctx)])
    } else {
        task
    }
}

/// The screen's messages. `update` wraps it to read the rich content of a loaded pull request.
fn apply(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) | Msg::OpenIde(_) | Msg::Rich(_) => Task::none(),
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
            state.pull_pages = 1;
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
            state.pull_pages = 1;
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
            state.pull_pages = 1;
            state.data.pulls.clear();
            refresh(state, ctx)
        }
        Msg::MorePulls => {
            state.pull_pages = state.pull_pages.max(1) + 1;
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
            if modal == Modal::NewPull {
                // The base starts on the repository's default branch; the head is the reader's pick.
                state.pr_title.clear();
                state.pr_head = None;
                state.pr_base = state
                    .data
                    .repos
                    .iter()
                    .find(|r| r.name == state.repo)
                    .map(|r| r.default_branch.clone());
            }
            if modal == Modal::Merge {
                state.merge_style = MergeStyle::Merge;
            }
            Task::none()
        }
        Msg::PrTitle(title) => {
            state.pr_title = title;
            Task::none()
        }
        Msg::PrHead(name) => {
            state.pr_head = Some(name);
            Task::none()
        }
        Msg::PrBase(name) => {
            state.pr_base = Some(name);
            Task::none()
        }
        Msg::CreatePull => {
            let title = state.pr_title.trim().to_string();
            if title.is_empty() {
                state.error = Some("Give the pull request a title".into());
                return Task::none();
            }
            let (Some(head), Some(base)) = (state.pr_head.clone(), state.pr_base.clone()) else {
                state.error =
                    Some("Pick the branch to merge from and the branch to merge into".into());
                return Task::none();
            };
            if head == base {
                state.error = Some("Pick two different branches".into());
                return Task::none();
            }
            let payload = json!({ "title": title, "head": head, "base": base, "body": state.body.text().trim() });
            let path = format!("/v1/git/repos/{}/pulls", state.repo);
            let client = ctx.client.clone();
            client::task(
                move || client.post::<Pull>(&path, &payload),
                Msg::PullCreated,
            )
        }
        Msg::PullCreated(res) => match res {
            Ok(pull) => {
                state.modal = Modal::None;
                state.notice = Some(format!("#{} opened", pull.number));
                state.tab = Tab::Pulls;
                state.pr = Some(pull.number);
                state.data.clear_tab();
                refresh(state, ctx)
            }
            Err(e) => {
                state.error = Some(e);
                Task::none()
            }
        },
        Msg::Comment(action) => {
            state.comment.perform(action);
            Task::none()
        }
        Msg::SendComment => {
            let body = state.comment.text().trim().to_string();
            let Some(n) = state.pr else {
                return Task::none();
            };
            if body.is_empty() {
                return Task::none();
            }
            let path = format!("/v1/git/repos/{}/pulls/{n}/comments", state.repo);
            let payload = json!({ "body": body });
            let client = ctx.client.clone();
            client::task(
                move || {
                    client
                        .post::<Value>(&path, &payload)
                        .map(|_| "Comment posted".to_string())
                },
                Msg::Commented,
            )
        }
        Msg::Commented(res) => match res {
            Ok(notice) => {
                state.comment = text_editor::Content::new();
                state.notice = Some(notice);
                refresh(state, ctx)
            }
            Err(e) => {
                state.error = Some(e);
                Task::none()
            }
        },
        Msg::SetState(to) => {
            let Some(n) = state.pr else {
                return Task::none();
            };
            let path = format!("/v1/git/repos/{}/pulls/{n}/state", state.repo);
            let payload = json!({ "state": to });
            let client = ctx.client.clone();
            let done = if to == "closed" { "closed" } else { "reopened" };
            client::task(
                move || {
                    client
                        .post::<Value>(&path, &payload)
                        .map(|_| format!("#{n} {done}"))
                },
                Msg::Acted,
            )
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
            client::task(
                move || {
                    client
                        .post::<Value>("/v1/git/repos", &payload)
                        .map(|_| name)
                },
                Msg::Created,
            )
        }
        Msg::Created(res) => match res {
            Ok(name) => {
                state.modal = Modal::None;
                state.repo = name;
                state.notice = Some("Repository created".into());
                state.tab = Tab::Code;
                state.pr = None;
                state.pull_pages = 1;
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
            client::task(
                move || {
                    client
                        .call("POST", &path, Some(&payload))
                        .map(|_| format!("{label} sent"))
                },
                Msg::Acted,
            )
        }
        Msg::Merge => {
            let Some(n) = state.pr else {
                return Task::none();
            };
            let path = format!("/v1/git/repos/{}/pulls/{n}/merge", state.repo);
            let payload = json!({ "style": state.merge_style.key() });
            let client = ctx.client.clone();
            client::task(
                move || {
                    client
                        .call("POST", &path, Some(&payload))
                        .map(|_| format!("#{n} merged"))
                },
                Msg::Acted,
            )
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
                    ui::empty(
                        "git",
                        "The git host is offline",
                        "Start the Forgejo container to browse repositories."
                    ),
                    ui::error_line(e),
                ]
                .into(),
            ),
            None => ui::sheet(ui::view_head("Git", vec![]), ui::loading()),
        },
        Some(false) => ui::sheet(
            ui::view_head("Git", vec![]),
            ui::empty("git", "The git host is not configured.", ""),
        ),
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
    let create = ui::btn(
        "New repository",
        None,
        Kind::Primary,
        Some(Msg::Modal(Modal::NewRepo)),
    );
    let body: Element<'a, Msg> = if state.data.repos.is_empty() {
        ui::empty(
            "git",
            "No repositories yet",
            "Create one for the agents to clone and push to.",
        )
    } else {
        let mut rows: Vec<Element<'a, Msg>> = Vec::new();
        for r in &state.data.repos {
            rows.push(repo_row(r));
            rows.push(rule_line());
        }
        column(rows).into()
    };
    sheet(state, ui::view_head("Git", vec![create]), body)
}

fn repo_row<'a>(r: &Repo) -> Element<'a, Msg> {
    let description = if r.description.is_empty() {
        "No description".to_string()
    } else {
        r.description.clone()
    };
    let name = text(r.name.clone())
        .font(ui::SANS_SEMIBOLD)
        .size(14.0)
        .color(INK);
    let info = column![name, ui::prose(description).size(13.0).color(INK_2)].spacing(2);
    let pulls = if r.open_prs == 1 {
        "pull request"
    } else {
        "pull requests"
    };
    let meta = column![
        ui::sans(r.default_branch.clone(), 11.0, INK_3),
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
        line = line.push(chip("project", Msg::Go(Id::Projects)));
    }
    line.padding([10, 12]).into()
}

fn repo_page<'a>(state: &'a State) -> Element<'a, Msg> {
    let back = ui::btn("Repositories", None, Kind::Ghost, Some(Msg::Home));
    if !state.data.repos.iter().any(|r| r.name == state.repo) {
        return sheet(
            state,
            ui::view_head(&state.repo, vec![back]),
            ui::empty("git", "No such repository", "It may have been removed."),
        );
    }
    let url = container(ui::mono(clone_url(state), 13.0, INK))
        .padding([4, 8])
        .style(|_: &Theme| container::Style {
            background: Some(crate::theme::palette().sheet.into()),
            border: Border {
                color: crate::theme::palette().rule,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        });
    let clone = row![
        ui::label("Clone"),
        url,
        ui::btn("Copy", None, Kind::Ghost, Some(Msg::Copy)),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    let tabs: Vec<Element<'a, Msg>> = [
        (Tab::Code, "Code"),
        (Tab::Commits, "Commits"),
        (Tab::Branches, "Branches"),
        (Tab::Pulls, "Pull requests"),
    ]
    .into_iter()
    .map(|(tab, label)| {
        let kind = if tab == state.tab {
            Kind::Secondary
        } else {
            Kind::Ghost
        };
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
    sheet(
        state,
        ui::view_head(&state.repo, vec![back]),
        content.into(),
    )
}

fn code_view<'a>(state: &'a State) -> Element<'a, Msg> {
    if state.data.branches.is_empty() {
        return ui::empty(
            "git",
            "Nothing pushed yet",
            "This repository has no branches.",
        );
    }
    let mut items: Vec<Element<'a, Msg>> = vec![
        branch_picker(state),
        crumbs(state),
        dir_table(&state.data.tree),
    ];
    if let Some(file) = &state.data.file {
        // The file's branch: the one browsed, else the repository's default. No branch, no link.
        let branch = if state.git_ref.is_empty() {
            state
                .data
                .repos
                .iter()
                .find(|r| r.name == state.repo)
                .map(|r| r.default_branch.clone())
        } else {
            Some(state.git_ref.clone())
        };
        if let Some(branch) = branch.filter(|b| !b.is_empty()) {
            let target = ide::target(
                &format!("repo:{}@{branch}", state.repo),
                false,
                Some(state.file_path.as_str()),
            );
            items.push(ui::btn_sm(
                "Open in IDE",
                Some("square-code"),
                Kind::Ghost,
                Some(Msg::OpenIde(target)),
            ));
        }
        items.push(file_block(&state.file_path, file));
    } else if let Some(readme) = &state.data.readme {
        if let Some(source) = &readme.text {
            // The web shows the README as plain text in a file box, not rendered.
            let body = container(ui::prose(source.clone()).size(14.0)).padding([12, 14]);
            items.push(filebox(
                ui::sans("README.md", 13.0, INK).into(),
                body.into(),
            ));
        }
    }
    column(items).spacing(14).into()
}

/// The branch picker: the repository's branches with the open one selected (code and commits tabs).
fn branch_picker<'a>(state: &'a State) -> Element<'a, Msg> {
    let names: Vec<String> = state.data.branches.iter().map(|b| b.name.clone()).collect();
    let picker = pick_list(names, Some(state.git_ref.clone()), Msg::Branch);
    row![ui::label("Branch"), picker]
        .spacing(10)
        .align_y(Alignment::Center)
        .into()
}

fn crumbs<'a>(state: &'a State) -> Element<'a, Msg> {
    let mut items: Vec<Element<'a, Msg>> = Vec::new();
    for (i, (label_text, to)) in crumb_trail(&state.repo, &state.path)
        .into_iter()
        .enumerate()
    {
        if i > 0 {
            items.push(ui::sans("/", 13.0, INK_3).into());
        }
        items.push(link(&label_text, Msg::Dir(to)));
    }
    row(items).spacing(6).align_y(Alignment::Center).into()
}

fn dir_table<'a>(tree: &[Entry]) -> Element<'a, Msg> {
    if tree.is_empty() {
        return ui::empty("git", "Empty directory", "");
    }
    let mut sorted: Vec<&Entry> = tree.iter().collect();
    sorted.sort_by(|a, b| {
        (a.kind != "dir")
            .cmp(&(b.kind != "dir"))
            .then_with(|| a.name.cmp(&b.name))
    });
    let mut rows: Vec<Element<'a, Msg>> = vec![
        row![
            cell(Length::Fill, ui::label("Name")),
            cell(Length::Fixed(120.0), ui::label("Size"))
        ]
        .padding([8, 0])
        .into(),
    ];
    for e in sorted {
        let dir = e.kind == "dir";
        let label = if dir {
            format!("{}/", e.name)
        } else {
            e.name.clone()
        };
        let on_press = if dir {
            Msg::Dir(e.path.clone())
        } else {
            Msg::File(e.path.clone())
        };
        let size = if dir {
            "—".to_string()
        } else {
            fmt_size(e.size)
        };
        rows.push(
            row![
                cell(Length::Fill, link(&label, on_press)),
                cell(
                    Length::Fixed(120.0),
                    container(ui::sans(size, 13.0, INK_2))
                        .width(Length::Fill)
                        .align_x(Alignment::End)
                ),
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
                // Line number (right-aligned, ruled off from the text), then the line.
                let rows: Vec<Element<'a, Msg>> = lines
                    .iter()
                    .enumerate()
                    .map(|(i, line)| {
                        row![
                            container(ui::sans((i + 1).to_string(), 13.0, INK_3))
                                .width(Length::Fixed(44.0))
                                .padding([0, 8])
                                .align_x(Alignment::End),
                            rule_col(),
                            ui::mono(line.to_string(), 13.0, INK),
                        ]
                        .spacing(12)
                        .into()
                    })
                    .collect();
                ui::scroll(column(rows).padding([8, 0]))
                    .width(Length::Fill)
                    .into()
            }
        }
    };
    let head = row![
        ui::mono(path.to_string(), 13.0, INK),
        Space::new().width(Length::Fill),
        ui::faint(fmt_size(file.size))
    ]
    .align_y(Alignment::Center);
    filebox(head.into(), body)
}

/// A file box: a head row over a rule, then the body, in the sheet ground with rule edges (the
/// web's `.git-filebox`).
fn filebox<'a>(head: Element<'a, Msg>, body: Element<'a, Msg>) -> Element<'a, Msg> {
    let head = container(head).padding([8, 12]).width(Length::Fill);
    container(column![head, rule_line(), body])
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(crate::theme::palette().sheet.into()),
            border: Border {
                color: crate::theme::palette().rule,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into()
}

/// A hairline rule in the web's `--rule` colour.
fn rule_line<'a>() -> Element<'a, Msg> {
    container(Space::new().height(1.0))
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(crate::theme::palette().rule.into()),
            ..Default::default()
        })
        .into()
}

/// A vertical rule, as tall as its row.
fn rule_col<'a>() -> Element<'a, Msg> {
    container(Space::new().width(1.0).height(Length::Fill))
        .style(|_: &Theme| container::Style {
            background: Some(crate::theme::palette().rule.into()),
            ..Default::default()
        })
        .into()
}

/// A bare link in mono (the web's `.git-link`): crumbs, folder and file names, the trail.
fn link<'a>(label_text: &str, msg: Msg) -> Element<'a, Msg> {
    button(ui::sans(label_text.to_string(), 13.0, INK))
        .padding(0)
        .style(|_: &Theme, _| button::Style {
            background: None,
            text_color: INK,
            ..Default::default()
        })
        .on_press(msg)
        .into()
}

/// A repository's project chip: a ruled box in serif 12 (the web's `.git-chip`).
fn chip<'a>(label_text: &str, msg: Msg) -> Element<'a, Msg> {
    button(
        text(label_text.to_string())
            .font(theme::SANS)
            .size(12.0)
            .color(INK_2),
    )
    .padding([1, 8])
    .style(|_: &Theme, status: button::Status| {
        let edge = if matches!(status, button::Status::Hovered) {
            INK_2
        } else {
            crate::theme::palette().rule
        };
        button::Style {
            background: None,
            text_color: INK_2,
            border: Border {
                color: edge,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        }
    })
    .on_press(msg)
    .into()
}

fn commits_view<'a>(state: &'a State) -> Element<'a, Msg> {
    if state.data.branches.is_empty() {
        return ui::empty(
            "git",
            "Nothing pushed yet",
            "This repository has no branches.",
        );
    }
    let log: Element<'a, Msg> = if state.data.commits.is_empty() {
        ui::empty("git", "No commits yet", "")
    } else {
        list(state.data.commits.iter().map(commit_row).collect())
    };
    column![branch_picker(state), log].spacing(12).into()
}

fn commit_row<'a>(k: &Commit) -> Element<'a, Msg> {
    row![
        cell(Length::Fixed(80.0), ui::sans(k.short.clone(), 13.0, INK_2)),
        cell(
            Length::Fill,
            ui::prose(first_line(&k.message).to_string()).size(14.0)
        ),
        cell(Length::Fixed(140.0), ui::faint(k.author.clone())),
        cell(Length::Fixed(90.0), ui::faint(ago(&k.at))),
        cell(
            Length::Fixed(60.0),
            ui::mono(
                format!("+{}", k.additions),
                13.0,
                crate::theme::palette().ok
            )
        ),
        cell(
            Length::Fixed(60.0),
            ui::sans(
                format!("−{}", k.deletions),
                13.0,
                crate::theme::palette().crit
            )
        ),
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
                cell(Length::Fixed(200.0), ui::sans(b.name.clone(), 13.0, INK)),
                cell(
                    Length::Fill,
                    ui::prose(first_line(&b.commit.message).to_string()).size(14.0)
                ),
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
    let filters: Vec<Element<'a, Msg>> = [
        (PullState::Open, "Open"),
        (PullState::Closed, "Closed"),
        (PullState::All, "All"),
    ]
    .into_iter()
    .map(|(filter, label)| {
        let kind = if filter == state.pulls {
            Kind::Secondary
        } else {
            Kind::Ghost
        };
        ui::btn(label, None, kind, Some(Msg::Pulls(filter)))
    })
    .collect();
    let body = if state.data.pulls.is_empty() {
        ui::empty("git", "No pull requests here", "")
    } else {
        list(state.data.pulls.iter().map(pull_row).collect())
    };
    let new_pr = ui::btn(
        "New pull request",
        None,
        Kind::Primary,
        Some(Msg::Modal(Modal::NewPull)),
    );
    let head = row![
        row(filters).spacing(6),
        Space::new().width(Length::Fill),
        new_pr
    ]
    .align_y(Alignment::Center);
    let mut items: Vec<Element<'a, Msg>> = vec![head.into(), body];
    if state.data.pulls_more {
        items.push(ui::btn(
            "Load more",
            None,
            Kind::Ghost,
            Some(Msg::MorePulls),
        ));
    }
    column(items).spacing(12).into()
}

fn pull_row<'a>(p: &Pull) -> Element<'a, Msg> {
    button(
        row![
            cell(
                Length::Fixed(60.0),
                ui::sans(format!("#{}", p.number), 13.0, INK_2)
            ),
            cell(
                Length::Fill,
                text(p.title.clone())
                    .font(SANS_SEMIBOLD)
                    .size(15.0)
                    .color(INK)
            ),
            cell(Length::Fixed(80.0), state_badge(p)),
            cell(
                Length::Fixed(220.0),
                ui::sans(format!("{} → {}", p.head, p.base), 12.0, INK_2)
            ),
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
    let rctx = rich_ctx(state);
    let p = &d.pull;
    let open = p.state == "open";
    let merged = p.merged;
    let mergeable = open && p.review == "approved" && p.mergeable;
    // The trail sits above the title, as on the web (repo / Pull requests / #n).
    let trail = row![
        link(&state.repo, Msg::Tab(Tab::Code)),
        ui::sans("/", 13.0, INK_3),
        link("Pull requests", Msg::Tab(Tab::Pulls)),
        ui::sans("/", 13.0, INK_3),
        ui::sans(format!("#{}", p.number), 13.0, INK_2),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let mut meta = row![
        ui::sans(format!("#{}", p.number), 13.0, INK_2),
        state_badge(p),
        review_badge(&p.review),
        ui::faint(p.author.clone()),
        ui::sans(format!("{} → {}", p.head, p.base), 12.0, INK_2),
        ui::faint(format!("updated {}", ago(&p.updated_at))),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    if let (true, Some(at)) = (merged, &p.merged_at) {
        meta = meta.push(ui::faint(format!("merged {}", ago(at))));
    }
    let review_in_ide = ide::target(&format!("repo:{}@{}", state.repo, p.head), true, None);
    let mut buttons = row![
        ui::btn(
            "Approve",
            None,
            Kind::Primary,
            Some(Msg::Modal(Modal::Review(ReviewEvent::Approve)))
        ),
        ui::btn(
            "Request changes",
            None,
            Kind::Danger,
            Some(Msg::Modal(Modal::Review(ReviewEvent::RequestChanges)))
        ),
        ui::btn(
            "Comment",
            None,
            Kind::Ghost,
            Some(Msg::Modal(Modal::Review(ReviewEvent::Comment)))
        ),
        ui::btn(
            "Open in IDE",
            Some("square-code"),
            Kind::Ghost,
            Some(Msg::OpenIde(review_in_ide))
        ),
    ]
    .spacing(8);
    // A merged pull request is closed in Forgejo: it has no Merge, Close or Reopen.
    if !merged {
        let toggle = if open {
            ui::btn("Close", None, Kind::Ghost, Some(Msg::SetState("closed")))
        } else {
            ui::btn("Reopen", None, Kind::Ghost, Some(Msg::SetState("open")))
        };
        buttons = buttons
            .push(ui::btn(
                "Merge",
                None,
                Kind::Primary,
                mergeable.then_some(Msg::Modal(Modal::Merge)),
            ))
            .push(toggle);
    }
    // The web explains a disabled Merge in its tooltip; the desktop says it under the buttons.
    let actions: Element<'a, Msg> = if mergeable || merged {
        buttons.into()
    } else {
        column![
            buttons,
            ui::faint("Needs an approving review and no conflicts")
        ]
        .spacing(6)
        .into()
    };
    let description: Element<'a, Msg> = if d.body.is_empty() {
        ui::faint("No description provided.").into()
    } else {
        container(rich::view(&d.body, &rctx, &state.cards, &on_rich))
            .max_width(620)
            .into()
    };
    let files: Element<'a, Msg> = if d.files.is_empty() {
        ui::faint("No files.").into()
    } else {
        list(d.files.iter().map(file_row).collect())
    };
    let reviews: Element<'a, Msg> = if d.reviews.is_empty() {
        ui::faint("No reviews yet.").into()
    } else {
        column(
            d.reviews
                .iter()
                .map(review_row)
                .collect::<Vec<Element<'a, Msg>>>(),
        )
        .spacing(12)
        .into()
    };
    let body = column![
        meta,
        actions,
        ui::card("Description", description),
        ui::card(&format!("Files changed ({})", d.files.len()), files),
        ui::card("Diff", diff_view(&d.diff)),
        ui::card(&format!("Reviews ({})", d.reviews.len()), reviews),
        ui::card(
            &format!("Discussion ({})", d.discussion.len()),
            discussion_view(state, d, &rctx)
        ),
    ]
    .spacing(16);
    let head = column![
        container(trail).padding(Padding {
            top: 24.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0
        }),
        ui::view_head(&p.title, vec![])
    ];
    sheet(state, head.into(), body.into())
}

/// The discussion: its comments in order, then the comment box. Ctrl/Cmd+Enter posts the comment.
fn discussion_view<'a>(
    state: &'a State,
    d: &'a PullDetail,
    rctx: &rich::RichCtx,
) -> Element<'a, Msg> {
    let comments: Element<'a, Msg> = if d.discussion.is_empty() {
        ui::faint("No comments yet.").into()
    } else {
        column(
            d.discussion
                .iter()
                .map(|c| comment_row(c, rctx, &state.cards))
                .collect::<Vec<Element<'a, Msg>>>(),
        )
        .spacing(12)
        .into()
    };
    let editor = text_editor(&state.comment)
        .placeholder("Write a comment (Ctrl+Enter posts)")
        .height(Length::Fixed(96.0))
        .on_action(Msg::Comment)
        .key_binding(|kp| {
            if kp.key == keyboard::Key::Named(keyboard::key::Named::Enter) && kp.modifiers.command()
            {
                Some(text_editor::Binding::Custom(Msg::SendComment))
            } else {
                text_editor::Binding::from_key_press(kp)
            }
        });
    let can_send = state.pr.is_some() && !state.comment.text().trim().is_empty();
    let send = ui::btn(
        "Post comment",
        None,
        Kind::Primary,
        can_send.then_some(Msg::SendComment),
    );
    column![
        comments,
        editor,
        row![Space::new().width(Length::Fill), send]
    ]
    .spacing(12)
    .into()
}

/// One comment: its author and age, then its text.
fn comment_row<'a>(c: &Comment, rctx: &rich::RichCtx, cards: &rich::Cards) -> Element<'a, Msg> {
    let head = row![
        text(c.author.clone()).font(SANS_BOLD).size(14.0).color(INK),
        ui::faint(ago(&c.at))
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    column![head, rich::view(&c.body, rctx, cards, &on_rich)]
        .spacing(6)
        .into()
}

fn file_row<'a>(f: &PullFile) -> Element<'a, Msg> {
    row![
        cell(Length::Fill, ui::mono(f.path.clone(), 13.0, INK)),
        cell(
            Length::Fixed(60.0),
            ui::mono(
                format!("+{}", f.additions),
                13.0,
                crate::theme::palette().ok
            )
        ),
        cell(
            Length::Fixed(60.0),
            ui::sans(
                format!("−{}", f.deletions),
                13.0,
                crate::theme::palette().crit
            )
        ),
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
        text(rv.author.clone())
            .font(SANS_BOLD)
            .size(14.0)
            .color(INK),
        ui::badge(&s.replace('_', " "), tone),
        ui::faint(ago(&rv.at)),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let mut col = column![head].spacing(6);
    if !rv.body.is_empty() {
        col = col.push(ui::prose(rv.body.clone()).size(14.0));
    }
    col.into()
}

/// The diff as mono lines: additions and deletions on a faint tint, hunk headers on a grey tint,
/// file headers semibold (the web's `.git-diff`).
fn diff_view<'a>(diff: &str) -> Element<'a, Msg> {
    let lines: Vec<Element<'a, Msg>> = diff
        .split('\n')
        .map(|line| {
            let (color, tint, font) = diff_style(line);
            container(text(line.to_string()).font(font).size(12.5).color(color))
                .padding([0, 14])
                .width(Length::Fill)
                .style(move |_: &Theme| container::Style {
                    background: tint.map(Into::into),
                    ..Default::default()
                })
                .into()
        })
        .collect();
    ui::scroll(column(lines).padding([8, 0]))
        .width(Length::Fill)
        .into()
}

/// A diff line's text colour, tint and font, by its marker.
fn diff_style(line: &str) -> (Color, Option<Color>, Font) {
    if line.starts_with("@@") {
        (
            INK_3,
            Some(Color {
                a: 0.06,
                ..crate::theme::palette().ink
            }),
            theme::SANS,
        )
    } else if line.starts_with("+++") || line.starts_with("---") {
        (INK_2, None, ui::SANS_SEMIBOLD)
    } else if line.starts_with('+') {
        (
            INK,
            Some(Color {
                a: 0.12,
                ..crate::theme::palette().ok
            }),
            theme::SANS,
        )
    } else if line.starts_with('-') {
        (
            INK,
            Some(Color {
                a: 0.12,
                ..crate::theme::palette().crit
            }),
            theme::SANS,
        )
    } else {
        (INK, None, theme::SANS)
    }
}

/// A pull request's state: merged or open in the ok tone, closed muted.
fn state_badge<'a>(p: &Pull) -> Element<'a, Msg> {
    if p.merged {
        return ui::badge("merged", Tone::Ok);
    }
    let tone = if p.state == "open" {
        Tone::Ok
    } else {
        Tone::Muted
    };
    ui::badge(&p.state, tone)
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
        Modal::NewPull => ("New pull request".to_string(), new_pull_form(state)),
        Modal::Review(event) => (
            format!("{} on #{n}", event.label()),
            review_form(state, event),
        ),
        Modal::Merge => (format!("Merge #{n}"), merge_form(state)),
    };
    let boxed = container(ui::card(&title, body))
        .width(Length::Fixed(520.0))
        .style(theme::boxed);
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
            text_input("repo-name", &state.new_name)
                .on_input(Msg::NewName)
                .style(theme::input)
                .into(),
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

/// The new pull request's form: title, the head and base branches (base starts on the repository's
/// default), and an optional description.
fn new_pull_form<'a>(state: &'a State) -> Element<'a, Msg> {
    let names: Vec<String> = state.data.branches.iter().map(|b| b.name.clone()).collect();
    let head = pick_list(names.clone(), state.pr_head.clone(), Msg::PrHead);
    let base = pick_list(names, state.pr_base.clone(), Msg::PrBase);
    column![
        field(
            "Title",
            text_input("What this change does", &state.pr_title)
                .on_input(Msg::PrTitle)
                .on_submit(Msg::CreatePull)
                .style(theme::input)
                .into(),
        ),
        field("From (head branch)", head.into()),
        field("Into (base branch)", base.into()),
        field(
            "Description",
            text_editor(&state.body)
                .placeholder("Optional")
                .on_action(Msg::Body)
                .height(Length::Fixed(120.0))
                .into(),
        ),
        form_actions(Msg::CreatePull, "Open pull request", Kind::Primary),
    ]
    .spacing(14)
    .into()
}

fn review_form<'a>(state: &'a State, event: ReviewEvent) -> Element<'a, Msg> {
    let placeholder = if event == ReviewEvent::Approve {
        "Optional note"
    } else {
        "Write the review"
    };
    let kind = if event == ReviewEvent::RequestChanges {
        Kind::Danger
    } else {
        Kind::Primary
    };
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
    column![
        ui::prose(text),
        field("Style", style.into()),
        form_actions(Msg::Merge, "Merge", Kind::Primary)
    ]
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
        items.push(rule_line());
    }
    column(items).into()
}

fn clone_url(state: &State) -> String {
    format!(
        "http://plaza.gateway/git/{}/{}.git",
        state.data.org, state.repo
    )
}

/// Percent-encodes a query value.
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The crumbs of a folder path: the repository at the root (to ""), then each folder with its own path.
fn crumb_trail(repo: &str, path: &str) -> Vec<(String, String)> {
    let mut out = vec![(repo.to_string(), String::new())];
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    for (i, part) in parts.iter().enumerate() {
        out.push((part.to_string(), parts[..=i].join("/")));
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
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(t);
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
    let rest = s
        .get(19..)?
        .trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_tree_response_parses_entries() {
        let json = r#"[{"name":"src","path":"src","kind":"dir","size":0},{"name":"README.md","path":"README.md","kind":"file","size":12}]"#;
        let tree: Vec<Entry> = serde_json::from_str(json).expect("tree parses");
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].kind, "dir");
        assert_eq!(tree[1].path, "README.md");
        assert_eq!(tree[1].size, 12);
    }

    #[test]
    fn git_file_response_parses_text_and_binary() {
        let text: FileView = serde_json::from_str(
            r#"{"path":"a.rs","ref":"main","size":5,"text":"fn a","binary":false}"#,
        )
        .expect("text parses");
        assert_eq!(text.text.as_deref(), Some("fn a"));
        assert!(!text.binary);
        let bin: FileView = serde_json::from_str(
            r#"{"path":"a.png","ref":"main","size":2048,"text":null,"binary":true}"#,
        )
        .expect("binary parses");
        assert!(bin.binary);
        assert!(bin.text.is_none());
        assert_eq!(fmt_size(bin.size), "2.0 KB");
    }

    #[test]
    fn git_commits_response_parses_log() {
        let json = r#"[{"id":"9f2c1ab4e0","short":"9f2c1ab","message":"hub: fix\n\nbody","author":"Zarathos","at":"2026-10-12T10:00:00Z","additions":12,"deletions":3}]"#;
        let log: Vec<Commit> = serde_json::from_str(json).expect("commits parse");
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].short, "9f2c1ab");
        assert_eq!(first_line(&log[0].message), "hub: fix");
        assert_eq!((log[0].additions, log[0].deletions), (12, 3));
    }

    #[test]
    fn git_pull_detail_keeps_the_comment_count_and_parses_the_discussion() {
        // The hub's detail: the row's `comments` is a count; the comments are under `discussion`.
        let json = r#"{
            "number": 7, "title": "Add the tab", "state": "open", "author": "agent-5",
            "head": "feature", "base": "main", "updated_at": "2026-10-02T09:30:00Z",
            "comments": 2, "review": "pending", "mergeable": true,
            "body": "Adds it.", "diff": "+x\n",
            "reviews": [{"author": "agent-3", "state": "APPROVED", "body": "", "at": "2026-10-02T09:10:00Z"}],
            "discussion": [
                {"author": "agent-5", "body": "first", "at": "2026-10-02T09:00:00Z"},
                {"author": "agent-3", "body": "second", "at": "2026-10-02T09:30:00Z"}
            ],
            "files": [{"path": "a.rs", "additions": 1, "deletions": 0}]
        }"#;
        let detail: PullDetail = serde_json::from_str(json).expect("detail parses");
        assert_eq!(detail.pull.number, 7);
        assert_eq!(detail.pull.comments, 2, "comments is the count");
        assert_eq!(detail.discussion.len(), 2);
        assert_eq!(detail.discussion[0].author, "agent-5");
        assert_eq!(detail.discussion[1].body, "second");
        assert_eq!(detail.reviews.len(), 1);
        assert_eq!(detail.files[0].path, "a.rs");
    }

    #[test]
    fn git_pull_row_says_whether_it_merged_and_defaults_to_not_merged() {
        // A merged pull request: closed, with merged true and the merge time.
        let merged: Pull = serde_json::from_str(
            r#"{"number": 9, "title": "t", "state": "closed", "author": "a", "head": "h", "base": "main",
                "updated_at": "2026-10-03T09:30:00Z", "comments": 0, "review": "approved", "mergeable": false,
                "merged": true, "merged_at": "2026-10-03T09:30:00Z"}"#,
        )
        .expect("merged row parses");
        assert!(merged.merged);
        assert_eq!(merged.merged_at.as_deref(), Some("2026-10-03T09:30:00Z"));
        assert_eq!(merged.state, "closed");

        // An older hub's row has neither field: it reads as not merged.
        let older: Pull = serde_json::from_str(
            r#"{"number": 10, "title": "t", "state": "closed", "author": "a", "head": "h", "base": "main",
                "updated_at": "", "comments": 0, "review": "pending", "mergeable": false}"#,
        )
        .expect("older row parses");
        assert!(!older.merged);
        assert!(older.merged_at.is_none());
    }

    #[test]
    fn git_pull_detail_without_a_discussion_is_empty() {
        let json = r#"{
            "number": 8, "title": "t", "state": "closed", "author": "a", "head": "h", "base": "main",
            "updated_at": "", "comments": 0, "review": "pending", "mergeable": false,
            "body": "", "diff": "", "reviews": [], "files": []
        }"#;
        let detail: PullDetail = serde_json::from_str(json).expect("detail parses");
        assert_eq!(detail.pull.comments, 0);
        assert!(detail.discussion.is_empty());
    }

    #[test]
    fn git_crumbs_walk_the_folder_path() {
        assert_eq!(
            crumb_trail("plaza", ""),
            vec![("plaza".to_string(), String::new())]
        );
        let trail = crumb_trail("plaza", "src/console/screens");
        let expect: Vec<(String, String)> = vec![
            ("plaza".to_string(), String::new()),
            ("src".to_string(), "src".to_string()),
            ("console".to_string(), "src/console".to_string()),
            ("screens".to_string(), "src/console/screens".to_string()),
        ];
        assert_eq!(trail, expect);
    }
}
