//! Live HTML artifacts in the desktop window. Each panel's sandboxed frame is a native webview, built into the app's
//! window through `iced_wry` on the platforms that link WebKit or WebView2; elsewhere the frame is the still card and
//! the screen offers "Open in browser". The frame loads the hub's content-origin capability URL, so it keeps the web
//! console's rules: navigation stays on that origin, new windows and downloads are refused, and links and files wait
//! for the owner's confirmation held here. The page takes the app's palette through the bridge's `__apzTheme`.

use std::path::{Path, PathBuf};

use iced::{Element, Subscription, Task, window};
use serde_json::{Map, Value};

use crate::theme;

/// Whether this build draws live frames: the platforms that link a webview (the arm64 Linux packages do not).
pub const LIVE: bool = cfg!(any(
    windows,
    target_os = "macos",
    all(target_os = "linux", target_arch = "x86_64")
));

/// The most live frames at once: the focused one and the one opened before it.
pub const LIVE_LIMIT: usize = 2;

/// The height a frame may take, in logical pixels.
const HEIGHT_MIN: f32 = 120.0;
const HEIGHT_MAX: f32 = 2000.0;

/// What the frames tell the screen that owns them, and what the screen sends them.
#[derive(Debug, Clone)]
pub enum FrameMsg {
    /// Shows this panel's artifact (at `version`) in a live frame loaded from `url`, the content origin's capability URL.
    Run {
        artifact: String,
        version: u32,
        panel: String,
        url: String,
    },
    /// A message from a frame's page, as the JSON envelope of [`envelope`].
    Ipc(String),
    /// The panel was used: its frame becomes the most recently used one.
    Focus(String),
    /// Closes this panel's frame.
    Close(String),
    /// The owner's answer to the prompt that [`Frames::prompt`] shows.
    Confirm(bool),
    /// The app's window to build a frame in, or none when there is no window (internal).
    Window(Option<window::Id>),
    /// The native webview was built, or its build failed (internal).
    Built(Result<(), String>),
}

/// The panels with a frame, least recently used first. Past [`LIVE_LIMIT`], the first one is evicted.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Recency {
    order: Vec<String>,
}

impl Recency {
    /// Marks `panel` the most recently used, and returns the panel evicted to keep the limit, if there was one.
    pub fn touch(&mut self, panel: &str) -> Option<String> {
        self.order.retain(|p| p != panel);
        self.order.push(panel.to_owned());
        (self.order.len() > LIVE_LIMIT).then(|| self.order.remove(0))
    }

    /// Forgets `panel`, when its frame goes.
    pub fn remove(&mut self, panel: &str) {
        self.order.retain(|p| p != panel);
    }
}

/// A message from an artifact page's bridge (`content_bridge.js`), parsed.
#[derive(Debug, Clone, PartialEq)]
pub enum Bridge {
    /// The page loaded; the host answers with its theme.
    Ready,
    /// The page's height in logical pixels, clamped to the range a frame may take.
    Height(f32),
    /// A link the page asks the host to open: http, https or mailto.
    Open(String),
    /// A file the page asks the host to save, as text.
    Download { name: String, text: String },
}

/// Parses one bridge message (the page's JSON body). Nothing comes back for another protocol version, an unknown kind,
/// a link the host does not open, or a malformed body.
pub fn parse_bridge(body: &str) -> Option<Bridge> {
    let value: Value = serde_json::from_str(body).ok()?;
    if value.get("apz").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    match value.get("type")?.as_str()? {
        "ready" => Some(Bridge::Ready),
        "height" => {
            let height = value.get("height")?.as_f64()?;
            height
                .is_finite()
                .then(|| Bridge::Height((height as f32).clamp(HEIGHT_MIN, HEIGHT_MAX)))
        }
        "open" => {
            let url = text("url")?;
            is_openable(&url).then_some(Bridge::Open(url))
        }
        "download" => Some(Bridge::Download {
            name: safe_name(&text("name")?),
            text: text("data")?,
        }),
        _ => None,
    }
}

/// Whether the host opens this link: web and mail links only.
fn is_openable(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:")
}

