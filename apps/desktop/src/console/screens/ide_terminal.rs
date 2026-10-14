//! The IDE's terminal: a shell in an agent's VM, drawn from the emulator in `apz-vt` and wired to the
//! hub's `WS /v1/ide/terminal/{workspace}` socket (subprotocol `apz.ide.terminal.v1`).
//!
//! Three parts, none of them tied to the IDE's layout: the wire (the frames, the key encoding and the
//! socket thread that reconnects and re-attaches), the state ([`Terminal`]: its emulator, status,
//! selection and size, and [`TerminalTabs`] holding up to [`MAX_TABS`] of them), and the surface (an
//! iced canvas that draws the grid, takes the keys, the mouse and the wheel, and reports its size in
//! cells). The IDE screen owns a [`TerminalTabs`], maps its subscription and view, and routes its
//! messages back through `update`.
//!
//! A terminal's shell outlives its socket: closing the socket never ends it, a reconnect re-attaches
//! by the term id kept per workspace and tab in memory, and only [`Msg::Kill`] ends a shell.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self as std_mpsc, TryRecvError};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use apz_http::client::Target;
use apz_http::ws::{self, Limits, Message, Socket};
use apz_vt as vt;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use iced::futures::channel::mpsc;
use iced::futures::SinkExt;
use iced::keyboard;
use iced::mouse;
use iced::widget::canvas::{self, Canvas};
use iced::widget::{button, column, row, text};
use iced::{Color, Element, Length, Pixels, Point, Rectangle, Size, Subscription, Task};
use serde_json::{Value, json};

use crate::theme::{self, MONO, Palette};

/// The subprotocol the hub's terminal socket is asked for.
pub const PROTOCOL: &str = "apz.ide.terminal.v1";
/// Terminals one workspace's tabs may hold (the hub's own cap per workspace is the same).
pub const MAX_TABS: usize = 4;

/// The grid's text size, and the cell it makes: Martian Mono advances 0.7 em (a test holds the font to it).
const TEXT_SIZE: f32 = 13.0;
const CELL_W: f32 = TEXT_SIZE * 0.7;
const CELL_H: f32 = TEXT_SIZE * 1.2;
/// The space between the window's edge and the first cell.
const PAD: f32 = 6.0;
/// Rows the wheel scrolls per notch.
const WHEEL_LINES: f32 = 3.0;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long one socket read may wait, so commands are answered promptly.
const POLL: Duration = Duration::from_millis(20);
/// A ping when the socket has been quiet this long (the hub closes one silent for ten minutes).
const PING_EVERY: Duration = Duration::from_secs(60);
/// Reconnect waits, in order; the last one repeats.
const BACKOFF: [Duration; 4] = [Duration::from_secs(1), Duration::from_secs(2), Duration::from_secs(5), Duration::from_secs(10)];
/// A connection that stayed up this long resets the backoff.
const RESET_AFTER: Duration = Duration::from_secs(30);
/// How long a parked socket thread sleeps between looks at its commands.
const PARK: Duration = Duration::from_millis(100);
/// After the hub refuses an open, how long before the next try.
const OPEN_RETRY: Duration = Duration::from_secs(2);
/// Output bytes sent in one input frame.
const WRITE_CHUNK: usize = 8 * 1024;
/// Messages held between the socket thread and the UI.
const BUFFER: usize = 256;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// The term id and the last offset of each open shell, by workspace, tab slot and hub: a reconnect, or
/// a re-opened IDE, re-attaches to the same shell.
static RESUME: Mutex<BTreeMap<String, Resume>> = Mutex::new(BTreeMap::new());

#[derive(Debug, Clone)]
struct Resume {
    term: String,
    offset: u64,
}

fn resumes() -> MutexGuard<'static, BTreeMap<String, Resume>> {
    RESUME.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------------------------------------------------------------------------------------------
// The wire: frames and the keys they carry.
// ---------------------------------------------------------------------------------------------

/// The socket's path for a workspace: its id as one percent-encoded path segment (`agent:12` is
/// `agent%3A12`; the hub decodes it).
pub fn socket_path(workspace: &str) -> String {
    let mut path = String::from("/v1/ide/terminal/");
    for byte in workspace.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            path.push(char::from(byte));
        } else {
            path.push_str(&format!("%{byte:02X}"));
        }
    }
    path
}

/// A frame the hub sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Server {
    Opened(String),
    Out { data: Vec<u8>, offset: u64 },
    Lost(u64),
    Exit(Option<i64>),
    Error(String),
    Pong,
}

/// Reads one hub frame; a frame it does not know, or one malformed, is `None` and is ignored.
pub fn parse_server(text: &str) -> Option<Server> {
    let value: Value = serde_json::from_str(text).ok()?;
    match value.get("t")?.as_str()? {
        "opened" => Some(Server::Opened(value.get("term")?.as_str()?.to_string())),
        "out" => {
            let data = STANDARD.decode(value.get("data")?.as_str()?).ok()?;
            let offset = value.get("offset")?.as_u64()?;
            Some(Server::Out { data, offset })
        }
        "lost" => Some(Server::Lost(value.get("bytes")?.as_u64()?)),
        "exit" => Some(Server::Exit(value.get("code").and_then(Value::as_i64))),
        "error" => Some(Server::Error(value.get("message").and_then(Value::as_str).unwrap_or_default().to_string())),
        "pong" => Some(Server::Pong),
        _ => None,
    }
}

fn open_frame(cols: u16, rows: u16) -> String {
    json!({ "t": "open", "cols": cols, "rows": rows }).to_string()
}

fn attach_frame(term: &str, offset: u64) -> String {
    json!({ "t": "attach", "term": term, "offset": offset }).to_string()
}

fn input_frame(bytes: &[u8]) -> String {
    json!({ "t": "in", "data": STANDARD.encode(bytes) }).to_string()
}

fn resize_frame(cols: u16, rows: u16) -> String {
    json!({ "t": "resize", "cols": cols, "rows": rows }).to_string()
}

fn kill_frame() -> String {
    json!({ "t": "kill" }).to_string()
}

fn ping_frame() -> String {
    json!({ "t": "ping" }).to_string()
}

/// A key the terminal sends, as the shell reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
}

/// The modifiers held with a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// The control byte of Ctrl with `c` (Ctrl-A is 0x01, Ctrl-[ is ESC), if it has one.
fn ctrl_byte(c: char) -> Option<u8> {
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        '@' | ' ' => Some(0),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        '?' => Some(0x7f),
        _ => None,
    }
}

/// The xterm modifier parameter: 1 plus shift (1), alt (2), ctrl (4). 1 means none.
fn modifier_param(mods: Mods) -> u8 {
    1 + u8::from(mods.shift) + 2 * u8::from(mods.alt) + 4 * u8::from(mods.ctrl)
}

/// An arrow or Home/End: `ESC O x` in application cursor mode, `CSI x` otherwise, `CSI 1;m x` with modifiers.
fn csi_letter(letter: u8, m: u8, app_cursor: bool) -> Vec<u8> {
    if m > 1 {
        format!("\x1b[1;{m}{}", char::from(letter)).into_bytes()
    } else if app_cursor {
        vec![0x1b, b'O', letter]
    } else {
        vec![0x1b, b'[', letter]
    }
}

/// A key with a number before its `~`: `CSI n ~`, `CSI n;m ~` with modifiers.
fn csi_tilde(code: u8, m: u8) -> Vec<u8> {
    if m > 1 { format!("\x1b[{code};{m}~").into_bytes() } else { format!("\x1b[{code}~").into_bytes() }
}

