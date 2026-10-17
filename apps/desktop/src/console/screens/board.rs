//! The Board screen (parity with the web console's `board` view): the agents' shared feed of
//! posts, newest first, with an author filter and a search. Each post shows its role, time,
//! subject and body (clamped behind "Show more"), its files, and its thread of comments with
//! the owner's reply composer under it. The owner also posts to the board from the foot of the
//! screen (with `@name` mentions and `@all`), pauses and resumes its deliveries, and exports the
//! thread as Markdown.
//!
//! The owner's pieces are public so the IDE's board panel can reuse them: `Composer` (the post
//! box's text and posting state), `composer_view`, `pause_bar` and `paused_banner`. They emit this
//! screen's `Msg`, which the embedding screen forwards to `update`.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use iced::font::{self, Font};
use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, pick_list, row, text, text_editor, text_input};
use iced::{Alignment, Border, Element, Length, Padding, Task, Theme};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Id;
use crate::console::{Ctx, client, rich, ui};
use crate::theme::SANS;

const SANS_SEMIBOLD: Font = Font {
    weight: font::Weight::Semibold,
    ..SANS
};

/// A thread shows its last comments; the rest on request.
const THREAD_PREVIEW: usize = 3;
const REPLY_MAX: usize = 1000;
/// The longest post the board takes (the hub's body cap is 4 KiB; a post is shown in characters here).
const POST_MAX: usize = 4000;
/// The most `@name` suggestions a composer offers at once.
const SUGGESTIONS: usize = 6;
/// A body longer than this many lines or characters is clamped behind "Show more".
const CLAMP_LINES: usize = 3;
const CLAMP_CHARS: usize = 280;

#[derive(Debug, Clone, Deserialize)]
struct Comment {
    agent_cid: Option<u64>,
    #[serde(default)]
    author: String,
    body: String,
    created_at: f64,
    #[serde(default)]
    attachments: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Post {
    id: String,
    agent_cid: u64,
    role: String,
    subject: String,
    #[serde(default)]
    body: String,
    created_at: f64,
    #[serde(default)]
    comments: Vec<Comment>,
    #[serde(default)]
    attachments: Vec<String>,
    /// The task the post is about, or null.
    #[serde(default)]
    task: Option<String>,
    /// Whether the owner wrote the post (its `agent_cid` is then 0).
    #[serde(default)]
    owner: bool,
    /// The names the owner mentioned (`all` for everyone).
    #[serde(default)]
    mentions: Vec<String>,
}

/// The owner's post box: its text and whether a post is on its way. It owns its state, so the IDE's
/// board panel can embed it with its own copy.
#[derive(Debug, Default)]
pub struct Composer {
    content: text_editor::Content,
    posting: bool,
}

#[derive(Debug, Deserialize)]
struct Board {
    posts: Vec<Post>,
    /// Whether the whole board's deliveries are paused.
    #[serde(default)]
    paused: bool,
}

/// A file's metadata (`GET /v1/files/{id}`).
#[derive(Debug, Clone, Deserialize)]
struct FileMeta {
    #[serde(default)]
    title: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    content_type: String,
    #[serde(default)]
    size: u64,
}

/// One entry of the author filter; `cid` None is "All authors".
#[derive(Debug, Clone, PartialEq, Eq)]
struct Author {
    cid: Option<u64>,
    name: String,
}

impl fmt::Display for Author {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

#[derive(Debug, Default)]
pub struct State {
    pub error: Option<String>,
    posts: Vec<Post>,
    loaded: bool,
    failed: bool,
    /// Agent names by cid (from the agent list); an unknown agent shows as "agent N".
    names: HashMap<u64, String>,
    /// File metadata by id. `pending` ones are on their way; `missing` ones failed this round.
    files: HashMap<String, FileMeta>,
    pending: HashSet<String>,
    missing: HashSet<String>,
    /// The author filter (an agent cid), and the search text.
    author: Option<u64>,
    query: String,
    /// Whether the whole board's deliveries are paused (the hub's answer).
    paused: bool,
    /// The owner's post box, the names an `@` can complete to (`all` first), and a line of confirmation.
    compose: Composer,
    mentionable: Vec<String>,
    notice: Option<String>,
    /// Posts whose whole body is open, and whose whole thread is showing.
    body_open: HashSet<String>,
    thread_open: HashSet<String>,
    /// Posts with a reply composer open, their unsent text, and the replies on their way.
    composing: HashSet<String>,
    drafts: HashMap<String, String>,
    /// The editors the open composers type in, kept beside their drafts (each draft mirrors its editor).
    editors: HashMap<String, text_editor::Content>,
    sending: HashSet<String>,
    /// The rich content of the posts: the hub's cards and images for their references (`console::rich`).
    cards: rich::Cards,
    /// A live refresh is in flight; an event that came meanwhile runs once more when it lands.
    refreshing: bool,
    again: bool,
}

impl State {
    fn name_of(&self, cid: u64) -> String {
        self.names
            .get(&cid)
            .cloned()
            .unwrap_or_else(|| format!("agent {cid}"))
    }

    /// Takes the agents' names from a roster the caller already holds (the IDE's sidebar), so a post's author
    /// reads by name at once rather than when the board's own agent list lands.
    pub fn adopt_names(&mut self, names: impl IntoIterator<Item = (u64, String)>) {
        self.names = names.into_iter().collect();
    }

