//! Rich content in agent text (docs/design/workspace.md, section 3): the references in a text become chips with hover
//! cards, images show inline, and videos and HTML files show a card whose Open button leaves the app. The classifier is
//! `apz_rich`; the cards come from the hub's unfurl (`POST /v1/unfurl`), cached here for five minutes. Paragraphs with no
//! reference draw exactly as `ui::markdown` draws them, so only text that has a reference looks different.
//!
//! Each surface keeps a [`Cards`] in its state and does four things with it: `prefetch` when it loads text (a task that
//! asks the hub for the references it has not cached yet, and fetches images), `apply` for every [`Ev`] it gets back,
//! `context` to build the classifier's context for its view, and `view` where the text is drawn. Navigation is the
//! surface's: [`RichAction::route`] names the screen an action opens, and the IDE handles [`RichAction::OpenPath`].

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::RwLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use apz_rich::{AgentName, Ctx, Ref, Span};
use iced::widget::image::Handle as RasterHandle;
use iced::widget::svg::Handle as SvgHandle;
use iced::widget::{Space, button, column, container, image, row, svg, text, tooltip};
use iced::{Alignment, Border, Color, ContentFit, Element, Font, Length, Theme};
use serde_json::{Value, json};

use super::client::Client;
use super::screens::Id;
use super::ui::{self, Inline, Kind};
use crate::theme::{self, MONO, SANS, SANS_MEDIUM, SANS_SEMIBOLD};

/// The classifier's context, as the view and the prefetch read it.
pub type RichCtx = Ctx;

/// A hub answer is kept this long before it is asked for again (the hub keeps its own, shorter-lived, cache).
pub const TTL_MS: u64 = 5 * 60 * 1000;
/// The agent roster is read again after this long, and only for a text that names an agent.
const ROSTER_TTL_MS: u64 = 60 * 1000;
/// Most references one unfurl request carries (the hub's limit).
pub const BATCH: usize = 50;
/// Most images one load fetches.
const IMAGES_PER_LOAD: usize = 12;
/// The largest image fetched, in bytes.
const IMAGE_MAX: usize = 8 * 1024 * 1024;
const IMAGE_TIMEOUT: Duration = Duration::from_secs(10);
const IMAGE_MAX_HEIGHT: f32 = 360.0;
const CARD_WIDTH: f32 = 320.0;
const HOVER_DELAY: Duration = Duration::from_millis(300);
const RADIUS: f32 = 6.0;
/// A merged pull request's colour: the one hue the palette does not carry.
const MERGED: Color = Color::from_rgb8(0xb3, 0x8a, 0xff);

/// What a chip, link or image asks the surface to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RichAction {
    /// Open an https link in the system's browser (a web page, a video or an HTML file).
    OpenUrl(String),
    /// Open an agent's page.
    OpenAgent(u64),
    /// Open a task's page.
    OpenTask(u64),
    /// Open a file of the workspace in the IDE, at a line.
    OpenPath { path: String, line: Option<u32> },
    /// Open a library file or deliverable.
    OpenLibrary(String),
}

impl RichAction {
    /// The screen this action opens, when it opens one. The router sends the message there and does not update the
    /// screen it came from.
    pub fn route(&self) -> Option<Id> {
        match self {
            RichAction::OpenAgent(cid) => Some(Id::Agent(*cid)),
            RichAction::OpenTask(number) => Some(Id::Task(format!("t{number}"))),
            RichAction::OpenLibrary(_) => Some(Id::Library),
            RichAction::OpenUrl(_) | RichAction::OpenPath { .. } => None,
        }
    }
}

/// What a surface's rich content sends back: a click, or the answers a load fetched.
#[derive(Debug, Clone)]
pub enum Ev {
    Action(RichAction),
    Loaded(Box<Loaded>),
}

/// One load's results. `asked` and `images_asked` are the keys it was asked to fetch, so their pending marks clear
/// even when the hub gave no answer for them.
#[derive(Debug, Clone)]
pub struct Loaded {
    at: u64,
    asked: Vec<String>,
    answers: Vec<(String, Value)>,
    roster_asked: bool,
    roster: Option<Vec<AgentName>>,
    images_asked: Vec<String>,
    images: Vec<(String, ImageBytes)>,
}

/// An image's content type and bytes, or why it was not fetched.
type ImageBytes = Result<(String, Vec<u8>), String>;

/// A picture the view can draw.
#[derive(Clone)]
enum Picture {
    Raster(RasterHandle),
    Svg(SvgHandle),
    Failed,
}

/// The cards, the images and the roster a surface shows rich content with.
#[derive(Default)]
pub struct Cards {
    /// The hub's answer for each reference (keyed by the reference's JSON), and when it came.
    answers: HashMap<String, (Value, u64)>,
    /// References a load is fetching now.
    pending: HashSet<String>,
    /// The agents `@Name` can name, as the hub last listed them.
    roster: Vec<AgentName>,
    roster_at: Option<u64>,
    roster_pending: bool,
    /// Images by URL (a hub-relative path for a library file).
    images: HashMap<String, Picture>,
    image_pending: HashSet<String>,
    /// The hub's base URL, as the last load saw it: its own `/v1/files` and `/v1/library` links are library references.
    hub: Option<String>,
}

impl std::fmt::Debug for Cards {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cards")
            .field("answers", &self.answers.len())
            .field("images", &self.images.len())
            .field("roster", &self.roster.len())
            .finish()
    }
}

impl Cards {
    fn answer(&self, key: &str) -> Option<&Value> {
        self.answers.get(key).map(|(answer, _)| answer)
    }

    fn fresh(&self, key: &str, now: u64) -> bool {
        self.answers
            .get(key)
            .is_some_and(|(_, at)| now.saturating_sub(*at) < TTL_MS)
    }

    /// The references that need asking: unfurlable, not fresh, not already being fetched. Marks them pending.
    fn claim_refs(&mut self, refs: Vec<Ref>, now: u64) -> Vec<Ref> {
        let mut out = Vec::new();
        for r in refs {
            let k = key(&r);
            if !self.fresh(&k, now) && self.pending.insert(k) {
                out.push(r);
            }
        }
        out
    }

    /// The images that need fetching, at most `IMAGES_PER_LOAD`. Marks them pending.
    fn claim_images(&mut self, urls: Vec<String>) -> Vec<String> {
        let mut out = Vec::new();
        for url in urls {
            if out.len() == IMAGES_PER_LOAD {
                break;
            }
            if !self.images.contains_key(&url) && self.image_pending.insert(url.clone()) {
                out.push(url);
            }
        }
        out
    }