/// The file name a page asks to save under: its last path part, where each run of characters other than ASCII
/// letters, digits, `.`, `_`, `-` and spaces becomes one `_`. No leading or trailing dots or spaces, at most 120
/// characters, and `download` when nothing is left.
fn safe_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let mut cleaned = String::new();
    let mut in_gap = false;
    for c in base.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ' ') {
            cleaned.push(c);
            in_gap = false;
        } else if !in_gap {
            cleaned.push('_');
            in_gap = true;
        }
    }
    let capped: String = cleaned
        .trim()
        .trim_start_matches('.')
        .chars()
        .take(120)
        .collect();
    let trimmed = capped.trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        "download".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// The JSON a frame's message travels as between the webview and the update loop: its panel and the page's body.
pub fn envelope(panel: &str, body: &str) -> String {
    serde_json::json!({ "panel": panel, "body": body }).to_string()
}

/// Splits an [`envelope`] back into its panel and its page's body.
pub fn split_envelope(json: &str) -> Option<(String, String)> {
    let value: Value = serde_json::from_str(json).ok()?;
    let panel = value.get("panel")?.as_str()?.to_owned();
    let body = value.get("body")?.as_str()?.to_owned();
    Some((panel, body))
}

/// The custom properties a page takes from the app's palette: every colour token as `--name`, and the two font stacks.
pub fn theme_vars() -> Map<String, Value> {
    let palette = theme::palette();
    let mut vars = Map::new();
    for &name in theme::Palette::NAMES {
        if let Some(color) = palette.get(name) {
            vars.insert(format!("--{name}"), Value::String(css_color(color)));
        }
    }
    vars.insert(
        "--font".to_owned(),
        Value::String(family_stack(theme::SANS, "sans-serif")),
    );
    vars.insert(
        "--font-mono".to_owned(),
        Value::String(family_stack(theme::MONO, "monospace")),
    );
    vars
}

/// The script that hands a page its theme: `window.__apzTheme(vars, mode)`, where the mode is `light` or `dark`.
pub fn theme_script(vars: &Map<String, Value>, light: bool) -> String {
    let mode = if light { "light" } else { "dark" };
    format!(
        "window.__apzTheme({}, '{mode}');",
        Value::Object(vars.clone())
    )
}

/// A colour as CSS: hex when it is opaque, `rgba()` otherwise.
fn css_color(color: iced::Color) -> String {
    let byte = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    if color.a >= 1.0 {
        format!(
            "#{:02x}{:02x}{:02x}",
            byte(color.r),
            byte(color.g),
            byte(color.b)
        )
    } else {
        format!(
            "rgba({}, {}, {}, {:.3})",
            byte(color.r),
            byte(color.g),
            byte(color.b),
            color.a.clamp(0.0, 1.0)
        )
    }
}

/// A font as a CSS stack: its family, then the generic fallback.
fn family_stack(font: iced::Font, fallback: &str) -> String {
    match font.family {
        iced::font::Family::Name(name) => format!("'{name}', {fallback}"),
        _ => fallback.to_owned(),
    }
}

/// Where a saved file goes: the Downloads folder, or the home folder when there is none.
fn save_download(name: &str, text: &str) {
    let Some(dir) = dirs::download_dir().or_else(dirs::home_dir) else {
        eprintln!("agentplaza-desktop: no Downloads folder to save {name} in");
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let path = unique_path(&dir, &safe_name(name));
    if let Err(error) = std::fs::write(&path, text) {
        eprintln!(
            "agentplaza-desktop: saving {} failed: {error}",
            path.display()
        );
    }
}

/// The first free path for `name` in `dir`: `name`, then `name (2)`, `name (3)` and so on.
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) => (stem, format!(".{ext}")),
        None => (name, String::new()),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|path| !path.exists())
        .unwrap_or(first)
}

/// What a frame asked for and the owner has not answered yet.
#[derive(Debug, Clone, PartialEq)]
enum Request {
    Open(String),
    Download { name: String, text: String },
}

/// A request from one panel's frame, waiting for the owner's answer.
#[derive(Debug, Clone, PartialEq)]
struct Pending {
    panel: String,
    request: Request,
}

/// Where a frame is in its life: waiting for the window, being built, or live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Window,
    Building,
    Live,
}

/// One panel's frame. A frame retired while it was being built stays, marked `discard`, until its build ends, so that
/// the builds still match their results in order.
struct Slot {
    panel: String,
    artifact: String,
    version: u32,
    url: String,
    native: native::Frame,
    stage: Stage,
    discard: bool,
    /// The height the page reported, once it has.
    height: Option<f32>,
}

impl Slot {
    /// The key of this frame: its artifact, the version it shows, and its panel. Messages and lookups use it.
    fn key(&self) -> String {
        frame_key(&self.artifact, self.version, &self.panel)
    }
}