    /// Whether the whole board's deliveries are paused.
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// The owner's post box.
    pub fn composer(&self) -> &Composer {
        &self.compose
    }

    /// The names an `@` can complete to (`all` first).
    pub fn mentionable(&self) -> &[String] {
        &self.mentionable
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Loaded(Result<Value, String>),
    Agents(Result<Value, String>),
    File(String, Result<Value, String>),
    /// The author filter changed (None is "All authors").
    Author(Option<u64>),
    Query(String),
    ToggleBody(String),
    ToggleThread(String),
    ToggleReply(String),
    /// An edit in a reply composer (the editor applies it; its text becomes the draft).
    Edit(String, text_editor::Action),
    Send(String),
    /// A reply went out: the post, the text sent, and the result.
    Sent(String, String, Result<Value, String>),
    Cancel(String),
    /// A hub event: a board change reloads the screen.
    Event(crate::console::events::Event),
    /// An edit in the post box.
    Compose(text_editor::Action),
    /// The `@all` chip: adds `@all` to the post.
    ComposeAll,
    /// A suggestion picked: it completes the `@name` being typed.
    ComposePick(String),
    /// Posts the box's text to the board.
    ComposeSend,
    /// A post went out (or did not): the text sent, and the result.
    Posted(String, Result<Value, String>),
    /// Pauses the board's deliveries, or resumes them.
    Pause,
    Paused(Result<Value, String>),
    /// Saves the board's thread as Markdown: the save dialog, then the file.
    Export,
    ExportPicked(Option<PathBuf>),
    Exported(Result<String, String>),
    /// A rich-content click (a chip, a link, an Open button) or a load of its cards and images.
    Rich(rich::Ev),
}

/// A rich-content click or load, as this screen's message.
fn on_rich(action: rich::RichAction) -> Msg {
    Msg::Rich(rich::Ev::Action(action))
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        Msg::Rich(rich::Ev::Action(action)) => action.route(),
        _ => None,
    }
}

/// Asks the hub for the cards and images that the posts' and comments' references need and the cards do not hold.
fn prefetch_posts(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let texts: Vec<&str> = state
        .posts
        .iter()
        .flat_map(|p| {
            std::iter::once(p.body.as_str()).chain(p.comments.iter().map(|c| c.body.as_str()))
        })
        .collect();
    let rctx = rich::context(&state.cards, None, None, None);
    rich::prefetch(&mut state.cards, &ctx.client, &rctx, &texts).map(Msg::Rich)
}

/// Loads the board and the agent names (called when it opens and on every poll tick).
pub fn load(_state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let board = ctx.client.clone();
    let agents = ctx.client.clone();
    Task::batch([
        client::task(move || board.get::<Value>("/v1/board"), Msg::Loaded),
        client::task(move || agents.call("GET", "/v1/agents", None), Msg::Agents),
    ])
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    let landed = matches!(msg, Msg::Loaded(_));
    let task = apply(state, msg, ctx);
    if !landed {
        return task;
    }
    state.refreshing = false;
    if std::mem::take(&mut state.again) {
        state.refreshing = true;
        return Task::batch([task, load(state, ctx)]);
    }
    task
}

/// The screen's messages. `update` wraps it to keep the live refresh's in-flight flag.
fn apply(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Event(event) => {
            if !matches!(event.kind.as_str(), "board" | "board_paused") {
                return Task::none();
            }
            if state.refreshing {
                state.again = true;
                Task::none()
            } else {
                state.refreshing = true;
                load(state, ctx)
            }
        }
        Msg::Go(_) => Task::none(),
        // The cards are read where they are drawn (`view`); a click is the screen's (`go`).
        Msg::Rich(ev) => {
            rich::apply(&mut state.cards, ev);
            Task::none()
        }
        Msg::Loaded(res) => {
            match res.and_then(|v| serde_json::from_value::<Board>(v).map_err(|e| e.to_string())) {
                Ok(board) => {
                    state.posts = board.posts;
                    state.paused = board.paused;
                    state.failed = false;
                }
                Err(_) => state.failed = true,
            }
            state.loaded = true;
            state.missing.clear();
            if let Some(cid) = state.author
                && !state.posts.iter().any(|p| p.agent_cid == cid)
            {
                state.author = None;
            }
            Task::batch([fetch_files(state, ctx), prefetch_posts(state, ctx)])
        }
        Msg::Agents(Ok(list)) => {
            state.names = agent_names(&list);
            state.mentionable = mention_names(&list);
            Task::none()
        }
        Msg::Agents(Err(_)) => Task::none(),
        Msg::File(id, res) => {
            state.pending.remove(&id);
            match res.and_then(|v| serde_json::from_value::<FileMeta>(v).map_err(|e| e.to_string()))
            {
                Ok(meta) => {
                    state.files.insert(id, meta);
                }
                Err(_) => {
                    state.missing.insert(id);
                }
            }
            Task::none()
        }
        Msg::Author(cid) => {
            state.author = cid;
            Task::none()
        }
        Msg::Query(q) => {
            state.query = q;
            Task::none()
        }
        Msg::ToggleBody(id) => {
            if !state.body_open.remove(&id) {
                state.body_open.insert(id);
            }
            Task::none()
        }
        Msg::ToggleThread(id) => {
            if !state.thread_open.remove(&id) {
                state.thread_open.insert(id);
            }
            Task::none()
        }
        Msg::ToggleReply(id) => {
            if !state.composing.remove(&id) {
                let seed = state.drafts.get(&id).cloned().unwrap_or_default();
                state
                    .editors
                    .entry(id.clone())
                    .or_insert_with(|| text_editor::Content::with_text(&seed));
                state.composing.insert(id);
            }
            Task::none()
        }
        Msg::Edit(id, action) => {
            let seed = state.drafts.get(&id).cloned().unwrap_or_default();
            let editor = state
                .editors
                .entry(id.clone())
                .or_insert_with(|| text_editor::Content::with_text(&seed));
            editor.perform(action);
            let text = editor.text();
            state.drafts.insert(id, text);
            Task::none()
        }
        Msg::Cancel(id) => {
            state.drafts.remove(&id);
            state.editors.remove(&id);
            state.composing.remove(&id);
            Task::none()
        }
        Msg::Send(id) => {
            let text = state
                .drafts
                .get(&id)
                .map(|t| t.trim().to_string())
                .unwrap_or_default();
            if text.is_empty() || text.chars().count() > REPLY_MAX || state.sending.contains(&id) {
                return Task::none();
            }
            state.sending.insert(id.clone());
            let client = ctx.client.clone();
            let path = format!("/v1/board/{id}/comments");
            let body = json!({ "text": &text });
            let key = id.clone();
            client::task(
                move || client.post::<Value>(&path, &body),
                move |r| Msg::Sent(key.clone(), text.clone(), r),
            )
        }
        Msg::Compose(action) => {
            state.compose.content.perform(action);
            Task::none()
        }
        Msg::ComposeAll => {
            let text = state.compose.content.text();
            let mut text = text.trim_end_matches('\n').to_string();
            if !text.is_empty() && !text.ends_with(char::is_whitespace) {
                text.push(' ');
            }
            text.push_str("@all ");
            state.compose.content = text_editor::Content::with_text(&text);
            Task::none()
        }
        Msg::ComposePick(name) => {
            let text = state.compose.content.text();
            let text = text.trim_end_matches('\n');
            // The word being typed starts after the last whitespace; it is replaced by the name.
            let start = text
                .char_indices()
                .rev()
                .find(|(_, c)| c.is_whitespace())
                .map_or(0, |(i, c)| i + c.len_utf8());
            let next = format!("{}@{name} ", &text[..start]);
            state.compose.content = text_editor::Content::with_text(&next);
            Task::none()
        }
        Msg::ComposeSend => {
            let text = state.compose.content.text().trim().to_string();
            if text.is_empty() || text.chars().count() > POST_MAX || state.compose.posting {
                return Task::none();
            }
            state.compose.posting = true;
            let client = ctx.client.clone();
            let body = json!({ "body": &text, "mentions": mentions_in(&text) });
            let sent = text.clone();
            client::task(
                move || client.post::<Value>("/v1/board/posts", &body),
                move |r| Msg::Posted(sent.clone(), r),
            )
        }
        Msg::Posted(sent, res) => {
            state.compose.posting = false;
            match res {
                Ok(_) => {
                    // The box clears only if nothing was typed while the post went out.
                    if state.compose.content.text().trim() == sent {
                        state.compose.content = text_editor::Content::new();
                    }
                    load(state, ctx)
                }
                Err(e) => {
                    state.error = Some(e);
                    Task::none()
                }
            }
        }
        Msg::Pause => {
            let client = ctx.client.clone();
            let body = json!({ "on": !state.paused });
            client::task(
                move || client.post::<Value>("/v1/board/pause", &body),
                Msg::Paused,
            )
        }
        Msg::Paused(res) => match res {
            Ok(v) => {
                state.paused = v
                    .get("paused")
                    .and_then(Value::as_bool)
                    .unwrap_or(state.paused);
                load(state, ctx)
            }
            Err(e) => {
                state.error = Some(e);
                Task::none()
            }
        },
        Msg::Export => Task::perform(
            async {
                rfd::AsyncFileDialog::new()
                    .set_title("Save the board as Markdown")
                    .set_file_name("agentplaza-board-all.md")
                    .add_filter("Markdown", &["md"])
                    .save_file()
                    .await
                    .map(|file| file.path().to_path_buf())
            },
            Msg::ExportPicked,
        ),
        Msg::ExportPicked(None) => Task::none(),
        Msg::ExportPicked(Some(path)) => {
            let c = ctx.client.clone();
            client::task(
                move || {
                    let bytes = raw_get(&c, "/v1/board/export?format=md")?;
                    std::fs::write(&path, &bytes)
                        .map_err(|e| format!("The file could not be written: {e}"))?;
                    Ok(path.display().to_string())
                },
                Msg::Exported,
            )
        }
        Msg::Exported(Ok(path)) => {
            state.notice = Some(format!("Saved the board to {path}"));
            Task::none()
        }
        Msg::Exported(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::Sent(id, sent, res) => {
            state.sending.remove(&id);
            match res {
                Ok(_) => {
                    // The composer closes only if nothing was typed while the reply went out.
                    if state.drafts.get(&id).map(|d| d.trim()) == Some(sent.as_str()) {
                        state.drafts.remove(&id);
                        state.editors.remove(&id);
                        state.composing.remove(&id);
                    }
                    state.thread_open.insert(id);
                    load(state, ctx)
                }
                Err(e) => {
                    state.error = Some(e);
                    Task::none()
                }
            }
        }
    }
}

/// Fetches the metadata of the files the board carries that are not known yet.
fn fetch_files(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let mut wanted: Vec<String> = Vec::new();
    for p in &state.posts {
        let comment_files = p.comments.iter().flat_map(|c| c.attachments.iter());
        for id in p.attachments.iter().chain(comment_files) {
            let known = state.files.contains_key(id)
                || state.pending.contains(id)
                || state.missing.contains(id);
            if !known && !wanted.contains(id) {
                wanted.push(id.clone());
            }
        }
    }
    let mut tasks = Vec::new();
    for id in wanted {
        state.pending.insert(id.clone());
        let client = ctx.client.clone();
        let path = format!("/v1/files/{id}");
        let key = id;
        tasks.push(client::task(
            move || client.get::<Value>(&path),
            move |r| Msg::File(key.clone(), r),
        ));
    }
    Task::batch(tasks)
}

/// The names an `@` can complete to, from the agent list: `all`, then each agent's name and role, once each.
fn mention_names(list: &Value) -> Vec<String> {
    let agents = list
        .as_array()
        .or_else(|| list.get("agents").and_then(Value::as_array));
    let mut out: Vec<String> = vec!["all".to_string()];
    for a in agents.into_iter().flatten() {
        for key in ["name", "role"] {
            let name = a.get(key).and_then(Value::as_str).unwrap_or("").trim();
            if !name.is_empty() && !out.iter().any(|seen| seen.eq_ignore_ascii_case(name)) {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// The bytes of a GET (the export's Markdown); a non-2xx answer becomes `Err` with the hub's `error` text.
fn raw_get(c: &client::Client, path: &str) -> Result<Vec<u8>, String> {
    let reply = apz_http::client::request(&c.target, "GET", path, None, c.timeout)
        .map_err(|e| e.to_string())?;
    if (200..300).contains(&reply.status) {
        return Ok(reply.body);
    }
    let error = serde_json::from_slice::<Value>(&reply.body)
        .ok()
        .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string));
    Err(error.unwrap_or_else(|| format!("HTTP {}", reply.status)))
}

/// The agent names from the agent list, keyed by cid (an empty name shows as "Unnamed").
fn agent_names(list: &Value) -> HashMap<u64, String> {
    let agents = list
        .as_array()
        .or_else(|| list.get("agents").and_then(Value::as_array));
    agents
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let cid = a.get("cid")?.as_u64()?;
            let name = a.get("name").and_then(Value::as_str).unwrap_or("").trim();
            Some((
                cid,
                if name.is_empty() {
                    "Unnamed".to_string()
                } else {
                    name.to_string()
                },
            ))
        })
        .collect()
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let options = author_options(state);
    let selected = options.iter().find(|a| a.cid == state.author).cloned();
    let picker =
        pick_list(options, selected, |a: Author| Msg::Author(a.cid)).width(Length::Fixed(200.0));
    let search = text_input("Search subject or body", &state.query)
        .on_input(Msg::Query)
        .padding(6.0)
        .width(Length::Fixed(260.0));
    let head = ui::view_head(
        "Board",
        vec![picker.into(), search.into(), pause_bar(state.paused)],
    );

    let mut feed: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(e) = &state.error {
        feed.push(ui::error_line(e));
    }
    if state.failed {
        feed.push(ui::error_line("Could not load the board."));
    } else if !state.loaded {
        feed.push(ui::loading());
    } else if state.posts.is_empty() {
        feed.push(ui::empty("board", "The board is empty.", ""));
    } else {
        // Numbered newest first (p{n}), before the filters, as the web numbers them.
        let needle = state.query.trim().to_lowercase();
        let mut ordered: Vec<&'a Post> = state.posts.iter().collect();
        ordered.sort_by(|x, y| y.created_at.total_cmp(&x.created_at));
        let total = ordered.len();
        let shown: Vec<(usize, &'a Post)> = ordered
            .into_iter()
            .enumerate()
            .map(|(i, p)| (total - i, p))
            .filter(|(_, p)| state.author.is_none_or(|cid| p.agent_cid == cid))
            .filter(|(_, p)| {
                needle.is_empty()
                    || format!("{}\n{}", p.subject, p.body)
                        .to_lowercase()
                        .contains(&needle)
            })
            .collect();
        if shown.is_empty() {
            feed.push(ui::empty("board", "Nothing on the board matches.", ""));
        }
        for (no, p) in shown {
            feed.push(entry(state, no, p, false));
            feed.push(ui::hairline());
        }
    }
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if state.paused {
        parts.push(paused_banner());
    }
    if let Some(notice) = &state.notice {
        parts.push(ui::faint(notice.clone()).into());
    }
    parts.push(
        ui::scroll(column(feed).width(Length::Fill).max_width(760.0))
            .height(Length::Fill)
            .into(),
    );
    parts.push(
        container(composer_view(&state.compose, &state.mentionable))
            .width(Length::Fill)
            .padding(Padding {
                top: 8.0,
                right: 10.0,
                bottom: 10.0,
                left: 10.0,
            })
            .into(),
    );
    ui::sheet(head, column(parts).height(Length::Fill).into())
}

/// Pause/Resume and Export for the board. Reusable: the IDE's board panel shows the same bar.
pub fn pause_bar<'a>(paused: bool) -> Element<'a, Msg> {
    let pause = ui::btn(
        if paused {
            "Resume board"
        } else {
            "Pause board"
        },
        None,
        ui::Kind::Secondary,
        Some(Msg::Pause),
    );
    let export = ui::btn("Export", None, ui::Kind::Secondary, Some(Msg::Export));
    row![pause, export]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
}

/// The banner a paused board shows: posts are kept, their mentions wait for the resume.
pub fn paused_banner<'a>() -> Element<'a, Msg> {
    let warn = crate::theme::palette().warn;
    container(text("Paused. Posts are saved, but their mentions are not delivered to agents until the board resumes.").size(13.0).color(warn))
        .width(Length::Fill)
        .padding(Padding { top: 6.0, right: 10.0, bottom: 6.0, left: 10.0 })
        .style(move |_: &Theme| container::Style { border: Border { color: warn, width: 1.0, radius: 0.0.into() }, ..Default::default() })
        .into()
}

/// The owner's post box: the `@name` suggestions for the word being typed, an `@all` chip, the text
/// (Ctrl/Cmd+Enter sends), and Send. `mentionable` is the names an `@` can complete to.
pub fn composer_view<'a>(compose: &'a Composer, mentionable: &[String]) -> Element<'a, Msg> {
    let text = compose.content.text();
    let suggestions: Vec<Element<'a, Msg>> = match trailing_mention(&text) {
        Some(query) => suggestions_for(query, mentionable)
            .into_iter()
            .map(|name| {
                let label = format!("@{name}");
                ui::btn(&label, None, ui::Kind::Ghost, Some(Msg::ComposePick(name)))
            })
            .collect(),
        None => Vec::new(),
    };
    let input = text_editor(&compose.content)
        .placeholder("Message the team… (@ to mention)")
        .padding(6.0)
        .height(Length::Fixed(96.0))
        .key_binding(|kp| {
            let enter = kp.key == iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter);
            if enter && (kp.modifiers.control() || kp.modifiers.command()) {
                Some(text_editor::Binding::Custom(Msg::ComposeSend))
            } else {
                text_editor::Binding::from_key_press(kp)
            }
        })
        .on_action(Msg::Compose);
    let len = text.chars().count();
    let can_send = !text.trim().is_empty() && len <= POST_MAX && !compose.posting;
    let all = ui::btn("@all", None, ui::Kind::Secondary, Some(Msg::ComposeAll));
    let send = ui::btn(
        "Send",
        None,
        ui::Kind::Primary,
        can_send.then_some(Msg::ComposeSend),
    );
    column![
        row![all, ui::chip_row(suggestions)]
            .spacing(8)
            .align_y(Alignment::Center),
        input,
        row![
            container(ui::faint(format!("{len}/{POST_MAX}"))).width(Length::Fill),
            send
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ]
    .spacing(8)
    .into()
}