/// The bytes a key sends, given the modifiers and whether the program asked for application cursor keys.
/// Alt sends ESC before the key's own bytes.
pub fn key_bytes(key: Key, mods: Mods, app_cursor: bool) -> Vec<u8> {
    let m = modifier_param(mods);
    let with_alt = |bytes: Vec<u8>| -> Vec<u8> {
        if mods.alt {
            let mut out = vec![0x1b];
            out.extend(bytes);
            out
        } else {
            bytes
        }
    };
    match key {
        Key::Char(c) => {
            if mods.ctrl
                && let Some(byte) = ctrl_byte(c)
            {
                return with_alt(vec![byte]);
            }
            let mut buf = [0u8; 4];
            with_alt(c.encode_utf8(&mut buf).as_bytes().to_vec())
        }
        Key::Enter => with_alt(vec![b'\r']),
        Key::Tab if mods.shift => b"\x1b[Z".to_vec(),
        Key::Tab => with_alt(vec![b'\t']),
        Key::Backspace => with_alt(vec![if mods.ctrl { 0x08 } else { 0x7f }]),
        Key::Escape => with_alt(vec![0x1b]),
        Key::Up => csi_letter(b'A', m, app_cursor),
        Key::Down => csi_letter(b'B', m, app_cursor),
        Key::Right => csi_letter(b'C', m, app_cursor),
        Key::Left => csi_letter(b'D', m, app_cursor),
        Key::Home => csi_letter(b'H', m, app_cursor),
        Key::End => csi_letter(b'F', m, app_cursor),
        Key::Insert => csi_tilde(2, m),
        Key::Delete => csi_tilde(3, m),
        Key::PageUp => csi_tilde(5, m),
        Key::PageDown => csi_tilde(6, m),
        Key::F(n @ 1..=4) => {
            let letter = b"PQRS"[usize::from(n) - 1];
            if m > 1 { format!("\x1b[1;{m}{}", char::from(letter)).into_bytes() } else { vec![0x1b, b'O', letter] }
        }
        Key::F(n) => {
            let code = match n {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                _ => 24,
            };
            csi_tilde(code, m)
        }
    }
}

/// The bytes a paste sends: its line ends as the shell reads them (CR), and in brackets when the
/// program asked for bracketed paste.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let body = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        format!("\x1b[200~{body}\x1b[201~").into_bytes()
    } else {
        body.into_bytes()
    }
}

// ---------------------------------------------------------------------------------------------
// The socket thread: one per terminal, owning its socket, its reconnects and its re-attach.
// ---------------------------------------------------------------------------------------------

/// A command from the UI to the socket thread.
#[derive(Debug)]
enum Command {
    Size(u16, u16),
    Input(Vec<u8>),
    Kill,
    Restart,
}

/// The UI's end of the socket thread's command channel.
#[derive(Clone)]
pub struct Commands(std_mpsc::Sender<Command>);

impl std::fmt::Debug for Commands {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Commands")
    }
}

/// What the socket thread tells the UI.
#[derive(Debug, Clone)]
pub enum Link {
    /// The thread is running: its command channel.
    Ready(Commands),
    /// The socket is up (the shell is not yet open or attached).
    Connected,
    /// The shell is open or attached, under this term id.
    Opened(String),
    /// Output from the shell, in order.
    Output(Vec<u8>),
    /// Output the hub dropped before the client's offset.
    Lost(u64),
    /// The shell ended, with its exit code when it gave one.
    Exited(Option<i64>),
    /// The shell was killed.
    Killed,
    /// The socket dropped or could not connect; the thread retries.
    Disconnected(String),
    /// The hub refused a frame.
    Notice(String),
}

/// What the terminal is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Starting,
    Connecting,
    Live,
    Reconnecting,
    Ended(Option<i64>),
    Killed,
}

/// The data the socket thread is started with: the hub, the workspace and the tab's slot.
#[derive(Clone)]
struct Feed {
    id: u64,
    target: Target,
    workspace: String,
    slot: usize,
}

impl Hash for Feed {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.target.base.hash(state);
        self.workspace.hash(state);
        self.slot.hash(state);
    }
}

fn resume_key(feed: &Feed) -> String {
    format!("{}\u{1f}{}\u{1f}{}", feed.target.base, feed.workspace, feed.slot)
}

/// The thread's state across connections.
struct Session {
    key: String,
    term: Option<String>,
    offset: u64,
    /// The view's size, as the UI last reported it.
    size: Option<(u16, u16)>,
    /// The size the hub was last told, on this connection.
    sent_size: Option<(u16, u16)>,
    /// The shell should be open: set at start and on restart, cleared when it ends or is killed.
    want_open: bool,
    kill: bool,
    attaching: bool,
    opening: bool,
    open_after: Instant,
    last_sent: Instant,
    commands: std_mpsc::Receiver<Command>,
}

impl Session {
    fn new(feed: &Feed, commands: std_mpsc::Receiver<Command>) -> Session {
        let key = resume_key(feed);
        // A fresh emulator starts from the shell's oldest retained output, so the offset starts at 0.
        let term = resumes().get(&key).map(|resume| resume.term.clone());
        Session {
            key,
            term,
            offset: 0,
            size: None,
            sent_size: None,
            want_open: true,
            kill: false,
            attaching: false,
            opening: false,
            open_after: Instant::now(),
            last_sent: Instant::now(),
            commands,
        }
    }

    /// Whether a socket is worth connecting: a shell to attach to, or one to open once the size is known.
    fn wants_socket(&self) -> bool {
        self.term.is_some() || (self.want_open && self.size.is_some())
    }

    fn forget(&mut self) {
        self.term = None;
        self.attaching = false;
        self.opening = false;
        resumes().remove(&self.key);
    }
}

/// How one connection ended.
enum Outcome {
    /// The UI is gone: the thread stops.
    Closed,
    /// The socket broke; the thread reconnects.
    Dropped(String),
    /// The connection is over on purpose (the shell ended or was killed): the thread parks.
    Done,
}

