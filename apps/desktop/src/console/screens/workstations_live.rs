//! The live view of a workstation's screen, and the owner's takeover of it (docs/design/workstations.md, sections 7
//! and 9). The picture is `WS /v1/stations/{id}/screen` (subprotocol `apz.station.screen.v1`): a binary frame is a
//! 4-byte big-endian header length, the JSON header `{w,h,format,ts,seq}`, then the image; the text messages are
//! `started`, `ended`, `error` and `pong`. The takeover is `WS /v1/stations/{id}/control` (`apz.station.control.v1`):
//! the owner sends `take`, `input` and `give_back`; the hub answers `taken`, `given_back`, `error` and `ended`.
//!
//! Each socket runs on its own thread, as the hub's event stream does (`console/events.rs`), and hands what it reads
//! to the screen as messages. A frame the screen is too slow for is dropped; a control answer is never dropped.
//! While the owner holds the screen, the pointer over the picture and the keys (when no field on the page has
//! them) become `InputAction`s (crates/apz-station/src/input.rs), in the picture's own pixels.

use std::hash::{Hash, Hasher};
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use apz_http::client::Target;
use apz_http::ws::{self, Limits, Message, Socket};
use iced::futures::channel::mpsc;
use iced::futures::{SinkExt, Stream, StreamExt};
use iced::keyboard::key::Named;
use iced::keyboard::{self, Key};
use iced::widget::{column, container, image, mouse_area, pick_list, responsive, row};
use iced::{ContentFit, Element, Event, Length, Point, Size, Subscription, Task, event};
use serde_json::{Value, json};

use super::workstations::{Station, Takeover, takeover_text};
use crate::console::ui;
use crate::theme;

const SCREEN_PROTOCOL: &str = "apz.station.screen.v1";
const CONTROL_PROTOCOL: &str = "apz.station.control.v1";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long one read may wait, so a thread notices when its screen has gone.
const RECV_WAIT: Duration = Duration::from_millis(60);
const MIN_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(16);
/// Messages held between a thread and the screen; a frame beyond them is dropped.
const BUFFER: usize = 8;
/// The pointer's moves the owner sends at most this often (a press always sends where the pointer is).
const MOVE_EVERY: Duration = Duration::from_millis(50);
/// Pixels of a wheel's pixel delta that make one notch.
const PIXELS_PER_NOTCH: f32 = 40.0;
/// The rates and scales the pickers offer. A frame is one message, so the socket limits match the hub's.
pub const FPS_CHOICES: [FpsPick; 5] =
    [FpsPick(1), FpsPick(2), FpsPick(5), FpsPick(10), FpsPick(15)];
pub const SCALE_CHOICES: [ScalePick; 3] = [ScalePick(0.25), ScalePick(0.5), ScalePick(1.0)];
const LIMITS: Limits = Limits {
    max_frame: 16 << 20,
    max_message: 16 << 20,
    ping_every: Duration::from_secs(20),
    dead_after: Duration::from_secs(45),
};

/// A decoded frame: the picture, its size in pixels, and its sequence number.
#[derive(Debug, Clone)]
pub struct Shown {
    pub handle: image::Handle,
    pub w: u32,
    pub h: u32,
    pub seq: u64,
}

/// One binary frame, split: its size, sequence number and image bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub seq: u64,
    pub image: Vec<u8>,
}

#[derive(serde::Deserialize)]
struct Header {
    w: u32,
    h: u32,
    format: String,
    #[serde(default)]
    seq: u64,
}

/// Splits one binary frame: 4 bytes of big-endian header length, the JSON header, then the image. A frame with a
/// header that is not JSON, a size of zero, an unknown format or no image is refused.
pub fn parse_frame(bytes: &[u8]) -> Result<Frame, String> {
    let length: [u8; 4] = bytes
        .get(..4)
        .and_then(|head| head.try_into().ok())
        .ok_or("the frame is shorter than its header length")?;
    let end = 4 + u32::from_be_bytes(length) as usize;
    let head = bytes
        .get(4..end)
        .ok_or("the frame's header runs past its end")?;
    let header: Header =
        serde_json::from_slice(head).map_err(|e| format!("the frame's header is not JSON: {e}"))?;
    if header.w == 0 || header.h == 0 {
        return Err("the frame has no size".into());
    }
    if !matches!(header.format.as_str(), "png" | "jpeg") {
        return Err(format!(
            "the frame's format {:?} is not png or jpeg",
            header.format
        ));
    }
    let image = bytes[end..].to_vec();
    if image.is_empty() {
        return Err("the frame has no image".into());
    }
    Ok(Frame {
        w: header.w,
        h: header.h,
        seq: header.seq,
        image,
    })
}