/// The `@partial` the text ends with, without its `@`, when the last word is one.
fn trailing_mention(text: &str) -> Option<&str> {
    let word = text.rsplit(char::is_whitespace).next()?;
    let query = word.strip_prefix('@')?;
    query
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .then_some(query)
}

/// The names an `@partial` can complete to: `all` first, then the names and roles that contain it.
fn suggestions_for(query: &str, mentionable: &[String]) -> Vec<String> {
    let needle = query.to_lowercase();
    std::iter::once("all")
        .chain(
            mentionable
                .iter()
                .map(String::as_str)
                .filter(|name| *name != "all"),
        )
        .filter(|name| name.to_lowercase().contains(&needle))
        .take(SUGGESTIONS)
        .map(str::to_string)
        .collect()
}

/// The mentions in a post's text: each `@name` (letters, digits, dots, dashes, underscores), in order, once.
fn mentions_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        let after_word = i > 0 && (chars[i - 1].is_ascii_alphanumeric() || chars[i - 1] == '_');
        if *c != '@' || after_word {
            continue;
        }
        let name: String = chars[i + 1..]
            .iter()
            .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
            .collect();
        let name = name.trim_end_matches('.').to_string();
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// The author filter's options: "All authors", then the agents that have posted, by name.
fn author_options(state: &State) -> Vec<Author> {
    let mut seen: Vec<(u64, String)> = Vec::new();
    for p in state.posts.iter().filter(|p| !p.owner) {
        if !seen.iter().any(|(cid, _)| *cid == p.agent_cid) {
            seen.push((p.agent_cid, state.name_of(p.agent_cid)));
        }
    }
    seen.sort_by_key(|x| x.1.to_lowercase());
    let mut out = vec![Author {
        cid: None,
        name: "All authors".to_string(),
    }];
    out.extend(seen.into_iter().map(|(cid, name)| Author {
        cid: Some(cid),
        name,
    }));
    out
}

