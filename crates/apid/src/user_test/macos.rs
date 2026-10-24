//! The macOS app target (PLAN 19.3, W47): an app on a Mac node, read through the macOS Accessibility API and driven and
//! captured through the station's workstation tools (`apz-desktop-macos` on the node).
//!
//! - The tree is the app's (`station_ui_tree` with the app's name): each node is an element. An element's id is its child
//!   path from the app's root (`root` for the root), so an action finds the same node again.
//! - An action is a `station_ui_act` on the node the id names: its path, role and name are the selector, and the station
//!   refuses a selector that no longer matches, so a tree that moved on is not acted on by mistake. A click invokes the
//!   control; typing sets its value; a key goes to the focused control (`station_key`).
//! - The picture is `station_screenshot`, a PNG.
//!
//! Every call goes through [`Calls`] as the target's agent, under the pixel lease the target holds (the screen tools need
//! one). The lease is released when the target is dropped.

use std::collections::BTreeMap;
use std::time::Instant;

use apz_desktop_macos::simctl::shell_line;
use base64::Engine as _;
use serde::Deserialize;
use serde_json::json;

use super::desk::{Calls, release_lease};
use super::target::{Action, Element, Screenshot, Target, Tree};

/// The screenshot scale the checker reads (the station's default).
const SCALE: f64 = 0.5;
/// The most elements one tree carries.
const MAX_ELEMENTS: usize = 400;

/// One app on a Mac node, under a pixel lease the target holds.
pub struct MacApp<'a> {
    calls: Box<dyn Calls + 'a>,
    station: String,
    lease: String,
    app: String,
    depth: u8,
    started: Instant,
    /// The controls of the last observed tree, by element id.
    seen: BTreeMap<String, Seen>,
}

/// What an element was in the station's tree: the raw role and name the selector needs, its path, and the actions it takes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Seen {
    role: String,
    name: Option<String>,
    path: Vec<u32>,
    actions: Vec<String>,
}

impl Seen {
    fn describe(&self) -> String {
        match &self.name {
            Some(name) => format!("{} \"{name}\"", self.role),
            None => self.role.clone(),
        }
    }
}

/// A node of the station's UI tree, as `apz_desktop::UiNode` encodes it (its id and bounds are not needed here).
#[derive(Deserialize)]
struct Node {
    #[serde(default)]
    role: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    actions: Vec<String>,
    #[serde(default)]
    children: Vec<Node>,
}

impl<'a> MacApp<'a> {
    /// A target on `app` of the station `station`, under the pixel lease `lease` the calls' agent holds there.
    pub fn new(
        calls: Box<dyn Calls + 'a>,
        station: &str,
        lease: String,
        app: &str,
        depth: u8,
    ) -> Self {
        Self {
            calls,
            station: station.to_string(),
            lease,
            app: app.to_string(),
            depth,
            started: Instant::now(),
            seen: BTreeMap::new(),
        }
    }

    /// Starts the app fresh: it is quit when it runs (an app that is not running is left alone), then launched. The quit is
    /// an AppleScript on the node, so the app's name must be one a script can hold between its quotes.
    pub fn relaunch(&self) -> Result<(), String> {
        if self
            .app
            .chars()
            .any(|c| c == '"' || c == '\\' || c.is_control())
        {
            return Err(format!(
                "{:?} is not an app name a script can quit",
                self.app
            ));
        }
        let quit = format!("tell application \"{}\" to quit", self.app);
        let script = shell_line(&["osascript".into(), "-e".into(), quit]);
        self.calls.call(
            "app_script",
            &json!({"station": self.station, "bridge": "cli", "script": script, "timeout_ms": 15000}),
        )?;
        self.calls
            .call(
                "station_app_launch",
                &json!({"station": self.station, "app": self.app}),
            )
            .map(|_| ())
    }

    fn target_of(&self, element: &str) -> Result<Seen, String> {
        self.seen
            .get(element)
            .cloned()
            .ok_or_else(|| format!("there is no element {element} on {} now", self.app))
    }