    /// Whether the roster is read now: a text names an agent, the roster is not pending, and it is older than its TTL.
    fn claim_roster(&mut self, texts: &[&str], now: u64) -> bool {
        let stale = self
            .roster_at
            .is_none_or(|at| now.saturating_sub(at) >= ROSTER_TTL_MS);
        if !self.roster_pending && stale && texts.iter().any(|t| t.contains('@')) {
            self.roster_pending = true;
            return true;
        }
        false
    }

    /// Stores one load's results and clears what it was asked to fetch.
    pub fn absorb(&mut self, loaded: Loaded) {
        for k in &loaded.asked {
            self.pending.remove(k);
        }
        for (k, answer) in loaded.answers {
            self.answers.insert(k, (answer, loaded.at));
        }
        if loaded.roster_asked {
            self.roster_pending = false;
            self.roster_at = Some(loaded.at);
            if let Some(roster) = loaded.roster {
                self.roster = roster;
            }
        }
        for url in &loaded.images_asked {
            self.image_pending.remove(url);
        }
        for (url, result) in loaded.images {
            let picture = match result {
                Ok((content_type, bytes)) if content_type == "image/svg+xml" => {
                    Picture::Svg(SvgHandle::from_memory(bytes))
                }
                Ok((_, bytes)) => Picture::Raster(RasterHandle::from_bytes(bytes)),
                Err(_) => Picture::Failed,
            };
            self.images.insert(url, picture);
        }
    }

    fn picture(&self, url: &str) -> Option<&Picture> {
        self.images.get(url)
    }
}

/// Handles a surface's rich message: a click opens an https link (the other actions are the surface's to route), and a
/// load is stored.
pub fn apply(cards: &mut Cards, ev: Ev) {
    match ev {
        Ev::Action(RichAction::OpenUrl(url)) => {
            let _ = ui::open_url(&url);
        }
        Ev::Action(_) => {}
        Ev::Loaded(loaded) => cards.absorb(*loaded),
    }
}

/// The plaza's git host, as the hub's overview last named it: the forge's web origin (`git_host`). None when the hub
/// has none. The shell keeps it from each overview load, and every surface's classifier context reads it, so a plaza
/// git link is a chip on the desktop as it is on the web.
static PLAZA_GIT_HOST: RwLock<Option<String>> = RwLock::new(None);

/// Keeps the plaza's git host from a `/v1/overview` answer.
pub fn set_plaza_git_host(overview: &Value) {
    if let Ok(mut slot) = PLAZA_GIT_HOST.write() {
        *slot = git_host_of(overview);
    }
}

/// The `git_host` of a `/v1/overview` answer, trimmed. Null, empty, or absent (an older hub) is None.
pub fn git_host_of(overview: &Value) -> Option<String> {
    overview
        .get("git_host")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .map(str::to_string)
}

/// The classifier's context for a surface: its project and repository, its workspace's paths (the IDE only), the
/// roster the cards hold, the plaza's git host, and the hub's own host (its `/v1/files` and `/v1/library` links are
/// library references).
pub fn context(
    cards: &Cards,
    project: Option<&str>,
    project_repo: Option<&str>,
    paths: Option<&BTreeSet<String>>,
) -> RichCtx {
    let plaza_git_host = PLAZA_GIT_HOST.read().ok().and_then(|slot| slot.clone());
    build_context(cards, project, project_repo, paths, plaza_git_host)
}

fn build_context(
    cards: &Cards,
    project: Option<&str>,
    project_repo: Option<&str>,
    paths: Option<&BTreeSet<String>>,
    plaza_git_host: Option<String>,
) -> RichCtx {
    Ctx {
        project: project.map(str::to_string),
        project_repo: project_repo.map(str::to_string),
        paths: paths.cloned().unwrap_or_default(),
        agents: cards.roster.clone(),
        plaza_git_host,
        hub_host: cards.hub.clone(),
    }
}

/// Asks the hub for the cards and images of the references in `texts` that the cards do not hold yet, and reads the
/// roster when a text names an agent. Nothing to ask is no task.
pub fn prefetch(
    cards: &mut Cards,
    client: &Client,
    ctx: &RichCtx,
    texts: &[&str],
) -> iced::Task<Ev> {
    let now = now_ms();
    cards.hub = Some(client.base().to_string());
    let mut ctx = ctx.clone();
    ctx.hub_host = cards.hub.clone();
    let mut seen = HashSet::new();
    let mut refs = Vec::new();
    let mut urls = Vec::new();
    for text in texts {
        for span in apz_rich::classify(text, &ctx) {
            let Span::Ref { r, .. } = span else { continue };
            match &r {
                Ref::Image { url } => {
                    if !urls.contains(url) {
                        urls.push(url.clone());
                    }
                }
                other if unfurls(other) && seen.insert(key(other)) => {
                    refs.push(r.clone());
                }
                _ => {}
            }
        }
    }
    let refs = cards.claim_refs(refs, now);
    let images = cards.claim_images(urls);
    let roster_asked = cards.claim_roster(texts, now);
    if refs.is_empty() && images.is_empty() && !roster_asked {
        return iced::Task::none();
    }
    let have: HashSet<String> = cards.images.keys().cloned().collect();
    let texts: Vec<String> = if roster_asked {
        texts.iter().map(|t| t.to_string()).collect()
    } else {
        Vec::new()
    };
    let job = Job {
        client: client.clone(),
        ctx,
        texts,
        refs,
        images,
        roster_asked,
        have,
        now,
    };
    iced::Task::perform(async move { job.run() }, |loaded| {
        Ev::Loaded(Box::new(loaded))
    })
}

/// One load, run off the interface's thread.
struct Job {
    client: Client,
    ctx: RichCtx,
    texts: Vec<String>,
    refs: Vec<Ref>,
    images: Vec<String>,
    roster_asked: bool,
    have: HashSet<String>,
    now: u64,
}