/// One post: its number, the by-line (author link, role, count, Reply), subject, body, files, thread, time. In the
/// IDE's panel (`panel`) there is no number column, the by-line wraps, and the body takes the panel's whole width.
fn entry<'a>(state: &'a State, no: usize, p: &'a Post, panel: bool) -> Element<'a, Msg> {
    // The owner's own post names the owner (it has no agent to open); an agent's opens the agent.
    let name: Element<'a, Msg> = if p.owner {
        text("Owner")
            .font(SANS_SEMIBOLD)
            .size(14.5)
            .color(crate::theme::palette().ink)
            .into()
    } else {
        button(
            text(state.name_of(p.agent_cid))
                .font(SANS_SEMIBOLD)
                .size(14.5)
                .color(crate::theme::palette().ink),
        )
        .padding(0.0)
        .style(|_: &Theme, _| button::Style {
            text_color: crate::theme::palette().ink,
            ..Default::default()
        })
        .on_press(Msg::Go(Id::Agent(p.agent_cid)))
        .into()
    };
    let mut by_parts: Vec<Element<'a, Msg>> = vec![name];
    if !p.owner {
        by_parts.push(ui::label(&role_label(&p.role)).into());
    }
    by_parts.push(ui::label(&count_label(p.comments.len())).into());
    if !p.mentions.is_empty() {
        let names = p
            .mentions
            .iter()
            .map(|m| format!("@{m}"))
            .collect::<Vec<_>>()
            .join(" ");
        by_parts.push(ui::mono(names, 11.0, crate::theme::palette().signal).into());
    }
    if let Some(task) = p.task.as_deref().filter(|t| !t.is_empty()) {
        by_parts.push(task_link(task));
    }
    by_parts.push(ui::btn(
        "Reply",
        None,
        ui::Kind::Ghost,
        Some(Msg::ToggleReply(p.id.clone())),
    ));
    // The by-line wraps in the panel, where it is narrow; the screen's one line is the same row.
    let by: Element<'a, Msg> = if panel {
        row(by_parts)
            .spacing(10)
            .align_y(Alignment::Center)
            .wrap()
            .vertical_spacing(6)
            .into()
    } else {
        row(by_parts).spacing(10).align_y(Alignment::Center).into()
    };

    let mut matter: Vec<Element<'a, Msg>> = vec![
        by,
        text(p.subject.clone())
            .font(SANS_SEMIBOLD)
            .size(16.0)
            .color(crate::theme::palette().ink)
            .into(),
    ];
    if let Some(body) = body(state, p) {
        matter.push(body);
    }
    if let Some(files) = attachments(state, &p.attachments) {
        matter.push(files);
    }
    if let Some(thread) = thread(state, p) {
        matter.push(thread);
    }
    let when = ui::sans(ago(p.created_at), 11.0, crate::theme::palette().ink_3);
    if panel {
        return row![
            column(matter).spacing(6).width(Length::Fill),
            container(when).padding(Padding {
                top: 2.0,
                right: 0.0,
                bottom: 0.0,
                left: 0.0
            }),
        ]
        .spacing(12)
        .padding(Padding {
            top: 12.0,
            right: 4.0,
            bottom: 12.0,
            left: 4.0,
        })
        .align_y(Alignment::Start)
        .into();
    }
    // The web's grid: the post number (56px, right-aligned), the matter, then the time at the far end.
    let number = container(ui::sans(
        format!("p{no}"),
        11.0,
        crate::theme::palette().ink_3,
    ))
    .width(Length::Fixed(56.0))
    .align_x(Alignment::End);
    row![
        number,
        column(matter).spacing(6).width(Length::Fill),
        container(when).padding(Padding {
            top: 2.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0
        }),
    ]
    .spacing(16)
    .padding(Padding {
        top: 14.0,
        right: 10.0,
        bottom: 14.0,
        left: 10.0,
    })
    .align_y(Alignment::Start)
    .into()
}

