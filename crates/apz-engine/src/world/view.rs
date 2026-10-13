//! The floor's view logic, platform-agnostic: the overlay tags over the machines, the agent labels and the robot pick. Pointer input is in `input`, and the host adapter maps its events in.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use glam::Vec3;

use super::{display_name, Bot, Floor, Phase};
use crate::canvas::Canvas;
use crate::fleet::{self, FleetScene};
use crate::hardware::Status;
use crate::scene::{Camera, Overlay, Tex};

/// The machine tag's look: the compact label of the agents' Floor (`labels.rs`), in the same colours.
const TAG_SHEET: &str = "#111726";
const TAG_INK: &str = "#e3e8f4";
const TAG_SHADOW: &str = "#000000";
const TAG_SIGNAL: &str = "#ff7a2e";
const TAG_FONT: &str = "500 9.5px monospace";
const TAG_H: f32 = 18.0;
const TAG_GAP: f32 = 6.0;
const TAG_CACHE_LIMIT: usize = 64;

/// The overlay tags over the machines: each machine's name and status square, above its label.
/// `hovered` is the machine under the pointer (its tag gets the signal border).
pub fn machine_overlays(fleet: &FleetScene, camera: &Camera, width: f32, height: f32, scale: f32, hovered: Option<&str>) -> Vec<Overlay> {
    if width <= 0.0 || height <= 0.0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for tag in fleet.tags() {
        let Some((x, y)) = camera.to_screen(Vec3::from(tag.pos), width, height) else { continue };
        let painted = painted_tag(&tag.name, tag.status, hovered == Some(tag.id.as_str()), scale);
        let x = x - painted.box_w / 2.0;
        let y = y - TAG_GAP - painted.box_h;
        let (w, h) = (painted.box_w + 2.0, painted.box_h + 2.0);
        if x + w < 0.0 || x > width || y + h < 0.0 || y > height {
            continue;
        }
        out.push(Overlay { tex: painted.tex, x, y, w, h });
    }
    out
}

/// A painted tag: its texture and the box's size in logical px.
#[derive(Clone)]
struct PaintedTag {
    tex: Tex,
    box_w: f32,
    box_h: f32,
}

static TAG_CACHE: OnceLock<Mutex<HashMap<String, PaintedTag>>> = OnceLock::new();

/// The tag's texture, from the cache or painted now.
fn painted_tag(name: &str, status: Status, lit: bool, scale: f32) -> PaintedTag {
    let led = fleet::status_led(status).to_srgb_u8();
    let key = format!("{name}\u{1f}{led:?}\u{1f}{lit}\u{1f}{scale:.2}");
    let cache = TAG_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache.lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
        return hit.clone();
    }
    let (canvas, box_w, box_h) = draw_tag(name, led, lit, scale);
    let painted = PaintedTag { tex: canvas.to_texture(), box_w, box_h };
    let mut map = cache.lock().unwrap_or_else(|p| p.into_inner());
    if map.len() > TAG_CACHE_LIMIT {
        map.clear();
    }
    map.insert(key, painted.clone());
    painted
}

/// The advance width of `text` in `font`, in logical px.
fn text_width(font: &str, text: &str) -> f32 {
    let mut measure = Canvas::new(1, 1);
    measure.set_font(font);
    measure.measure_text(text)
}

/// Paints the tag at `scale`: the sheet box, the status square (`led`, RGB bytes) and the name.
fn draw_tag(name: &str, led: [u8; 3], lit: bool, scale: f32) -> (Canvas, f32, f32) {
    // Border 1, padding 5, the 6px square, a gap of 5, the name, padding 5, border 1.
    let box_w = text_width(TAG_FONT, name) + 23.0;
    let device = |v: f32| (v * scale).ceil().max(1.0) as u32;
    let mut c = Canvas::new(device(box_w + 2.0), device(TAG_H + 2.0));
    c.scale(scale, scale);
    c.set_fill(TAG_SHADOW);
    c.fill_rect(2.0, 2.0, box_w, TAG_H);
    c.set_fill(TAG_SHEET);
    c.fill_rect(0.0, 0.0, box_w, TAG_H);
    c.set_stroke(if lit { TAG_SIGNAL } else { TAG_INK });
    c.set_line_width(1.0);
    c.stroke_rect(0.5, 0.5, box_w - 1.0, TAG_H - 1.0);
    c.set_fill(&format!("rgb({}, {}, {})", led[0], led[1], led[2]));
    c.fill_rect(6.0, 6.0, 6.0, 6.0);
    c.set_fill(TAG_INK);
    c.set_text_baseline("middle");
    c.set_font(TAG_FONT);
    c.fill_text(name, 17.0, 9.0);
    (c, box_w, TAG_H)
}

