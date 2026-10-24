//! The macOS app target (PLAN 19.3, W47): one app on a Mac node, read through the macOS Accessibility API and driven and
//! captured through the station's workstation tools (`apz-desktop-macos` on the node), on the owner's screen.
//!
//! - The session is the owner's screen: the target's lease is `owner_screen` (the Mac daemon serves no virtual display), and
//!   every call is the target's agent's, under that lease, through the station's choke point (its laws and audit).
//! - The app is launched from its bundle path (`open -n -a`) and quit by its bundle id at the end of the step. Its name is
//!   the scope of every tree read; the target never reads, acts on or captures another app's controls or windows.
//! - The tree is the app's: each node is an element, and an element's id is its child path from the app's root.
//! - An action on a control that offers `invoke` is a `station_ui_act` on the node its selector names (path, role and
//!   name, so a tree that moved on is refused). A control that offers no invoke is clicked at its centre, but only when that
//!   point lies inside one of the app's windows; anything else is refused and nothing is sent.
//! - A key goes to the focused control after the app is brought to the front.
//! - A picture is the screen's, cropped to the app's windows. The window rectangles are in screen points; the picture's
//!   pixels are scaled from them by the picture's size over the display's.

use std::collections::BTreeMap;
use std::io::Cursor;
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

/// A rectangle of the screen in points: x, y, width, height.
type Rect = (i32, i32, u32, u32);

/// One app on a Mac node, under an owner-screen lease the target holds.
pub struct MacApp<'a> {
    calls: Box<dyn Calls + 'a>,
    station: String,
    lease: String,
    app: String,
    bundle: String,
    path: String,
    depth: u8,
    started: Instant,
    /// The controls of the last observed tree, by element id.
    seen: BTreeMap<String, Seen>,
    /// The app's windows in the last observed tree (points): the scope of its pictures and pointer input.
    windows: Vec<Rect>,
    /// The display's size in points, read once.
    display: Option<(u32, u32)>,
    /// The size in pixels of the last full picture the station sent (the pointer's coordinates are read from it).
    picture: Option<(u32, u32)>,
}

/// What an element was in the station's tree: the raw role and name the selector needs, its path, the actions it takes and
/// its bounds in points.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Seen {
    role: String,
    name: Option<String>,
    path: Vec<u32>,
    actions: Vec<String>,
    bounds: Option<Rect>,
}

impl Seen {
    fn describe(&self) -> String {
        match &self.name {
            Some(name) => format!("{} \"{name}\"", self.role),
            None => self.role.clone(),
        }
    }
}

/// A node of the station's UI tree, as `apz_desktop::UiNode` encodes it (its id is not needed here).
#[derive(Deserialize)]
struct Node {
    #[serde(default)]
    role: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    bounds: Option<Rect>,
    #[serde(default)]
    actions: Vec<String>,
    #[serde(default)]
    children: Vec<Node>,
}

/// Whether a rectangle has a size a window can have.
fn has_area(rect: &Rect) -> bool {
    rect.2 > 0 && rect.3 > 0
}

/// Whether the point (`x`, `y`) lies in `rect` (its left and top edges in, its right and bottom edges out).
fn inside(rect: &Rect, x: i64, y: i64) -> bool {
    let (left, top) = (i64::from(rect.0), i64::from(rect.1));
    x >= left && y >= top && x < left + i64::from(rect.2) && y < top + i64::from(rect.3)
}

impl<'a> MacApp<'a> {
    /// A target on `app` (its name, the scope of its tree and pictures), launched from `path` and quit by `bundle`, under the
    /// owner-screen lease `lease` the calls' agent holds on `station`.
    pub fn new(
        calls: Box<dyn Calls + 'a>,
        station: &str,
        lease: String,
        app: &str,
        bundle: &str,
        path: &str,
        depth: u8,
    ) -> Self {
        Self {
            calls,
            station: station.to_string(),
            lease,
            app: app.to_string(),
            bundle: bundle.to_string(),
            path: path.to_string(),
            depth,
            started: Instant::now(),
            seen: BTreeMap::new(),
            windows: Vec::new(),
            display: None,
            picture: None,
        }
    }

    /// Starts the app fresh: a copy of it that runs is quit, then the bundle is opened as a new instance.
    pub fn relaunch(&self) -> Result<(), String> {
        let _ = quit(self.calls.as_ref(), &self.station, &self.bundle);
        self.calls
            .call(
                "app_script",
                &json!({"station": self.station, "bridge": "cli", "script": open_line(&self.path), "timeout_ms": 30000}),
            )?;
        Ok(())
    }