/// Sends `bytes` to the UI, waiting while its buffer is full. False when the UI is gone.
fn deliver(out: &mut mpsc::Sender<Msg>, msg: Msg) -> bool {
    let mut msg = msg;
    loop {
        match out.try_send(msg) {
            Ok(()) => return true,
            Err(err) if err.is_disconnected() => return false,
            Err(err) => {
                msg = err.into_inner();
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

fn send(socket: &mut Socket, st: &mut Session, frame: String) -> io::Result<()> {
    st.last_sent = Instant::now();
    socket.send(&Message::Text(frame))
}

/// Applies one command. `socket` is `None` while offline: a kill of a shell waits for the next
/// connection, and input is dropped.
fn apply(st: &mut Session, cmd: Command, socket: Option<&mut Socket>, out: &mut mpsc::Sender<Msg>) -> Result<(), Outcome> {
    match cmd {
        // The resize itself goes out from `serve`, once the shell is open on this connection.
        Command::Size(cols, rows) => st.size = Some((cols, rows)),
        Command::Input(bytes) => {
            if let Some(socket) = socket
                && st.term.is_some()
            {
                for chunk in bytes.chunks(WRITE_CHUNK) {
                    send(socket, st, input_frame(chunk)).map_err(|err| Outcome::Dropped(err.to_string()))?;
                }
            }
        }
        Command::Kill => {
            if st.term.is_none() && !st.opening {
                st.want_open = false;
                st.kill = false;
                st.forget();
                if !deliver(out, Msg::Link(Link::Killed)) {
                    return Err(Outcome::Closed);
                }
            } else {
                // A shell that is attaching or opening is killed once it has an id.
                st.kill = true;
            }
        }
        Command::Restart => {
            st.want_open = true;
            st.kill = false;
            st.offset = 0;
            st.forget();
        }
    }
    Ok(())
}

/// Applies every command waiting, without blocking. Err when the thread should stop.
fn drain(st: &mut Session, mut socket: Option<&mut Socket>, out: &mut mpsc::Sender<Msg>) -> Result<(), Outcome> {
    loop {
        match st.commands.try_recv() {
            Ok(cmd) => apply(st, cmd, socket.as_deref_mut(), out)?,
            Err(TryRecvError::Empty) => return Ok(()),
            Err(TryRecvError::Disconnected) => return Err(Outcome::Closed),
        }
    }
}

/// Waits `total` while answering commands offline. False when the thread should stop.
fn pause(st: &mut Session, out: &mut mpsc::Sender<Msg>, total: Duration) -> bool {
    let end = Instant::now() + total;
    loop {
        if out.is_closed() || drain(st, None, out).is_err() {
            return false;
        }
        let now = Instant::now();
        if now >= end {
            return true;
        }
        std::thread::sleep((end - now).min(PARK));
    }
}

/// One connection: attaches or opens the shell, then reads frames and sends commands until it ends.
fn serve(socket: &mut Socket, st: &mut Session, out: &mut mpsc::Sender<Msg>) -> Outcome {
    // Nothing is pending on a new connection: an open or attach from the last one was lost with it.
    st.attaching = false;
    st.opening = false;
    st.sent_size = None;
    if let Some(term) = st.term.clone() {
        if let Err(err) = send(socket, st, attach_frame(&term, st.offset)) {
            return Outcome::Dropped(err.to_string());
        }
        st.attaching = true;
    }
    loop {
        if out.is_closed() {
            return Outcome::Closed;
        }
        if let Err(outcome) = drain(st, Some(&mut *socket), out) {
            return outcome;
        }
        if st.kill && st.term.is_some() {
            let _ = send(socket, st, kill_frame());
            st.want_open = false;
            st.kill = false;
            st.forget();
            return if deliver(out, Msg::Link(Link::Killed)) { Outcome::Done } else { Outcome::Closed };
        }
        if st.kill && st.term.is_none() && !st.opening {
            // A kill waited for an open that the hub refused: there is no shell left to end.
            st.want_open = false;
            st.kill = false;
            return if deliver(out, Msg::Link(Link::Killed)) { Outcome::Done } else { Outcome::Closed };
        }
        if st.term.is_none() && !st.opening && st.want_open && let Some((cols, rows)) = st.size && Instant::now() >= st.open_after {
            st.offset = 0;
            if let Err(err) = send(socket, st, open_frame(cols, rows)) {
                return Outcome::Dropped(err.to_string());
            }
            st.sent_size = Some((cols, rows));
            st.opening = true;
        }
        // The hub learns the view's size once the shell is open, and again whenever it changes.
        let shell_open = st.term.is_some() && !st.attaching;
        if let Some((cols, rows)) = st.size
            && shell_open
            && st.sent_size != Some((cols, rows))
        {
            if let Err(err) = send(socket, st, resize_frame(cols, rows)) {
                return Outcome::Dropped(err.to_string());
            }
            st.sent_size = Some((cols, rows));
        }
        if st.last_sent.elapsed() >= PING_EVERY
            && let Err(err) = send(socket, st, ping_frame())
        {
            return Outcome::Dropped(err.to_string());
        }
        match socket.recv(POLL) {
            Ok(Some(Message::Text(text))) => {
                if let Some(frame) = parse_server(&text)
                    && let Some(outcome) = on_frame(frame, st, out)
                {
                    return outcome;
                }
            }
            Ok(Some(Message::Binary(_))) | Ok(None) => {}
            Err(err) => return Outcome::Dropped(err.to_string()),
        }
    }
}

/// Acts on one hub frame. `Some` ends the connection.
fn on_frame(frame: Server, st: &mut Session, out: &mut mpsc::Sender<Msg>) -> Option<Outcome> {
    let sent = match frame {
        Server::Opened(term) => {
            st.term = Some(term.clone());
            st.attaching = false;
            st.opening = false;
            resumes().insert(st.key.clone(), Resume { term: term.clone(), offset: st.offset });
            deliver(out, Msg::Link(Link::Opened(term)))
        }
        Server::Out { data, offset } => {
            st.offset = offset + data.len() as u64;
            if let Some(resume) = resumes().get_mut(&st.key) {
                resume.offset = st.offset;
            }
            deliver(out, Msg::Link(Link::Output(data)))
        }
        Server::Lost(bytes) => deliver(out, Msg::Link(Link::Lost(bytes))),
        Server::Exit(code) => {
            st.want_open = false;
            st.kill = false;
            st.forget();
            let sent = deliver(out, Msg::Link(Link::Exited(code)));
            return Some(if sent { Outcome::Done } else { Outcome::Closed });
        }
        Server::Error(message) => {
            let sent = deliver(out, Msg::Link(Link::Notice(message)));
            if st.attaching {
                // The shell is gone: open a new one on the next pass.
                st.forget();
                st.open_after = Instant::now();
            } else if st.opening {
                st.opening = false;
                st.open_after = Instant::now() + OPEN_RETRY;
            }
            sent
        }
        Server::Pong => true,
    };
    if sent { None } else { Some(Outcome::Closed) }
}

/// The socket thread: connects when there is a shell to attach or open, reconnects with backoff, and
/// parks when the shell has ended or been killed. It stops when the UI drops its subscription.
fn supervise(feed: Feed, commands: std_mpsc::Receiver<Command>, mut out: mpsc::Sender<Msg>) {
    let mut st = Session::new(&feed, commands);
    let path = socket_path(&feed.workspace);
    let mut backoff = 0;
    loop {
        if out.is_closed() {
            return;
        }
        if st.wants_socket() {
            match ws::connect(&feed.target, &path, Some(PROTOCOL), CONNECT_TIMEOUT, Limits::default()) {
                Ok(mut socket) => {
                    if !deliver(&mut out, Msg::Link(Link::Connected)) {
                        return;
                    }
                    let up = Instant::now();
                    match serve(&mut socket, &mut st, &mut out) {
                        Outcome::Closed => return,
                        Outcome::Dropped(reason) => {
                            if !deliver(&mut out, Msg::Link(Link::Disconnected(reason))) {
                                return;
                            }
                        }
                        Outcome::Done => {}
                    }
                    if up.elapsed() >= RESET_AFTER {
                        backoff = 0;
                    }
                }
                Err(err) => {
                    if !deliver(&mut out, Msg::Link(Link::Disconnected(err.to_string()))) {
                        return;
                    }
                }
            }
        }
        let wait = if st.wants_socket() {
            let wait = BACKOFF[backoff];
            backoff = (backoff + 1).min(BACKOFF.len() - 1);
            wait
        } else {
            backoff = 0;
            PARK
        };
        if !pause(&mut st, &mut out, wait) {
            return;
        }
    }
}

/// The subscription's stream: it hands the UI the thread's command channel, then forwards what the
/// thread sends. Dropping the subscription drops the stream, and the thread stops at its next look.
fn link(feed: Feed) -> impl iced::futures::Stream<Item = Msg> {
    iced::stream::channel(BUFFER, move |mut out: mpsc::Sender<Msg>| async move {
        let (tx, rx) = std_mpsc::channel::<Command>();
        if out.send(Msg::Link(Link::Ready(Commands(tx)))).await.is_err() {
            return;
        }
        let thread_out = out.clone();
        std::thread::spawn(move || supervise(feed, rx, thread_out));
        std::future::pending::<()>().await;
    })
}

// ---------------------------------------------------------------------------------------------
// The state: a terminal and its tabs.
// ---------------------------------------------------------------------------------------------

/// A point in the text: an absolute line id (see `apz_vt::Screen::abs_of_index`) and a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub line: u64,
    pub col: usize,
}

/// A selection from where the drag began to where it is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Pos,
    pub head: Pos,
}

impl Selection {
    /// The selection's ends, first to last.
    pub fn range(self) -> (Pos, Pos) {
        if self.anchor <= self.head { (self.anchor, self.head) } else { (self.head, self.anchor) }
    }
}

/// A message a terminal takes.
#[derive(Debug, Clone)]
pub enum Msg {
    Link(Link),
    /// Bytes for the shell: keys, a paste, or a wheel's arrows.
    Input(Vec<u8>),
    /// The view's size in cells, from its bounds.
    Resized(usize, usize),
    /// Scrolls the view: positive is back into the history.
    Scroll(isize),
    SelectStart(Pos),
    SelectTo(Pos),
    SelectEnd,
    Copy,
    Paste,
    Pasted(Option<String>),
    /// Ends the shell.
    Kill,
    /// Starts a new shell after the last one ended or was killed.
    Restart,
}

/// One terminal: its emulator, the socket thread's commands, its status and its selection.
pub struct Terminal {
    id: u64,
    target: Target,
    workspace: String,
    slot: usize,
    emu: vt::Emulator,
    commands: Option<Commands>,
    status: Status,
    notice: Option<String>,
    size: Option<(usize, usize)>,
    selection: Option<Selection>,
}

impl Terminal {
    /// A terminal for `workspace`'s shell on `target`'s hub. `slot` tells it apart from the workspace's
    /// other tabs, and keeps its term id across a re-opened IDE (0 to 3).
    pub fn new(target: Target, workspace: impl Into<String>, slot: usize) -> Terminal {
        Terminal {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            target,
            workspace: workspace.into(),
            slot,
            emu: vt::Emulator::new(80, 24),
            commands: None,
            status: Status::Starting,
            notice: None,
            size: None,
            selection: None,
        }
    }

    /// The tab's slot.
    pub fn slot(&self) -> usize {
        self.slot
    }

    /// What the terminal is doing.
    pub fn status(&self) -> &Status {
        &self.status
    }

    /// The workspace whose shell this is.
    pub fn workspace(&self) -> &str {
        &self.workspace
    }

    /// The screen, as the emulator has drawn it so far.
    pub fn screen(&self) -> &vt::Screen {
        self.emu.screen()
    }

    fn command(&self, cmd: Command) {
        if let Some(commands) = &self.commands {
            let _ = commands.0.send(cmd);
        }
    }

    fn input(&mut self, bytes: Vec<u8>) {
        self.emu.screen_mut().reset_view();
        self.command(Command::Input(bytes));
    }

    /// Applies a message; the task is what the UI must still do (a clipboard read or write).
    pub fn update(&mut self, msg: Msg) -> Task<Msg> {
        match msg {
            Msg::Link(link) => self.on_link(link),
            Msg::Input(bytes) => self.input(bytes),
            Msg::Resized(cols, rows) => {
                if self.size != Some((cols, rows)) {
                    self.size = Some((cols, rows));
                    self.emu.resize(cols, rows);
                    self.command(Command::Size(cols as u16, rows as u16));
                }
            }
            Msg::Scroll(lines) => self.emu.scroll_view(lines),
            Msg::SelectStart(pos) => self.selection = Some(Selection { anchor: pos, head: pos }),
            Msg::SelectTo(pos) => {
                if let Some(selection) = self.selection.as_mut() {
                    selection.head = pos;
                }
            }
            Msg::SelectEnd => {
                if self.selection.is_some_and(|s| s.anchor == s.head) {
                    self.selection = None;
                }
            }
            Msg::Copy => {
                if let Some(selection) = self.selection {
                    let (start, end) = selection.range();
                    let text = self.emu.screen().text_between((start.line, start.col), (end.line, end.col));
                    if !text.is_empty() {
                        return iced::clipboard::write(text);
                    }
                }
            }
            Msg::Paste => return iced::clipboard::read().map(Msg::Pasted),
            Msg::Pasted(Some(text)) => {
                let bytes = paste_bytes(&text, self.emu.screen().bracketed_paste());
                return Task::done(Msg::Input(bytes));
            }
            Msg::Pasted(None) => {}
            Msg::Kill => self.command(Command::Kill),
            Msg::Restart => {
                if matches!(self.status, Status::Ended(_) | Status::Killed) {
                    let (cols, rows) = self.size.unwrap_or((80, 24));
                    self.emu = vt::Emulator::new(cols, rows);
                    self.selection = None;
                    self.notice = None;
                    self.status = Status::Starting;
                    self.command(Command::Restart);
                }
            }
        }
        Task::none()
    }

    fn on_link(&mut self, link: Link) {
        let finished = matches!(self.status, Status::Ended(_) | Status::Killed);
        match link {
            Link::Ready(commands) => {
                self.commands = Some(commands);
                if let Some((cols, rows)) = self.size {
                    self.command(Command::Size(cols as u16, rows as u16));
                }
            }
            Link::Connected => {
                if !finished {
                    self.status = Status::Connecting;
                }
            }
            Link::Opened(_) => {
                self.status = Status::Live;
                self.notice = None;
            }
            Link::Output(bytes) => {
                let was_alt = self.emu.screen().alt_screen();
                self.emu.feed(&bytes);
                if self.emu.screen().alt_screen() != was_alt {
                    // Selections are text in the main screen's lines; the alternate screen has none.
                    self.selection = None;
                }
                let replies = self.emu.screen_mut().take_replies();
                if !replies.is_empty() {
                    self.command(Command::Input(replies));
                }
                let _ = self.emu.screen_mut().take_bell();
            }
            Link::Lost(bytes) => self.notice = Some(format!("{bytes} bytes of output were dropped")),
            Link::Exited(code) => self.status = Status::Ended(code),
            Link::Killed => self.status = Status::Killed,
            Link::Disconnected(reason) => {
                if !finished {
                    self.status = Status::Reconnecting;
                }
                self.notice = Some(reason);
            }
            Link::Notice(message) => self.notice = Some(message),
        }
    }

    fn caption(&self) -> String {
        let base = match &self.status {
            Status::Starting => "starting".to_string(),
            Status::Connecting => "connecting".to_string(),
            Status::Live => "live".to_string(),
            Status::Reconnecting => "reconnecting".to_string(),
            Status::Ended(Some(code)) => format!("the shell ended with code {code}"),
            Status::Ended(None) => "the shell ended".to_string(),
            Status::Killed => "closed".to_string(),
        };
        match &self.notice {
            Some(notice) => format!("{base} - {notice}"),
            None => base,
        }
    }

    /// The subscription that feeds the socket thread's messages to this terminal.
    pub fn subscription(&self) -> Subscription<Msg> {
        let feed = Feed {
            id: self.id,
            target: self.target.clone(),
            workspace: self.workspace.clone(),
            slot: self.slot,
        };
        Subscription::run_with(feed, |feed: &Feed| link(feed.clone()))
    }

    /// The terminal's view: its grid on a canvas, and a line below it with the status.
    pub fn view(&self) -> Element<'_, Msg> {
        let palette = theme::palette();
        let surface = Canvas::new(Surface { screen: self.emu.screen(), selection: self.selection })
            .width(Length::Fill)
            .height(Length::Fill);
        let mut bar = row![text(self.caption()).font(MONO).size(11).color(palette.ink_3)].spacing(10);
        if matches!(self.status, Status::Ended(_) | Status::Killed) {
            bar = bar.push(button(text("RESTART").font(MONO).size(11)).on_press(Msg::Restart));
        }
        column![surface, bar].spacing(4).into()
    }
}

/// Up to [`MAX_TABS`] terminals on one workspace.
pub struct TerminalTabs {
    target: Target,
    workspace: String,
    tabs: Vec<Terminal>,
    active: usize,
}

/// A message the tabs take: a tab's own message (by its slot), or a change of tab.
#[derive(Debug, Clone)]
pub enum TabsMsg {
    Select(usize),
    Add,
    /// Ends the shell in this position and closes its tab once the hub confirms it.
    Close(usize),
    Tab(usize, Msg),
}

impl TerminalTabs {
    /// The tabs for `workspace`, with one terminal open.
    pub fn new(target: Target, workspace: impl Into<String>) -> TerminalTabs {
        let mut tabs = TerminalTabs { target, workspace: workspace.into(), tabs: Vec::new(), active: 0 };
        tabs.add();
        tabs
    }

    /// How many terminals are open.
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// Whether no terminal is open.
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// The position of the tab shown.
    pub fn active(&self) -> usize {
        self.active
    }

    /// The terminal at `index`.
    pub fn get(&self, index: usize) -> Option<&Terminal> {
        self.tabs.get(index)
    }

    /// Opens another terminal, if there is room. True when one was opened.
    pub fn add(&mut self) -> bool {
        if self.tabs.len() >= MAX_TABS {
            return false;
        }
        let slot = (0..MAX_TABS).find(|slot| self.tabs.iter().all(|tab| tab.slot != *slot)).unwrap_or(0);
        self.tabs.push(Terminal::new(self.target.clone(), self.workspace.clone(), slot));
        self.active = self.tabs.len() - 1;
        true
    }

    /// Applies a message; the task is what the UI must still do.
    pub fn update(&mut self, msg: TabsMsg) -> Task<TabsMsg> {
        match msg {
            TabsMsg::Select(index) => {
                if index < self.tabs.len() {
                    self.active = index;
                }
                Task::none()
            }
            TabsMsg::Add => {
                self.add();
                Task::none()
            }
            TabsMsg::Close(index) => {
                let Some(tab) = self.tabs.get_mut(index) else {
                    return Task::none();
                };
                if tab.commands.is_none() {
                    // No socket thread to ask: the tab goes now.
                    self.remove(index);
                } else {
                    let _ = tab.update(Msg::Kill);
                }
                Task::none()
            }
            TabsMsg::Tab(slot, Msg::Link(Link::Killed)) => {
                if let Some(index) = self.tabs.iter().position(|tab| tab.slot == slot) {
                    self.remove(index);
                }
                Task::none()
            }
            TabsMsg::Tab(slot, msg) => match self.tabs.iter_mut().find(|tab| tab.slot == slot) {
                Some(tab) => tab.update(msg).map(move |m| TabsMsg::Tab(slot, m)),
                None => Task::none(),
            },
        }
    }

    fn remove(&mut self, index: usize) {
        self.tabs.remove(index);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        }
    }

    /// The subscriptions of every terminal.
    pub fn subscription(&self) -> Subscription<TabsMsg> {
        Subscription::batch(self.tabs.iter().map(|tab| {
            // The slot is part of each subscription's identity, and comes back with its messages.
            tab.subscription().with(tab.slot).map(|(slot, msg)| TabsMsg::Tab(slot, msg))
        }))
    }

    /// The tab strip above the active terminal.
    pub fn view(&self) -> Element<'_, TabsMsg> {
        let mut strip = row![].spacing(6);
        for index in 0..self.tabs.len() {
            let label = if index == self.active { format!("[TERM {}]", index + 1) } else { format!("TERM {}", index + 1) };
            strip = strip
                .push(button(text(label).font(MONO).size(11)).on_press(TabsMsg::Select(index)))
                .push(button(text("X").font(MONO).size(11)).on_press(TabsMsg::Close(index)));
        }
        if self.tabs.len() < MAX_TABS {
            strip = strip.push(button(text("+").font(MONO).size(11)).on_press(TabsMsg::Add));
        }
        let body: Element<'_, TabsMsg> = match self.tabs.get(self.active) {
            Some(tab) => {
                let slot = tab.slot;
                tab.view().map(move |msg| TabsMsg::Tab(slot, msg))
            }
            None => text("No terminal").font(MONO).size(12).into(),
        };
        column![strip, body].spacing(6).into()
    }
}