/// The task a post is about, as a link to it on the Tasks screen.
fn task_link<'a>(task: &str) -> Element<'a, Msg> {
    // One line, always: a task's name is a chip, and a narrow panel wraps the by-line rather than the chip.
    button(
        ui::sans(format!("task {task}"), 11.0, crate::theme::palette().signal)
            .wrapping(Wrapping::None),
    )
    .padding(0.0)
    .style(|_: &Theme, _| button::Style {
        background: None,
        text_color: crate::theme::palette().signal,
        ..Default::default()
    })
    .on_press(Msg::Go(Id::Task(task.to_string())))
    .into()
}

/// A post's body, clamped to a few lines until "Show more". None when it only repeats the subject.
fn body<'a>(state: &'a State, p: &'a Post) -> Option<Element<'a, Msg>> {
    let src = p.body.trim();
    if src.is_empty() || src == p.subject.trim() {
        return None;
    }
    let open = state.body_open.contains(&p.id);
    let clamped = is_long(src);
    let shown = if open || !clamped {
        src.to_string()
    } else {
        clamp(src)
    };
    let rctx = rich::context(&state.cards, None, None, None);
    let mut col: Vec<Element<'a, Msg>> = vec![rich::view(&shown, &rctx, &state.cards, &on_rich)];
    if clamped {
        let label = if open { "Show less" } else { "Show more" };
        col.push(ui::btn(
            label,
            None,
            ui::Kind::Ghost,
            Some(Msg::ToggleBody(p.id.clone())),
        ));
    }
    Some(column(col).spacing(6).into())
}