    /// Brings the app to the front, so that keys and pointer input reach its windows.
    fn activate(&self) -> Result<(), String> {
        self.script(&format!(
            "tell application id \"{}\" to activate",
            self.bundle
        ))
    }

    fn script(&self, applescript: &str) -> Result<(), String> {
        let line = shell_line(&["osascript".into(), "-e".into(), applescript.into()]);
        let answer = self.calls.call(
            "app_script",
            &json!({"station": self.station, "bridge": "cli", "script": line, "timeout_ms": 15000}),
        )?;
        if answer["exit_code"].as_i64() == Some(0) {
            Ok(())
        } else {
            Err(format!(
                "the script on {} failed: {}",
                self.station,
                answer["output"].as_str().unwrap_or_default().trim()
            ))
        }
    }

    fn target_of(&self, element: &str) -> Result<Seen, String> {
        self.seen
            .get(element)
            .cloned()
            .ok_or_else(|| format!("there is no element {element} on {} now", self.app))
    }

    /// The display's size in points: the desktop's root bounds, read once (a depth-one read names no window).
    fn display_size(&mut self) -> Result<(u32, u32), String> {
        if let Some(size) = self.display {
            return Ok(size);
        }
        let desk = self.calls.call(
            "station_ui_tree",
            &json!({"station": self.station, "lease": self.lease, "depth": 1}),
        )?;
        let root: Node = serde_json::from_value(desk).map_err(|e| format!("the display: {e}"))?;
        let (_, _, w, h) = root
            .bounds
            .filter(has_area)
            .ok_or("the station gave the display no size")?;
        self.display = Some((w, h));
        Ok((w, h))
    }

    /// Whether (`x`, `y`) in points lies in one of the app's windows.
    fn in_app_window(&self, x: i64, y: i64) -> bool {
        self.windows.iter().any(|rect| inside(rect, x, y))
    }

    /// A pointer click at the centre of the element's bounds, refused unless that point is in the app's windows.
    fn pointer_click(&mut self, seen: &Seen) -> Result<(), String> {
        let (x, y, w, h) = seen
            .bounds
            .ok_or_else(|| format!("{} has no bounds to click at", seen.describe()))?;
        let (cx, cy) = (
            i64::from(x) + i64::from(w) / 2,
            i64::from(y) + i64::from(h) / 2,
        );
        if !self.in_app_window(cx, cy) {
            return Err(format!(
                "{} is outside the windows of {}: pointer input there is refused",
                seen.describe(),
                self.app
            ));
        }
        let (dw, _) = self.display_size()?;
        let (pw, _) = self.picture.ok_or("no picture of the screen to aim at")?;
        let scale = f64::from(pw) / f64::from(dw);
        self.activate()?;
        let point = |v: i64| (v as f64 * scale).round() as i64;
        self.calls
            .call(
                "station_click",
                &json!({
                    "station": self.station,
                    "lease": self.lease,
                    "x": point(cx),
                    "y": point(cy),
                    "scale": SCALE,
                    "button": "left",
                    "count": 1,
                }),
            )
            .map(|_| ())
    }

    /// A control action (`invoke`, `set_value`) on the element `seen` describes, when the control takes it.
    fn ui_act(&self, seen: &Seen, act: &str, value: Option<&str>) -> Result<(), String> {
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

/// The line that opens a bundle as a new instance of its app.
fn open_line(path: &str) -> String {
    shell_line(&["open".into(), "-n".into(), "-a".into(), path.into()])
}

/// Quits the app `bundle` on `station` (it is asked to quit, and so may save).
pub fn quit(calls: &dyn Calls, station: &str, bundle: &str) -> Result<(), String> {
    let script = format!("tell application id \"{bundle}\" to quit");
    let line = shell_line(&["osascript".into(), "-e".into(), script]);
    let answer = calls.call(
        "app_script",
        &json!({"station": station, "bridge": "cli", "script": line, "timeout_ms": 15000}),
    )?;
    if answer["exit_code"].as_i64() == Some(0) {
        Ok(())
    } else {
        Err(format!(
            "{bundle} did not quit on {station}: {}",
            answer["output"].as_str().unwrap_or_default().trim()
        ))
    }
}

/// The target's role for a station role: the roles a driver acts on and the text and headings a checker reads.
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

/// Flattens the station's tree into elements, and remembers each element's selector and bounds.
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
                bounds: node.bounds,
            },
        );
    }
    for (i, child) in node.children.iter().enumerate() {
        path.push(i as u32);
        flatten(child, path, elements, seen);
        path.pop();
    }
}

