//! The machines' name tags, drawn by the renderer as overlays: a sheet box with a 1px ink border and the 2px print
//! shadow, a 6px status square, then the name in Martian Mono 600 10.5px, painted once per content with the canvas and
//! cached.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use glam::Vec3;

use crate::canvas::Canvas;
use crate::fleet::FleetScene;
use crate::scene::Color;
use crate::scene::{Camera, Overlay, Tex};

const SHEET: &str = "#111726";
const INK: &str = "#e3e8f4";
const SHADOW: &str = "#000000";
/// `LABEL_CSS`: font 600 10.5px/1.35 var(--mono), padding 3px 7px, gap 6px, max-width 200px.
const TAG_FONT: &str = "600 10.5px \"Martian Mono\", monospace";
const PAD_X: f32 = 7.0;
const SQUARE: f32 = 6.0;
const TAG_GAP_X: f32 = 6.0;
const TAG_MAX_W: f32 = 200.0;
/// The tag's height: the border 1 and the padding 3 each side around a 10.5 × 1.35 line.
pub(crate) const TAG_H: f32 = 22.0;

/// Logical px between a machine's tag and its label anchor (floor.ts translateY(-6px)).
const TAG_GAP: f32 = 6.0;
/// The tag cache's size: every machine slot's tag in each of its four lamp colours. A smaller cache than the fleet
/// would be cleared and repainted on every frame (128 tags took 15 ms a frame against a cache of 64).
const TAG_CACHE_LIMIT: usize = 4 * crate::fleet::MACHINE_SLOTS;

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
        let painted = painted_tag(&tag.name, fleet.palette().lamp(tag.lamp), scale);
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
fn painted_tag(name: &str, lamp: Color, scale: f32) -> PaintedTag {
    let led = lamp.to_srgb_u8();
    let key = format!("{name}\u{1f}{led:?}\u{1f}{scale:.2}");
    let cache = TAG_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache.lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
        return hit.clone();
    }
    let (canvas, box_w, box_h) = paint_tag(
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

/// The width of `text` in `font`, in logical px.
fn text_width(font: &str, text: &str) -> f32 {
    let mut measure = Canvas::new(1, 1);
    measure.set_font(font);
    measure.measure_text(text)
}

/// logical px.
pub(crate) fn paint_tag(name: &str, square: &str, scale: f32) -> (Canvas, f32, f32) {
    let room = TAG_MAX_W - 2.0 - 2.0 * PAD_X - SQUARE - TAG_GAP_X;
    let mut shown = name.to_string();
    if text_width(TAG_FONT, &shown) > room {
        while !shown.is_empty() && text_width(TAG_FONT, &format!("{shown}…")) > room {
            shown.pop();
        }
        shown.push('…');
    }
    let box_w = 2.0 + 2.0 * PAD_X + SQUARE + TAG_GAP_X + text_width(TAG_FONT, &shown);
    let device = |v: f32| (v * scale).ceil().max(1.0) as u32;
    let mut c = Canvas::new(device(box_w + 2.0), device(TAG_H + 2.0));
    c.scale(scale, scale);
    c.set_fill(SHADOW);
    c.fill_rect(2.0, 2.0, box_w, TAG_H);
    c.set_fill(SHEET);
    c.fill_rect(0.0, 0.0, box_w, TAG_H);
    c.set_stroke(INK);
    c.set_line_width(1.0);
    c.stroke_rect(0.5, 0.5, box_w - 1.0, TAG_H - 1.0);
    let mid = TAG_H / 2.0;
    c.set_fill(square);
    c.fill_rect(1.0 + PAD_X, mid - SQUARE / 2.0, SQUARE, SQUARE);
    c.set_fill(INK);
    c.set_text_baseline("middle");
    c.set_font(TAG_FONT);
    c.fill_text(&shown, 1.0 + PAD_X + SQUARE + TAG_GAP_X, mid);
    (c, box_w, TAG_H)
}