impl Job {
    fn run(self) -> Loaded {
        let roster = if self.roster_asked {
            read_roster(&self.client)
        } else {
            None
        };
        let mut ctx = self.ctx;
        let mut refs = self.refs.clone();
        if let Some(roster) = &roster {
            // Names the roster has just learned are now references: ask for them too.
            ctx.agents = roster.clone();
            let mut known: HashSet<String> = refs.iter().map(key).collect();
            for text in &self.texts {
                for span in apz_rich::classify(text, &ctx) {
                    if let Span::Ref { r, .. } = span
                        && unfurls(&r)
                        && known.insert(key(&r))
                    {
                        refs.push(r);
                    }
                }
            }
        }
        let mut answers = Vec::new();
        for batch in batches(&refs) {
            let keys: Vec<String> = batch.iter().map(key).collect();
            let cards = post_unfurl(&self.client, batch).unwrap_or_default();
            for (i, k) in keys.into_iter().enumerate() {
                let answer = cards
                    .get(i)
                    .filter(|a| a.get("ok").is_some())
                    .cloned()
                    .unwrap_or_else(|| json!({"ok": false, "error": "unavailable"}));
                answers.push((k, answer));
            }
        }
        // A library image is fetched once its card says it is an image.
        let mut urls = self.images.clone();
        for (k, answer) in &answers {
            let Ok(Ref::Library { id }) = serde_json::from_str::<Ref>(k) else {
                continue;
            };
            let url = format!("/v1/files/{id}/content");
            let image = answer["card"]["content_type"]
                .as_str()
                .is_some_and(|c| c.starts_with("image/"));
            if image && !self.have.contains(&url) && !urls.contains(&url) {
                urls.push(url);
            }
        }
        urls.truncate(IMAGES_PER_LOAD);
        // The images a load fetches: the claimed ones first, then the library's images the cards named.
        let images = urls
            .iter()
            .map(|url| (url.clone(), fetch_image(&self.client, url)))
            .collect();
        Loaded {
            at: self.now,
            asked: self.refs.iter().map(key).collect(),
            answers,
            roster_asked: self.roster_asked,
            roster,
            images_asked: self.images,
            images,
        }
    }
}

/// The agents the hub lists, as `@Name` can name them. None when the read fails.
fn read_roster(client: &Client) -> Option<Vec<AgentName>> {
    let value = client.call("GET", "/v1/agents", None).ok()?;
    let list = value
        .as_array()
        .or_else(|| value.get("agents").and_then(Value::as_array))?;
    Some(
        list.iter()
            .filter_map(|a| {
                let name = a.get("name").and_then(Value::as_str)?.trim();
                let cid = a.get("cid").and_then(Value::as_u64)?;
                (!name.is_empty()).then(|| AgentName {
                    name: name.to_string(),
                    cid,
                })
            })
            .collect(),
    )
}

/// `POST /v1/unfurl` for a batch: the cards in the order asked, each `{ok, card}` or `{ok, error}`.
fn post_unfurl(client: &Client, refs: &[Ref]) -> Result<Vec<Value>, String> {
    let body = json!({ "refs": refs });
    let answer: Value = client.post("/v1/unfurl", &body)?;
    Ok(answer
        .get("cards")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

/// The bytes of an image: a hub path, or an https URL on another host (no credential is sent there).
fn fetch_image(client: &Client, url: &str) -> Result<(String, Vec<u8>), String> {
    let (target, path) = image_target(client, url)?;
    let reply = apz_http::client::request(&target, "GET", &path, None, IMAGE_TIMEOUT)
        .map_err(|e| e.to_string())?;
    if !(200..300).contains(&reply.status) {
        return Err(format!("HTTP {}", reply.status));
    }
    if reply.body.len() > IMAGE_MAX {
        return Err("too large".into());
    }
    let content_type = reply
        .content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !content_type.starts_with("image/") {
        return Err("not an image".into());
    }
    Ok((content_type, reply.body))
}

/// Where an image comes from: the hub (a path, or the hub's own origin) with the owner's client, or another https host
/// with no credential. Plain http to another host is refused.
fn image_target(client: &Client, url: &str) -> Result<(apz_http::client::Target, String), String> {
    let base = client.base();
    if url.starts_with('/') {
        return Ok((client.target.clone(), path_of(url)));
    }
    if let Some(rest) = url.strip_prefix(base)
        && (rest.is_empty() || rest.starts_with('/'))
    {
        let path = if rest.is_empty() {
            "/".to_string()
        } else {
            path_of(rest)
        };
        return Ok((client.target.clone(), path));
    }
    let Some(rest) = url.strip_prefix("https://") else {
        return Err("not an https image".into());
    };
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if host.is_empty() {
        return Err("no host".into());
    }
    let target = apz_http::client::Target {
        base: format!("https://{host}"),
        pin: None,
        bearer: None,
        headers: Vec::new(),
    };
    Ok((target, path_of(path)))
}

/// A URL's path and query, without its fragment.
fn path_of(url: &str) -> String {
    let path = url.split('#').next().unwrap_or("");
    if path.is_empty() {
        "/".to_string()
    } else {
        path.to_string()
    }
}

/// Whether the hub unfurls a reference (the rest are workspace paths, media or plain links).
fn unfurls(r: &Ref) -> bool {
    !matches!(
        r,
        Ref::Path { .. }
            | Ref::Image { .. }
            | Ref::Video { .. }
            | Ref::Html { .. }
            | Ref::Link { .. }
            | Ref::MentionAll
            | Ref::PlazaGitRun { .. }
    )
}

/// The cache key of a reference: its JSON, as the hub's own cache keys it.
fn key(r: &Ref) -> String {
    serde_json::to_string(r).unwrap_or_default()
}

/// The chunks of references one request carries.
pub fn batches(refs: &[Ref]) -> Vec<&[Ref]> {
    refs.chunks(BATCH).collect()
}

/// The text a surface draws its agent-written text with: paragraphs, lists, tables and quotes as `ui::markdown` draws
/// them, and each reference in them as a chip, link, image or card.
pub fn view<'a, M: Clone + 'a>(
    source: &str,
    ctx: &RichCtx,
    cards: &Cards,
    on: &dyn Fn(RichAction) -> M,
) -> Element<'a, M> {
    ui::markdown_with(source, &|body, base, size, color| {
        paragraph(body, base, size, color, ctx, cards, on)
    })
}

/// One paragraph. With no reference it is the plain paragraph; otherwise its words and chips flow in one wrapping row,
/// and its images and cards sit under it.
fn paragraph<'a, M: Clone + 'a>(
    body: &str,
    base: Font,
    size: f32,
    color: Color,
    ctx: &RichCtx,
    cards: &Cards,
    on: &dyn Fn(RichAction) -> M,
) -> Element<'a, M> {
    let spans = apz_rich::classify(body, ctx);
    if !spans.iter().any(|s| matches!(s, Span::Ref { .. })) {
        return ui::rich_para(ui::inline(body), base, size, color);
    }
    let mut flow: Vec<Element<'a, M>> = Vec::new();
    let mut blocks: Vec<Element<'a, M>> = Vec::new();
    for span in spans {
        match span {
            Span::Text(t) | Span::Code(t) => words(&mut flow, ui::inline(&t), base, size, color),
            Span::Ref {
                raw,
                r,
                text: label,
            } => match &r {
                Ref::Image { url } => blocks.push(image_block(&r, url, cards, on)),
                Ref::Library { id }
                    if cards
                        .answer(&key(&r))
                        .and_then(|a| a["card"]["content_type"].as_str())
                        .is_some_and(|c| c.starts_with("image/")) =>
                {
                    blocks.push(image_block(
                        &r,
                        &format!("/v1/files/{id}/content"),
                        cards,
                        on,
                    ));
                }
                Ref::Video { url } | Ref::Html { url } => blocks.push(media_card(&r, url, on)),
                Ref::Link { url } => flow.push(link(
                    &label.unwrap_or_else(|| apz_rich::label(&r)),
                    url,
                    size,
                    on,
                )),
                _ => flow.push(chip(&r, &raw, ctx, cards, on)),
            },
        }
    }
    let line: Element<'a, M> = row(flow)
        .spacing(4)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .wrap()
        .vertical_spacing(4)
        .into();
    let mut col = vec![line];
    col.extend(blocks);
    column(col).spacing(8).width(Length::Fill).into()
}