/// The key of a panel's frame at one version of an artifact.
fn frame_key(artifact: &str, version: u32, panel: &str) -> String {
    format!("{artifact}/{version}/{panel}")
}

/// The live frames of the artifact panels, and the bookkeeping the screen needs: the most recently used frames and
/// the owner's pending answer.
#[derive(Default)]
pub struct Frames {
    slots: Vec<Slot>,
    recency: Recency,
    pending: Option<Pending>,
}

impl Frames {
    /// Applies one message; the task is the window work a frame needs (building its webview).
    pub fn update(&mut self, msg: FrameMsg) -> Task<FrameMsg> {
        match msg {
            FrameMsg::Run {
                artifact,
                version,
                panel,
                url,
            } => self.run(artifact, version, panel, url),
            FrameMsg::Ipc(json) => {
                self.receive(&json);
                Task::none()
            }
            FrameMsg::Focus(panel) => {
                if self
                    .slot(&panel)
                    .is_some_and(|slot| slot.stage == Stage::Live)
                {
                    self.touch(&panel);
                }
                Task::none()
            }
            FrameMsg::Close(panel) => {
                self.retire(&panel);
                Task::none()
            }
            FrameMsg::Confirm(yes) => {
                self.confirm(yes);
                Task::none()
            }
            FrameMsg::Window(window) => self.build(window),
            FrameMsg::Built(result) => {
                self.built(result);
                Task::none()
            }
        }
    }

    /// The subscription that delivers the live frames' messages, as [`FrameMsg::Ipc`].
    pub fn subscription(&self) -> Subscription<FrameMsg> {
        Subscription::batch(self.slots.iter().filter(|slot| !slot.discard).map(|slot| {
            slot.native
                .messages()
                .with(slot.key())
                .map(|(key, body)| FrameMsg::Ipc(envelope(&key, &body)))
        }))
    }