/// The link to the picture: connecting, live, reconnecting after a loss (with the reason), or ended by the hub.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Link {
    #[default]
    Connecting,
    Live,
    Reconnecting(String),
    Ended(String),
}

/// A session the picture can show: `shared` (the owner's own screen) or an agent's pixel session by its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

impl std::fmt::Display for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

impl Choice {
    fn shared() -> Choice {
        Choice {
            value: "shared".into(),
            label: "The owner's screen (shared)".into(),
        }
    }
}

/// A frame rate the picker offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FpsPick(pub u8);

impl std::fmt::Display for FpsPick {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} fps", self.0)
    }
}

/// A scale the picker offers: the fraction of the screen the frames keep.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScalePick(pub f32);

impl std::fmt::Display for ScalePick {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:.0}%", self.0 * 100.0)
    }
}

/// A mouse button the owner pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    Left,
    Right,
    Middle,
}

impl Press {
    fn name(self) -> &'static str {
        match self {
            Press::Left => "left",
            Press::Right => "right",
            Press::Middle => "middle",
        }
    }
}

/// A key as the owner pressed it: a named key or a character, the text it typed, and the modifiers held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyName {
    /// A key's name for the daemon's chords (`enter`, `esc`, `up`, `f5`, …).
    Named(&'static str),
    Char(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub logo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyIn {
    pub key: KeyName,
    pub text: Option<String>,
    pub mods: Mods,
}

/// What the screen's stream reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    Started,
    /// The hub ended the session; the picture does not come back by itself.
    Ended(String),
    /// A message the hub refused; the stream goes on.
    Error(String),
    /// The socket failed; the thread tries again.
    Lost(String),
}

/// What the takeover socket reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlEvent {
    /// The hub gave the owner the session, under this policy (the hub's name for it).
    Taken(String),
    /// The hub refused a frame: the take itself, or one input.
    Refused(String),
    GivenBack,
    /// The socket closed, with the reason to show (empty when there is none).
    Closed(String),
}

/// A command for the takeover thread.
#[derive(Debug)]
pub enum Cmd {
    Input(Value),
    GiveBack,
}

/// The takeover thread's command channel, handed to the screen once the thread runs.
#[derive(Clone)]
pub struct CmdTx(std_mpsc::Sender<Cmd>);

impl std::fmt::Debug for CmdTx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CmdTx")
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    Shown(Shown),
    Stream(StreamEvent),
    Pick(Choice),
    FpsPicked(FpsPick),
    ScalePicked(ScalePick),
    Reconnect,
    Take,
    GiveBack,
    Control(ControlEvent),
    ControlTx(CmdTx),
    /// The pointer over the picture, in the picture's pixels.
    Pointer(i32, i32),
    Press(Press),
    Release(Press),
    /// A wheel turn: its horizontal and vertical amounts, and whether they are pixels (else lines).
    Wheel(f32, f32, bool),
    Key(KeyIn),
}

/// The live view's state, for one station's page.
#[derive(Debug)]
pub struct State {
    pub station: String,
    pub choice: Choice,
    /// The sessions the station offers now: the owner's screen, then each agent's pixel session.
    pub sessions: Vec<Choice>,
    pub fps: u8,
    pub scale: f32,
    pub frame: Option<Shown>,
    pub link: Link,
    /// Bumped by Reconnect: the picture's stream starts over.
    generation: u64,
    /// Bumped by each Take: a new takeover socket starts.
    take_seq: u64,
    /// The owner has asked for the screen; its socket runs until it is given back.
    taking: bool,
    /// The hub has given the owner the screen, under this policy.
    taken: Option<Takeover>,
    cmd: Option<CmdTx>,
    pointer: (i32, i32),
    last_move: Option<Instant>,
    /// The last problem to show under the picture.
    pub notice: Option<String>,
}