/// The plain pieces of a paragraph as words, one widget each, so they wrap around the chips.
fn words<'a, M: 'a>(
    flow: &mut Vec<Element<'a, M>>,
    parts: Vec<Inline>,
    base: Font,
    size: f32,
    color: Color,
) {
    let p = theme::palette();
    for part in parts {
        let (body, font, color) = match part {
            Inline::Text(s) => (s, base, color),
            Inline::Bold(s) => (
                s,
                Font {
                    weight: iced::font::Weight::Semibold,
                    ..base
                },
                color,
            ),
            Inline::Italic(s) => (
                s,
                Font {
                    style: iced::font::Style::Italic,
                    ..base
                },
                color,
            ),
            Inline::Code(s) => (s, MONO, p.ink_2),
            Inline::Link(label) => (label, base, p.signal),
        };
        for word in body.split_whitespace() {
            flow.push(
                text(word.to_string())
                    .font(font)
                    .size(size)
                    .color(color)
                    .into(),
            );
        }
    }
}

/// A reference as a chip: its glyph and short label, coloured by its state, opening its action; a hover card when the
/// hub has unfurled it.
fn chip<'a, M: Clone + 'a>(
    r: &Ref,
    raw: &str,
    ctx: &RichCtx,
    cards: &Cards,
    on: &dyn Fn(RichAction) -> M,
) -> Element<'a, M> {
    let p = theme::palette();
    let answer = cards.answer(&key(r));
    let card = answer.and_then(|a| a.get("card")).filter(|c| !c.is_null());
    let accent = tone_color(chip_tone(r, card));
    let content = row![
        glyph(r, accent),
        text(chip_label(r, ctx))
            .font(SANS_MEDIUM)
            .size(12.0)
            .color(p.ink)
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    let mut b = button(content)
        .padding([2, 8])
        .style(move |_: &Theme, status: button::Status| {
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: Some(if hovered { p.hover } else { p.sheet }.into()),
                text_color: p.ink,
                border: Border {
                    color: accent,
                    width: 1.0,
                    radius: RADIUS.into(),
                },
                ..Default::default()
            }
        });
    if let Some(action) = action_for(r, raw, ctx) {
        b = b.on_press(on(action));
    }
    let chip: Element<'a, M> = b.into();
    if !unfurls(r) {
        return chip;
    }
    tooltip(chip, card_panel(r, answer), tooltip::Position::Bottom)
        .delay(HOVER_DELAY)
        .gap(6)
        .padding(0.0)
        .style(move |_: &Theme| container::Style {
            background: Some(p.sheet.into()),
            border: Border {
                color: p.rule,
                width: 1.0,
                radius: RADIUS.into(),
            },
            ..Default::default()
        })
        .into()
}

