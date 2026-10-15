//! The HTML canvas 2D context the web floor paints its textures and plaques with
//! (`CanvasRenderingContext2D`), on tiny-skia, so `textures.ts` and friends port call for call.
//! Colours are CSS strings (`#rrggbb`, `rgb()`, `rgba()`, `hsl()`, `hsla()`, named basics).
//! Text uses the system's fonts (DejaVu on Linux, Arial / Courier New on macOS, Arial / Consolas on
//! Windows; mono for `monospace`, sans otherwise), else the bundled Martian Mono, via ab_glyph.

use super::scene::{Tex, Texture};
use ab_glyph::{Font, FontVec, OutlineCurve, Point as GlyphPoint, PxScale, ScaleFont};
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::sync::OnceLock;
use tiny_skia::{
    BlendMode, Color, FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Paint, Path,
    PathBuilder, Pixmap, Point, RadialGradient, Shader, SpreadMode, Stroke, Transform,
};

/// A drawing surface: RGBA, straight alpha out, the canvas's state stack, a current path.
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    /// Premultiplied on the tiny-skia side; `rgba()` demultiplies. At least 1x1 so the
    /// pixmap always exists even for a zero-sized canvas.
    pixmap: Pixmap,
    /// The current path in user space; the transform is applied when it is filled or stroked.
    path: PathBuilder,
    /// The current point of the open subpath, `None` when the path is empty.
    current: Option<(f32, f32)>,
    /// Where the open subpath started (`closePath` returns there).
    start: (f32, f32),
    state: State,
    stack: Vec<State>,
}

/// Straight-alpha colour, each channel 0..1.
type Rgba = [f32; 4];

#[derive(Clone)]
enum Style {
    Solid(Rgba),
    Linear {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        stops: Vec<(f32, Rgba)>,
    },
    /// tiny-skia's radial gradient runs from a zero-radius focal point to a circle of `r1`,
    /// which is the canvas gradient when `r0` is 0; a non-zero `r0` is not supported.
    Radial {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        r1: f32,
        stops: Vec<(f32, Rgba)>,
    },
}

#[derive(Clone, Copy)]
enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy)]
enum Baseline {
    Alphabetic,
    Middle,
    Top,
    Bottom,
}

#[derive(Clone, Copy)]
struct FontSpec {
    size: f32,
    mono: bool,
    bold: bool,
    /// The family names Martian Mono (the app's `--mono`): the bundled face, on the desktop too.
    martian: bool,
}

impl Default for FontSpec {
    fn default() -> Self {
        FontSpec {
            size: 10.0,
            mono: false,
            bold: false,
            martian: false,
        }
    }
}

/// Everything `save()` pushes and `restore()` pops.
#[derive(Clone)]
struct State {
    fill: Style,
    stroke: Style,
    line_width: f32,
    alpha: f32,
    cap: LineCap,
    join: LineJoin,
    font: FontSpec,
    align: Align,
    baseline: Baseline,
    transform: Transform,
}

impl Default for State {
    fn default() -> Self {
        let black = Style::Solid([0.0, 0.0, 0.0, 1.0]);
        State {
            fill: black.clone(),
            stroke: black,
            line_width: 1.0,
            alpha: 1.0,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            font: FontSpec::default(),
            align: Align::Left,
            baseline: Baseline::Alphabetic,
            transform: Transform::identity(),
        }
    }
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Canvas {
        let pixmap = Pixmap::new(width.max(1), height.max(1)).expect("canvas pixmap");
        Canvas {
            width,
            height,
            pixmap,
            path: PathBuilder::new(),
            current: None,
            start: (0.0, 0.0),
            state: State::default(),
            stack: Vec::new(),
        }
    }

    // ---- State (fillStyle, strokeStyle, lineWidth, globalAlpha, lineCap, lineJoin, font, textAlign, textBaseline)
    /// An unparsable colour is ignored, as the canvas ignores it.
    pub fn set_fill(&mut self, css: &str) {
        if let Some(c) = parse_css_colour(css) {
            self.state.fill = Style::Solid(c);
        }
    }

    pub fn set_stroke(&mut self, css: &str) {
        if let Some(c) = parse_css_colour(css) {
            self.state.stroke = Style::Solid(c);
        }
    }