// ---------------------------------------------------------------------------------------------
// The surface: the grid on an iced canvas.
// ---------------------------------------------------------------------------------------------

/// The grid's size in cells for a view of `bounds`, never below one by one.
fn grid_size(bounds: Rectangle) -> (usize, usize) {
    let cols = ((bounds.width - 2.0 * PAD) / CELL_W).floor().max(1.0) as usize;
    let rows = ((bounds.height - 2.0 * PAD) / CELL_H).floor().max(1.0) as usize;
    (cols.min(vt::MAX_SIZE), rows.min(vt::MAX_SIZE))
}

/// The text position under `point` (relative to the view), clamped to the grid.
fn cell_at(screen: &vt::Screen, point: Point) -> Pos {
    let col = ((point.x - PAD) / CELL_W).floor().clamp(0.0, (screen.cols() - 1) as f32) as usize;
    let row = ((point.y - PAD) / CELL_H).floor().clamp(0.0, (screen.rows() - 1) as f32) as usize;
    Pos { line: screen.abs_of_view_row(row), col }
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    Color { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: a.a }
}

/// One of the six ANSI hues, from the theme's roles.
fn hue(n: u8, p: &Palette) -> Color {
    match n {
        1 => p.crit,
        2 => p.ok,
        3 => p.warn,
        4 => p.info,
        5 => mix(p.crit, p.info, 0.5),
        6 => mix(p.info, p.ok, 0.5),
        _ => p.ink_2,
    }
}