/// A chip's glyph: the GitHub mark, the plaza's (the AgentPlaza mark), an agent's status dot, a task or file icon.
fn glyph<'a, M: 'a>(r: &Ref, accent: Color) -> Element<'a, M> {
    let p = theme::palette();
    match r {
        Ref::GithubPull { .. }
        | Ref::GithubIssue { .. }
        | Ref::GithubCommit { .. }
        | Ref::GithubRepo { .. }
        | Ref::GithubFile { .. }
        | Ref::GithubRelease { .. }
        | Ref::GithubRun { .. }
        | Ref::Gist { .. } => ui::brand_logo("github", 12.0),
        Ref::PlazaGitPull { .. }
        | Ref::PlazaGitIssue { .. }
        | Ref::PlazaGitCommit { .. }
        | Ref::PlazaGitRepo { .. }
        | Ref::PlazaGitFile { .. }
        | Ref::PlazaGitRelease { .. }
        | Ref::PlazaGitRun { .. } => ui::hive(12.0),
        Ref::Agent { .. } => container(Space::new())
            .width(Length::Fixed(8.0))
            .height(Length::Fixed(8.0))
            .style(move |_: &Theme| container::Style {
                background: Some(accent.into()),
                border: Border {
                    radius: 4.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into(),
        Ref::Task { .. } => ui::icon("tasks", 12.0, p.ink_2),
        Ref::Path { dir: true, .. } => ui::icon("projects", 12.0, p.ink_2),
        Ref::Path { .. } => ui::icon("file", 12.0, p.ink_2),
        Ref::Library { .. } | Ref::LibraryTool { .. } => ui::icon("library", 12.0, p.ink_2),
        _ => Space::new().width(Length::Fixed(0.0)).into(),
    }
}

/// The short label a chip shows: `o/r #12`, `#528`, `@Name` (an agent named by its id shows the roster's name).
fn chip_label(r: &Ref, ctx: &RichCtx) -> String {
    match r {
        Ref::Agent {
            name: None,
            cid: Some(cid),
        } => ctx
            .agents
            .iter()
            .find(|a| a.cid == *cid)
            .map_or_else(|| apz_rich::label(r), |a| format!("@{}", a.name)),
        _ => apz_rich::label(r),
    }
}

/// The colour a chip takes from its state. PR and issue: open ok, draft secondary, merged purple, closed error.
/// Agent: working info, offline muted. Task: done ok, claimed or awaiting review info, failed or cancelled error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChipTone {
    Ok,
    Draft,
    Merged,
    Closed,
    Info,
    Muted,
    Crit,
    Plain,
}

pub fn chip_tone(r: &Ref, card: Option<&Value>) -> ChipTone {
    let field = |name: &str| {
        card.and_then(|c| c.get(name))
            .and_then(Value::as_str)
            .unwrap_or("")
    };
    match r {
        Ref::GithubPull { .. }
        | Ref::PlazaGitPull { .. }
        | Ref::GithubIssue { .. }
        | Ref::PlazaGitIssue { .. } => match field("state") {
            "open" => ChipTone::Ok,
            "draft" => ChipTone::Draft,
            "merged" => ChipTone::Merged,
            "closed" => ChipTone::Closed,
            _ => ChipTone::Plain,
        },
        Ref::Agent { .. } => match field("status") {
            "working" => ChipTone::Info,
            "offline" | "asleep" | "dozing" => ChipTone::Muted,
            _ => ChipTone::Plain,
        },
        Ref::Task { .. } => match field("status") {
            "done" | "completed" => ChipTone::Ok,
            "claimed" | "awaiting_review" | "working" => ChipTone::Info,
            "failed" | "cancelled" | "withdrawn" | "rejected" => ChipTone::Crit,
            _ => ChipTone::Plain,
        },
        _ => ChipTone::Plain,
    }
}

fn tone_color(tone: ChipTone) -> Color {
    let p = theme::palette();
    match tone {
        ChipTone::Ok => p.ok,
        ChipTone::Draft | ChipTone::Muted => p.ink_3,
        ChipTone::Merged => MERGED,
        ChipTone::Closed | ChipTone::Crit => p.crit,
        ChipTone::Info => p.info,
        ChipTone::Plain => p.rule,
    }
}

/// The action a reference opens: an https page (its own link, or the one the author wrote), an agent's page, a task, a
/// workspace file at its line, or a library file. None when there is nothing to open.
pub fn action_for(r: &Ref, raw: &str, ctx: &RichCtx) -> Option<RichAction> {
    Some(match r {
        Ref::Agent { name, cid } => {
            let cid = cid.or_else(|| {
                name.as_deref()
                    .and_then(|n| ctx.agents.iter().find(|a| a.name == n).map(|a| a.cid))
            })?;
            RichAction::OpenAgent(cid)
        }
        Ref::Task { number } => RichAction::OpenTask(*number),
        Ref::Path { dir: true, .. } | Ref::MentionAll | Ref::LibraryTool { .. } => return None,
        Ref::Path { path, line, .. } => RichAction::OpenPath {
            path: path.clone(),
            line: *line,
        },
        Ref::Library { id } => RichAction::OpenLibrary(id.clone()),
        Ref::Image { url } | Ref::Video { url } | Ref::Html { url } | Ref::Link { url } => {
            RichAction::OpenUrl(url.clone())
        }
        other => RichAction::OpenUrl(web_url(other, raw, ctx)?),
    })
}

/// The web page of a GitHub or plaza git reference: the link the author wrote when it is https, else the page the
/// reference names.
fn web_url(r: &Ref, raw: &str, ctx: &RichCtx) -> Option<String> {
    if let Some(url) = written_url(raw) {
        return Some(url);
    }
    let plaza = ctx.plaza_git_host.as_deref().map(|h| {
        let h = h.trim_end_matches('/');
        if h.starts_with("http://") || h.starts_with("https://") {
            h.to_string()
        } else {
            format!("https://{h}")
        }
    });
    let gh = "https://github.com";
    Some(match r {
        Ref::GithubPull {
            owner,
            repo,
            number,
        } => format!("{gh}/{owner}/{repo}/pull/{number}"),
        Ref::GithubIssue {
            owner,
            repo,
            number,
        } => format!("{gh}/{owner}/{repo}/issues/{number}"),
        Ref::GithubCommit { owner, repo, sha } => format!("{gh}/{owner}/{repo}/commit/{sha}"),
        Ref::GithubRepo { owner, repo } => format!("{gh}/{owner}/{repo}"),
        Ref::GithubRelease { owner, repo, tag } => {
            format!("{gh}/{owner}/{repo}/releases/tag/{tag}")
        }
        Ref::GithubRun { owner, repo, id } => format!("{gh}/{owner}/{repo}/actions/runs/{id}"),
        Ref::GithubFile {
            owner,
            repo,
            reference,
            path,
            line,
            dir,
        } => {
            let kind = if *dir { "tree" } else { "blob" };
            let anchor = line.map(|n| format!("#L{n}")).unwrap_or_default();
            format!("{gh}/{owner}/{repo}/{kind}/{reference}/{path}{anchor}")
        }
        Ref::Gist { id } => format!("https://gist.github.com/{id}"),
        Ref::PlazaGitPull {
            owner,
            repo,
            number,
        } => format!("{}/{owner}/{repo}/pulls/{number}", plaza?),
        Ref::PlazaGitIssue {
            owner,
            repo,
            number,
        } => format!("{}/{owner}/{repo}/issues/{number}", plaza?),
        Ref::PlazaGitCommit { owner, repo, sha } => {
            format!("{}/{owner}/{repo}/commit/{sha}", plaza?)
        }
        Ref::PlazaGitRepo { owner, repo } => format!("{}/{owner}/{repo}", plaza?),
        Ref::PlazaGitRelease { owner, repo, tag } => {
            format!("{}/{owner}/{repo}/releases/tag/{tag}", plaza?)
        }
        Ref::PlazaGitFile {
            owner,
            repo,
            reference,
            path,
            line,
            ..
        } => {
            let anchor = line.map(|n| format!("#L{n}")).unwrap_or_default();
            format!(
                "{}/{owner}/{repo}/src/branch/{reference}/{path}{anchor}",
                plaza?
            )
        }
        Ref::PlazaGitRun { .. } => return None,
        _ => return None,
    })
}

/// The https URL the author wrote: a bare link, or the target of `[label](url)`.
fn written_url(raw: &str) -> Option<String> {
    if raw.starts_with("https://") {
        return Some(raw.to_string());
    }
    let target = raw.strip_suffix(')')?.rsplit_once("](")?.1;
    target.starts_with("https://").then(|| target.to_string())
}

/// A hover card's panel: the reference's card, or why there is none.
fn card_panel<'a, M: 'a>(r: &Ref, answer: Option<&Value>) -> Element<'a, M> {
    let p = theme::palette();
    let body: Element<'a, M> = match answer {
        None => ui::faint("Loading preview…").into(),
        Some(a) if a.get("ok").and_then(Value::as_bool) != Some(true) => {
            ui::faint(format!("Preview unavailable: {}", reason(a))).into()
        }
        Some(a) => card_body(r, &a["card"]),
    };
    container(body)
        .padding(12)
        .width(Length::Fixed(CARD_WIDTH))
        .style(move |_: &Theme| container::Style {
            background: Some(p.sheet.into()),
            border: Border {
                color: p.rule,
                width: 1.0,
                radius: RADIUS.into(),
            },
            ..Default::default()
        })
        .into()
}