fn is_long(s: &str) -> bool {
    s.lines().count() > CLAMP_LINES || s.chars().count() > CLAMP_CHARS
}

fn clamp(s: &str) -> String {
    let head: String = s.lines().take(CLAMP_LINES).collect::<Vec<_>>().join("\n");
    let head: String = head.chars().take(CLAMP_CHARS).collect();
    format!("{head}…")
}

/// The files under a post or a comment, one line each; None when it carries none.
fn attachments<'a>(state: &State, ids: &[String]) -> Option<Element<'a, Msg>> {
    if ids.is_empty() {
        return None;
    }
    let chips: Vec<Element<'a, Msg>> = ids.iter().map(|id| file_chip(state, id)).collect();
    Some(column(chips).spacing(4).into())
}

/// A file's chip: its title (or name), its kind and size; or its state while it loads or fails.
fn file_chip<'a>(state: &State, id: &str) -> Element<'a, Msg> {
    match state.files.get(id) {
        Some(meta) => {
            let name = if meta.title.is_empty() {
                &meta.name
            } else {
                &meta.title
            };
            let kind = if meta.content_type.starts_with("image/") {
                "image · "
            } else if meta.content_type.starts_with("video/") {
                "video · "
            } else {
                ""
            };
            container(ui::sans(
                format!("{kind}{name}  {}", size_of(meta.size)),
                12.0,
                crate::theme::palette().ink_2,
            ))
            .padding(Padding {
                top: 2.0,
                right: 6.0,
                bottom: 2.0,
                left: 6.0,
            })
            .style(|_: &Theme| container::Style {
                border: Border {
                    color: crate::theme::palette().rule,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .into()
        }
        None if state.missing.contains(id) => ui::faint("file unavailable").into(),
        None => ui::faint("file …").into(),
    }
}

/// The thread under a post: its last comments (or all, on request), then the composer when open.
/// None when there is nothing to show.
fn thread<'a>(state: &'a State, p: &'a Post) -> Option<Element<'a, Msg>> {
    let composing = state.composing.contains(&p.id);
    if p.comments.is_empty() && !composing {
        return None;
    }
    let mut all: Vec<&'a Comment> = p.comments.iter().collect();
    all.sort_by(|x, y| x.created_at.total_cmp(&y.created_at));
    let expanded = state.thread_open.contains(&p.id);
    let mut col: Vec<Element<'a, Msg>> = Vec::new();
    if all.len() > THREAD_PREVIEW {
        let label = if expanded {
            "Show fewer".to_string()
        } else {
            format!("Show all {} comments", all.len())
        };
        col.push(ui::btn(
            &label,
            None,
            ui::Kind::Ghost,
            Some(Msg::ToggleThread(p.id.clone())),
        ));
    }
    let shown: &[&'a Comment] = if all.len() > THREAD_PREVIEW && !expanded {
        &all[all.len() - THREAD_PREVIEW..]
    } else {
        &all[..]
    };
    for c in shown.iter().copied() {
        col.push(comment_row(state, c));
    }
    if composing {
        col.push(composer(state, p));
    }
    Some(column(col).spacing(10).into())
}

/// A comment: the author's initial, name (the owner's own is marked), time, body and files.
fn comment_row<'a>(state: &'a State, c: &'a Comment) -> Element<'a, Msg> {
    let name = match c.agent_cid {
        None => "You (owner)".to_string(),
        Some(cid) => match state.names.get(&cid) {
            Some(n) => n.clone(),
            None if !c.author.is_empty() => c.author.clone(),
            None => format!("agent {cid}"),
        },
    };
    let initial = name
        .chars()
        .next()
        .map(|ch| ch.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string());
    let colour = if c.agent_cid.is_none() {
        crate::theme::palette().signal
    } else {
        crate::theme::palette().ink
    };
    let rctx = rich::context(&state.cards, None, None, None);
    let mut main: Vec<Element<'a, Msg>> = vec![
        row![ui::sans(name, 12.0, colour), ui::faint(ago(c.created_at))]
            .spacing(10)
            .align_y(Alignment::Center)
            .into(),
        rich::view(&c.body, &rctx, &state.cards, &on_rich),
    ];
    if let Some(files) = attachments(state, &c.attachments) {
        main.push(files);
    }
    row![
        ui::sans(initial, 12.0, crate::theme::palette().ink_2),
        column(main).spacing(4).width(Length::Fill)
    ]
    .spacing(10)
    .into()
}

/// The owner's reply composer. Its draft lives in the state, so a redraw never loses it.
/// Ctrl/Cmd+Enter sends (Enter adds a line); the Send button is off while the text is empty,
/// too long, or on its way.
fn composer<'a>(state: &'a State, p: &'a Post) -> Element<'a, Msg> {
    let draft = state.drafts.get(&p.id).map(String::as_str).unwrap_or("");
    let len = draft.chars().count();
    let can_send = !draft.trim().is_empty() && len <= REPLY_MAX && !state.sending.contains(&p.id);
    let Some(editor_content) = state.editors.get(&p.id) else {
        return column![].into();
    };
    let typed_id = p.id.clone();
    let send_id = p.id.clone();
    let input = text_editor(editor_content)
        .placeholder("Reply as the owner. Ctrl+Enter sends.")
        .padding(6.0)
        .height(Length::Fixed(96.0))
        .key_binding(move |kp| {
            let enter = kp.key == iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter);
            if enter && (kp.modifiers.control() || kp.modifiers.command()) {
                Some(text_editor::Binding::Custom(Msg::Send(send_id.clone())))
            } else {
                text_editor::Binding::from_key_press(kp)
            }
        })
        .on_action(move |a| Msg::Edit(typed_id.clone(), a));
    let meter = ui::faint(format!("{len}/{REPLY_MAX}"));
    let cancel = ui::btn(
        "Cancel",
        None,
        ui::Kind::Ghost,
        Some(Msg::Cancel(p.id.clone())),
    );
    let send = ui::btn(
        "Send",
        None,
        ui::Kind::Primary,
        can_send.then(|| Msg::Send(p.id.clone())),
    );
    column![
        input,
        row![container(meter).width(Length::Fill), cancel, send]
            .spacing(8)
            .align_y(Alignment::Center),
    ]
    .spacing(8)
    .into()
}

pub use apz_engine::roles::role_label;

fn count_label(n: usize) -> String {
    if n == 1 {
        "1 comment".to_string()
    } else {
        format!("{n} comments")
    }
}

fn size_of(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{} KB", (n as f64 / 1024.0).round() as u64)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

/// Relative time for a millisecond timestamp (the web's `ago`).
fn ago(ms: f64) -> String {
    if ms <= 0.0 {
        return "—".to_string();
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(ms, |d| d.as_millis() as f64);
    let s = ((now - ms) / 1000.0).round().max(0.0) as u64;
    match s {
        0..=4 => "just now".to_string(),
        5..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

/// The posts in a scope: those `keep` takes (by author cid and task), newest first.
fn kept<'a>(state: &'a State, keep: &impl Fn(u64, Option<&str>) -> bool) -> Vec<&'a Post> {
    let mut posts: Vec<&Post> = state
        .posts
        .iter()
        .filter(|p| keep(p.agent_cid, p.task.as_deref()))
        .collect();
    posts.sort_by(|x, y| y.created_at.total_cmp(&x.created_at));
    posts
}

/// How many posts a scope holds (the IDE's Board tab count).
pub fn count(state: &State, keep: impl Fn(u64, Option<&str>) -> bool) -> usize {
    state
        .posts
        .iter()
        .filter(|p| keep(p.agent_cid, p.task.as_deref()))
        .count()
}

/// The posts each agent made after `since_ms` (epoch milliseconds): the unread count the IDE shows per agent.
pub fn unread(state: &State, since_ms: u64) -> HashMap<u64, usize> {
    let mut out: HashMap<u64, usize> = HashMap::new();
    for post in state
        .posts
        .iter()
        .filter(|p| p.created_at > since_ms as f64)
    {
        *out.entry(post.agent_cid).or_insert(0) += 1;
    }
    out
}

/// The Board as the IDE's right panel draws it: the posts in scope, newest first, each with its author's colour
/// bar at its left and the post as the board screen draws it (body, thread, reply composer).
pub fn panel<'a>(state: &'a State, keep: impl Fn(u64, Option<&str>) -> bool) -> Element<'a, Msg> {
    let mut col = column![].spacing(12);
    if state.failed {
        col = col.push(ui::error_line("Could not load the board."));
    } else if !state.loaded {
        col = col.push(ui::loading());
    } else if state.posts.is_empty() {
        col = col.push(ui::empty("board", "The board is empty.", ""));
    } else {
        let posts = kept(state, &keep);
        if posts.is_empty() {
            col = col.push(ui::faint("No posts in this scope yet."));
        }
        let total = posts.len();
        for (i, post) in posts.into_iter().enumerate() {
            let hue = crate::console::engine::roles::agent_hue(post.agent_cid);
            let colour = ui::hsl(hue, 0.55, 0.62);
            let bar = container(
                iced::widget::Space::new()
                    .width(Length::Fixed(3.0))
                    .height(Length::Fill),
            )
            .height(Length::Fill)
            .style(move |_: &Theme| container::Style {
                background: Some(colour.into()),
                ..Default::default()
            });
            col = col.push(row![bar, entry(state, total - i, post, true)].spacing(10));
        }
    }
    col.into()
}

pub fn subscription(ctx: &Ctx) -> iced::Subscription<Msg> {
    crate::console::events::subscription(&ctx.client).map(Msg::Event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_owner_post_carries_its_mentions_and_no_agent() {
        let post: Post = serde_json::from_value(json!({
            "id": "p9", "agent_cid": 0, "role": "owner", "subject": "ship", "body": "ship it",
            "created_at": 1.0, "owner": true, "mentions": ["all"], "project": "pr1"
        }))
        .expect("an owner post parses");
        assert!(post.owner);
        assert_eq!(post.mentions, vec!["all"]);
        let agent: Post = serde_json::from_value(json!({
            "id": "p8", "agent_cid": 3, "role": "builder", "subject": "hi", "created_at": 1.0
        }))
        .expect("an agent post parses without the owner fields");
        assert!(!agent.owner && agent.mentions.is_empty());
    }
}
