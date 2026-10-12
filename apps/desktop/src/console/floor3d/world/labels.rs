//! The robots' name labels, drawn by the floor's renderer as overlays (`LABEL_CSS` and `roleTag`
//! in `floor.ts`): each label is painted once per content with the canvas (a square sheet box,
//! 1px ink border, the 2px print shadow, the mono name, the role as a square tag with the
//! district colour's 3px stripe, what the agent is doing on the second line) and cached.

use super::Floor;
use crate::console::floor3d::scene::Overlay;

impl Floor {
    /// The labels for a `width` × `height` (logical px) view at `scale` device pixels per logical
    /// pixel, back to front, each centred 6 px above its robot's head.
    pub fn label_overlays(&self, width: f32, height: f32, scale: f32) -> Vec<Overlay> {
        let _ = (width, height, scale);
        Vec::new() // contract: LBL
    }
}