    /// The frame of one panel at one version of an artifact, or nothing when that version has no live frame. A frame
    /// of another version never stands in for it, so a screen showing version N+1 never shows version N's page.
    pub fn view_of<'a>(
        &'a self,
        artifact: &str,
        version: u32,
        panel: &str,
        height: f32,
    ) -> Option<Element<'a, FrameMsg>> {
        let slot = self.slot_by_key(&frame_key(artifact, version, panel))?;
        let height = slot.height.unwrap_or(height).clamp(HEIGHT_MIN, HEIGHT_MAX);
        slot.native.placeholder(height)
    }

    /// Hides every live webview, or shows them again. The native views sit above iced's content, so this is called
    /// while a menu, popover or dialog is open.
    pub fn hide_all(&self, hidden: bool) {
        for slot in self
            .slots
            .iter()
            .filter(|slot| !slot.discard && slot.stage == Stage::Live)
        {
            slot.native.show(!hidden);
        }
    }

    /// Sends the app's current theme to every live frame.
    pub fn send_theme(&self) {
        let script = theme_script(&theme_vars(), theme::is_light());
        for slot in self
            .slots
            .iter()
            .filter(|slot| !slot.discard && slot.stage == Stage::Live)
        {
            slot.native.evaluate(&script);
        }
    }

    /// The question the owner must answer, when a frame asked to open a link or save a file.
    pub fn prompt(&self) -> Option<String> {
        self.pending.as_ref().map(|pending| match &pending.request {
            Request::Open(url) => format!("Open {url} in your browser?"),
            Request::Download { name, text } => {
                format!("Save {name} ({} bytes) to Downloads?", text.len())
            }
        })
    }

    /// The live frame of `panel`, if it has one that is not being retired.
    fn slot(&self, panel: &str) -> Option<&Slot> {
        self.slots
            .iter()
            .find(|slot| slot.panel == panel && !slot.discard)
    }

    /// The live frame with this key (see [`frame_key`]), if it is not being retired.
    fn slot_by_key(&self, key: &str) -> Option<&Slot> {
        self.slots
            .iter()
            .find(|slot| !slot.discard && slot.key() == key)
    }

    /// Starts a frame for the panel. The window comes back as [`FrameMsg::Window`], and then the webview is built.
    fn run(
        &mut self,
        artifact: String,
        version: u32,
        panel: String,
        url: String,
    ) -> Task<FrameMsg> {
        let unchanged = self.slot(&panel).is_some_and(|slot| {
            slot.stage == Stage::Live
                && slot.artifact == artifact
                && slot.version == version
                && slot.url == url
        });
        if unchanged {
            self.touch(&panel);
            return Task::none();
        }
        let Some(native) = native::Frame::new(&url) else {
            return Task::none();
        };
        self.retire(&panel);
        self.slots.push(Slot {
            panel: panel.clone(),
            artifact,
            version,
            url,
            native,
            stage: Stage::Window,
            discard: false,
            height: None,
        });
        self.touch(&panel);
        window::latest().map(FrameMsg::Window)
    }

    /// Marks the panel the most recently used. Past the limit, the least recently used frame is retired.
    fn touch(&mut self, panel: &str) {
        if let Some(evicted) = self.recency.touch(panel) {
            self.retire(&evicted);
        }
    }

    /// Retires the panel's frames: a live one is destroyed now, and one still being built is dropped when its build ends.
    fn retire(&mut self, panel: &str) {
        self.recency.remove(panel);
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.panel == panel)
        {
            self.pending = None;
        }
        for slot in self.slots.iter_mut().filter(|slot| slot.panel == panel) {
            slot.discard = true;
            if slot.stage == Stage::Live {
                slot.native.destroy();
            }
        }
        self.slots
            .retain(|slot| slot.stage != Stage::Live || !slot.discard);
    }

    /// Builds the webview of the first frame that waits for the window.
    fn build(&mut self, window: Option<window::Id>) -> Task<FrameMsg> {
        let Some(index) = self
            .slots
            .iter()
            .position(|slot| slot.stage == Stage::Window)
        else {
            return Task::none();
        };
        match window {
            Some(id) => {
                self.slots[index].stage = Stage::Building;
                self.slots[index].native.create(id)
            }
            None => {
                let gone = self.slots.remove(index);
                if !gone.discard {
                    self.recency.remove(&gone.panel);
                }
                Task::none()
            }
        }
    }

    /// Takes the result of the first build in progress: the webview joins its frame, or the frame is dropped.
    fn built(&mut self, result: Result<(), String>) {
        let Some(index) = self
            .slots
            .iter()
            .position(|slot| slot.stage == Stage::Building)
        else {
            return;
        };
        let adopted = result.is_ok() && self.slots[index].native.adopt();
        if !adopted {
            if let Err(error) = result {
                eprintln!("agentplaza-desktop: artifact frame did not start: {error}");
            }
            let gone = self.slots.remove(index);
            if !gone.discard {
                self.recency.remove(&gone.panel);
            }
        } else if self.slots[index].discard {
            let mut gone = self.slots.remove(index);
            gone.native.destroy();
        } else {
            self.slots[index].stage = Stage::Live;
        }
    }

    /// Handles one page message, from the JSON envelope of a frame.
    fn receive(&mut self, json: &str) {
        let Some((key, body)) = split_envelope(json) else {
            return;
        };
        let Some(bridge) = parse_bridge(&body) else {
            return;
        };
        let Some(panel) = self.slot_by_key(&key).map(|slot| slot.panel.clone()) else {
            return;
        };
        match bridge {
            Bridge::Ready => self.send_theme_to(&key),
            Bridge::Height(height) => {
                if let Some(slot) = self
                    .slots
                    .iter_mut()
                    .find(|slot| !slot.discard && slot.key() == key)
                {
                    slot.height = Some(height);
                }
            }
            Bridge::Open(url) => {
                self.pending = Some(Pending {
                    panel,
                    request: Request::Open(url),
                })
            }
            Bridge::Download { name, text } => {
                self.pending = Some(Pending {
                    panel,
                    request: Request::Download { name, text },
                });
            }
        }
    }

    /// Sends the app's theme to one live frame, by its key.
    fn send_theme_to(&self, key: &str) {
        if let Some(slot) = self
            .slot_by_key(key)
            .filter(|slot| slot.stage == Stage::Live)
        {
            slot.native
                .evaluate(&theme_script(&theme_vars(), theme::is_light()));
        }
    }

    /// Answers the pending prompt: a link opens in the system browser, a file is saved to Downloads.
    fn confirm(&mut self, yes: bool) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        if !yes {
            return;
        }
        match pending.request {
            Request::Open(url) => {
                if let Err(error) = open::that_detached(&url) {
                    eprintln!("agentplaza-desktop: opening {url} failed: {error}");
                }
            }
            Request::Download { name, text } => save_download(&name, &text),
        }
    }
}