    /// Non-positive or non-finite widths are ignored, as on the canvas.
    pub fn set_line_width(&mut self, w: f32) {
        if w > 0.0 && w.is_finite() {
            self.state.line_width = w;
        }
    }

    /// Values outside 0..=1 are ignored, as on the canvas.
    pub fn set_alpha(&mut self, a: f32) {
        if (0.0..=1.0).contains(&a) {
            self.state.alpha = a;
        }
    }

    /// "butt" | "round" | "square".
    pub fn set_line_cap(&mut self, cap: &str) {
        self.state.cap = match cap {
            "butt" => LineCap::Butt,
            "round" => LineCap::Round,
            "square" => LineCap::Square,
            _ => return,
        };
    }

    /// "miter" | "round" | "bevel".
    pub fn set_line_join(&mut self, join: &str) {
        self.state.join = match join {
            "miter" => LineJoin::Miter,
            "round" => LineJoin::Round,
            "bevel" => LineJoin::Bevel,
            _ => return,
        };
    }

    /// A CSS font shorthand as the web uses it, e.g. "700 28px monospace", "bold 20px sans-serif".
    /// A string without a pixel size is ignored, as on the canvas.
    pub fn set_font(&mut self, font: &str) {
        if let Some(spec) = parse_font(font) {
            self.state.font = spec;
        }
    }

    /// "left" | "center" | "right" | "start" | "end".
    pub fn set_text_align(&mut self, align: &str) {
        self.state.align = match align {
            "left" | "start" => Align::Left,
            "center" => Align::Center,
            "right" | "end" => Align::Right,
            _ => return,
        };
    }

    /// "alphabetic" | "middle" | "top" | "bottom".
    pub fn set_text_baseline(&mut self, baseline: &str) {
        self.state.baseline = match baseline {
            "alphabetic" => Baseline::Alphabetic,
            "middle" => Baseline::Middle,
            "top" | "hanging" => Baseline::Top,
            "bottom" | "ideographic" => Baseline::Bottom,
            _ => return,
        };
    }

    /// `createLinearGradient(x0, y0, x1, y1)` + `addColorStop`s, set as the fill.
    pub fn set_fill_linear(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, stops: &[(f32, &str)]) {
        self.state.fill = Style::Linear {
            x0,
            y0,
            x1,
            y1,
            stops: parse_stops(stops),
        };
    }

    /// `createRadialGradient(x0, y0, r0, x1, y1, r1)` + stops, set as the fill.
    #[allow(clippy::too_many_arguments)]
    pub fn set_fill_radial(
        &mut self,
        x0: f32,
        y0: f32,
        r0: f32,
        x1: f32,
        y1: f32,
        r1: f32,
        stops: &[(f32, &str)],
    ) {
        let _ = r0; // tiny-skia's radial gradient starts from a zero-radius focal point
        self.state.fill = Style::Radial {
            x0,
            y0,
            x1,
            y1,
            r1,
            stops: parse_stops(stops),
        };
    }

    pub fn save(&mut self) {
        self.stack.push(self.state.clone());
    }

    pub fn restore(&mut self) {
        if let Some(s) = self.stack.pop() {
            self.state = s;
        }
    }

    pub fn translate(&mut self, x: f32, y: f32) {
        self.concat(Transform::from_translate(x, y));
    }

    /// Radians, as the canvas takes them.
    pub fn rotate(&mut self, radians: f32) {
        self.concat(Transform::from_rotate(radians.to_degrees()));
    }

    pub fn scale(&mut self, x: f32, y: f32) {
        self.concat(Transform::from_scale(x, y));
    }

    fn concat(&mut self, m: Transform) {
        self.state.transform = self.state.transform.pre_concat(m);
    }