impl State {
    pub fn new(station: &str) -> State {
        State {
            station: station.to_string(),
            choice: Choice::shared(),
            sessions: vec![Choice::shared()],
            fps: 5,
            scale: 0.5,
            frame: None,
            link: Link::Connecting,
            generation: 0,
            take_seq: 0,
            taking: false,
            taken: None,
            cmd: None,
            pointer: (0, 0),
            last_move: None,
            notice: None,
        }
    }

    /// Takes the station's sessions from its leases: a pixel lease's session is one to view. A session that is gone
    /// falls back to the owner's screen.
    pub fn sync(&mut self, station: &Station) {
        self.sessions = std::iter::once(Choice::shared())
            .chain(station.leases.iter().filter_map(|l| {
                let session = l.session?;
                (l.kind == "pixel").then(|| Choice {
                    value: session.to_string(),
                    label: format!("Agent session {session}"),
                })
            }))
            .collect();
        self.choice = self
            .sessions
            .iter()
            .find(|c| c.value == self.choice.value)
            .cloned()
            .unwrap_or_else(Choice::shared);
    }

    /// Whether the session the owner picked is one the station still offers.
    fn offered(&self) -> bool {
        self.sessions.iter().any(|c| c.value == self.choice.value)
    }

    /// Whether the owner holds the screen now.
    pub fn holding(&self) -> bool {
        self.taken.is_some()
    }

    /// Ends the takeover's socket with the screen: the hub gives the session back when it closes.
    pub fn leave(&mut self) {
        reset_take(self);
    }
}

impl Default for State {
    fn default() -> State {
        State::new("")
    }
}

/// The picture's stream: its station, session and what it asks, and a generation for Reconnect.
#[derive(Clone)]
struct ScreenFeed {
    target: Target,
    station: String,
    session: String,
    fps: u8,
    scale: f32,
    generation: u64,
}

impl Hash for ScreenFeed {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.target.base.hash(state);
        self.station.hash(state);
        self.session.hash(state);
        self.fps.hash(state);
        self.scale.to_bits().hash(state);
        self.generation.hash(state);
    }
}

/// The takeover socket's identity: a station, a session, and the take it belongs to.
#[derive(Clone)]
struct ControlFeed {
    target: Target,
    station: String,
    session: String,
    take: u64,
}

impl Hash for ControlFeed {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.target.base.hash(state);
        self.station.hash(state);
        self.session.hash(state);
        self.take.hash(state);
    }
}

/// The view's subscriptions: the picture while the page shows a station that is not revoked, the takeover socket
/// while the owner has asked for the screen, and the keys while the owner holds it.
pub fn subscription(state: &State, target: &Target, on: bool) -> Subscription<Msg> {
    let mut subs = Vec::new();
    if on && !state.station.is_empty() {
        subs.push(Subscription::run_with(
            ScreenFeed {
                target: target.clone(),
                station: state.station.clone(),
                session: state.choice.value.clone(),
                fps: state.fps,
                scale: state.scale,
                generation: state.generation,
            },
            screen,
        ));
    }
    if on && state.taking {
        subs.push(Subscription::run_with(
            ControlFeed {
                target: target.clone(),
                station: state.station.clone(),
                session: state.choice.value.clone(),
                take: state.take_seq,
            },
            control,
        ));
    }
    if state.holding() {
        subs.push(event::listen_with(keys));
    }
    Subscription::batch(subs)
}

fn screen(feed: &ScreenFeed) -> impl Stream<Item = Msg> + use<> {
    let feed = feed.clone();
    iced::stream::channel(BUFFER, move |mut out: mpsc::Sender<Msg>| async move {
        let (mut tx, mut rx) = mpsc::channel::<Msg>(BUFFER);
        std::thread::spawn(move || run_screen(&feed, &mut tx));
        while let Some(msg) = rx.next().await {
            if out.send(msg).await.is_err() {
                break;
            }
        }
    })
}

fn control(feed: &ControlFeed) -> impl Stream<Item = Msg> + use<> {
    let feed = feed.clone();
    iced::stream::channel(BUFFER, move |mut out: mpsc::Sender<Msg>| async move {
        let (mut tx, mut rx) = mpsc::channel::<Msg>(BUFFER);
        std::thread::spawn(move || run_control(&feed, &mut tx));
        while let Some(msg) = rx.next().await {
            if out.send(msg).await.is_err() {
                break;
            }
        }
    })
}