/// The native webview of one frame, where a webview is linked.
#[cfg(any(
    windows,
    target_os = "macos",
    all(target_os = "linux", target_arch = "x86_64")
))]
mod native {
    use iced::{Element, Length, Subscription, Task, window};

    use super::FrameMsg;

    /// One native webview: the frame of one panel, built in the app's window.
    pub struct Frame(iced_wry::WebViewController);

    impl Frame {
        /// The frame for `url`, the hub's content-origin capability URL. Nothing when the URL has no origin.
        pub fn new(url: &str) -> Option<Self> {
            let origin = origin_of(url)?;
            let config = iced_wry::WebViewConfig::default()
                .url(url)
                .devtools(false)
                .customize(move |builder| {
                    let taken = std::mem::replace(builder, wry::WebViewBuilder::new());
                    *builder = taken
                        // Private storage: nothing the page keeps outlives the frame.
                        .with_incognito(true)
                        // The frame stays on its own origin; any other page is left where it is.
                        .with_navigation_handler(move |next| same_origin(&origin, &next))
                        // A new window is refused; the page's links go through the bridge's `open` message instead.
                        .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
                        // Downloads go through the bridge's `download` message, which asks the owner first.
                        .with_download_started_handler(|_, _| false);
                });
            Some(Self(iced_wry::WebViewController::new(config)))
        }

        /// Starts building the webview in `window`; the result returns as [`FrameMsg::Built`].
        pub fn create(&mut self, window: window::Id) -> Task<FrameMsg> {
            self.0.create_task(window, FrameMsg::Built)
        }

        /// Takes the built webview out of its staging. False when the build left none.
        pub fn adopt(&mut self) -> bool {
            self.0.take_staged();
            self.0.is_active()
        }

        /// Shows or hides the webview.
        pub fn show(&self, visible: bool) {
            self.0.set_visible(visible);
        }

        /// Runs a script in the page.
        pub fn evaluate(&self, script: &str) {
            self.0.evaluate_script(script);
        }

        /// Destroys the webview.
        pub fn destroy(&mut self) {
            self.0.destroy();
        }

        /// The page's messages, as their JSON bodies.
        pub fn messages(&self) -> Subscription<String> {
            self.0.ipc_subscription().map(|message| message.body)
        }

        /// The placeholder that reserves the frame's space and carries its bounds to the webview.
        pub fn placeholder<'a>(&'a self, height: f32) -> Option<Element<'a, FrameMsg>> {
            Some(
                iced_wry::webview::<FrameMsg>(&self.0)
                    .width(Length::Fill)
                    .height(Length::Fixed(height))
                    .into(),
            )
        }
    }

    /// The scheme and authority of `url` (`http://127.0.0.1:7791`): the origin a frame may stay on.
    fn origin_of(url: &str) -> Option<String> {
        let (scheme, rest) = url.split_once("://")?;
        let authority = rest.split(['/', '?', '#']).next()?;
        (!scheme.is_empty() && !authority.is_empty()).then(|| {
            format!(
                "{}://{}",
                scheme.to_ascii_lowercase(),
                authority.to_ascii_lowercase()
            )
        })
    }

    /// Whether a navigation to `next` may happen in a frame of `origin`: its own origin, or the blank page it starts on.
    fn same_origin(origin: &str, next: &str) -> bool {
        next.starts_with("about:blank") || origin_of(next).as_deref() == Some(origin)
    }

    #[cfg(test)]
    mod tests {
        use super::{origin_of, same_origin};

        #[test]
        fn origin_is_scheme_and_authority() {
            assert_eq!(
                origin_of("http://127.0.0.1:7791/v1/artifacts/a/frame?v=1").as_deref(),
                Some("http://127.0.0.1:7791")
            );
            assert_eq!(origin_of("not a url"), None);
        }

        #[test]
        fn navigation_stays_on_the_frame_origin() {
            let origin = "http://127.0.0.1:7791";
            assert!(same_origin(origin, "http://127.0.0.1:7791/v1/other"));
            assert!(same_origin(origin, "about:blank"));
            assert!(!same_origin(origin, "https://evil.example/"));
            assert!(!same_origin(origin, "http://127.0.0.1:9999/"));
        }
    }
}

/// Without a linked webview the frame is never made: the screen keeps the still card.
#[cfg(not(any(
    windows,
    target_os = "macos",
    all(target_os = "linux", target_arch = "x86_64")
)))]
#[allow(dead_code)]
mod native {
    use iced::{Element, Subscription, Task, window};

    use super::FrameMsg;