/// The picture cropped to `rect` (in its pixels). The picture is a PNG of 8-bit channels.
fn crop(png_bytes: &[u8], rect: (u32, u32, u32, u32)) -> Result<Vec<u8>, String> {
    let (x, y, w, h) = rect;
    let mut reader = png::Decoder::new(Cursor::new(png_bytes))
        .read_info()
        .map_err(|e| format!("the station's picture: {e}"))?;
    let size = reader
        .output_buffer_size()
        .ok_or("the station's picture is too large")?;
    let mut buf = vec![0u8; size];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("the station's picture: {e}"))?;
    let channels = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        other => {
            return Err(format!(
                "the station's picture is {other:?}, not a plain one"
            ));
        }
    };
    if info.bit_depth != png::BitDepth::Eight {
        return Err("the station's picture is not 8 bits a channel".into());
    }
    if x.checked_add(w).is_none_or(|right| right > info.width)
        || y.checked_add(h).is_none_or(|bottom| bottom > info.height)
        || w == 0
        || h == 0
    {
        return Err("the app's windows are not on the picture".into());
    }
    let mut out = Vec::with_capacity(w as usize * h as usize * channels);
    for row in y..y + h {
        let start = row as usize * info.line_size + x as usize * channels;
        out.extend_from_slice(&buf[start..start + w as usize * channels]);
    }
    let mut encoded = Vec::new();
    let mut encoder = png::Encoder::new(&mut encoded, w, h);
    encoder.set_color(info.color_type);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("the cropped picture: {e}"))?;
    writer
        .write_image_data(&out)
        .map_err(|e| format!("the cropped picture: {e}"))?;
    writer
        .finish()
        .map_err(|e| format!("the cropped picture: {e}"))?;
    Ok(encoded)
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
        self.windows = root
            .children
            .iter()
            .filter(|window| window.role == "window")
            .filter_map(|window| window.bounds.filter(has_area))
            .collect();
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
        if self.windows.is_empty() {
            return Err(format!("no window of {} is on screen to capture", self.app));
        }
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
        let (pw, ph) = (
            shot["w"].as_u64().unwrap_or(0) as u32,
            shot["h"].as_u64().unwrap_or(0) as u32,
        );
        if pw == 0 || ph == 0 {
            return Err("the station sent a picture with no size".into());
        }
        self.picture = Some((pw, ph));
        let (dw, dh) = self.display_size()?;
        let (sx, sy) = (f64::from(pw) / f64::from(dw), f64::from(ph) / f64::from(dh));
        // The union of the app's windows, in the picture's pixels, clipped to the picture.
        let (mut left, mut top, mut right, mut bottom) = (u32::MAX, u32::MAX, 0u32, 0u32);
        for &(x, y, w, h) in &self.windows {
            let l = (f64::from(x) * sx).floor().max(0.0) as u32;
            let t = (f64::from(y) * sy).floor().max(0.0) as u32;
            let r = ((f64::from(x) + f64::from(w)) * sx)
                .ceil()
                .min(f64::from(pw)) as u32;
            let b = ((f64::from(y) + f64::from(h)) * sy)
                .ceil()
                .min(f64::from(ph)) as u32;
            if r > l && b > t {
                left = left.min(l);
                top = top.min(t);
                right = right.max(r);
                bottom = bottom.max(b);
            }
        }
        if right <= left || bottom <= top {
            return Err(format!("the windows of {} are not on the screen", self.app));
        }
        let cropped = crop(&bytes, (left, top, right - left, bottom - top))?;
        Ok(Screenshot {
            mime: "image/png".into(),
            bytes: cropped,
        })
    }

    fn act(&mut self, action: &Action) -> Result<(), String> {
        match action {
            Action::Click { element } => {
                let seen = self.target_of(element)?;
                if seen.actions.iter().any(|offered| offered == "invoke") {
                    self.ui_act(&seen, "invoke", None)
                } else {
                    self.pointer_click(&seen)
                }
            }
            Action::Type { element, text } => {
                let seen = self.target_of(element)?;
                if !seen.actions.iter().any(|offered| offered == "set_value") {
                    return Err(format!(
                        "{} takes no set_value (its accessibility actions: {})",
                        seen.describe(),
                        if seen.actions.is_empty() {
                            "none".to_string()
                        } else {
                            seen.actions.join(", ")
                        }
                    ));
                }
                self.ui_act(&seen, "set_value", Some(text))
            }
            Action::Press { key } => {
                self.activate()?;
                self.calls
                    .call(
                        "station_key",
                        &json!({"station": self.station, "lease": self.lease, "combo": key}),
                    )
                    .map(|_| ())
            }
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
