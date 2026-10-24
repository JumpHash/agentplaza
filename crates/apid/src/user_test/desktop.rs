//! The desktop targets (PLAN 19.3): a Linux or Windows workstation's screen, read and driven the way an agent drives it.
//!
//! A station is a machine the hub reaches only through its daemon's link, so a desktop target is a station with a pixel
//! lease, and every call goes through the station tools (`stations/tools.rs`) and their choke point: the laws, the audit
//! and the lease rules apply to a user test as they do to an agent. On the station the daemon answers from its own
//! backends: the tree from AT-SPI on Linux (`apz-desktop-atspi`, through the X11 session of `apz-desktop-x11`) or from UI
//! Automation on Windows (`apz-desktop-windows`, in the station VM); the picture from XShm or desktop duplication; the
//! input from XTest or SendInput. The hub's side is one [`Gui`] target over a [`Calls`]: [`Hub`] in production, a fake
//! station in the tests (`desktop_test.rs`).
//!
//! The Linux target runs on a Linux station, whose daemon starts its own Xvfb. A `gui` fork with an agent inside it is
//! not built: a fork has no daemon to answer the tools.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{ErrorKind, Read};
use std::time::{Duration, Instant};

use apz_link::inbound::Duplex;
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Value, json};

use super::target::{Action, Element, Screenshot, Target, Tree};
use crate::state::App;
use crate::stations::tools;

/// The operating systems a desktop target can run on: the station's facts name them.
pub const LINUX: &str = "linux";
pub const WINDOWS: &str = "windows";
/// The one station a Linux desktop target names: the lane's test fork, whose agent answers as a station would.
pub const FORK_STATION: &str = "lane-fork";
/// How long one request to the fork's agent may take (a screenshot of a large session included).
const CALL: Duration = Duration::from_secs(60);
/// The depth of the tree the driver reads (the daemon clamps a depth to 8).
const DEPTH: u8 = 8;
/// The most elements one tree carries.
const MAX_ELEMENTS: usize = 400;

/// The station tools a desktop target calls: `station_list`, `station_lease`, `station_release`, `station_ui_tree`,
/// `station_ui_act`, `station_click`, `station_type`, `station_key` and `station_screenshot`, with their JSON inputs.
pub trait Calls {
    fn call(&self, tool: &str, input: &Value) -> Result<Value, String>;
}

impl<T: Calls + ?Sized> Calls for &T {
    fn call(&self, tool: &str, input: &Value) -> Result<Value, String> {
        (**self).call(tool, input)
    }
}

/// The station tools as the run's own session (its agent `cid`), through the choke point: the laws and the audit see
/// the session that runs the step.
pub struct Hub<'a> {
    pub app: &'a App,
    pub cid: u64,
}

impl Calls for Hub<'_> {
    fn call(&self, tool: &str, input: &Value) -> Result<Value, String> {
        tools::call(self.app, self.cid, tool, input)
    }
}

/// The agent in a lane's test fork, as the Linux desktop target's one station. It holds one connection to the fork's port
/// stream (`dial` makes it), and sends a request per line and reads a reply per line (the agent's wire, `apz-gui-agent`).
/// `station_list` answers from the agent's `hello`: the fork is online and runs the OS the agent reports.
pub struct Forked<'a> {
    dial: RefCell<Box<dyn FnMut() -> Result<Box<dyn Duplex>, String> + 'a>>,
    conn: RefCell<Option<Box<dyn Duplex>>>,
    pending: RefCell<Vec<u8>>,
}

impl<'a> Forked<'a> {
    pub fn new(dial: impl FnMut() -> Result<Box<dyn Duplex>, String> + 'a) -> Forked<'a> {
        Forked {
            dial: RefCell::new(Box::new(dial)),
            conn: RefCell::new(None),
            pending: RefCell::new(Vec::new()),
        }
    }

    /// One request and its reply. A connection that fails is dropped, and the next request dials a new one.
    fn send(&self, request: &Value) -> Result<Value, String> {
        let mut slot = self.conn.borrow_mut();
        if slot.is_none() {
            *slot = Some((self.dial.borrow_mut())()?);
        }
        let Some(conn) = slot.as_mut() else {
            return Err("the desktop agent has no connection".into());
        };
        let outcome = exchange(conn.as_mut(), &mut self.pending.borrow_mut(), request);
        if outcome.is_err() {
            *slot = None;
            self.pending.borrow_mut().clear();
        }
        outcome
    }
}

impl Calls for Forked<'_> {
    fn call(&self, tool: &str, input: &Value) -> Result<Value, String> {
        if tool == "station_list" {
            let hello = self.send(&json!({"tool": "hello", "input": {}}))?;
            let os = hello["os"].as_str().unwrap_or_default();
            return Ok(json!([{
                "id": FORK_STATION,
                "online": true,
                "facts": {"os": os},
            }]));
        }
        self.send(&json!({"tool": tool, "input": input}))
    }
}