    /// Stands in for the native webview where none is linked.
    pub struct Frame;

    impl Frame {
        pub fn new(_url: &str) -> Option<Self> {
            None
        }

        pub fn create(&mut self, _window: window::Id) -> Task<FrameMsg> {
            Task::none()
        }

        pub fn adopt(&mut self) -> bool {
            false
        }

        pub fn show(&self, _visible: bool) {}

        pub fn evaluate(&self, _script: &str) {}

        pub fn destroy(&mut self) {}

        pub fn messages(&self) -> Subscription<String> {
            Subscription::none()
        }

        pub fn placeholder<'a>(&'a self, _height: f32) -> Option<Element<'a, FrameMsg>> {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recency_evicts_the_least_recently_used_past_the_limit() {
        let mut recency = Recency::default();
        assert_eq!(recency.touch("a"), None);
        assert_eq!(recency.touch("b"), None);
        assert_eq!(recency.touch("c"), Some("a".to_owned()));
        assert_eq!(recency.order, ["b", "c"]);
        assert_eq!(recency.touch("b"), None);
        assert_eq!(recency.order, ["c", "b"]);
        recency.remove("c");
        assert_eq!(recency.order, ["b"]);
    }

    #[test]
    fn bridge_messages_parse() {
        assert_eq!(
            parse_bridge(r#"{"apz":1,"type":"ready"}"#),
            Some(Bridge::Ready)
        );
        assert_eq!(
            parse_bridge(r#"{"apz":1,"type":"height","height":300}"#),
            Some(Bridge::Height(300.0))
        );
        assert_eq!(
            parse_bridge(r#"{"apz":1,"type":"height","height":50}"#),
            Some(Bridge::Height(120.0))
        );
        assert_eq!(
            parse_bridge(r#"{"apz":1,"type":"height","height":9999}"#),
            Some(Bridge::Height(2000.0))
        );
        assert_eq!(
            parse_bridge(r#"{"apz":1,"type":"open","url":"https://example.com/a"}"#),
            Some(Bridge::Open("https://example.com/a".to_owned()))
        );
        assert_eq!(
            parse_bridge(
                r#"{"apz":1,"type":"download","name":"../../etc/notes.csv","mime":"text/csv","data":"a,b"}"#
            ),
            Some(Bridge::Download {
                name: "notes.csv".to_owned(),
                text: "a,b".to_owned()
            })
        );
    }

    #[test]
    fn bridge_rejects_what_the_host_must_not_act_on() {
        assert_eq!(
            parse_bridge(r#"{"apz":1,"type":"open","url":"javascript:alert(1)"}"#),
            None
        );
        assert_eq!(parse_bridge(r#"{"apz":2,"type":"ready"}"#), None);
        assert_eq!(parse_bridge(r#"{"type":"ready"}"#), None);
        assert_eq!(parse_bridge(r#"{"apz":1,"type":"unknown"}"#), None);
        assert_eq!(parse_bridge("not json"), None);
    }

    #[test]
    fn envelope_round_trips() {
        let json = envelope("chart-1", r#"{"apz":1,"type":"ready"}"#);
        assert_eq!(
            split_envelope(&json),
            Some((
                "chart-1".to_owned(),
                r#"{"apz":1,"type":"ready"}"#.to_owned()
            ))
        );
        assert_eq!(split_envelope("{}"), None);
    }

    #[test]
    fn theme_script_hands_over_the_vars_and_the_mode() {
        let mut vars = Map::new();
        vars.insert("--ink".to_owned(), Value::String("#ffffff".to_owned()));
        assert_eq!(
            theme_script(&vars, true),
            r##"window.__apzTheme({"--ink":"#ffffff"}, 'light');"##
        );
        assert_eq!(
            theme_script(&vars, false),
            r##"window.__apzTheme({"--ink":"#ffffff"}, 'dark');"##
        );
    }

    #[test]
    fn saved_names_are_plain_file_names() {
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name(".hidden"), "hidden");
        assert_eq!(safe_name(""), "download");
        assert_eq!(safe_name("C:evil.txt"), "C_evil.txt");
        assert_eq!(safe_name("a::b:c.csv"), "a_b_c.csv");
        assert_eq!(safe_name("..\\..\\x.txt"), "x.txt");
        assert_eq!(safe_name("..."), "download");
        assert_eq!(safe_name("name. "), "name");
        assert_eq!(safe_name(&"a".repeat(200)).len(), 120);
    }
}