    // ---- Rectangles
    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        if let Some(p) = rect_path(x, y, w, h) {
            self.fill_with(&p);
        }
    }

    pub fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        if let Some(p) = rect_path(x, y, w, h) {
            self.stroke_with(&p);
        }
    }

    pub fn clear_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        if let Some(p) = rect_path(x, y, w, h) {
            let paint = Paint {
                shader: Shader::SolidColor(Color::from_rgba8(0, 0, 0, 0)),
                blend_mode: BlendMode::Clear,
                anti_alias: true,
                ..Paint::default()
            };
            self.pixmap
                .fill_path(&p, &paint, FillRule::Winding, self.state.transform, None);
        }
    }

    // ---- Paths
    pub fn begin_path(&mut self) {
        self.path = PathBuilder::new();
        self.current = None;
    }

    pub fn move_to(&mut self, x: f32, y: f32) {
        self.add_move(x, y);
    }

    pub fn line_to(&mut self, x: f32, y: f32) {
        self.add_line(x, y);
    }

    pub fn close_path(&mut self) {
        if self.current.is_some() {
            self.path.close();
            self.current = Some(self.start);
        }
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.add_move(x, y);
        self.add_line(x + w, y);
        self.add_line(x + w, y + h);
        self.add_line(x, y + h);
        self.close_path();
    }

    /// `arc(x, y, r, start, end, counterclockwise)`.
    pub fn arc(&mut self, x: f32, y: f32, r: f32, start: f32, end: f32, ccw: bool) {
        let sweep = arc_sweep(start, end, ccw);
        self.sweep_arc(x, y, r, r, 0.0, start, sweep);
    }

    /// `arcTo(x1, y1, x2, y2, r)`: a tangent arc between the two lines from the current point.
    pub fn arc_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, r: f32) {
        if r < 0.0 {
            return;
        }
        let Some((x0, y0)) = self.current else {
            self.add_move(x1, y1);
            return;
        };
        let (ux0, uy0) = (x0 - x1, y0 - y1);
        let (ux2, uy2) = (x2 - x1, y2 - y1);
        let (l0, l2) = (
            (ux0 * ux0 + uy0 * uy0).sqrt(),
            (ux2 * ux2 + uy2 * uy2).sqrt(),
        );
        if r == 0.0 || l0 == 0.0 || l2 == 0.0 {
            self.add_line(x1, y1);
            return;
        }
        let (v0x, v0y) = (ux0 / l0, uy0 / l0);
        let (v2x, v2y) = (ux2 / l2, uy2 / l2);
        let phi = (v0x * v2x + v0y * v2y).clamp(-1.0, 1.0).acos();
        if !(1e-6..=PI - 1e-6).contains(&phi) {
            self.add_line(x1, y1); // colinear: no corner to round
            return;
        }
        let half = phi / 2.0;
        let d = r / half.tan();
        let (bx, by) = (v0x + v2x, v0y + v2y);
        let bl = (bx * bx + by * by).sqrt();
        let dc = r / half.sin();
        let (cx, cy) = (x1 + bx / bl * dc, y1 + by / bl * dc);
        let a0 = (y1 + v0y * d - cy).atan2(x1 + v0x * d - cx);
        let a1 = (y1 + v2y * d - cy).atan2(x1 + v2x * d - cx);
        // The arc between the two tangent points is the minor one.
        let mut sweep = (a1 - a0).rem_euclid(TAU);
        if sweep > PI {
            sweep -= TAU;
        }
        // sweep_arc starts with a line to the first tangent point, which is the line from the corner.
        self.sweep_arc(cx, cy, r, r, 0.0, a0, sweep);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ellipse(
        &mut self,
        x: f32,
        y: f32,
        rx: f32,
        ry: f32,
        rotation: f32,
        start: f32,
        end: f32,
        ccw: bool,
    ) {
        let sweep = arc_sweep(start, end, ccw);
        self.sweep_arc(x, y, rx, ry, rotation, start, sweep);
    }

    pub fn quadratic_curve_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        if self.current.is_none() {
            self.add_move(cx, cy);
        }
        self.path.quad_to(cx, cy, x, y);
        self.current = Some((x, y));
    }

    pub fn bezier_curve_to(&mut self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) {
        if self.current.is_none() {
            self.add_move(c1x, c1y);
        }
        self.path.cubic_to(c1x, c1y, c2x, c2y, x, y);
        self.current = Some((x, y));
    }

    pub fn fill(&mut self) {
        if let Some(p) = self.path.clone().finish() {
            self.fill_with(&p);
        }
    }

    pub fn stroke(&mut self) {
        if let Some(p) = self.path.clone().finish() {
            self.stroke_with(&p);
        }
    }

    // ---- Text
    pub fn fill_text(&mut self, text: &str, x: f32, y: f32) {
        if let Some(p) = self.text_path(text, x, y) {
            self.fill_with(&p);
        }
    }

    pub fn stroke_text(&mut self, text: &str, x: f32, y: f32) {
        if let Some(p) = self.text_path(text, x, y) {
            self.stroke_with(&p);
        }
    }

    /// The advance width of `text` in the current font (`measureText(text).width`).
    /// Zero when the font file is missing.
    pub fn measure_text(&self, text: &str) -> f32 {
        let Some(font) = font_for(&self.state.font) else {
            return 0.0;
        };
        let scaled = font.as_scaled(PxScale::from(self.state.font.size));
        text.chars()
            .map(|c| scaled.h_advance(scaled.glyph_id(c)))
            .sum()
    }

    // ---- Out
    /// The pixels, RGBA8, straight (not premultiplied) alpha, row by row from the top.
    pub fn rgba(&self) -> Vec<u8> {
        if self.width == 0 || self.height == 0 {
            return Vec::new();
        }
        let data = self.pixmap.data();
        let mut out = Vec::with_capacity(data.len());
        for px in data.chunks_exact(4) {
            let a = px[3] as u32;
            if a == 0 {
                out.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            let un = |c: u8| ((c as u32 * 255 + a / 2) / a).min(255) as u8;
            out.extend_from_slice(&[un(px[0]), un(px[1]), un(px[2]), a as u8]);
        }
        out
    }

    /// A clamped, linear, sRGB texture of the pixels (three's `CanvasTexture` defaults as the floor sets them).
    pub fn to_texture(&self) -> Tex {
        // A canvas texture, as three's CanvasTexture: its top row is v = 1 on a mesh (flipY).
        std::sync::Arc::new(Texture {
            flip_y: true,
            ..Texture::new(self.width, self.height, self.rgba())
        })
    }

    // ---- Internals

    /// Fills `path` (user space, under the current transform) with the fill style.
    fn fill_with(&mut self, path: &Path) {
        let Some(paint) = paint_for(&self.state.fill, self.state.alpha) else {
            return;
        };
        self.pixmap
            .fill_path(path, &paint, FillRule::Winding, self.state.transform, None);
    }

    /// Strokes `path` with the stroke style and the line settings.
    fn stroke_with(&mut self, path: &Path) {
        let Some(paint) = paint_for(&self.state.stroke, self.state.alpha) else {
            return;
        };
        let stroke = Stroke {
            width: self.state.line_width,
            miter_limit: 10.0,
            line_cap: self.state.cap,
            line_join: self.state.join,
            ..Stroke::default()
        };
        self.pixmap
            .stroke_path(path, &paint, &stroke, self.state.transform, None);
    }

    /// A subpath move: starts a new subpath at (x, y).
    fn add_move(&mut self, x: f32, y: f32) {
        self.path.move_to(x, y);
        self.current = Some((x, y));
        self.start = (x, y);
    }

    /// A line: `lineTo`, or a move when there is no current point.
    fn add_line(&mut self, x: f32, y: f32) {
        if self.current.is_none() {
            self.add_move(x, y);
        } else {
            self.path.line_to(x, y);
            self.current = Some((x, y));
        }
    }

    /// Appends an elliptical arc of signed `sweep` radians from angle `start`, as cubic
    /// Béziers of at most a quarter turn each, after a line to its start point.
    #[allow(clippy::too_many_arguments)]
    fn sweep_arc(&mut self, cx: f32, cy: f32, rx: f32, ry: f32, rot: f32, start: f32, sweep: f32) {
        if !(rx >= 0.0 && ry >= 0.0) {
            return;
        }
        let (sr, cr) = rot.sin_cos();
        let point = |t: f32| {
            let (s, c) = t.sin_cos();
            (
                cx + rx * c * cr - ry * s * sr,
                cy + rx * c * sr + ry * s * cr,
            )
        };
        let tangent = |t: f32| {
            let (s, c) = t.sin_cos();
            (-rx * s * cr - ry * c * sr, -rx * s * sr + ry * c * cr)
        };
        let (x0, y0) = point(start);
        self.add_line(x0, y0);
        let n = (sweep.abs() / FRAC_PI_2).ceil() as u32;
        if n == 0 {
            return;
        }
        let d = sweep / n as f32;
        let k = 4.0 / 3.0 * (d / 4.0).tan();
        let mut a = start;
        for _ in 0..n {
            let b = a + d;
            let (p0, p3) = (point(a), point(b));
            let (t0, t3) = (tangent(a), tangent(b));
            self.path.cubic_to(
                p0.0 + k * t0.0,
                p0.1 + k * t0.1,
                p3.0 - k * t3.0,
                p3.1 - k * t3.1,
                p3.0,
                p3.1,
            );
            a = b;
        }
        self.current = Some(point(a));
    }

    /// The glyph outlines of `text` as a path in user space, aligned and baselined at (x, y).
    fn text_path(&self, text: &str, x: f32, y: f32) -> Option<Path> {
        let font = font_for(&self.state.font)?;
        let scaled = font.as_scaled(PxScale::from(self.state.font.size));
        let width = self.measure_text(text);
        let left = match self.state.align {
            Align::Left => x,
            Align::Center => x - width / 2.0,
            Align::Right => x - width,
        };
        let (ascent, descent) = (scaled.ascent(), -scaled.descent());
        let base = y + match self.state.baseline {
            Baseline::Alphabetic => 0.0,
            Baseline::Top => ascent,
            Baseline::Bottom => -descent,
            Baseline::Middle => (ascent - descent) / 2.0,
        };
        let sf = scaled.scale_factor();
        // Font units are y-up; canvas pixels are y-down.
        let map = |pen: f32, p: GlyphPoint| (pen + p.x * sf.horizontal, base - p.y * sf.vertical);

        let mut pb = PathBuilder::new();
        let mut pen = left;
        let mut open = false;
        let mut at: Option<(f32, f32)> = None;
        for c in text.chars() {
            let id = scaled.glyph_id(c);
            if let Some(outline) = font.outline(id) {
                for curve in &outline.curves {
                    let start_pt = match *curve {
                        OutlineCurve::Line(a, _)
                        | OutlineCurve::Quad(a, _, _)
                        | OutlineCurve::Cubic(a, _, _, _) => a,
                    };
                    let start = map(pen, start_pt);
                    if at != Some(start) {
                        if open {
                            pb.close();
                        }
                        pb.move_to(start.0, start.1);
                        open = true;
                    }
                    match *curve {
                        OutlineCurve::Line(_, b) => {
                            let b = map(pen, b);
                            pb.line_to(b.0, b.1);
                            at = Some(b);
                        }
                        OutlineCurve::Quad(_, b, c) => {
                            let (b, c) = (map(pen, b), map(pen, c));
                            pb.quad_to(b.0, b.1, c.0, c.1);
                            at = Some(c);
                        }
                        OutlineCurve::Cubic(_, b, c, d) => {
                            let (b, c, d) = (map(pen, b), map(pen, c), map(pen, d));
                            pb.cubic_to(b.0, b.1, c.0, c.1, d.0, d.1);
                            at = Some(d);
                        }
                    }
                }
            }
            pen += scaled.h_advance(id);
        }
        if open {
            pb.close();
        }
        pb.finish()
    }
}