/// Why a card is missing, as the card shows it.
fn reason(answer: &Value) -> &'static str {
    match answer.get("error").and_then(Value::as_str) {
        Some("not_found") => "not found",
        Some("rate_limited") => "rate limited, try again shortly",
        Some("unsupported") => "no preview for this kind",
        _ => "unavailable",
    }
}

fn card_body<'a, M: 'a>(r: &Ref, card: &Value) -> Element<'a, M> {
    let items = match r {
        Ref::GithubPull { .. } | Ref::PlazaGitPull { .. } => pull_lines(card),
        Ref::GithubIssue { .. } | Ref::PlazaGitIssue { .. } => issue_lines(card),
        Ref::GithubCommit { .. } | Ref::PlazaGitCommit { .. } => commit_lines(card),
        Ref::GithubRepo { .. } | Ref::PlazaGitRepo { .. } => repo_lines(card),
        Ref::GithubFile { .. } | Ref::PlazaGitFile { .. } => file_lines(card),
        Ref::GithubRelease { .. } | Ref::PlazaGitRelease { .. } => release_lines(card),
        Ref::GithubRun { .. } | Ref::PlazaGitRun { .. } => run_lines(card),
        Ref::Gist { .. } => gist_lines(card),
        Ref::Agent { .. } => agent_lines(card),
        Ref::Task { .. } => task_lines(card),
        Ref::Library { .. } => library_lines(card),
        Ref::LibraryTool { .. } => tool_lines(card),
        _ => Vec::new(),
    };
    column(items).spacing(5).width(Length::Fill).into()
}

fn str_of<'v>(v: &'v Value, key: &str) -> &'v str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn num_of(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(Value::as_u64)
}

fn login_of(v: &Value) -> &str {
    v.get("author")
        .and_then(|a| a.get("login"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn line<'a, M: 'a>(s: impl Into<String>, size: f32, color: Color) -> Element<'a, M> {
    ui::sans(s.into(), size, color).into()
}

fn title<'a, M: 'a>(s: &str) -> Element<'a, M> {
    text(s.to_string())
        .font(SANS_SEMIBOLD)
        .size(14.0)
        .color(theme::palette().ink)
        .into()
}

fn meta<'a, M: 'a>(s: impl Into<String>) -> Element<'a, M> {
    ui::faint(s.into()).into()
}

fn excerpt<'a, M: 'a>(v: &Value) -> Option<Element<'a, M>> {
    let body = str_of(v, "body_excerpt");
    (!body.is_empty()).then(|| line(ui::ellipsize(body, 220), 12.0, theme::palette().ink_2))
}

/// A state as a badge: open ok, merged signal, closed danger, anything else (draft, done, idle) muted.
fn state_badge<'a, M: 'a>(state: &str) -> Element<'a, M> {
    let tone = match state {
        "open" | "working" => ui::Tone::Ok,
        "merged" => ui::Tone::Signal,
        "closed" | "failed" | "cancelled" => ui::Tone::Danger,
        _ => ui::Tone::Muted,
    };
    ui::badge(state, tone)
}

fn pull_lines<'a, M: 'a>(c: &Value) -> Vec<Element<'a, M>> {
    let mut out = vec![
        title(str_of(c, "title")),
        meta(format!("{} · {}", login_of(c), str_of(c, "state"))),
    ];
    out.push(
        ui::mono(
            format!("{} → {}", str_of(c, "head"), str_of(c, "base")),
            11.0,
            theme::palette().ink_2,
        )
        .into(),
    );
    out.push(meta(format!(
        "+{} −{} · {} comments",
        num_of(c, "additions").unwrap_or(0),
        num_of(c, "deletions").unwrap_or(0),
        num_of(c, "comments").unwrap_or(0)
    )));
    out.extend(excerpt(c));
    out
}

fn issue_lines<'a, M: 'a>(c: &Value) -> Vec<Element<'a, M>> {
    let labels: Vec<&str> = c["labels"]
        .as_array()
        .map(|l| l.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut out = vec![
        title(str_of(c, "title")),
        meta(format!("{} · {}", login_of(c), str_of(c, "state"))),
    ];
    if !labels.is_empty() {
        out.push(ui::mono(labels.join(", "), 11.0, theme::palette().ink_2).into());
    }
    out.push(meta(format!(
        "{} comments",
        num_of(c, "comments").unwrap_or(0)
    )));
    out.extend(excerpt(c));
    out
}

fn commit_lines<'a, M: 'a>(c: &Value) -> Vec<Element<'a, M>> {
    let author = c["author"]
        .get("login")
        .and_then(Value::as_str)
        .or_else(|| c["author"].get("name").and_then(Value::as_str))
        .unwrap_or("");
    let mut out = vec![title(str_of(c, "title")), meta(author.to_string())];
    out.push(meta(format!(
        "{} files · +{} −{}",
        num_of(c, "files").unwrap_or(0),
        num_of(c, "additions").unwrap_or(0),
        num_of(c, "deletions").unwrap_or(0)
    )));
    out
}

fn repo_lines<'a, M: 'a>(c: &Value) -> Vec<Element<'a, M>> {
    let mut out = vec![title(str_of(c, "name"))];
    out.extend(
        (!str_of(c, "description").is_empty())
            .then(|| line(str_of(c, "description"), 12.0, theme::palette().ink_2)),
    );
    out.push(meta(format!(
        "{} stars · {}",
        num_of(c, "stars").unwrap_or(0),
        str_of(c, "language")
    )));
    out
}

fn file_lines<'a, M: 'a>(c: &Value) -> Vec<Element<'a, M>> {
    let mut out = vec![
        ui::mono(str_of(c, "path").to_string(), 12.0, theme::palette().ink).into(),
        meta(format!("ref {}", str_of(c, "ref"))),
    ];
    if c.get("dir").and_then(Value::as_bool) == Some(true) {
        out.push(meta(format!(
            "{} entries",
            num_of(c, "entries").unwrap_or(0)
        )));
        let names: Vec<&str> = c["names"]
            .as_array()
            .map(|n| n.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        out.push(ui::mono(names.join("  "), 11.0, theme::palette().ink_2).into());
    } else if c.get("binary").and_then(Value::as_bool) == Some(true) {
        out.push(meta("binary file"));
    } else if let Some(lines) = c["lines"].as_array() {
        let shown: Vec<&str> = lines.iter().filter_map(Value::as_str).collect();
        out.push(ui::mono(shown.join("\n"), 11.0, theme::palette().ink_2).into());
    }
    out
}

