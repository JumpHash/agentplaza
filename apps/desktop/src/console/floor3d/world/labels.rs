//! The robots' name labels, drawn by the floor's renderer as overlays (`LABEL_CSS` and `roleTag`
//! in `floor.ts`): each label is painted once per content with the canvas (a square sheet box,
//! 1px ink border, the 2px print shadow) and cached. A working or offline agent gets a compact tag:
//! its name and a status square. The selected agent gets the full box: the mono name, the role as
//! a square tag with the district colour's 3px stripe, what it is doing on the second line. An
//! idle agent that is not selected gets no label.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use super::view::Label;
use super::Floor;
use crate::console::floor3d::canvas::Canvas;
use crate::console::floor3d::scene::{Color, Overlay, Tex};

// The console's dark theme (tokens.css): a night-navy sheet, pale ink, a black print shadow.
const SHEET: &str = "#111726";
const INK: &str = "#e3e8f4";
const SHADOW: &str = "#000000";
const SIGNAL: &str = "#ff7a2e";
const MUTED: &str = "#6c7790";
const NAME_FONT: &str = "500 10px monospace";
const ROLE_FONT: &str = "9.5px monospace";
const LINE_FONT: &str = "10px monospace";
const COMPACT_FONT: &str = "500 9.5px monospace";
/// The full box's height: padding 5, the name row 18, the gap 2, the second line 12, padding 5, border 2.
const BOX_H: f32 = 44.0;
/// The compact tag's height: the border 1 and the padding 5 each side around a 6px status square.
const COMPACT_H: f32 = 18.0;
/// Entries kept in the texture cache before it starts again.
const CACHE_LIMIT: usize = 256;
/// Logical px between a label's bottom and the robot's anchor.
const GAP: f32 = 6.0;

/// A painted label: its texture and the sheet box's size in logical px (the texture is 2px
/// larger on the right and bottom, for the shadow).
#[derive(Clone)]
struct Painted {
    tex: Tex,
    box_w: f32,
    box_h: f32,
}

static CACHE: OnceLock<Mutex<HashMap<String, Painted>>> = OnceLock::new();

impl Floor {
    /// The labels for a `width` × `height` (logical px) view at `scale` device pixels per logical
    /// pixel, back to front, each centred `GAP` px above its robot's head. Labels off the view are
    /// left out.
    pub fn label_overlays(&self, width: f32, height: f32, scale: f32) -> Vec<Overlay> {
        if width <= 0.0 || height <= 0.0 {
            return Vec::new();
        }
        let camera = self.orbit.camera(width / height);
        let mut items: Vec<(f32, Overlay)> = Vec::new();
        for label in self.labels(width, height) {
            if !label.is_shown() {
                continue;
            }
            let Some(bot) = self.bot(label.cid) else { continue };
            // View space looks down -Z: the farther a robot, the more negative its z.
            let depth = camera.view().transform_point3(bot.robot.label_anchor()).z;
            let p = painted(&label, scale);
            let x = label.x - p.box_w / 2.0;
            let y = label.y - GAP - p.box_h;
            let (w, h) = (p.box_w + 2.0, p.box_h + 2.0);
            if x + w < 0.0 || x > width || y + h < 0.0 || y > height {
                continue;
            }
            items.push((depth, Overlay { tex: p.tex, x, y, w, h }));
        }
        items.sort_by(|a, b| a.0.total_cmp(&b.0));
        items.into_iter().map(|(_, overlay)| overlay).collect()
    }
}