    /// A control action (`invoke`, `set_value`) on the element `seen` describes, when the control takes it.
    fn ui_act(&self, seen: &Seen, act: &str, value: Option<&str>) -> Result<(), String> {
        if !seen.actions.iter().any(|offered| offered == act) {
            let offered = if seen.actions.is_empty() {
                "no action".to_string()
            } else {
                seen.actions.join(", ")
            };
            return Err(format!(
                "{} takes no {act} (its accessibility actions: {offered})",
                seen.describe()
            ));
        }
        let mut selector = json!({"app": self.app, "role": seen.role, "path": seen.path});
        if let Some(name) = &seen.name {
            selector["name"] = json!(name);
        }
        let mut input =
            json!({"station": self.station, "lease": self.lease, "selector": selector, "act": act});
        if let Some(value) = value {
            input["value"] = json!(value);
        }
        self.calls.call("station_ui_act", &input).map(|_| ())
    }
}

/// The target's role for a station role: the roles a driver acts on (`ACTIONABLE`) and the text and headings a checker reads.
fn target_role(raw: &str) -> &str {
    match raw {
        "button" => "button",
        "link" => "link",
        "text_field" | "text_area" | "search_field" | "secure_text_field" | "combo_box" => {
            "textbox"
        }
        "check_box" | "radio_button" | "switch" => "checkbox",
        "static_text" => "text",
        other => other,
    }
}

/// Flattens the station's tree into elements, and remembers each element's selector.
fn flatten(
    node: &Node,
    path: &mut Vec<u32>,
    elements: &mut Vec<Element>,
    seen: &mut BTreeMap<String, Seen>,
) {
    let id = if path.is_empty() {
        "root".to_string()
    } else {
        path.iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(".")
    };
    if !node.role.is_empty() && elements.len() < MAX_ELEMENTS {
        elements.push(Element {
            id: id.clone(),
            role: target_role(&node.role).to_string(),
            label: node.name.clone().unwrap_or_default(),
            value: node.value.clone().unwrap_or_default(),
        });
        seen.insert(
            id,
            Seen {
                role: node.role.clone(),
                name: node.name.clone(),
                path: path.clone(),
                actions: node.actions.clone(),
            },
        );
    }
    for (i, child) in node.children.iter().enumerate() {
        path.push(i as u32);
        flatten(child, path, elements, seen);
        path.pop();
    }
}

impl Target for MacApp<'_> {
    fn kind(&self) -> &str {
        "macos"
    }

    fn tree(&mut self) -> Result<Tree, String> {
        let value = self.calls.call(
            "station_ui_tree",
            &json!({"station": self.station, "lease": self.lease, "app": self.app, "depth": self.depth}),
        )?;
        let root: Node = serde_json::from_value(value)
            .map_err(|e| format!("the station's tree of {}: {e}", self.app))?;
        let mut elements = Vec::new();
        self.seen.clear();
        flatten(&root, &mut Vec::new(), &mut elements, &mut self.seen);
        // The screen's title is its frontmost window's, else the app's.
        let title = root
            .children
            .iter()
            .find(|window| window.role == "window")
            .and_then(|window| window.name.clone())
            .or_else(|| root.name.clone())
            .unwrap_or_else(|| self.app.clone());
        Ok(Tree { title, elements })
    }

    fn screenshot(&mut self) -> Result<Screenshot, String> {
        let shot = self.calls.call(
            "station_screenshot",
            &json!({"station": self.station, "lease": self.lease, "scale": SCALE, "format": "png"}),
        )?;
        let data = shot["data_b64"]
            .as_str()
            .ok_or("the station sent no picture")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| format!("the station's picture: {e}"))?;
        Ok(Screenshot {
            mime: "image/png".into(),
            bytes,
        })
    }

    fn act(&mut self, action: &Action) -> Result<(), String> {
        match action {
            Action::Click { element } => {
                let seen = self.target_of(element)?;
                self.ui_act(&seen, "invoke", None)
            }
            Action::Type { element, text } => {
                let seen = self.target_of(element)?;
                self.ui_act(&seen, "set_value", Some(text))
            }
            Action::Press { key } => self
                .calls
                .call(
                    "station_key",
                    &json!({"station": self.station, "lease": self.lease, "combo": key}),
                )
                .map(|_| ()),
        }
    }

    fn clock_ms(&mut self) -> Option<u64> {
        Some(self.started.elapsed().as_millis() as u64)
    }
}

impl Drop for MacApp<'_> {
    fn drop(&mut self) {
        if let Err(why) = release_lease(self.calls.as_ref(), &self.station, &self.lease) {
            eprintln!("apid: a macOS user test's lease {}: {why}", self.lease);
        }
    }
}
