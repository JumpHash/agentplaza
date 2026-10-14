//! The robots' and the machines' name tags, drawn by the floor's renderer as overlays: floor.ts's `LABEL_CSS` tag (a
//! sheet box with a 1px ink border and the 2px print shadow, a 6px status square, then the name in Martian Mono 600
//! 10.5px), painted once per content with the canvas and cached. A working or offline agent carries one, and so do the
//! selected and the hovered agent; an idle agent that is neither carries none. The card holds the rest.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use super::view::Label;
use super::Floor;
use crate::canvas::Canvas;
use crate::scene::{Overlay, Tex};

// The console's dark theme (tokens.css): a night-navy sheet, pale ink, a black print shadow, the status squares.
const SHEET: &str = "#111726";
const INK: &str = "#e3e8f4";
const SHADOW: &str = "#000000";
const SIGNAL: &str = "#ff7a2e";
const OK: &str = "#5fe0a0";
const MUTED: &str = "#6c7790";
/// `LABEL_CSS`: font 600 10.5px/1.35 var(--mono), padding 3px 7px, gap 6px, max-width 200px.
const TAG_FONT: &str = "600 10.5px \"Martian Mono\", monospace";
const PAD_X: f32 = 7.0;
const SQUARE: f32 = 6.0;
const TAG_GAP_X: f32 = 6.0;
const TAG_MAX_W: f32 = 200.0;
/// The tag's height: the border 1 and the padding 3 each side around a 10.5 × 1.35 line.
pub(crate) const TAG_H: f32 = 22.0;
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
    let key = format!("{}\u{1f}{}\u{1f}{:.2}", label.name, square_of(label), scale);
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
    // The first label painted for the key wins, so concurrent callers share one texture.
    map.entry(key).or_insert(painted).clone()
}

/// The advance width of `text` in `font`, in logical px.
fn text_width(font: &str, text: &str) -> f32 {
    let mut measure = Canvas::new(1, 1);
    measure.set_font(font);
    measure.measure_text(text)
}

/// The status square's colour (`.floor-key`): signal orange while working, green idle, grey offline.
fn square_of(label: &Label) -> &'static str {
    if label.working {
        SIGNAL
    } else if label.idle {
        OK
    } else {
        MUTED
    }
}

/// Paints the label at `scale`; returns the canvas and the sheet box's size in logical px.
fn draw(label: &Label, scale: f32) -> (Canvas, f32, f32) {
    paint_tag(&label.name, square_of(label), scale)
}

/// floor.ts's `LABEL_CSS` tag at `scale`: the sheet box, the status square in `square` (a CSS colour), then `name`,
/// cut with an ellipsis past the 200px box. Returns the canvas (2px larger for the print shadow) and the box's size in
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::AgentInfo;

    fn info(cid: u64, status: &str) -> AgentInfo {
        AgentInfo {
            cid,
            name: "ada".to_string(),
            role: "researcher".to_string(),
            status: status.to_string(),
            tier: "worker".to_string(),
            model: "model".to_string(), species: None,
        }
    }

    /// A floor with one agent in `status` on the workstation's board, the view reset to a 640 × 480 stage.
    fn floor_with(status: &str) -> Floor {
        let mut floor = Floor::new();
        let model = crate::hardware::model("board/workstation").expect("the workstation board parses");
        floor.show_board("test".into(), model, crate::hardware::Params::default(), crate::hardware::Status::Online);
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
    fn idle_agent_has_a_tag_only_when_selected_or_hovered() {
        let mut floor = floor_with("idle");
        assert!(floor.label_overlays(640.0, 480.0, 2.0).is_empty());

        floor.selected = Some(1);
        let overlays = floor.label_overlays(640.0, 480.0, 2.0);
        assert_eq!(overlays.len(), 1);
        assert_eq!(overlays[0].h, TAG_H + 2.0);

        floor.selected = None;
        floor.hovered = Some(1);
        assert_eq!(floor.label_overlays(640.0, 480.0, 2.0).len(), 1, "a hovered idle agent shows its tag");
    }

    #[test]
    fn working_and_offline_agents_get_the_one_tag_selected_or_not() {
        for status in ["working", "offline"] {
            let mut floor = floor_with(status);
            let overlays = floor.label_overlays(640.0, 480.0, 2.0);
            assert_eq!(overlays.len(), 1, "{status}");
            assert_eq!(overlays[0].h, TAG_H + 2.0, "{status}");

            floor.selected = Some(1);
            let overlays = floor.label_overlays(640.0, 480.0, 2.0);
            assert_eq!(overlays[0].h, TAG_H + 2.0, "{status} selected");
        }
    }

    #[test]
    fn a_long_name_is_cut_to_the_200px_box() {
        let (_, w, _) = paint_tag(&"x".repeat(80), OK, 1.0);
        assert!(w <= TAG_MAX_W + 0.5, "the box is {w} wide");
    }

    #[test]
    fn selected_agent_has_one_label_among_three_bots() {
        let mut floor = Floor::new();
        let model = crate::hardware::model("board/workstation").expect("the workstation board parses");
        floor.show_board("test".into(), model, crate::hardware::Params::default(), crate::hardware::Status::Online);
        floor.sync(&[info(1, "working"), info(2, "working"), info(3, "working")]);
        floor.reset_view(640.0 / 480.0);
        floor.selected = Some(2);

        let overlays = floor.label_overlays(640.0, 480.0, 2.0);
        assert_eq!(overlays.len(), 3, "one overlay per bot");
        assert!(overlays.iter().all(|o| o.h == TAG_H + 2.0), "every agent carries the one tag");

        let labels = floor.labels(640.0, 480.0);
        assert_eq!(labels.iter().filter(|l| l.selected).count(), 1);
        assert_eq!(labels.iter().filter(|l| l.cid == 2).count(), 1, "the selected cid appears once");
        assert_eq!(labels.len(), 3, "labels: {:?}", labels.iter().map(|l| l.cid).collect::<Vec<_>>());
    }

    #[test]
    fn plaza_fits_the_stage_for_its_aspect() {
        // Every point the fit frames (rooms, dais, robots and their labels) projects inside the
        // stage, within the fill margin, and the plaza uses most of the tighter axis.
        for aspect in [1024.0 / 615.0, 1000.0 / 700.0, 0.8] {
            let mut floor = floor_with("idle");
            floor.reset_view(aspect);
            let cam = floor.orbit.camera(aspect);
            let back = (cam.eye - cam.target).normalize();
            let right = glam::Vec3::Y.cross(back).normalize();
            let up = back.cross(right);
            let tan_v = (crate::controls::FOV_DEG / 2.0).to_radians().tan();
            let tan_h = tan_v * aspect;
            let (mut wide, mut tall) = (0.0f32, 0.0f32);
            for p in floor.fit_points() {
                let v = p - cam.eye;
                let depth = -v.dot(back);
                let (nx, ny) = (v.dot(right) / (depth * tan_h), v.dot(up) / (depth * tan_v));
                assert!(nx.abs() <= 0.86 && ny.abs() <= 0.86, "aspect {aspect}: a point at ({nx}, {ny})");
                wide = wide.max(nx.abs());
                tall = tall.max(ny.abs());
            }
            assert!(wide.max(tall) >= 0.8, "aspect {aspect}: the plaza fills only {}", wide.max(tall));
        }
    }
}