/// The label's texture and box size, from the cache or painted now.
fn painted(label: &Label, scale: f32) -> Painted {
    let key = format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{:.2}\u{1f}{}\u{1f}{}",
        label.name, label.role, label.line, label.working, label.highlighted, label.hue, label.selected, scale
    );
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache.lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
        return hit.clone();
    }
    let (canvas, box_w, box_h) = draw(label, scale);
    let painted = Painted { tex: canvas.to_texture(), box_w, box_h };
    let mut map = cache.lock().unwrap_or_else(|p| p.into_inner());
    if map.len() > CACHE_LIMIT {
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

/// Paints the label at `scale`; returns the canvas and the sheet box's size in logical px.
fn draw(label: &Label, scale: f32) -> (Canvas, f32, f32) {
    if label.selected {
        draw_full(label, scale)
    } else {
        draw_compact(label, scale)
    }
}

/// The compact tag: the sheet box with the name and a status square (signal orange while working,
/// grey when offline).
fn draw_compact(label: &Label, scale: f32) -> (Canvas, f32, f32) {
    let name_w = text_width(COMPACT_FONT, &label.name);
    // Border 1, padding 5, the 6px square, a gap of 5, the name, padding 5, border 1.
    let box_w = name_w + 23.0;

    let device = |v: f32| (v * scale).ceil().max(1.0) as u32;
    let mut c = Canvas::new(device(box_w + 2.0), device(COMPACT_H + 2.0));
    c.scale(scale, scale);

    c.set_fill(SHADOW);
    c.fill_rect(2.0, 2.0, box_w, COMPACT_H);
    c.set_fill(SHEET);
    c.fill_rect(0.0, 0.0, box_w, COMPACT_H);
    c.set_stroke(if label.highlighted { SIGNAL } else { INK });
    c.set_line_width(1.0);
    c.stroke_rect(0.5, 0.5, box_w - 1.0, COMPACT_H - 1.0);

    c.set_fill(if label.working { SIGNAL } else { MUTED });
    c.fill_rect(6.0, 6.0, 6.0, 6.0);
    c.set_fill(INK);
    c.set_text_baseline("middle");
    c.set_font(COMPACT_FONT);
    c.fill_text(&label.name, 17.0, 9.0);

    (c, box_w, COMPACT_H)
}

/// The full box: the name, the role tag and the second line.
fn draw_full(label: &Label, scale: f32) -> (Canvas, f32, f32) {
    let role = label.role.replace('_', " ").to_uppercase();
    let name_w = text_width(NAME_FONT, &label.name);
    // Stripe 3, gap 4, padding 5 on the right, the border 1 and 1 on the left.
    let tag_w = text_width(ROLE_FONT, &role) + 14.0;
    let line_w = text_width(LINE_FONT, &label.line);
    // Padding 8 and the border on each side.
    let box_w = (name_w + 6.0 + tag_w).max(line_w) + 18.0;
    let box_h = BOX_H;

    let device = |v: f32| (v * scale).ceil().max(1.0) as u32;
    let mut c = Canvas::new(device(box_w + 2.0), device(box_h + 2.0));
    c.scale(scale, scale);

    // The print shadow, then the sheet with its 1px border (highlighted: signal orange).
    c.set_fill(SHADOW);
    c.fill_rect(2.0, 2.0, box_w, box_h);
    c.set_fill(SHEET);
    c.fill_rect(0.0, 0.0, box_w, box_h);
    c.set_stroke(if label.highlighted { SIGNAL } else { INK });
    c.set_line_width(1.0);
    c.stroke_rect(0.5, 0.5, box_w - 1.0, box_h - 1.0);

    // The name row: the name, then the role tag with the district colour's stripe.
    c.set_fill(INK);
    c.set_text_baseline("middle");
    c.set_font(NAME_FONT);
    c.fill_text(&label.name, 9.0, 15.0);
    let tag_x = 9.0 + name_w + 6.0;
    c.set_stroke(INK);
    c.stroke_rect(tag_x + 0.5, 6.5, tag_w - 1.0, 17.0);
    let [r, g, b] = Color::hsl(label.hue / 360.0, 0.75, 0.6).to_srgb_u8();
    c.set_fill(&format!("#{r:02x}{g:02x}{b:02x}"));
    c.fill_rect(tag_x + 1.0, 7.0, 3.0, 16.0);
    c.set_fill(INK);
    c.set_font(ROLE_FONT);
    c.fill_text(&role, tag_x + 8.0, 15.0);

    // The second line: what the agent is doing, signal orange while it works.
    c.set_fill(if label.working { SIGNAL } else { MUTED });
    c.set_font(LINE_FONT);
    c.fill_text(&label.line, 9.0, 32.0);

    (c, box_w, box_h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::floor3d::world::AgentInfo;

    fn info(cid: u64, status: &str) -> AgentInfo {
        AgentInfo {
            cid,
            name: "ada".to_string(),
            role: "researcher".to_string(),
            status: status.to_string(),
            tier: "worker".to_string(),
            model: "model".to_string(),
        }
    }

    /// A floor with one agent in `status`, the view reset to a 640 × 480 stage.
    fn floor_with(status: &str) -> Floor {
        let mut floor = Floor::new();
        floor.sync(&[info(1, status)]);
        floor.reset_view(640.0 / 480.0);
        floor
    }

    #[test]
    fn label_overlays_one_bot_gives_one_painted_overlay() {
        let floor = floor_with("working");
        let overlays = floor.label_overlays(640.0, 480.0, 2.0);
        assert_eq!(overlays.len(), 1);
        assert!(overlays[0].w > 0.0 && overlays[0].h > 0.0);

        let label = Label {
            cid: 1,
            name: "ada".to_string(),
            role: "researcher".to_string(),
            line: "reviewing".to_string(),
            hue: 120.0,
            x: 0.0,
            y: 0.0,
            working: true,
            idle: false,
            selected: true,
            highlighted: false,
        };
        let (canvas, _, _) = draw(&label, 2.0);
        assert!(canvas.rgba().chunks_exact(4).any(|p| p[3] > 0));
    }

    #[test]
    fn idle_unselected_agent_has_no_label_and_selected_idle_agent_gets_the_full_one() {
        let mut floor = floor_with("idle");
        assert!(floor.label_overlays(640.0, 480.0, 2.0).is_empty());

        floor.selected = Some(1);
        let overlays = floor.label_overlays(640.0, 480.0, 2.0);
        assert_eq!(overlays.len(), 1);
        assert_eq!(overlays[0].h, BOX_H + 2.0);
    }

    #[test]
    fn working_and_offline_agents_get_the_compact_tag_unless_selected() {
        for status in ["working", "offline"] {
            let mut floor = floor_with(status);
            let overlays = floor.label_overlays(640.0, 480.0, 2.0);
            assert_eq!(overlays.len(), 1, "{status}");
            assert_eq!(overlays[0].h, COMPACT_H + 2.0, "{status}");

            floor.selected = Some(1);
            let overlays = floor.label_overlays(640.0, 480.0, 2.0);
            assert_eq!(overlays[0].h, BOX_H + 2.0, "{status} selected");
        }
    }

    #[test]
    fn plaza_fits_the_stage_for_its_aspect() {
        // The fitted sphere's angular radius must fit the narrower half-angle of the frustum.
        for aspect in [1024.0 / 615.0, 1000.0 / 700.0, 0.8] {
            let mut floor = floor_with("idle");
            floor.reset_view(aspect);
            let eye = floor.orbit.camera(aspect).eye;
            let angular = (floor.plaza_radius() / eye.distance(floor.orbit.target)).asin();
            let half_v = (crate::console::floor3d::controls::FOV_DEG / 2.0).to_radians();
            let half = half_v.min((half_v.tan() * aspect).atan());
            assert!(angular <= half, "aspect {aspect}: {angular} > {half}");
        }
    }
}