/// Writes `request` and reads its reply from `conn`.
fn exchange(
    conn: &mut dyn Duplex,
    pending: &mut Vec<u8>,
    request: &Value,
) -> Result<Value, String> {
    let mut line = request.to_string();
    line.push('\n');
    conn.write_all(line.as_bytes())
        .and_then(|()| conn.flush())
        .map_err(|why| format!("the desktop agent: {why}"))?;
    let reply = read_line(conn, pending, Instant::now() + CALL)?;
    let reply: Value =
        serde_json::from_str(&reply).map_err(|why| format!("the desktop agent's reply: {why}"))?;
    if reply["ok"] == true {
        Ok(reply["value"].clone())
    } else {
        Err(reply["error"]
            .as_str()
            .unwrap_or("the desktop agent refused the call")
            .to_string())
    }
}

/// The next line of the connection, read through the poll interval of a port stream until `deadline`. Bytes past the
/// line stay in `pending` for the next reply.
fn read_line(
    conn: &mut dyn Read,
    pending: &mut Vec<u8>,
    deadline: Instant,
) -> Result<String, String> {
    loop {
        if let Some(end) = pending.iter().position(|b| *b == b'\n') {
            let rest = pending.split_off(end + 1);
            let mut line = std::mem::replace(pending, rest);
            line.truncate(end);
            return String::from_utf8(line)
                .map_err(|why| format!("the desktop agent's reply: {why}"));
        }
        if Instant::now() >= deadline {
            return Err("the desktop agent did not answer in time".into());
        }
        let mut chunk = vec![0u8; 64 * 1024];
        match conn.read(&mut chunk) {
            Ok(0) => return Err("the desktop agent closed the connection".into()),
            Ok(n) => pending.extend_from_slice(&chunk[..n]),
            Err(why)
                if matches!(
                    why.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) => {}
            Err(why) => return Err(format!("the desktop agent: {why}")),
        }
    }
}

/// The target's kind, for the report, from the operating system the step names.
pub fn kind_of(os: &str) -> Result<&'static str, String> {
    match os {
        LINUX => Ok("linux-desktop"),
        WINDOWS => Ok("windows-desktop"),
        other => Err(format!(
            "a desktop target runs on linux or windows, not {other}"
        )),
    }
}

/// A control the last tree showed: its child-index path (the daemon's selector), its box and its accessibility actions.
#[derive(Clone, Debug)]
struct Known {
    path: Vec<u32>,
    role: String,
    bounds: (i32, i32, u32, u32),
    actions: Vec<String>,
}

/// One node of the daemon's UI tree, as `apz_desktop::UiNode` encodes it.
#[derive(Deserialize)]
struct Node {
    role: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    value: Option<String>,
    bounds: (i32, i32, u32, u32),
    #[serde(default)]
    actions: Vec<String>,
    #[serde(default)]
    children: Vec<Node>,
}

/// A leased station's desktop as a target. The lease is held for the target's life and released when it goes.
pub struct Gui<'a> {
    calls: Box<dyn Calls + 'a>,
    kind: &'static str,
    station: String,
    lease: String,
    known: BTreeMap<String, Known>,
    started: Instant,
}

impl<'a> Gui<'a> {
    /// Opens the desktop of `station`: the station must be online, report `os` in its facts, and grant a pixel session
    /// to the lease the target takes. A lease that is still queued is released again and the open fails.
    pub fn open(calls: Box<dyn Calls + 'a>, station: &str, os: &str) -> Result<Gui<'a>, String> {
        let kind = kind_of(os)?;
        let rows = calls.call("station_list", &json!({}))?;
        let row = rows
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["id"] == station))
            .ok_or_else(|| format!("there is no station {station}"))?;
        if row["online"] != true {
            return Err(format!("station {station} is not online"));
        }
        let runs = row["facts"]["os"].as_str().unwrap_or("unknown");
        if runs != os {
            return Err(format!(
                "station {station} runs {runs}, and the step's desktop is {os}"
            ));
        }
        let view = calls.call(
            "station_lease",
            &json!({"station": station, "kind": "pixel"}),
        )?;
        let lease = view["id"]
            .as_str()
            .ok_or_else(|| format!("station {station} answered a lease without an id"))?
            .to_string();
        if view["session"].is_null() {
            let _ = calls.call(
                "station_release",
                &json!({"station": station, "lease": lease}),
            );
            return Err(format!(
                "station {station} has no pixel session yet (queued at {})",
                view["position"]
            ));
        }
        Ok(Gui {
            calls,
            kind,
            station: station.to_string(),
            lease,
            known: BTreeMap::new(),
            started: Instant::now(),
        })
    }

    /// A station tool, with the station and the lease the target holds.
    fn send(&self, tool: &str, mut input: Value) -> Result<Value, String> {
        input["station"] = json!(self.station);
        input["lease"] = json!(self.lease);
        self.calls.call(tool, &input)
    }

    /// The control an element id named by the last tree.
    fn control(&self, element: &str) -> Result<&Known, String> {
        self.known
            .get(element)
            .ok_or_else(|| format!("no element {element} on the screen"))
    }

    /// A pointer click at the middle of the control's box. A control with no box is pressed through its `invoke` action.
    fn click(&self, element: &str) -> Result<(), String> {
        let known = self.control(element)?;
        let (x, y, w, h) = known.bounds;
        if w > 0 && h > 0 {
            let (cx, cy) = (x + (w / 2) as i32, y + (h / 2) as i32);
            self.send("station_click", json!({"x": cx, "y": cy, "scale": 1.0}))?;
        } else if known.actions.iter().any(|a| a == "invoke") {
            let selector = json!({"path": known.path});
            self.send(
                "station_ui_act",
                json!({"selector": selector, "act": "invoke"}),
            )?;
        }
        Ok(())
    }
}