/// A name label to draw over the scene, in widget pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub cid: u64,
    pub name: String,
    pub role: String,
    /// The second line: what it is doing.
    pub line: String,
    pub hue: f32,
    pub x: f32,
    pub y: f32,
    pub working: bool,
    /// The agent is idle: its label is left out unless it is selected.
    pub idle: bool,
    /// The selected agent: drawn with its full label, even when idle.
    pub selected: bool,
    pub highlighted: bool,
}

impl Label {
    /// Whether the label is drawn at all: an idle agent's only when it is selected.
    pub fn is_shown(&self) -> bool {
        self.selected || !self.idle
    }
}

/// The second line of a label and the card (`shown` in floor.ts).
pub fn shown(bot: &Bot) -> String {
    if bot.agent.status == "offline" {
        "offline".to_string()
    } else if bot.social.as_ref().is_some_and(|s| s.phase == Phase::Talk) {
        "· · ·".to_string()
    } else {
        bot.activity.clone()
    }
}

impl Floor {
    /// The labels for a `width` × `height` view (`updateLabels`). Robots behind the camera are left out.
    pub fn labels(&self, width: f32, height: f32) -> Vec<Label> {
        if width <= 0.0 || height <= 0.0 {
            return Vec::new();
        }
        let camera = self.orbit.camera(width / height);
        let mut out = Vec::with_capacity(self.bots.len());
        for bot in &self.bots {
            // A robot's name tag appears once the robot has popped in (the ENTER's wave hides it until then).
            if !bot.robot.group.get().visible {
                continue;
            }
            let Some((x, y)) = camera.to_screen(bot.robot.label_anchor(), width, height) else { continue };
            let cid = bot.agent.cid;
            out.push(Label {
                cid,
                name: display_name(cid, &bot.agent.name),
                role: bot.agent.role.clone(),
                line: shown(bot),
                hue: self.districts.get(bot.district).map_or(0.0, |d| d.hue),
                x,
                y,
                working: bot.agent.status == "working",
                idle: bot.agent.status == "idle",
                selected: self.selected == Some(cid),
                highlighted: self.hovered == Some(cid) || self.selected == Some(cid),
            });
        }
        out
    }

    /// The robot under the pointer (`pick`): the nearest whose bounding sphere the ray hits. The
    /// sphere is centred 0.6 above the robot's position, radius 0.75.
    pub fn pick(&self, x: f32, y: f32, width: f32, height: f32) -> Option<u64> {
        if width <= 0.0 || height <= 0.0 {
            return None;
        }
        let camera = self.orbit.camera(width / height);
        let (origin, dir) = camera.ray(x, y, width, height);
        let dir = dir.normalize_or_zero();
        let mut best: Option<(f32, u64)> = None;
        for bot in &self.bots {
            let centre = bot.robot.group.position() + Vec3::Y * 0.6;
            let Some(t) = ray_sphere(origin, dir, centre, 0.75) else { continue };
            if best.is_none_or(|(bt, _)| t < bt) {
                best = Some((t, bot.agent.cid));
            }
        }
        best.map(|(_, cid)| cid)
    }
}

/// The distance along a unit ray to a sphere, if the ray reaches it.
fn ray_sphere(origin: Vec3, dir: Vec3, centre: Vec3, radius: f32) -> Option<f32> {
    let oc = origin - centre;
    let b = oc.dot(dir);
    let c = oc.length_squared() - radius * radius;
    let disc = b * b - c;
    if disc < 0.0 {
        return None;
    }
    let s = disc.sqrt();
    // The near side, or the far side when the origin is inside the sphere.
    let t = if -b - s >= 0.0 { -b - s } else { -b + s };
    (t >= 0.0).then_some(t)
}