/// A palette entry: the 16 ANSI colours from the theme's roles, the 6x6x6 cube and the greys exactly.
/// `bg` picks the black and bright-black entries' background roles (the page and the rule).
fn indexed(n: u8, p: &Palette, bg: bool) -> Color {
    match n {
        0 => {
            if bg {
                p.paper
            } else {
                p.ink_3
            }
        }
        1..=7 => hue(n, p),
        8 => {
            if bg {
                p.rule
            } else {
                p.ink_3
            }
        }
        9..=14 => mix(hue(n - 8, p), p.ink, 0.25),
        15 => p.ink,
        16..=231 => {
            const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
            let i = usize::from(n - 16);
            Color::from_rgb8(LEVELS[i / 36], LEVELS[(i / 6) % 6], LEVELS[i % 6])
        }
        232..=255 => {
            let v = 8 + 10 * (n - 232);
            Color::from_rgb8(v, v, v)
        }
    }
}

/// A cell's foreground and background, with the inverse attribute applied and the defaults resolved.
fn colors(style: vt::Style, p: &Palette) -> (Color, Color) {
    let fg = match style.fg {
        vt::Color::Default => p.ink,
        vt::Color::Indexed(n) => indexed(n, p, false),
        vt::Color::Rgb(r, g, b) => Color::from_rgb8(r, g, b),
    };
    let bg = match style.bg {
        vt::Color::Default => p.paper,
        vt::Color::Indexed(n) => indexed(n, p, true),
        vt::Color::Rgb(r, g, b) => Color::from_rgb8(r, g, b),
    };
    if style.inverse { (bg, fg) } else { (fg, bg) }
}

fn in_range(start: Pos, end: Pos, line: u64, col: usize) -> bool {
    let at = Pos { line, col };
    start <= at && at <= end
}

/// A run of cells in one colour and weight, drawn as one text: where it starts, its colour, weight and text.
type Run = (usize, Color, bool, String);

fn flush(frame: &mut canvas::Frame, run: &mut Option<Run>, top: f32) {
    if let Some((start, color, bold, content)) = run.take() {
        draw_text(frame, PAD + start as f32 * CELL_W, top, content, color, bold);
    }
}

fn draw_text(frame: &mut canvas::Frame, x: f32, y: f32, content: String, color: Color, bold: bool) {
    frame.fill_text(canvas::Text {
        content,
        position: Point::new(x, y),
        color,
        size: Pixels(TEXT_SIZE),
        font: if bold { crate::console::ui::MONO_BOLD } else { MONO },
        ..Default::default()
    });
}

/// Keys with a name the terminal knows; a typed character is `Char`.
fn key_of(key: &keyboard::Key) -> Option<Key> {
    use keyboard::key::Named;
    match key {
        keyboard::Key::Character(text) => {
            let mut chars = text.chars();
            let c = chars.next()?;
            chars.next().is_none().then_some(Key::Char(c))
        }
        keyboard::Key::Named(named) => Some(match named {
            Named::Enter => Key::Enter,
            Named::Tab => Key::Tab,
            Named::Backspace => Key::Backspace,
            Named::Escape => Key::Escape,
            Named::Space => Key::Char(' '),
            Named::ArrowUp => Key::Up,
            Named::ArrowDown => Key::Down,
            Named::ArrowLeft => Key::Left,
            Named::ArrowRight => Key::Right,
            Named::Home => Key::Home,
            Named::End => Key::End,
            Named::PageUp => Key::PageUp,
            Named::PageDown => Key::PageDown,
            Named::Insert => Key::Insert,
            Named::Delete => Key::Delete,
            Named::F1 => Key::F(1),
            Named::F2 => Key::F(2),
            Named::F3 => Key::F(3),
            Named::F4 => Key::F(4),
            Named::F5 => Key::F(5),
            Named::F6 => Key::F(6),
            Named::F7 => Key::F(7),
            Named::F8 => Key::F(8),
            Named::F9 => Key::F(9),
            Named::F10 => Key::F(10),
            Named::F11 => Key::F(11),
            Named::F12 => Key::F(12),
            _ => return None,
        }),
        keyboard::Key::Unidentified => None,
    }
}