/// The signed sweep of `arc()` from `start` to `end`: clockwise (increasing angle) by default,
/// anticlockwise when `ccw`; a full turn when the span is at least 2π.
fn arc_sweep(start: f32, end: f32, ccw: bool) -> f32 {
    if ccw {
        if start - end >= TAU {
            -TAU
        } else {
            -((start - end).rem_euclid(TAU))
        }
    } else if end - start >= TAU {
        TAU
    } else {
        (end - start).rem_euclid(TAU)
    }
}

/// A closed rectangle path in user space.
fn rect_path(x: f32, y: f32, w: f32, h: f32) -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(x, y);
    pb.line_to(x + w, y);
    pb.line_to(x + w, y + h);
    pb.line_to(x, y + h);
    pb.close();
    pb.finish()
}

fn to_color(c: Rgba, alpha: f32) -> Color {
    let cl = |v: f32| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
    Color::from_rgba(cl(c[0]), cl(c[1]), cl(c[2]), cl(c[3] * alpha))
        .unwrap_or(Color::from_rgba8(0, 0, 0, 0))
}

fn gradient_stops(stops: &[(f32, Rgba)], alpha: f32) -> Vec<GradientStop> {
    stops
        .iter()
        .map(|(o, c)| GradientStop::new(*o, to_color(*c, alpha)))
        .collect()
}

