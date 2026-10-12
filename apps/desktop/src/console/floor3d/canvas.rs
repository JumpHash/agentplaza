//! The HTML canvas 2D context the web floor paints its textures and plaques with
//! (`CanvasRenderingContext2D`), on tiny-skia, so `textures.ts` and friends port call for call.
//! Colours are CSS strings (`#rrggbb`, `rgb()`, `rgba()`, `hsl()`, `hsla()`, named basics).
//! Text uses the system's DejaVu fonts (mono for `monospace`, sans otherwise), via ab_glyph.

use super::scene::{Tex, Texture};

/// A drawing surface: RGBA, straight alpha out, the canvas's state stack, a current path.
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    // contract: C — private state (pixmap, path builder, transform, styles, state stack).
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Canvas {
        Canvas { width, height } // contract: C
    }

    // ---- State (fillStyle, strokeStyle, lineWidth, globalAlpha, lineCap, lineJoin, font, textAlign, textBaseline)
    pub fn set_fill(&mut self, css: &str) { let _ = css; }
    pub fn set_stroke(&mut self, css: &str) { let _ = css; }
    pub fn set_line_width(&mut self, w: f32) { let _ = w; }
    pub fn set_alpha(&mut self, a: f32) { let _ = a; }
    /// "butt" | "round" | "square".
    pub fn set_line_cap(&mut self, cap: &str) { let _ = cap; }
    /// "miter" | "round" | "bevel".
    pub fn set_line_join(&mut self, join: &str) { let _ = join; }
    /// A CSS font shorthand as the web uses it, e.g. "700 28px monospace", "bold 20px sans-serif".
    pub fn set_font(&mut self, font: &str) { let _ = font; }
    /// "left" | "center" | "right" | "start" | "end".
    pub fn set_text_align(&mut self, align: &str) { let _ = align; }
    /// "alphabetic" | "middle" | "top" | "bottom".
    pub fn set_text_baseline(&mut self, baseline: &str) { let _ = baseline; }
    /// `createLinearGradient(x0, y0, x1, y1)` + `addColorStop`s, set as the fill.
    pub fn set_fill_linear(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, stops: &[(f32, &str)]) { let _ = (x0, y0, x1, y1, stops); }
    /// `createRadialGradient(x0, y0, r0, x1, y1, r1)` + stops, set as the fill.
    #[allow(clippy::too_many_arguments)]
    pub fn set_fill_radial(&mut self, x0: f32, y0: f32, r0: f32, x1: f32, y1: f32, r1: f32, stops: &[(f32, &str)]) { let _ = (x0, y0, r0, x1, y1, r1, stops); }
    pub fn save(&mut self) {}
    pub fn restore(&mut self) {}
    pub fn translate(&mut self, x: f32, y: f32) { let _ = (x, y); }
    pub fn rotate(&mut self, radians: f32) { let _ = radians; }
    pub fn scale(&mut self, x: f32, y: f32) { let _ = (x, y); }

    // ---- Rectangles
    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32) { let _ = (x, y, w, h); }
    pub fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32) { let _ = (x, y, w, h); }
    pub fn clear_rect(&mut self, x: f32, y: f32, w: f32, h: f32) { let _ = (x, y, w, h); }

    // ---- Paths
    pub fn begin_path(&mut self) {}
    pub fn move_to(&mut self, x: f32, y: f32) { let _ = (x, y); }
    pub fn line_to(&mut self, x: f32, y: f32) { let _ = (x, y); }
    pub fn close_path(&mut self) {}
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) { let _ = (x, y, w, h); }
    /// `arc(x, y, r, start, end, counterclockwise)`.
    pub fn arc(&mut self, x: f32, y: f32, r: f32, start: f32, end: f32, ccw: bool) { let _ = (x, y, r, start, end, ccw); }
    pub fn arc_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, r: f32) { let _ = (x1, y1, x2, y2, r); }
    #[allow(clippy::too_many_arguments)]
    pub fn ellipse(&mut self, x: f32, y: f32, rx: f32, ry: f32, rotation: f32, start: f32, end: f32, ccw: bool) { let _ = (x, y, rx, ry, rotation, start, end, ccw); }
    pub fn quadratic_curve_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) { let _ = (cx, cy, x, y); }
    pub fn bezier_curve_to(&mut self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) { let _ = (c1x, c1y, c2x, c2y, x, y); }
    pub fn fill(&mut self) {}
    pub fn stroke(&mut self) {}

    // ---- Text
    pub fn fill_text(&mut self, text: &str, x: f32, y: f32) { let _ = (text, x, y); }
    pub fn stroke_text(&mut self, text: &str, x: f32, y: f32) { let _ = (text, x, y); }
    /// The advance width of `text` in the current font (`measureText(text).width`).
    pub fn measure_text(&self, text: &str) -> f32 { let _ = text; 0.0 }

    // ---- Out
    /// The pixels, RGBA8, straight (not premultiplied) alpha, row by row from the top.
    pub fn rgba(&self) -> Vec<u8> {
        vec![0; (self.width * self.height * 4) as usize] // contract: C
    }

    /// A clamped, linear, sRGB texture of the pixels (three's `CanvasTexture` defaults as the floor sets them).
    pub fn to_texture(&self) -> Tex {
        std::sync::Arc::new(Texture::new(self.width, self.height, self.rgba()))
    }
}