/// What a key press does: copy or paste (Ctrl+Shift+C and V), text typed as it is, or the key's bytes.
fn keystroke(screen: &vt::Screen, key: &keyboard::Key, modifiers: keyboard::Modifiers, text: Option<&str>) -> Option<canvas::Action<Msg>> {
    if modifiers.control()
        && modifiers.shift()
        && let keyboard::Key::Character(c) = key
    {
        match c.to_lowercase().as_str() {
            "c" => return Some(canvas::Action::publish(Msg::Copy).and_capture()),
            "v" => return Some(canvas::Action::publish(Msg::Paste).and_capture()),
            _ => {}
        }
    }
    let mods = Mods { shift: modifiers.shift(), alt: modifiers.alt(), ctrl: modifiers.control() };
    if !mods.ctrl
        && !mods.alt
        && let Some(typed) = text.filter(|typed| !typed.is_empty() && typed.chars().all(|c| !c.is_control()))
    {
        return Some(canvas::Action::publish(Msg::Input(typed.as_bytes().to_vec())).and_capture());
    }
    let key = key_of(key)?;
    let bytes = key_bytes(key, mods, screen.app_cursor());
    Some(canvas::Action::publish(Msg::Input(bytes)).and_capture())
}

/// The canvas's own state: whether it has the keyboard, whether a drag is running, and the size it
/// last reported.
#[derive(Debug, Default)]
struct State {
    focused: bool,
    dragging: bool,
    reported: Option<(usize, usize)>,
}

/// The grid, drawn from the screen; the selection is shown over it.
struct Surface<'a> {
    screen: &'a vt::Screen,
    selection: Option<Selection>,
}