/// The tiny-skia paint for a style at the canvas's global alpha; `None` when the gradient is degenerate.
fn paint_for(style: &Style, alpha: f32) -> Option<Paint<'static>> {
    let shader = match style {
        Style::Solid(c) => Shader::SolidColor(to_color(*c, alpha)),
        Style::Linear {
            x0,
            y0,
            x1,
            y1,
            stops,
        } => LinearGradient::new(
            Point::from_xy(*x0, *y0),
            Point::from_xy(*x1, *y1),
            gradient_stops(stops, alpha),
            SpreadMode::Pad,
            Transform::identity(),
        )?,
        Style::Radial {
            x0,
            y0,
            x1,
            y1,
            r1,
            stops,
        } => RadialGradient::new(
            Point::from_xy(*x0, *y0),
            Point::from_xy(*x1, *y1),
            *r1,
            gradient_stops(stops, alpha),
            SpreadMode::Pad,
            Transform::identity(),
        )?,
    };
    Some(Paint {
        shader,
        anti_alias: true,
        ..Paint::default()
    })
}

fn parse_stops(stops: &[(f32, &str)]) -> Vec<(f32, Rgba)> {
    stops
        .iter()
        .filter_map(|(o, c)| parse_css_colour(c).map(|col| (o.clamp(0.0, 1.0), col)))
        .collect()
}