impl Drop for Gui<'_> {
    fn drop(&mut self) {
        let released = self.calls.call(
            "station_release",
            &json!({"station": self.station, "lease": self.lease}),
        );
        if let Err(why) = released {
            eprintln!("apid: a desktop target's lease: {why}");
        }
    }
}

impl Target for Gui<'_> {
    fn kind(&self) -> &str {
        self.kind
    }

    fn tree(&mut self) -> Result<Tree, String> {
        let answer = self.send("station_ui_tree", json!({"depth": DEPTH}))?;
        let root: Node = serde_json::from_value(answer)
            .map_err(|why| format!("the station's UI tree: {why}"))?;
        self.known.clear();
        let mut elements = Vec::new();
        flatten(&root, &mut Vec::new(), &mut elements, &mut self.known);
        Ok(Tree {
            title: title_of(&root),
            elements,
        })
    }

    fn screenshot(&mut self) -> Result<Screenshot, String> {
        let shot = self.send("station_screenshot", json!({"scale": 1.0, "format": "png"}))?;
        let data = shot["data_b64"]
            .as_str()
            .ok_or("the station's screenshot has no picture")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|why| format!("the station's screenshot: {why}"))?;
        let mime = match shot["format"].as_str() {
            Some("jpeg") => "image/jpeg",
            _ => "image/png",
        };
        Ok(Screenshot {
            mime: mime.into(),
            bytes,
        })
    }

    fn act(&mut self, action: &Action) -> Result<(), String> {
        match action {
            Action::Click { element } => self.click(element),
            Action::Type { element, text } => {
                if self.control(element)?.role != "textbox" {
                    return Err(format!("{element} is not a textbox"));
                }
                self.click(element)?;
                self.send("station_type", json!({"text": text}))?;
                Ok(())
            }
            Action::Press { key } => {
                self.send("station_key", json!({"combo": key}))?;
                Ok(())
            }
        }
    }

    fn clock_ms(&mut self) -> Option<u64> {
        Some(u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX))
    }
}

/// The element id of the node at `path`: `n` for the root, `n.0.2` for the root's third child's first child.
fn element_id(path: &[u32]) -> String {
    path.iter().fold(String::from("n"), |mut id, index| {
        id.push('.');
        id.push_str(&index.to_string());
        id
    })
}

/// The screen's role of a daemon role: the ones the driver acts on are `button`, `link`, `textbox` and `checkbox`.
fn screen_role(role: &str) -> String {
    let mapped = match role {
        "toggle_button" => "button",
        "text_field" | "password_field" => "textbox",
        "check_box" => "checkbox",
        other => other,
    };
    mapped.to_string()
}

/// Flattens the tree in document order, recording each control's path for the actions.
fn flatten(
    node: &Node,
    path: &mut Vec<u32>,
    out: &mut Vec<Element>,
    known: &mut BTreeMap<String, Known>,
) {
    if out.len() < MAX_ELEMENTS {
        let id = element_id(path);
        let role = screen_role(&node.role);
        out.push(Element {
            id: id.clone(),
            role: role.clone(),
            label: node.name.clone().unwrap_or_default(),
            value: node.value.clone().unwrap_or_default(),
        });
        known.insert(
            id,
            Known {
                path: path.clone(),
                role,
                bounds: node.bounds,
                actions: node.actions.clone(),
            },
        );
    }
    for (i, child) in node.children.iter().enumerate() {
        path.push(u32::try_from(i).unwrap_or(u32::MAX));
        flatten(child, path, out, known);
        path.pop();
    }
}

/// The screen's title: the first named window of the tree, else the root's name.
fn title_of(root: &Node) -> String {
    fn window(node: &Node) -> Option<&str> {
        if node.role == "window"
            && let Some(name) = node.name.as_deref().filter(|name| !name.is_empty())
        {
            return Some(name);
        }
        node.children.iter().find_map(window)
    }
    window(root)
        .map(str::to_string)
        .or_else(|| root.name.clone())
        .unwrap_or_default()
}