impl canvas::Program<Msg> for Surface<'_> {
    type State = State;

    fn update(&self, state: &mut State, event: &canvas::Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<canvas::Action<Msg>> {
        let (cols, rows) = grid_size(bounds);
        if state.reported != Some((cols, rows)) {
            state.reported = Some((cols, rows));
            return Some(canvas::Action::publish(Msg::Resized(cols, rows)));
        }
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => match cursor.position_in(bounds) {
                Some(point) => {
                    state.focused = true;
                    state.dragging = true;
                    Some(canvas::Action::publish(Msg::SelectStart(cell_at(self.screen, point))))
                }
                None if state.focused => {
                    state.focused = false;
                    Some(canvas::Action::request_redraw())
                }
                None => None,
            },
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) if state.dragging => cursor.position().map(|point| {
                let inside = Point::new(point.x - bounds.x, point.y - bounds.y);
                canvas::Action::publish(Msg::SelectTo(cell_at(self.screen, inside)))
            }),
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if state.dragging => {
                state.dragging = false;
                Some(canvas::Action::publish(Msg::SelectEnd))
            }
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(bounds) => {
                let lines = match *delta {
                    mouse::ScrollDelta::Lines { y, .. } => (y * WHEEL_LINES).round() as isize,
                    mouse::ScrollDelta::Pixels { y, .. } => (y / CELL_H * WHEEL_LINES).round() as isize,
                };
                if lines == 0 {
                    None
                } else if self.screen.alt_screen() {
                    // A full-screen program takes the wheel as arrow keys, as a terminal does.
                    let key = if lines > 0 { Key::Up } else { Key::Down };
                    let one = key_bytes(key, Mods::default(), self.screen.app_cursor());
                    let bytes = one.repeat(lines.unsigned_abs());
                    Some(canvas::Action::publish(Msg::Input(bytes)).and_capture())
                } else {
                    Some(canvas::Action::publish(Msg::Scroll(lines)).and_capture())
                }
            }
            canvas::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, text, .. }) if state.focused => {
                keystroke(self.screen, key, *modifiers, text.as_deref())
            }
            _ => None,
        }
    }

    fn draw(&self, state: &State, renderer: &iced::Renderer, _theme: &iced::Theme, bounds: Rectangle, _cursor: mouse::Cursor) -> Vec<canvas::Geometry> {
        let p = theme::palette();
        let screen = self.screen;
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), p.paper);
        let range = self.selection.map(Selection::range);
        let cell = Size::new(CELL_W, CELL_H);
        for y in 0..screen.rows() {
            let Some(row) = screen.view_row(y) else {
                continue;
            };
            let line = screen.abs_of_view_row(y);
            let top = PAD + y as f32 * CELL_H;
            let mut run: Option<Run> = None;
            for col in 0..screen.cols() {
                let c = row.cells.get(col).copied().unwrap_or_default();
                let (fg, bg) = colors(c.style, &p);
                let x = PAD + col as f32 * CELL_W;
                if bg != p.paper {
                    frame.fill_rectangle(Point::new(x, top), cell, bg);
                }
                if range.is_some_and(|(start, end)| in_range(start, end, line, col)) {
                    frame.fill_rectangle(Point::new(x, top), cell, Color { a: 0.3, ..p.info });
                }
                if c.style.underline && c.width != 0 {
                    frame.fill_rectangle(Point::new(x, top + CELL_H - 2.0), Size::new(CELL_W, 1.0), fg);
                }
                match c.width {
                    0 => flush(&mut frame, &mut run, top),
                    2 => {
                        flush(&mut frame, &mut run, top);
                        draw_text(&mut frame, x, top, c.ch.to_string(), fg, c.style.bold);
                    }
                    _ if c.ch == ' ' => flush(&mut frame, &mut run, top),
                    _ => match &mut run {
                        Some((_, color, bold, content)) if *color == fg && *bold == c.style.bold => content.push(c.ch),
                        _ => {
                            flush(&mut frame, &mut run, top);
                            run = Some((col, fg, c.style.bold, c.ch.to_string()));
                        }
                    },
                }
            }
            flush(&mut frame, &mut run, top);
        }
        if let Some((cy, cx)) = screen.cursor_on_view() {
            let x = PAD + cx as f32 * CELL_W;
            let top = PAD + cy as f32 * CELL_H;
            if state.focused {
                frame.fill_rectangle(Point::new(x, top), cell, p.signal);
                if let Some(under) = screen.view_row(cy).and_then(|row| row.cells.get(cx)).filter(|c| c.width == 1 && c.ch != ' ') {
                    draw_text(&mut frame, x, top, under.ch.to_string(), p.paper, under.style.bold);
                }
            } else {
                let edge = 1.0;
                frame.fill_rectangle(Point::new(x, top), Size::new(CELL_W, edge), p.signal);
                frame.fill_rectangle(Point::new(x, top + CELL_H - edge), Size::new(CELL_W, edge), p.signal);
                frame.fill_rectangle(Point::new(x, top), Size::new(edge, CELL_H), p.signal);
                frame.fill_rectangle(Point::new(x + CELL_W - edge, top), Size::new(edge, CELL_H), p.signal);
            }
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, _state: &State, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        if cursor.is_over(bounds) { mouse::Interaction::Text } else { mouse::Interaction::default() }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    use apz_http::ws::Origins;
    use apz_http::{Request, Response};

    use super::*;

    fn bytes(key: Key, mods: Mods, app: bool) -> String {
        String::from_utf8(key_bytes(key, mods, app)).unwrap()
    }

    fn ctrl() -> Mods {
        Mods { ctrl: true, ..Mods::default() }
    }

    fn alt() -> Mods {
        Mods { alt: true, ..Mods::default() }
    }

    #[test]
    fn socket_paths_encode_the_workspace_as_one_segment() {
        assert_eq!(socket_path("agent:12"), "/v1/ide/terminal/agent%3A12");
        assert_eq!(socket_path("repo:app@feature/x"), "/v1/ide/terminal/repo%3Aapp%40feature%2Fx");
        assert_eq!(socket_path("agent-1_a.b~c"), "/v1/ide/terminal/agent-1_a.b~c");
    }

    #[test]
    fn hub_frames_parse_and_unknown_ones_are_skipped() {
        assert_eq!(parse_server(r#"{"t":"opened","term":"term-1"}"#), Some(Server::Opened("term-1".into())));
        let out = format!(r#"{{"t":"out","data":"{}","offset":10}}"#, STANDARD.encode(b"hi\x1b[0m"));
        assert_eq!(parse_server(&out), Some(Server::Out { data: b"hi\x1b[0m".to_vec(), offset: 10 }));
        assert_eq!(parse_server(r#"{"t":"lost","bytes":7}"#), Some(Server::Lost(7)));
        assert_eq!(parse_server(r#"{"t":"exit","code":0}"#), Some(Server::Exit(Some(0))));
        assert_eq!(parse_server(r#"{"t":"exit","code":null}"#), Some(Server::Exit(None)));
        assert_eq!(parse_server(r#"{"t":"error","message":"no such terminal"}"#), Some(Server::Error("no such terminal".into())));
        assert_eq!(parse_server(r#"{"t":"pong"}"#), Some(Server::Pong));
        assert_eq!(parse_server(r#"{"t":"novel"}"#), None);
        assert_eq!(parse_server(r#"{"t":"out","data":"!!","offset":0}"#), None, "bad base64 is ignored");
        assert_eq!(parse_server("not json"), None);
    }

    #[test]
    fn client_frames_have_the_protocol_fields() {
        let open: Value = serde_json::from_str(&open_frame(80, 24)).unwrap();
        assert_eq!((open["t"].as_str(), open["cols"].as_u64(), open["rows"].as_u64()), (Some("open"), Some(80), Some(24)));
        let attach: Value = serde_json::from_str(&attach_frame("term-1", 4096)).unwrap();
        assert_eq!((attach["t"].as_str(), attach["term"].as_str(), attach["offset"].as_u64()), (Some("attach"), Some("term-1"), Some(4096)));
        let input: Value = serde_json::from_str(&input_frame(b"ls\r")).unwrap();
        assert_eq!(STANDARD.decode(input["data"].as_str().unwrap()).unwrap(), b"ls\r");
        let resize: Value = serde_json::from_str(&resize_frame(100, 30)).unwrap();
        assert_eq!((resize["t"].as_str(), resize["cols"].as_u64(), resize["rows"].as_u64()), (Some("resize"), Some(100), Some(30)));
        assert_eq!(kill_frame(), r#"{"t":"kill"}"#);
        assert_eq!(ping_frame(), r#"{"t":"ping"}"#);
    }

    #[test]
    fn plain_and_control_keys_send_their_bytes() {
        assert_eq!(bytes(Key::Char('a'), Mods::default(), false), "a");
        assert_eq!(bytes(Key::Char('é'), Mods::default(), false), "é");
        assert_eq!(bytes(Key::Char('c'), ctrl(), false), "\u{3}", "Ctrl-C");
        assert_eq!(bytes(Key::Char('['), ctrl(), false), "\u{1b}", "Ctrl-[ is Escape");
        assert_eq!(bytes(Key::Char('d'), alt(), false), "\u{1b}d", "Alt sends ESC first");
        assert_eq!(bytes(Key::Enter, Mods::default(), false), "\r");
        assert_eq!(bytes(Key::Backspace, Mods::default(), false), "\u{7f}");
        assert_eq!(bytes(Key::Tab, Mods { shift: true, ..Mods::default() }, false), "\x1b[Z");
        assert_eq!(bytes(Key::Escape, Mods::default(), false), "\u{1b}");
    }

    #[test]
    fn arrows_follow_application_cursor_mode_and_modifiers() {
        assert_eq!(bytes(Key::Up, Mods::default(), false), "\x1b[A");
        assert_eq!(bytes(Key::Up, Mods::default(), true), "\x1bOA", "DECCKM sends SS3");
        assert_eq!(bytes(Key::Left, Mods { ctrl: true, ..Mods::default() }, false), "\x1b[1;5D", "Ctrl-Left");
        assert_eq!(bytes(Key::Home, Mods::default(), true), "\x1bOH");
        assert_eq!(bytes(Key::End, Mods::default(), false), "\x1b[F");
    }

    #[test]
    fn editing_and_function_keys() {
        assert_eq!(bytes(Key::PageUp, Mods::default(), false), "\x1b[5~");
        assert_eq!(bytes(Key::PageDown, Mods { shift: true, ..Mods::default() }, false), "\x1b[6;2~");
        assert_eq!(bytes(Key::Delete, Mods::default(), false), "\x1b[3~");
        assert_eq!(bytes(Key::Insert, Mods::default(), false), "\x1b[2~");
        assert_eq!(bytes(Key::F(1), Mods::default(), false), "\x1bOP");
        assert_eq!(bytes(Key::F(4), Mods::default(), false), "\x1bOS");
        assert_eq!(bytes(Key::F(5), Mods::default(), false), "\x1b[15~");
        assert_eq!(bytes(Key::F(12), Mods::default(), false), "\x1b[24~");
        assert_eq!(bytes(Key::F(5), ctrl(), false), "\x1b[15;5~");
    }

    #[test]
    fn paste_is_bracketed_only_when_the_program_asked() {
        assert_eq!(paste_bytes("a\nb", false), b"a\rb".to_vec());
        assert_eq!(paste_bytes("a\r\nb", false), b"a\rb".to_vec());
        assert_eq!(paste_bytes("ls", true), b"\x1b[200~ls\x1b[201~".to_vec());
    }

    #[test]
    fn the_grid_size_follows_the_view_and_the_cell() {
        let bounds = Rectangle { x: 0.0, y: 0.0, width: 2.0 * PAD + 10.0 * CELL_W, height: 2.0 * PAD + 5.0 * CELL_H };
        assert_eq!(grid_size(bounds), (10, 5));
        let tiny = Rectangle { x: 0.0, y: 0.0, width: 1.0, height: 1.0 };
        assert_eq!(grid_size(tiny), (1, 1));
    }

    #[test]
    fn the_cell_is_the_mono_font_advance() {
        use ab_glyph::{Font, FontRef};
        let font = FontRef::try_from_slice(include_bytes!("../../../../../assets/fonts/MartianMono-Regular.ttf")).unwrap();
        let advance = font.h_advance_unscaled(font.glyph_id('M')) / font.units_per_em().unwrap();
        assert!((advance - 0.7).abs() < 0.005, "Martian Mono advances {advance} em, not 0.7");
    }

    #[test]
    fn the_ansi_palette_maps_to_the_theme_roles_and_the_cube_is_exact() {
        let p = theme::palette();
        assert_eq!(indexed(1, &p, false), p.crit);
        assert_eq!(indexed(2, &p, false), p.ok);
        assert_eq!(indexed(15, &p, false), p.ink);
        assert_eq!(indexed(16, &p, false), Color::from_rgb8(0, 0, 0));
        assert_eq!(indexed(196, &p, false), Color::from_rgb8(255, 0, 0));
        assert_eq!(indexed(232, &p, false), Color::from_rgb8(8, 8, 8));
        assert_eq!(indexed(255, &p, false), Color::from_rgb8(238, 238, 238));
    }

    #[test]
    fn selections_order_their_ends_and_contain_their_cells() {
        let a = Pos { line: 5, col: 3 };
        let b = Pos { line: 4, col: 9 };
        assert_eq!(Selection { anchor: a, head: b }.range(), (b, a));
        assert!(in_range(b, a, 4, 9));
        assert!(in_range(b, a, 5, 0));
        assert!(!in_range(b, a, 5, 4));
        assert!(!in_range(b, a, 3, 99));
    }

    #[test]
    fn the_backoff_is_one_two_five_then_ten_seconds() {
        let secs: Vec<u64> = BACKOFF.iter().map(Duration::as_secs).collect();
        assert_eq!(secs, vec![1, 2, 5, 10]);
    }

    #[test]
    fn a_terminal_takes_its_hub_frames_into_the_screen() {
        let target = Target { base: "http://127.0.0.1:1".into(), ..Target::default() };
        let mut term = Terminal::new(target, "agent:1", 0);
        let _ = term.update(Msg::Link(Link::Output(b"ls\r\n".to_vec())));
        assert_eq!(term.screen().line_text(0), "ls");
        assert_eq!(term.status(), &Status::Starting);
        let _ = term.update(Msg::Link(Link::Opened("term-1".into())));
        assert_eq!(term.status(), &Status::Live);
        let _ = term.update(Msg::Link(Link::Exited(Some(3))));
        assert_eq!(term.status(), &Status::Ended(Some(3)));
        let _ = term.update(Msg::Link(Link::Disconnected("gone".into())));
        assert_eq!(term.status(), &Status::Ended(Some(3)), "an ended shell stays ended through a dropped socket");
    }

    /// Reads one client frame the hub received, and records its text.
    fn read_frame(socket: &mut Socket, frames: &std_mpsc::Sender<String>) -> Option<Value> {
        loop {
            match socket.recv(Duration::from_secs(5)) {
                Ok(Some(Message::Text(text))) => {
                    let _ = frames.send(text.clone());
                    return serde_json::from_str(&text).ok();
                }
                Ok(Some(Message::Binary(_))) => {}
                Ok(None) | Err(_) => return None,
            }
        }
    }

    fn send_frame(socket: &mut Socket, value: Value) -> io::Result<()> {
        socket.send(&Message::Text(value.to_string()))
    }

    /// One connection of a stand-in hub: the first opens a shell, streams `hello` and drops the socket
    /// (forcing a re-attach); the second attaches, echoes input and ends the shell on `kill`.
    fn hub_connection(socket: &mut Socket, connection: usize, frames: &std_mpsc::Sender<String>) {
        if connection == 0 {
            let _ = read_frame(socket, frames);
            let _ = send_frame(socket, json!({"t": "opened", "term": "term-1"}));
            let _ = send_frame(socket, json!({"t": "out", "data": STANDARD.encode(b"hello"), "offset": 0}));
            socket.close(1001, "restart");
        } else if connection == 1 {
            let _ = read_frame(socket, frames);
            let _ = send_frame(socket, json!({"t": "opened", "term": "term-1"}));
            if wait_for(socket, frames, "in").is_some() {
                let _ = send_frame(socket, json!({"t": "out", "data": STANDARD.encode(b"ls"), "offset": 5}));
            }
            let _ = wait_for(socket, frames, "kill");
            socket.close(1000, "killed");
        }
    }

    /// Reads client frames until one of kind `kind` (the others, such as the resize after an attach, are
    /// recorded and passed over).
    fn wait_for(socket: &mut Socket, frames: &std_mpsc::Sender<String>, kind: &str) -> Option<Value> {
        loop {
            let frame = read_frame(socket, frames)?;
            if frame["t"].as_str() == Some(kind) {
                return Some(frame);
            }
        }
    }

    /// A stand-in hub on loopback. Every client frame it receives is sent to `frames`.
    fn fake_hub(frames: std_mpsc::Sender<String>) -> apz_http::Handle {
        let connections = Arc::new(AtomicUsize::new(0));
        apz_http::Server::bind(0)
            .unwrap()
            .spawn(move |request: &Request| {
                if !request.path.starts_with("/v1/ide/terminal/") {
                    return Response::error(404, "no such path");
                }
                let connection = connections.fetch_add(1, Ordering::SeqCst);
                let frames = frames.clone();
                ws::accept(request, Origins::None, Some(PROTOCOL), Limits::default(), move |mut socket| {
                    hub_connection(&mut socket, connection, &frames);
                })
            })
            .unwrap()
    }

    /// Takes the thread's messages until one link event satisfies `done`, recording each event.
    fn until(rx: &mut mpsc::Receiver<Msg>, seen: &mut Vec<Link>, done: impl Fn(&Link) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match rx.try_recv() {
                Ok(Msg::Link(link)) => {
                    let stop = done(&link);
                    seen.push(link);
                    if stop {
                        return;
                    }
                }
                Ok(_) => {}
                Err(err) if err.is_closed() => panic!("the socket thread stopped: {seen:?}"),
                Err(_) => {
                    assert!(Instant::now() < deadline, "the link went quiet: {seen:?}");
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }

    #[test]
    fn the_socket_thread_opens_streams_reattaches_and_ends_the_shell() {
        let (frames_tx, frames_rx) = std_mpsc::channel::<String>();
        let hub = fake_hub(frames_tx);
        let target = Target { base: format!("http://127.0.0.1:{}", hub.port()), ..Target::default() };
        let feed = Feed { id: 9_001, target, workspace: "agent:7".into(), slot: 0 };
        let (commands, command_rx) = std_mpsc::channel::<Command>();
        let (out_tx, mut out_rx) = mpsc::channel::<Msg>(256);
        let thread = std::thread::spawn(move || supervise(feed, command_rx, out_tx));

        commands.send(Command::Size(80, 24)).unwrap();
        let mut seen = Vec::new();
        until(&mut out_rx, &mut seen, |link| matches!(link, Link::Opened(term) if term == "term-1"));
        until(&mut out_rx, &mut seen, |link| matches!(link, Link::Output(bytes) if bytes == b"hello"));
        until(&mut out_rx, &mut seen, |link| matches!(link, Link::Disconnected(_)));
        until(&mut out_rx, &mut seen, |link| matches!(link, Link::Opened(term) if term == "term-1"));
        commands.send(Command::Input(b"ls".to_vec())).unwrap();
        until(&mut out_rx, &mut seen, |link| matches!(link, Link::Output(bytes) if bytes == b"ls"));
        commands.send(Command::Kill).unwrap();
        until(&mut out_rx, &mut seen, |link| matches!(link, Link::Killed));

        let sent: Vec<Value> = frames_rx.try_iter().filter_map(|text| serde_json::from_str(&text).ok()).collect();
        let kinds: Vec<&str> = sent.iter().filter_map(|frame| frame["t"].as_str()).collect();
        // The re-attach tells the hub the view's size again, since the shell may have been resized elsewhere.
        assert_eq!(kinds, vec!["open", "attach", "resize", "in", "kill"]);
        assert_eq!(sent[0]["cols"].as_u64(), Some(80));
        assert_eq!(sent[1]["term"].as_str(), Some("term-1"));
        assert_eq!(sent[1]["offset"].as_u64(), Some(5), "the re-attach resumes after the five bytes it had");
        assert_eq!((sent[2]["cols"].as_u64(), sent[2]["rows"].as_u64()), (Some(80), Some(24)));
        assert_eq!(STANDARD.decode(sent[3]["data"].as_str().unwrap()).unwrap(), b"ls");

        drop(out_rx);
        drop(hub);
        thread.join().unwrap();
    }

    #[test]
    fn the_tabs_hold_at_most_four_and_reuse_a_closed_slot() {
        let target = Target { base: "http://127.0.0.1:1".into(), ..Target::default() };
        let mut tabs = TerminalTabs::new(target, "agent:1");
        assert_eq!(tabs.len(), 1);
        assert!(tabs.add() && tabs.add() && tabs.add());
        assert!(!tabs.add(), "a fifth terminal is refused");
        assert_eq!(tabs.len(), MAX_TABS);
        let slots: Vec<usize> = (0..tabs.len()).map(|i| tabs.get(i).unwrap().slot()).collect();
        assert_eq!(slots, vec![0, 1, 2, 3]);
        tabs.remove(1);
        assert!(tabs.add());
        assert_eq!(tabs.get(tabs.active()).unwrap().slot(), 1, "the freed slot comes back");
    }
}