/// A CSS colour: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb()`/`rgba()`, `hsl()`/`hsla()`
/// (comma or space separated, `%` for the channels that take it), or `white`/`black`/`transparent`.
pub(crate) fn parse_css_colour(css: &str) -> Option<Rgba> {
    let s = css.trim().to_ascii_lowercase();
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex(hex);
    }
    match s.as_str() {
        "transparent" => return Some([0.0, 0.0, 0.0, 0.0]),
        "white" => return Some([1.0, 1.0, 1.0, 1.0]),
        "black" => return Some([0.0, 0.0, 0.0, 1.0]),
        _ => {}
    }
    let open = s.find('(')?;
    if !s.ends_with(')') {
        return None;
    }
    let name = s[..open].trim();
    let args: Vec<&str> = s[open + 1..s.len() - 1]
        .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .collect();
    if args.len() != 3 && args.len() != 4 {
        return None;
    }
    let alpha = match args.get(3) {
        Some(a) => parse_alpha(a)?,
        None => 1.0,
    };
    match name {
        "rgb" | "rgba" => {
            let channel = |t: &str| -> Option<f32> {
                match t.strip_suffix('%') {
                    Some(p) => Some(p.parse::<f32>().ok()? / 100.0),
                    None => Some(t.parse::<f32>().ok()? / 255.0),
                }
            };
            Some([
                clamp01(channel(args[0])?),
                clamp01(channel(args[1])?),
                clamp01(channel(args[2])?),
                alpha,
            ])
        }
        "hsl" | "hsla" => {
            let h = args[0].trim_end_matches("deg").parse::<f32>().ok()?;
            let sat = clamp01(percent(args[1])?);
            let light = clamp01(percent(args[2])?);
            let [r, g, b] = hsl_to_rgb(h, sat, light);
            Some([r, g, b, alpha])
        }
        _ => None,
    }
}

fn clamp01(v: f32) -> f32 {
    if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }
}

fn percent(t: &str) -> Option<f32> {
    let v = t.strip_suffix('%').unwrap_or(t).parse::<f32>().ok()?;
    Some(v / 100.0)
}

fn parse_alpha(t: &str) -> Option<f32> {
    Some(clamp01(percent_or_unit(t)?))
}

/// An alpha written either as a fraction (`0.5`) or a percentage (`50%`).
fn percent_or_unit(t: &str) -> Option<f32> {
    match t.strip_suffix('%') {
        Some(p) => Some(p.parse::<f32>().ok()? / 100.0),
        None => t.parse::<f32>().ok(),
    }
}