/// Hands a message to the screen. A lossy message (a frame) is dropped when the screen is behind; any other waits for
/// room. False once the screen has gone.
fn deliver(out: &mut mpsc::Sender<Msg>, mut msg: Msg, lossy: bool) -> bool {
    loop {
        match out.try_send(msg) {
            Ok(()) => return true,
            Err(e) if e.is_disconnected() => return false,
            Err(_) if lossy => return true,
            Err(e) => {
                msg = e.into_inner();
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

/// Sleeps `total` in short steps; false when the screen has gone meanwhile.
fn pause(out: &mpsc::Sender<Msg>, total: Duration) -> bool {
    let end = Instant::now() + total;
    while !out.is_closed() {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        std::thread::sleep(left.min(Duration::from_millis(100)));
    }
    false
}

/// The picture's thread: connects, reads, and reconnects with backoff until the hub ends the session or the screen
/// goes.
fn run_screen(feed: &ScreenFeed, out: &mut mpsc::Sender<Msg>) {
    let path = format!(
        "/v1/stations/{}/screen?session={}&fps={}&scale={}",
        feed.station, feed.session, feed.fps, feed.scale
    );
    let mut backoff = MIN_BACKOFF;
    loop {
        let problem = match ws::connect(
            &feed.target,
            &path,
            Some(SCREEN_PROTOCOL),
            CONNECT_TIMEOUT,
            LIMITS,
        ) {
            Ok(mut socket) => match read_screen(&mut socket, out) {
                Ok(()) => return,
                Err(problem) => problem,
            },
            Err(e) => e.to_string(),
        };
        if !deliver(out, Msg::Stream(StreamEvent::Lost(problem)), false) || !pause(out, backoff) {
            return;
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Reads one picture socket until the hub ends the session (Ok) or the socket fails (Err, with the reason).
fn read_screen(socket: &mut Socket, out: &mut mpsc::Sender<Msg>) -> Result<(), String> {
    loop {
        if out.is_closed() {
            return Ok(());
        }
        match socket.recv(RECV_WAIT).map_err(|e| e.to_string())? {
            Some(Message::Binary(bytes)) => {
                if let Ok(frame) = parse_frame(&bytes) {
                    let shown = Shown {
                        handle: image::Handle::from_bytes(frame.image),
                        w: frame.w,
                        h: frame.h,
                        seq: frame.seq,
                    };
                    if !deliver(out, Msg::Shown(shown), true) {
                        return Ok(());
                    }
                }
            }
            Some(Message::Text(text)) => {
                if let Some(event) = stream_text(&text) {
                    let ended = matches!(event, StreamEvent::Ended(_));
                    if !deliver(out, Msg::Stream(event), false) || ended {
                        return Ok(());
                    }
                }
            }
            None => {}
        }
    }
}

/// The picture's text messages: started, ended and error; a pong is nothing to the screen.
pub fn stream_text(text: &str) -> Option<StreamEvent> {
    let value: Value = serde_json::from_str(text).ok()?;
    match value.get("t")?.as_str()? {
        "started" => Some(StreamEvent::Started),
        "ended" => Some(StreamEvent::Ended(reason_of(&value, "The session ended."))),
        "error" => Some(StreamEvent::Error(reason_of(
            &value,
            "The workstation refused the view.",
        ))),
        _ => None,
    }
}

fn reason_of(value: &Value, fallback: &str) -> String {
    value
        .get("reason")
        .or_else(|| value.get("error"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

/// The takeover thread: asks for the session, sends the owner's inputs, and gives the session back. Its socket closes
/// when the thread ends, which gives back what it holds.
fn run_control(feed: &ControlFeed, out: &mut mpsc::Sender<Msg>) {
    let (cmd_tx, cmd_rx) = std_mpsc::channel::<Cmd>();
    if !deliver(out, Msg::ControlTx(CmdTx(cmd_tx)), false) {
        return;
    }
    let path = format!(
        "/v1/stations/{}/control?session={}",
        feed.station, feed.session
    );
    let fail = |out: &mut mpsc::Sender<Msg>, text: String| {
        deliver(out, Msg::Control(ControlEvent::Refused(text)), false);
    };
    let mut socket = match ws::connect(
        &feed.target,
        &path,
        Some(CONTROL_PROTOCOL),
        CONNECT_TIMEOUT,
        LIMITS,
    ) {
        Ok(socket) => socket,
        Err(e) => return fail(out, e.to_string()),
    };
    if let Err(e) = socket.send(&Message::Text(json!({"t": "take"}).to_string())) {
        return fail(out, e.to_string());
    }
    loop {
        if out.is_closed() {
            return;
        }
        while let Ok(cmd) = cmd_rx.try_recv() {
            let frame = match cmd {
                Cmd::Input(action) => json!({"t": "input", "action": action}),
                Cmd::GiveBack => json!({"t": "give_back"}),
            };
            if let Err(e) = socket.send(&Message::Text(frame.to_string())) {
                deliver(
                    out,
                    Msg::Control(ControlEvent::Closed(e.to_string())),
                    false,
                );
                return;
            }
        }
        match socket.recv(RECV_WAIT) {
            Ok(Some(Message::Text(text))) => match control_text(&text) {
                Some(ControlEvent::GivenBack) => {
                    deliver(out, Msg::Control(ControlEvent::GivenBack), false);
                    socket.close(1000, "");
                    return;
                }
                Some(event @ ControlEvent::Closed(_)) => {
                    deliver(out, Msg::Control(event), false);
                    return;
                }
                Some(event) => {
                    deliver(out, Msg::Control(event), false);
                }
                None => {}
            },
            Ok(_) => {}
            Err(e) => {
                deliver(
                    out,
                    Msg::Control(ControlEvent::Closed(e.to_string())),
                    false,
                );
                return;
            }
        }
    }
}

/// The takeover socket's text messages: taken, given_back, error and ended; a pong is nothing to the screen.
pub fn control_text(text: &str) -> Option<ControlEvent> {
    let value: Value = serde_json::from_str(text).ok()?;
    match value.get("t")?.as_str()? {
        "taken" => Some(ControlEvent::Taken(
            value
                .get("policy")
                .and_then(Value::as_str)
                .unwrap_or("agent_waits")
                .to_string(),
        )),
        "given_back" => Some(ControlEvent::GivenBack),
        "error" => Some(ControlEvent::Refused(reason_of(
            &value,
            "The workstation refused the input.",
        ))),
        "ended" => Some(ControlEvent::Closed(reason_of(
            &value,
            "The session ended.",
        ))),
        _ => None,
    }
}

/// The policy the hub named, as the takeover policy; an unknown name is the default.
pub fn policy_of(name: &str) -> Takeover {
    serde_json::from_value(Value::String(name.to_string())).unwrap_or_default()
}

/// The key events the owner's keyboard sends while it holds the screen: only those no widget of the page took.
fn keys(event: Event, status: event::Status, _window: iced::window::Id) -> Option<Msg> {
    let Event::Keyboard(keyboard::Event::KeyPressed {
        key,
        text,
        modifiers,
        ..
    }) = event
    else {
        return None;
    };
    if status != event::Status::Ignored {
        return None;
    }
    let key = match key {
        Key::Named(named) => KeyName::Named(named_name(named)?),
        Key::Character(c) => KeyName::Char(c.chars().next().filter(|_| c.chars().count() == 1)?),
        Key::Unidentified => return None,
    };
    let mods = Mods {
        ctrl: modifiers.control(),
        shift: modifiers.shift(),
        alt: modifiers.alt(),
        logo: modifiers.logo(),
    };
    Some(Msg::Key(KeyIn {
        key,
        text: text.map(|t| t.to_string()),
        mods,
    }))
}

/// A named key's chord name for the daemon, or None for a key the owner cannot send.
pub fn named_name(named: Named) -> Option<&'static str> {
    Some(match named {
        Named::Enter => "enter",
        Named::Tab => "tab",
        Named::Space => "space",
        Named::Escape => "esc",
        Named::Backspace => "backspace",
        Named::Delete => "delete",
        Named::Insert => "insert",
        Named::Home => "home",
        Named::End => "end",
        Named::PageUp => "pageup",
        Named::PageDown => "pagedown",
        Named::ArrowUp => "up",
        Named::ArrowDown => "down",
        Named::ArrowLeft => "left",
        Named::ArrowRight => "right",
        Named::F1 => "f1",
        Named::F2 => "f2",
        Named::F3 => "f3",
        Named::F4 => "f4",
        Named::F5 => "f5",
        Named::F6 => "f6",
        Named::F7 => "f7",
        Named::F8 => "f8",
        Named::F9 => "f9",
        Named::F10 => "f10",
        Named::F11 => "f11",
        Named::F12 => "f12",
        _ => return None,
    })
}

/// The `key` or `type` action for one key press: a chord (with ctrl, alt, the logo key, or shift with a named key)
/// is a `key` action; otherwise the text typed is a `type` action; a named key alone is a `key` action. None when
/// the daemon could not name the key.
pub fn key_input(key: &KeyName, text: Option<&str>, mods: Mods) -> Option<Value> {
    let chord =
        mods.ctrl || mods.alt || mods.logo || (mods.shift && matches!(key, KeyName::Named(_)));
    if chord {
        let name = match key {
            KeyName::Named(name) => (*name).to_string(),
            KeyName::Char(c) if c.is_ascii_alphanumeric() || "-=[];'\\,./`".contains(*c) => {
                c.to_ascii_lowercase().to_string()
            }
            KeyName::Char(_) => return None,
        };
        let mut parts: Vec<&str> = Vec::new();
        if mods.ctrl {
            parts.push("ctrl");
        }
        if mods.shift {
            parts.push("shift");
        }
        if mods.alt {
            parts.push("alt");
        }
        if mods.logo {
            parts.push("cmd");
        }
        parts.push(&name);
        return Some(json!({"action": "key", "combo": parts.join("+")}));
    }
    if let Some(text) = text.filter(|t| !t.is_empty() && t.chars().all(|c| !c.is_control())) {
        return Some(json!({"action": "type", "text": text}));
    }
    match key {
        KeyName::Named(name) => Some(json!({"action": "key", "combo": name})),
        KeyName::Char(c) => Some(json!({"action": "type", "text": c.to_string()})),
    }
}

/// A wheel turn as the hub's scroll notches: `dx` (right is positive) and `dy` (down is positive). iced counts up as
/// positive, so the vertical amount turns over. A pixel delta is worth [`PIXELS_PER_NOTCH`] each notch, and any
/// movement is at least one notch.
pub fn wheel_notches(x: f32, y: f32, pixels: bool) -> (i32, i32) {
    fn notches(v: f32, pixels: bool) -> i32 {
        let n = if pixels { v / PIXELS_PER_NOTCH } else { v };
        let whole = n.round() as i32;
        if whole == 0 && n != 0.0 {
            n.signum() as i32
        } else {
            whole
        }
    }
    (notches(x, pixels), -notches(y, pixels))
}

/// A frame drawn to fit a box, centred: its width and height, and the left and top of the drawing in the box.
pub fn fit(boxed: (f32, f32), frame: (u32, u32)) -> (f32, f32, f32, f32) {
    let (bw, bh) = boxed;
    let (fw, fh) = (frame.0.max(1) as f32, frame.1.max(1) as f32);
    let scale = (bw / fw).min(bh / fh).max(0.0);
    let (dw, dh) = (fw * scale, fh * scale);
    (dw, dh, (bw - dw) / 2.0, (bh - dh) / 2.0)
}

/// A point in the box, as the frame's own pixels: clamped to the frame, so a point in the margin is at its edge.
pub fn frame_point(p: Point, boxed: (f32, f32), frame: (u32, u32)) -> (i32, i32) {
    let (dw, dh, left, top) = fit(boxed, frame);
    if dw <= 0.0 || dh <= 0.0 {
        return (0, 0);
    }
    let x = ((p.x - left) / dw * frame.0 as f32)
        .round()
        .clamp(0.0, frame.0.saturating_sub(1) as f32);
    let y = ((p.y - top) / dh * frame.1 as f32)
        .round()
        .clamp(0.0, frame.1.saturating_sub(1) as f32);
    (x as i32, y as i32)
}

/// Sends one input to the takeover thread, when the owner holds the screen.
fn send(state: &State, action: Value) {
    if let Some(cmd) = &state.cmd {
        let _ = cmd.0.send(Cmd::Input(action));
    }
}

fn reset_take(state: &mut State) {
    state.taking = false;
    state.taken = None;
    state.cmd = None;
    state.last_move = None;
}

pub fn update(state: &mut State, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Shown(shown) => {
            state.link = Link::Live;
            state.frame = Some(shown);
        }
        Msg::Stream(event) => match event {
            StreamEvent::Started => state.link = Link::Live,
            StreamEvent::Ended(reason) => state.link = Link::Ended(reason),
            StreamEvent::Error(error) => state.notice = Some(error),
            StreamEvent::Lost(problem) => state.link = Link::Reconnecting(problem),
        },
        Msg::Pick(choice) => {
            if !state.taking && choice != state.choice {
                state.choice = choice;
                state.frame = None;
                state.link = Link::Connecting;
            }
        }
        Msg::FpsPicked(FpsPick(fps)) => state.fps = fps,
        Msg::ScalePicked(ScalePick(scale)) => state.scale = scale,
        Msg::Reconnect => {
            state.generation += 1;
            state.link = Link::Connecting;
        }
        Msg::Take => {
            if !state.taking && state.offered() {
                state.taking = true;
                state.take_seq += 1;
                state.taken = None;
                state.cmd = None;
                state.notice = None;
            }
        }
        Msg::GiveBack => match &state.cmd {
            Some(cmd) => {
                let _ = cmd.0.send(Cmd::GiveBack);
            }
            None => state.taking = false,
        },
        Msg::Control(event) => match event {
            ControlEvent::Taken(policy) => {
                state.taken = Some(policy_of(&policy));
                state.notice = None;
            }
            ControlEvent::Refused(error) => {
                if state.taken.is_none() {
                    reset_take(state);
                }
                state.notice = Some(error);
            }
            ControlEvent::GivenBack => {
                reset_take(state);
                state.notice = Some("You gave the screen back.".into());
            }
            ControlEvent::Closed(reason) => {
                reset_take(state);
                state.notice = (!reason.is_empty()).then_some(reason);
            }
        },
        Msg::ControlTx(tx) => state.cmd = Some(tx),
        Msg::Pointer(x, y) => {
            state.pointer = (x, y);
            if state.holding()
                && state
                    .last_move
                    .is_none_or(|t: Instant| t.elapsed() >= MOVE_EVERY)
            {
                state.last_move = Some(Instant::now());
                send(state, json!({"action": "move", "x": x, "y": y}));
            }
        }
        Msg::Press(button) => {
            if state.holding() {
                let (x, y) = state.pointer;
                state.last_move = Some(Instant::now());
                send(state, json!({"action": "move", "x": x, "y": y}));
                send(state, json!({"action": "down", "button": button.name()}));
            }
        }
        Msg::Release(button) => {
            if state.holding() {
                send(state, json!({"action": "up", "button": button.name()}));
            }
        }
        Msg::Wheel(x, y, pixels) => {
            if state.holding() {
                let (px, py) = state.pointer;
                let (dx, dy) = wheel_notches(x, y, pixels);
                send(
                    state,
                    json!({"action": "scroll", "x": px, "y": py, "dx": dx, "dy": dy}),
                );
            }
        }
        Msg::Key(key) => {
            if state.holding()
                && let Some(action) = key_input(&key.key, key.text.as_deref(), key.mods)
            {
                send(state, action);
            }
        }
    }
    Task::none()
}

/// The picture's box: a fixed height, the page's width.
const PICTURE_HEIGHT: f32 = 420.0;

pub fn view<'a>(state: &'a State, station: &'a Station) -> Element<'a, Msg> {
    let live = !station.revoked;
    let mut controls: Vec<Element<'a, Msg>> = vec![
        pick_list(
            state.sessions.clone(),
            Some(state.choice.clone()),
            Msg::Pick,
        )
        .style(theme::pick_list)
        .text_size(13.0)
        .width(Length::Fixed(240.0))
        .into(),
        pick_list(
            FPS_CHOICES.to_vec(),
            Some(FpsPick(state.fps)),
            Msg::FpsPicked,
        )
        .style(theme::pick_list)
        .text_size(13.0)
        .width(Length::Fixed(96.0))
        .into(),
        pick_list(
            SCALE_CHOICES.to_vec(),
            Some(ScalePick(state.scale)),
            Msg::ScalePicked,
        )
        .style(theme::pick_list)
        .text_size(13.0)
        .width(Length::Fixed(96.0))
        .into(),
        link_chip(&state.link),
    ];
    if live && state.taking {
        controls.push(ui::btn(
            "Give back",
            None,
            ui::Kind::Danger,
            Some(Msg::GiveBack),
        ));
    } else if live {
        controls.push(ui::btn(
            "Take over",
            None,
            ui::Kind::Primary,
            Some(Msg::Take),
        ));
    }
    if live && matches!(state.link, Link::Reconnecting(_) | Link::Ended(_)) {
        controls.push(ui::btn(
            "Reconnect",
            None,
            ui::Kind::Secondary,
            Some(Msg::Reconnect),
        ));
    }

    let mut parts: Vec<Element<'a, Msg>> = vec![
        row(controls)
            .spacing(8)
            .align_y(iced::Alignment::Center)
            .into(),
    ];
    if let Some(banner) = banner(state, live) {
        parts.push(banner);
    }
    parts.push(picture(state, live));
    if let Some(shown) = &state.frame {
        parts.push(ui::faint(format!("{}×{} · frame {}", shown.w, shown.h, shown.seq)).into());
    }
    if let Some(notice) = &state.notice {
        parts.push(ui::error_line(notice.as_str()));
    }
    column(parts).spacing(10).width(Length::Fill).into()
}

fn link_chip<'a>(link: &Link) -> Element<'a, Msg> {
    match link {
        Link::Live => ui::badge("Live", ui::Tone::Ok),
        Link::Connecting => ui::badge("Connecting", ui::Tone::Muted),
        Link::Reconnecting(_) => ui::badge("Reconnecting", ui::Tone::Warn),
        Link::Ended(_) => ui::badge("Ended", ui::Tone::Muted),
    }
}

/// The owner's banner: that it holds the screen and the policy in force, or that it is asking for it.
fn banner<'a>(state: &'a State, live: bool) -> Option<Element<'a, Msg>> {
    if !live {
        return None;
    }
    if let Some(policy) = state.taken {
        return Some(
            container(
                column![
                    ui::label("You have the screen"),
                    ui::faint(takeover_text(policy)),
                    ui::faint("Your mouse and keyboard now drive the workstation. Give back when you are done."),
                ]
                .spacing(6),
            )
            .padding(12.0)
            .width(Length::Fill)
            .style(ui::frame(None, theme::WARN))
            .into(),
        );
    }
    state
        .taking
        .then(|| ui::faint("Asking the workstation for the screen…").into())
}

/// The picture: scaled to fit the box and centred. While the owner holds the screen, the pointer over it becomes
/// input in the picture's pixels.
fn picture<'a>(state: &'a State, live: bool) -> Element<'a, Msg> {
    let body: Element<'a, Msg> = match &state.frame {
        Some(shown) if live => {
            let handle = shown.handle.clone();
            let (w, h) = (shown.w, shown.h);
            let holding = state.holding();
            responsive(move |size: Size| -> Element<'a, Msg> {
                let boxed = (size.width, size.height);
                let (dw, dh, _, _) = fit(boxed, (w, h));
                let shown = container(
                    image(handle.clone())
                        .content_fit(ContentFit::Fill)
                        .width(Length::Fixed(dw))
                        .height(Length::Fixed(dh)),
                )
                .center(Length::Fill);
                if !holding {
                    return shown.into();
                }
                mouse_area(shown)
                    .on_move(move |p| {
                        let (x, y) = frame_point(p, boxed, (w, h));
                        Msg::Pointer(x, y)
                    })
                    .on_press(Msg::Press(Press::Left))
                    .on_release(Msg::Release(Press::Left))
                    .on_right_press(Msg::Press(Press::Right))
                    .on_right_release(Msg::Release(Press::Right))
                    .on_middle_press(Msg::Press(Press::Middle))
                    .on_middle_release(Msg::Release(Press::Middle))
                    .on_scroll(|delta| match delta {
                        iced::mouse::ScrollDelta::Lines { x, y } => Msg::Wheel(x, y, false),
                        iced::mouse::ScrollDelta::Pixels { x, y } => Msg::Wheel(x, y, true),
                    })
                    .into()
            })
            .into()
        }
        Some(_) => ui::empty(
            "machine",
            "The screen is off",
            "This workstation is revoked.",
        ),
        None if live => ui::empty(
            "machine",
            "No picture yet",
            "The screen appears once the workstation streams it.",
        ),
        None => ui::empty("machine", "No picture", "This workstation is revoked."),
    };
    container(body)
        .width(Length::Fill)
        .height(Length::Fixed(PICTURE_HEIGHT))
        .style(ui::frame(Some(theme::SUBSTRATE), theme::RULE))
        .into()
}

#[cfg(test)]
#[path = "workstations_live_tests.rs"]
mod tests;
