//! The floor's view logic, platform-agnostic: the overlay tags over the machines, the agent labels and the robot pick. Pointer input is in `input`, and the host adapter maps its events in.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use glam::Vec3;

use super::{Bot, Floor, Phase, display_name};
use crate::fleet::{self, FleetScene};
use crate::hardware::Status;
use crate::scene::{Camera, Overlay, Tex};

/// Logical px between a machine's tag and its label anchor (floor.ts translateY(-6px)).
const TAG_GAP: f32 = 6.0;
const TAG_CACHE_LIMIT: usize = 64;

/// The overlay tags over the machines: each machine's name and status square, above its label.
/// `hovered` is the machine under the pointer (kept for the host's API: the three.js tags did not change on hover).
pub fn machine_overlays(
    fleet: &FleetScene,
    camera: &Camera,
    width: f32,
    height: f32,
    scale: f32,
    hovered: Option<&str>,
) -> Vec<Overlay> {
    if width <= 0.0 || height <= 0.0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for tag in fleet.tags() {
        let Some((x, y)) = camera.to_screen(Vec3::from(tag.pos), width, height) else {
            continue;
        };
        let _ = hovered;
        let painted = painted_tag(&tag.name, tag.status, scale);
        let x = x - painted.box_w / 2.0;
        let y = y - TAG_GAP - painted.box_h;
        let (w, h) = (painted.box_w + 2.0, painted.box_h + 2.0);
        if x + w < 0.0 || x > width || y + h < 0.0 || y > height {
            continue;
        }
        out.push(Overlay {
            tex: painted.tex,
            x,
            y,
            w,
            h,
        });
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
fn painted_tag(name: &str, status: Status, scale: f32) -> PaintedTag {
    let led = fleet::status_led(status).to_srgb_u8();
    let key = format!("{name}\u{1f}{led:?}\u{1f}{scale:.2}");
    let cache = TAG_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache.lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
        return hit.clone();
    }
    let (canvas, box_w, box_h) = super::labels::paint_tag(
        name,
        &format!("rgb({}, {}, {})", led[0], led[1], led[2]),
        scale,
    );
    let painted = PaintedTag {
        tex: canvas.to_texture(),
        box_w,
        box_h,
    };
    let mut map = cache.lock().unwrap_or_else(|p| p.into_inner());
    if map.len() > TAG_CACHE_LIMIT {
        map.clear();
    }
    // The first tag painted for the key wins, so concurrent callers share one texture.
    map.entry(key).or_insert(painted).clone()
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
    /// The selected agent: its tag shows even when idle.
    pub selected: bool,
    /// The hovered or selected agent: its tag shows even when idle.
    pub highlighted: bool,
}

impl Label {
    /// Whether the label is drawn at all (floor.ts labelShown): an idle agent's only when it is selected or hovered.
    pub fn is_shown(&self) -> bool {
        self.selected || self.highlighted || !self.idle
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
            let Some((x, y)) = camera.to_screen(bot.robot.label_anchor(), width, height) else {
                continue;
            };
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

    /// The world point of a robot's label anchor on this view, or None when no robot of it has that cid.
    pub fn anchor_of(&self, cid: u64) -> Option<glam::Vec3> {
        self.bots
            .iter()
            .find(|b| b.agent.cid == cid)
            .map(|b| b.robot.label_anchor())
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
            let Some(t) = ray_sphere(origin, dir, centre, 0.75) else {
                continue;
            };
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