fn release_lines<'a, M: 'a>(c: &Value) -> Vec<Element<'a, M>> {
    let mut out = vec![
        title(str_of(c, "name")),
        meta(format!("{} · {}", str_of(c, "tag"), login_of(c))),
    ];
    out.extend(excerpt(c));
    out
}

fn run_lines<'a, M: 'a>(c: &Value) -> Vec<Element<'a, M>> {
    let shown = if str_of(c, "title").is_empty() {
        str_of(c, "name")
    } else {
        str_of(c, "title")
    };
    let state = if str_of(c, "conclusion").is_empty() {
        str_of(c, "state")
    } else {
        str_of(c, "conclusion")
    };
    vec![
        title(shown),
        meta(format!(
            "{} · {} · {}",
            state,
            str_of(c, "branch"),
            str_of(c, "actor")
        )),
    ]
}

fn gist_lines<'a, M: 'a>(c: &Value) -> Vec<Element<'a, M>> {
    vec![
        title(str_of(c, "title")),
        meta(format!(
            "{} · {} files",
            login_of(c),
            num_of(c, "files").unwrap_or(0)
        )),
    ]
}

fn agent_lines<'a, M: 'a>(c: &Value) -> Vec<Element<'a, M>> {
    let p = theme::palette();
    let mut out = vec![
        title(str_of(c, "name")),
        meta(format!("{} · {}", str_of(c, "role"), str_of(c, "model"))),
    ];
    out.push(state_badge(str_of(c, "status")));
    match c.get("task").filter(|t| !t.is_null()) {
        Some(task) => out.push(line(
            format!(
                "#{} {} ({})",
                num_of(task, "number").unwrap_or(0),
                str_of(task, "title"),
                str_of(task, "state")
            ),
            12.0,
            p.ink_2,
        )),
        None => out.push(meta("holds no task")),
    }
    out.extend(num_of(c, "last_active_ms").map(|ms| meta(format!("active {}", ago(ms)))));
    out
}

fn task_lines<'a, M: 'a>(c: &Value) -> Vec<Element<'a, M>> {
    let mut out = vec![title(str_of(c, "title")), state_badge(str_of(c, "status"))];
    let project = str_of(c, "project");
    out.push(meta(if project.is_empty() {
        "no project".to_string()
    } else {
        format!("project {project}")
    }));
    out.extend(num_of(c, "updated_ms").map(|ms| meta(format!("updated {}", ago(ms)))));
    out
}

fn library_lines<'a, M: 'a>(c: &Value) -> Vec<Element<'a, M>> {
    let mut out = vec![
        title(str_of(c, "name")),
        meta(format!(
            "{} · {}",
            str_of(c, "content_type"),
            human_size(num_of(c, "size").unwrap_or(0))
        )),
    ];
    out.push(meta(format!("by {}", str_of(c, "author"))));
    out.extend(num_of(c, "created_ms").map(|ms| meta(format!("created {}", ago(ms)))));
    out
}

fn tool_lines<'a, M: 'a>(c: &Value) -> Vec<Element<'a, M>> {
    let mut out = vec![title(&format!(
        "{} v{}",
        str_of(c, "name"),
        num_of(c, "version").unwrap_or(0)
    ))];
    out.extend(
        (!str_of(c, "description").is_empty())
            .then(|| line(str_of(c, "description"), 12.0, theme::palette().ink_2)),
    );
    out.push(meta(format!(
        "{} · {} files · {}",
        str_of(c, "state"),
        num_of(c, "files").unwrap_or(0),
        str_of(c, "author")
    )));
    out
}