fn parse_hex(h: &str) -> Option<Rgba> {
    let d: Vec<u8> = h
        .chars()
        .map(|c| c.to_digit(16).map(|v| v as u8))
        .collect::<Option<_>>()?;
    let (r, g, b, a) = match d.len() {
        3 => (d[0] * 17, d[1] * 17, d[2] * 17, 255),
        4 => (d[0] * 17, d[1] * 17, d[2] * 17, d[3] * 17),
        6 => (d[0] * 16 + d[1], d[2] * 16 + d[3], d[4] * 16 + d[5], 255),
        8 => (
            d[0] * 16 + d[1],
            d[2] * 16 + d[3],
            d[4] * 16 + d[5],
            d[6] * 16 + d[7],
        ),
        _ => return None,
    };
    Some([
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        a as f32 / 255.0,
    ])
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r + m, g + m, b + m]
}

/// Parses a CSS font shorthand: a pixel size, `bold`/weight ≥ 600 for bold, a family
/// containing `mono` or `courier` for the mono face. Without a pixel size it is invalid.
fn parse_font(css: &str) -> Option<FontSpec> {
    let lower = css.to_ascii_lowercase();
    let tokens: Vec<&str> = lower.split_whitespace().collect();
    let idx = tokens.iter().position(|t| t.ends_with("px"))?;
    let size = tokens[idx].trim_end_matches("px").parse::<f32>().ok()?;
    let bold = tokens[..idx]
        .iter()
        .any(|t| *t == "bold" || *t == "bolder" || t.parse::<u32>().map_or(false, |w| w >= 600));
    let family = tokens[idx + 1..].join(" ");
    let mono = family.contains("mono") || family.contains("courier");
    Some(FontSpec {
        size,
        mono,
        bold,
        martian: family.contains("martian"),
    })
}

/// The faces, indexed by `mono * 2 + bold`: the first system font found (DejaVu where Linux keeps
/// it, the platform's own sans and mono on macOS and Windows), else the bundled Martian Mono, so
/// labels always draw.
#[cfg(not(target_arch = "wasm32"))]
const FONT_FILES: [&[&str]; 4] = [
    &[
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "C:\\Windows\\Fonts\\arial.ttf",
    ],
    &[
        "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans-Bold.ttf",
        "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
        "C:\\Windows\\Fonts\\arialbd.ttf",
    ],
    &[
        "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/dejavu-sans-mono-fonts/DejaVuSansMono.ttf",
        "/System/Library/Fonts/Supplemental/Courier New.ttf",
        "C:\\Windows\\Fonts\\consola.ttf",
    ],
    &[
        "/usr/share/fonts/TTF/DejaVuSansMono-Bold.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf",
        "/usr/share/fonts/dejavu-sans-mono-fonts/DejaVuSansMono-Bold.ttf",
        "/System/Library/Fonts/Supplemental/Courier New Bold.ttf",
        "C:\\Windows\\Fonts\\consolab.ttf",
    ],
];

/// The bundled fallback (regular, bold), always present.
const BUNDLED: [&[u8]; 2] = [
    include_bytes!("../../../assets/fonts/MartianMono-Regular.ttf"),
    include_bytes!("../../../assets/fonts/MartianMono-Bold.ttf"),
];

const UNLOADED: OnceLock<Option<FontVec>> = OnceLock::new();
static FONTS: [OnceLock<Option<FontVec>>; 6] = [UNLOADED; 6];

/// The face for a font spec, loaded once: Martian Mono when the family names it (slots 4 and 5), else the system's.
fn font_for(spec: &FontSpec) -> Option<&'static FontVec> {
    let bundled = || FontVec::try_from_vec(BUNDLED[spec.bold as usize].to_vec()).ok();
    if spec.martian {
        return FONTS[4 + spec.bold as usize].get_or_init(bundled).as_ref();
    }
    let i = (spec.mono as usize) * 2 + spec.bold as usize;
    FONTS[i]
        .get_or_init(|| system_face(i).or_else(bundled))
        .as_ref()
}

/// The desktop's own face for slot `i`, the first of its files that reads. A browser has no file system: the
/// bundled face is used there.
#[cfg(not(target_arch = "wasm32"))]
fn system_face(i: usize) -> Option<FontVec> {
    FONT_FILES[i].iter().find_map(|path| {
        std::fs::read(path)
            .ok()
            .and_then(|d| FontVec::try_from_vec(d).ok())
    })
}

#[cfg(target_arch = "wasm32")]
fn system_face(_i: usize) -> Option<FontVec> {
    None
}