/// A library file or a deliverable's size, as the hover card shows it.
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// How long ago a millisecond instant was, as the console words it.
fn ago(ms: u64) -> String {
    let secs = now_ms().saturating_sub(ms) / 1000;
    match secs {
        0..=59 => format!("{secs}s ago"),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

/// An image, inline at most `IMAGE_MAX_HEIGHT` tall; a placeholder while it loads, and a link when it cannot.
fn image_block<'a, M: Clone + 'a>(
    r: &Ref,
    url: &str,
    cards: &Cards,
    on: &dyn Fn(RichAction) -> M,
) -> Element<'a, M> {
    let p = theme::palette();
    match cards.picture(url) {
        Some(Picture::Raster(handle)) => {
            container(image(handle.clone()).content_fit(ContentFit::Contain))
                .max_height(IMAGE_MAX_HEIGHT)
                .into()
        }
        Some(Picture::Svg(handle)) => {
            container(svg(handle.clone()).content_fit(ContentFit::Contain))
                .max_height(IMAGE_MAX_HEIGHT)
                .into()
        }
        Some(Picture::Failed) => media_card(r, url, on),
        None => container(ui::faint("Loading image…"))
            .padding(12)
            .width(Length::Fill)
            .style(move |_: &Theme| container::Style {
                border: Border {
                    color: p.rule,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .into(),
    }
}

/// A video or HTML file, or an image that did not load: its name, its kind, and an Open button that leaves the app.
fn media_card<'a, M: Clone + 'a>(
    r: &Ref,
    url: &str,
    on: &dyn Fn(RichAction) -> M,
) -> Element<'a, M> {
    let p = theme::palette();
    let (icon_name, kind) = match r {
        Ref::Video { .. } => ("play", "video"),
        Ref::Html { .. } => ("square-code", "html"),
        _ => ("file", "image"),
    };
    let open = ui::openable(url).then(|| on(RichAction::OpenUrl(url.to_string())));
    let open_button = ui::btn_sm("Open", Some("external"), Kind::Secondary, open);
    container(
        row![
            ui::icon(icon_name, 14.0, p.ink_2),
            column![ui::sans(apz_rich::label(r), 13.0, p.ink), ui::faint(kind)].spacing(2),
            Space::new().width(Length::Fill),
            open_button,
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .padding([8, 10])
    .width(Length::Fill)
    .style(ui::frame(None, p.rule))
    .into()
}

/// A link the author wrote to a page: its label in the signal colour; a click opens it outside the app.
fn link<'a, M: Clone + 'a>(
    label: &str,
    url: &str,
    size: f32,
    on: &dyn Fn(RichAction) -> M,
) -> Element<'a, M> {
    let p = theme::palette();
    let mut b = button(
        text(label.to_string())
            .font(SANS)
            .size(size)
            .color(p.signal),
    )
    .padding(0)
    .style(|_: &Theme, _: button::Status| button::Style {
        background: None,
        text_color: theme::palette().signal,
        border: Border::default(),
        ..Default::default()
    });
    if ui::openable(url) {
        b = b.on_press(on(RichAction::OpenUrl(url.to_string())));
    }
    b.into()
}

/// The current time, in milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pull(number: u64) -> Ref {
        Ref::GithubPull {
            owner: "o".into(),
            repo: "r".into(),
            number,
        }
    }

    fn ctx() -> RichCtx {
        Ctx {
            agents: vec![AgentName {
                name: "Ada".into(),
                cid: 7,
            }],
            hub_host: Some("127.0.0.1:7700".into()),
            ..Ctx::default()
        }
    }

    #[test]
    fn asks_go_out_in_batches_of_fifty() {
        let refs: Vec<Ref> = (1..=120).map(pull).collect();
        let sizes: Vec<usize> = batches(&refs).iter().map(|b| b.len()).collect();
        assert_eq!(sizes, vec![50, 50, 20]);
        assert!(batches(&[]).is_empty());
    }

    #[test]
    fn each_kind_opens_its_own_target() {
        let c = ctx();
        assert_eq!(
            action_for(&Ref::Task { number: 528 }, "#528", &c),
            Some(RichAction::OpenTask(528))
        );
        assert_eq!(
            action_for(
                &Ref::Agent {
                    name: Some("Ada".into()),
                    cid: None
                },
                "@Ada",
                &c
            ),
            Some(RichAction::OpenAgent(7))
        );
        assert_eq!(
            action_for(
                &Ref::Agent {
                    name: None,
                    cid: Some(9)
                },
                "agent:9",
                &c
            ),
            Some(RichAction::OpenAgent(9))
        );
        assert_eq!(
            action_for(
                &Ref::Agent {
                    name: Some("Nobody".into()),
                    cid: None
                },
                "@Nobody",
                &c
            ),
            None
        );
        assert_eq!(
            action_for(
                &Ref::Path {
                    path: "src/main.rs".into(),
                    line: Some(4),
                    dir: false
                },
                "src/main.rs:4",
                &c
            ),
            Some(RichAction::OpenPath {
                path: "src/main.rs".into(),
                line: Some(4)
            })
        );
        assert_eq!(
            action_for(
                &Ref::Path {
                    path: "crates/".into(),
                    line: None,
                    dir: true
                },
                "crates/",
                &c
            ),
            None
        );
        assert_eq!(
            action_for(&Ref::Library { id: "d1".into() }, "/v1/files/d1", &c),
            Some(RichAction::OpenLibrary("d1".into()))
        );
        assert_eq!(action_for(&Ref::MentionAll, "@all", &c), None);
        let video = Ref::Video {
            url: "https://x.io/a.mp4".into(),
        };
        assert_eq!(
            action_for(&video, "https://x.io/a.mp4", &c),
            Some(RichAction::OpenUrl("https://x.io/a.mp4".into()))
        );
    }

    #[test]
    fn a_github_link_opens_the_page_the_author_wrote_or_the_one_it_names() {
        let c = ctx();
        let r = pull(12);
        assert_eq!(
            action_for(&r, "https://github.com/o/r/pull/12", &c),
            Some(RichAction::OpenUrl("https://github.com/o/r/pull/12".into()))
        );
        assert_eq!(
            action_for(&r, "[PR](https://github.com/o/r/pull/12)", &c),
            Some(RichAction::OpenUrl("https://github.com/o/r/pull/12".into()))
        );
        let file = Ref::GithubFile {
            owner: "o".into(),
            repo: "r".into(),
            reference: "main".into(),
            path: "a.rs".into(),
            line: Some(10),
            dir: false,
        };
        assert_eq!(
            action_for(&file, "", &c),
            Some(RichAction::OpenUrl(
                "https://github.com/o/r/blob/main/a.rs#L10".into()
            ))
        );
        let plaza = Ref::PlazaGitPull {
            owner: "o".into(),
            repo: "r".into(),
            number: 3,
        };
        assert_eq!(
            action_for(&plaza, "PR #3", &c),
            None,
            "no plaza host is known, so no page"
        );
        let with_host = Ctx {
            plaza_git_host: Some("git.example.test".into()),
            ..Ctx::default()
        };
        assert_eq!(
            action_for(&plaza, "PR #3", &with_host),
            Some(RichAction::OpenUrl(
                "https://git.example.test/o/r/pulls/3".into()
            ))
        );
    }

    #[test]
    fn the_plaza_git_host_is_the_overviews_git_host() {
        assert_eq!(
            git_host_of(&json!({"git_host": "https://git.example.test"})),
            Some("https://git.example.test".into())
        );
        assert_eq!(
            git_host_of(&json!({"git_host": " https://git.example.test/ "})),
            Some("https://git.example.test/".into()),
            "the value is trimmed"
        );
        assert_eq!(git_host_of(&json!({"git_host": null})), None);
        assert_eq!(git_host_of(&json!({"git_host": ""})), None);
        assert_eq!(
            git_host_of(&json!({"agents": {}})),
            None,
            "an older hub's overview has no git_host"
        );
    }

    #[test]
    fn a_plaza_link_is_a_plaza_reference_once_the_context_knows_the_host() {
        let cards = Cards::default();
        let url = "https://git.example.test/o/r/pulls/3";
        let known = build_context(
            &cards,
            None,
            Some("o/r"),
            None,
            Some("https://git.example.test".into()),
        );
        assert_eq!(
            known.plaza_git_host.as_deref(),
            Some("https://git.example.test")
        );
        assert_eq!(
            apz_rich::classify_url(url, &known),
            Some(Ref::PlazaGitPull {
                owner: "o".into(),
                repo: "r".into(),
                number: 3,
            }),
            "the link is a chip"
        );
        let unknown = build_context(&cards, None, Some("o/r"), None, None);
        assert!(
            matches!(
                apz_rich::classify_url(url, &unknown),
                Some(Ref::Link { .. })
            ),
            "with no host it stays an ordinary link"
        );
    }

    #[test]
    fn image_urls_are_fetched_from_the_right_place() {
        let client = Client::new("http://127.0.0.1:7700", None, Some("secret".into()));
        let (target, path) =
            image_target(&client, "http://127.0.0.1:7700/v1/files/a/content").unwrap();
        assert_eq!(path, "/v1/files/a/content");
        assert_eq!(
            target.bearer.as_deref(),
            Some("secret"),
            "the hub gets the owner's credential"
        );
        let (target, path) =
            image_target(&client, "https://img.example.test/p/a.png?v=2#x").unwrap();
        assert_eq!(target.base, "https://img.example.test");
        assert_eq!(path, "/p/a.png?v=2");
        assert!(
            target.bearer.is_none(),
            "another host never gets the credential"
        );
        assert!(
            image_target(&client, "http://img.example.test/a.png").is_err(),
            "plain http to another host is refused"
        );
        assert!(image_target(&client, "https:///a.png").is_err());
    }
}
