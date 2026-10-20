//! The panel as a PNG, drawn once in Rust so the web and the desktop download the same image.
//!
//! [`panel_png`] draws a [`Panel`] whose body is a chart, bars or tiles on a `sheet` ground, in the colours of
//! resolved theme tokens, and encodes it. [`panel_pixmap`] stops before the encoding. Other bodies are refused.
//! The text is set in the embedded Geist faces: ab_glyph gives each glyph's outline, and tiny-skia fills it, so
//! the type is anti-aliased like the shapes. Everything is laid out in CSS pixels and scaled once by the
//! transform, so a panel at scale 2 is the same drawing at twice the resolution.

use crate::format::{format, ticks, time_label};
use crate::model::{BarRow, Body, Chart, Panel, Series, Status, Tile, Tone, XAxis};
use ab_glyph::{Font, FontRef, OutlineCurve, Point};
use apz_theme::{Rgba, Tokens};
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Stroke, StrokeDash,
    Transform,
};

const PAD: f32 = 16.0;
const GAP: f32 = 8.0;
const TITLE_PX: f32 = 15.0;
const SUB_PX: f32 = 12.0;
const LABEL_PX: f32 = 11.0;
const BAR_PX: f32 = 12.0;
const VALUE_PX: f32 = 22.0;
/// The most x-axis labels a chart prints; the rest of the points go unlabelled.
const X_LABELS: usize = 6;
/// The fill under an area, a stack or a band.
const AREA_ALPHA: f32 = 0.18;
/// The longest side of a PNG, in device pixels.
const MAX_SIDE: u32 = 8192;
const REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Geist-Regular.ttf");
const SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Geist-SemiBold.ttf");

/// The panel as PNG bytes, drawn with the given resolved tokens at `width` × `height` CSS pixels times `scale`.
///
/// Supports charts, bars and tiles. Any other body is an error, and so is a scale outside 0.1 to 4.
pub fn panel_png(
    panel: &Panel,
    tokens: &Tokens,
    width: u32,
    height: u32,
    scale: f32,
) -> Result<Vec<u8>, String> {
    panel_pixmap(panel, tokens, width, height, scale)?
        .encode_png()
        .map_err(|e| e.to_string())
}

/// The panel's pixels before encoding, `width × scale` by `height × scale` device pixels.
///
/// The ground is the `sheet` token, so every pixel outside the drawing is opaque.
pub fn panel_pixmap(
    panel: &Panel,
    tokens: &Tokens,
    width: u32,
    height: u32,
    scale: f32,
) -> Result<Pixmap, String> {
    if !(scale.is_finite() && (0.1..=4.0).contains(&scale)) {
        return Err(format!("the scale {scale} is outside 0.1 to 4"));
    }
    if width == 0 || height == 0 {
        return Err("a panel needs a width and a height".to_string());
    }
    let supported = matches!(
        panel.body,
        Body::Chart(_) | Body::Bars { .. } | Body::Tiles { .. }
    );
    if !supported {
        return Err("only chart, bars and tiles panels can be exported as PNG".to_string());
    }
    let px_w = (width as f32 * scale).round() as u32;
    let px_h = (height as f32 * scale).round() as u32;
    if px_w > MAX_SIDE || px_h > MAX_SIDE {
        return Err(format!(
            "a {px_w} x {px_h} PNG is larger than {MAX_SIDE} pixels a side"
        ));
    }
    let pixmap =
        Pixmap::new(px_w, px_h).ok_or_else(|| "the pixmap could not be allocated".to_string())?;
    let mut c = Canvas {
        pixmap,
        scale,
        css_w: width as f32,
        tokens,
        regular: load(REGULAR)?,
        semibold: load(SEMIBOLD)?,
    };
    let (w, h) = (width as f32, height as f32);
    let ground = c.tok("sheet", 1.0);
    c.pixmap.fill(ground);

    let ink = c.tok("ink", 1.0);
    let ink3 = c.tok("ink-3", 1.0);
    let title_base = PAD + TITLE_PX * 0.9;
    c.text(
        Weight::Semibold,
        TITLE_PX,
        PAD,
        title_base,
        &panel.title,
        ink,
    );
    if !panel.right.is_empty() {
        let right_w = c.width(Weight::Regular, LABEL_PX, &panel.right);
        c.text(
            Weight::Regular,
            LABEL_PX,
            w - PAD - right_w,
            title_base,
            &panel.right,
            ink3,
        );
    }
    let mut y = PAD + TITLE_PX * 1.4;
    if !panel.sub.is_empty() {
        c.text(
            Weight::Regular,
            SUB_PX,
            PAD,
            y + SUB_PX * 0.9,
            &panel.sub,
            ink3,
        );
        y += SUB_PX * 1.5;
    }
    match &panel.body {
        Body::Chart(chart) => {
            let items = chart_legend(&c, chart);
            if !items.is_empty() {
                y = legend(&mut c, &items, y, w);
            }
            draw_chart(&mut c, chart, (PAD, w - PAD, y + GAP, h - PAD));
        }
        Body::Bars { rows, note } => {
            let note_h = if note.is_empty() { 0.0 } else { LABEL_PX * 1.6 };
            draw_bars(&mut c, rows, (PAD, w - PAD, y + GAP, h - PAD - note_h));
            if !note.is_empty() {
                c.text(Weight::Regular, LABEL_PX, PAD, h - PAD - 2.0, note, ink3);
            }
        }
        Body::Tiles { tiles } => draw_tiles(&mut c, tiles, (PAD, w - PAD, y + GAP, h - PAD)),
        _ => {}
    }
    Ok(c.pixmap)
}

/// The token a tone names, as the themes spell it.
fn tone_token(tone: Tone) -> &'static str {
    match tone {
        Tone::C1 => "chart-1",
        Tone::C2 => "chart-2",
        Tone::C3 => "chart-3",
        Tone::C4 => "chart-4",
        Tone::C5 => "chart-5",
        Tone::C6 => "chart-6",
        Tone::C7 => "chart-7",
        Tone::C8 => "chart-8",
        Tone::Other => "chart-other",
        Tone::Ink3 => "ink-3",
        Tone::Rule => "rule",
        Tone::Ok => "ok",
        Tone::Warn => "warn",
        Tone::Crit => "crit",
        Tone::Info => "info",
        Tone::Signal => "signal",
    }
}

/// The tone a bar's status is drawn in.
fn status_tone(status: Status) -> Tone {
    match status {
        Status::Ok => Tone::Ok,
        Status::Warn => Tone::Warn,
        Status::Crit => Tone::Crit,
        Status::Info => Tone::Info,
        Status::Mute => Tone::Ink3,
    }
}

/// A token colour at the given extra alpha.
fn color(c: Rgba, alpha: f32) -> Color {
    Color::from_rgba(
        c.r.clamp(0.0, 1.0),
        c.g.clamp(0.0, 1.0),
        c.b.clamp(0.0, 1.0),
        (c.a * alpha).clamp(0.0, 1.0),
    )
    .unwrap_or(Color::BLACK)
}

fn paint(color: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;
    paint
}

fn load(bytes: &'static [u8]) -> Result<FontRef<'static>, String> {
    FontRef::try_from_slice(bytes).map_err(|e| format!("an embedded font does not parse: {e:?}"))
}

/// An open polyline, or a closed polygon, through the points; `None` for fewer than one point.
fn polygon(pts: &[(f32, f32)], closed: bool) -> Option<Path> {
    let (first, rest) = pts.split_first()?;
    let mut pb = PathBuilder::new();
    pb.move_to(first.0, first.1);
    for p in rest {
        pb.line_to(p.0, p.1);
    }
    if closed {
        pb.close();
    }
    pb.finish()
}

fn rect_pts(x: f32, y: f32, w: f32, h: f32) -> [(f32, f32); 4] {
    [(x, y), (x + w, y), (x + w, y + h), (x, y + h)]
}

/// The advance of a line of text at `px` CSS pixels, without kerning.
fn text_width(face: &FontRef<'static>, px: f32, text: &str) -> f32 {
    let k = px / face.units_per_em().unwrap_or(1000.0);
    text.chars()
        .map(|ch| face.h_advance_unscaled(face.glyph_id(ch)))
        .sum::<f32>()
        * k
}

/// Adds the outlines of a line of text to `pb`, with its baseline at `baseline`, and returns the advance.
fn text_path(
    pb: &mut PathBuilder,
    face: &FontRef<'static>,
    px: f32,
    x: f32,
    baseline: f32,
    text: &str,
) -> f32 {
    let k = px / face.units_per_em().unwrap_or(1000.0);
    let mut pen = x;
    for ch in text.chars() {
        let id = face.glyph_id(ch);
        let origin = pen;
        // Outline coordinates are in font units with y up; the canvas has y down.
        let map = |p: Point| (origin + p.x * k, baseline - p.y * k);
        if let Some(outline) = face.outline(id) {
            let mut at: Option<(f32, f32)> = None;
            for curve in outline.curves {
                match curve {
                    OutlineCurve::Line(a, b) => {
                        let (s, e) = (map(a), map(b));
                        if at != Some(s) {
                            pb.move_to(s.0, s.1);
                        }
                        pb.line_to(e.0, e.1);
                        at = Some(e);
                    }
                    OutlineCurve::Quad(a, b, c) => {
                        let (s, q, e) = (map(a), map(b), map(c));
                        if at != Some(s) {
                            pb.move_to(s.0, s.1);
                        }
                        pb.quad_to(q.0, q.1, e.0, e.1);
                        at = Some(e);
                    }
                    OutlineCurve::Cubic(a, b, c, d) => {
                        let (s, q1, q2, e) = (map(a), map(b), map(c), map(d));
                        if at != Some(s) {
                            pb.move_to(s.0, s.1);
                        }
                        pb.cubic_to(q1.0, q1.1, q2.0, q2.1, e.0, e.1);
                        at = Some(e);
                    }
                }
            }
        }
        pen += face.h_advance_unscaled(id) * k;
    }
    pen - x
}

#[derive(Clone, Copy)]
enum Weight {
    Regular,
    Semibold,
}

/// The drawing surface: a pixmap at `scale` device pixels per CSS pixel, the tokens and the two faces.
struct Canvas<'a> {
    pixmap: Pixmap,
    scale: f32,
    css_w: f32,
    tokens: &'a Tokens,
    regular: FontRef<'static>,
    semibold: FontRef<'static>,
}

impl Canvas<'_> {
    fn tok(&self, name: &str, alpha: f32) -> Color {
        color(self.tokens.get(name), alpha)
    }

    fn tone(&self, tone: Tone, alpha: f32) -> Color {
        self.tok(tone_token(tone), alpha)
    }

    fn face(&self, weight: Weight) -> &FontRef<'static> {
        match weight {
            Weight::Regular => &self.regular,
            Weight::Semibold => &self.semibold,
        }
    }

    fn width(&self, weight: Weight, px: f32, text: &str) -> f32 {
        text_width(self.face(weight), px, text)
    }

    fn transform(&self) -> Transform {
        Transform::from_scale(self.scale, self.scale)
    }

    fn fill(&mut self, path: &Path, color: Color) {
        let transform = self.transform();
        self.pixmap
            .fill_path(path, &paint(color), FillRule::Winding, transform, None);
    }

    fn fill_poly(&mut self, pts: &[(f32, f32)], color: Color) {
        if let Some(path) = polygon(pts, true) {
            self.fill(&path, color);
        }
    }

    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        self.fill_poly(&rect_pts(x, y, w, h), color);
    }

    fn stroke_poly(
        &mut self,
        pts: &[(f32, f32)],
        color: Color,
        width: f32,
        dashed: bool,
        closed: bool,
    ) {
        let Some(path) = polygon(pts, closed) else {
            return;
        };
        let stroke = Stroke {
            width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            dash: if dashed {
                StrokeDash::new(vec![4.0, 3.0], 0.0)
            } else {
                None
            },
            ..Stroke::default()
        };
        let transform = self.transform();
        self.pixmap
            .stroke_path(&path, &paint(color), &stroke, transform, None);
    }

    fn text(&mut self, weight: Weight, px: f32, x: f32, baseline: f32, text: &str, color: Color) {
        let mut pb = PathBuilder::new();
        text_path(&mut pb, self.face(weight), px, x, baseline, text);
        if let Some(path) = pb.finish() {
            self.fill(&path, color);
        }
    }
}

/// The legend's swatches and names, in series order, then the band.
fn chart_legend(c: &Canvas, chart: &Chart) -> Vec<(String, Color)> {
    let mut items: Vec<(String, Color)> = chart
        .series
        .iter()
        .map(|s| (s.name.clone(), c.tone(s.tone, 1.0)))
        .collect();
    if let Some(band) = &chart.band {
        items.push((band.name.clone(), c.tone(band.tone, 1.0)));
    }
    items
}

/// Lays the legend out in rows under `top` and returns the top of what follows.
fn legend(c: &mut Canvas, items: &[(String, Color)], top: f32, width: f32) -> f32 {
    let ink2 = c.tok("ink-2", 1.0);
    let right = width - PAD;
    let mut x = PAD;
    let mut base = top + LABEL_PX * 0.9;
    for (label, swatch) in items {
        let item = 14.0 + c.width(Weight::Regular, LABEL_PX, label) + 14.0;
        if x > PAD && x + item > right {
            x = PAD;
            base += 16.0;
        }
        c.fill_rect(x, base - 7.0, 10.0, 3.0, *swatch);
        c.text(Weight::Regular, LABEL_PX, x + 14.0, base, label, ink2);
        x += item;
    }
    base + 8.0
}

/// The runs of consecutive points a series has, each as (index, bottom, top) in value units. A `None` ends a
/// run: a gap, never a zero.
fn spans(values: &[Option<f64>], bottom: &[f64], n: usize) -> Vec<Vec<(usize, f64, f64)>> {
    let mut out = Vec::new();
    let mut run = Vec::new();
    for i in 0..n {
        match values.get(i).copied().flatten() {
            Some(v) => {
                let base = bottom.get(i).copied().unwrap_or(0.0);
                run.push((i, base, base + v));
            }
            None => {
                if !run.is_empty() {
                    out.push(std::mem::take(&mut run));
                }
            }
        }
    }
    if !run.is_empty() {
        out.push(run);
    }
    out
}

/// One series: its area (under the line, or between the stack below and the series) and its line.
#[allow(clippy::too_many_arguments)]
fn draw_series(
    c: &mut Canvas,
    s: &Series,
    bottom: &[f64],
    n: usize,
    area: bool,
    dashed: bool,
    x_at: &dyn Fn(usize) -> f32,
    y_of: &dyn Fn(f64) -> f32,
) {
    let line = c.tone(s.tone, 1.0);
    let fill = c.tone(s.tone, AREA_ALPHA);
    for run in spans(&s.values, bottom, n) {
        if area {
            let mut pts: Vec<(f32, f32)> = run
                .iter()
                .map(|&(i, _, top)| (x_at(i), y_of(top)))
                .collect();
            pts.extend(run.iter().rev().map(|&(i, base, _)| (x_at(i), y_of(base))));
            c.fill_poly(&pts, fill);
        }
        if run.len() >= 2 {
            let pts: Vec<(f32, f32)> = run
                .iter()
                .map(|&(i, _, top)| (x_at(i), y_of(top)))
                .collect();
            c.stroke_poly(&pts, line, 2.0, dashed, false);
        }
    }
}

/// A time chart or a label chart inside `region` (left, right, top, bottom in CSS pixels).
fn draw_chart(c: &mut Canvas, chart: &Chart, region: (f32, f32, f32, f32)) {
    let (left, right, top, bottom) = region;
    let unit = chart.unit;
    let labels_len = match &chart.x {
        XAxis::Labels { labels } => labels.len(),
        XAxis::Time { .. } => 0,
    };
    let n = chart
        .series
        .iter()
        .chain(&chart.ghosts)
        .map(|s| s.values.len())
        .fold(labels_len, usize::max);

    // The stack: each series sits on the sum of the ones before it when stacked, else on zero.
    let mut acc = vec![0.0f64; n];
    let mut bottoms: Vec<Vec<f64>> = Vec::with_capacity(chart.series.len());
    for s in &chart.series {
        bottoms.push(if chart.stacked {
            acc.clone()
        } else {
            vec![0.0; n]
        });
        if chart.stacked {
            for (i, slot) in acc.iter_mut().enumerate() {
                *slot += s.values.get(i).copied().flatten().unwrap_or(0.0);
            }
        }
    }
    let mut data_max = 0.0f64;
    if chart.stacked {
        for total in &acc {
            data_max = data_max.max(*total);
        }
    } else {
        for s in &chart.series {
            for v in s.values.iter().flatten() {
                data_max = data_max.max(*v);
            }
        }
    }
    for g in &chart.ghosts {
        for v in g.values.iter().flatten() {
            data_max = data_max.max(*v);
        }
    }
    if let Some(band) = &chart.band {
        for v in &band.hi {
            data_max = data_max.max(*v);
        }
    }
    if let Some(limit) = &chart.limit {
        data_max = data_max.max(limit.value);
    }
    let ticks_v = ticks(chart.y_max.unwrap_or(data_max));
    let ym = ticks_v[4];
    let tick_labels: Vec<String> = ticks_v.iter().map(|t| format(unit, *t)).collect();
    let label_w = tick_labels
        .iter()
        .map(|l| c.width(Weight::Regular, LABEL_PX, l))
        .fold(0.0f32, f32::max);
    let x0 = left + label_w + 8.0;
    let x1 = right - 6.0;
    let y0 = top + 6.0;
    let y1 = bottom - LABEL_PX * 2.0;

    let x_at = |i: usize| -> f32 {
        if n <= 1 {
            (x0 + x1) / 2.0
        } else {
            x0 + (x1 - x0) * i as f32 / (n - 1) as f32
        }
    };
    let y_of = |v: f64| -> f32 {
        let frac = (v / ym).clamp(0.0, 1.0) as f32;
        y1 - frac * (y1 - y0)
    };

    let grid = c.tok("chart-grid", 1.0);
    let axis = c.tok("chart-axis", 1.0);
    for (t, label) in ticks_v.iter().zip(&tick_labels) {
        let y = y_of(*t);
        c.stroke_poly(&[(x0, y), (x1, y)], grid, 1.0, false, false);
        let w = c.width(Weight::Regular, LABEL_PX, label);
        c.text(
            Weight::Regular,
            LABEL_PX,
            x0 - 6.0 - w,
            y + LABEL_PX * 0.35,
            label,
            axis,
        );
    }

    if let Some(band) = &chart.band {
        let m = band.hi.len().min(band.lo.len()).min(n);
        if m >= 2 {
            let mut pts: Vec<(f32, f32)> = (0..m).map(|i| (x_at(i), y_of(band.hi[i]))).collect();
            pts.extend((0..m).rev().map(|i| (x_at(i), y_of(band.lo[i]))));
            let fill = c.tone(band.tone, AREA_ALPHA);
            c.fill_poly(&pts, fill);
        }
    }

    for (s, bottom) in chart.series.iter().zip(&bottoms) {
        draw_series(
            c,
            s,
            bottom,
            n,
            chart.area || chart.stacked,
            s.dash,
            &x_at,
            &y_of,
        );
    }
    let zeros = vec![0.0f64; n];
    for g in &chart.ghosts {
        draw_series(c, g, &zeros, n, false, true, &x_at, &y_of);
    }

    let mark = c.tok("ink-3", 0.7);
    let ink2 = c.tok("ink-2", 1.0);
    for m in &chart.markers {
        if m.index < n {
            let x = x_at(m.index);
            c.stroke_poly(&[(x, y0), (x, y1)], mark, 1.0, true, false);
            c.text(
                Weight::Regular,
                LABEL_PX,
                x + 4.0,
                y0 + LABEL_PX,
                &m.label,
                ink2,
            );
        }
    }
    if let Some(limit) = &chart.limit {
        let y = y_of(limit.value);
        c.stroke_poly(&[(x0, y), (x1, y)], ink2, 1.0, true, false);
        let w = c.width(Weight::Regular, LABEL_PX, &limit.label);
        c.text(
            Weight::Regular,
            LABEL_PX,
            x1 - w,
            y - 4.0,
            &limit.label,
            ink2,
        );
    }

    let every = n.div_ceil(X_LABELS).max(1);
    for i in (0..n).step_by(every) {
        let text = match &chart.x {
            XAxis::Time { start_ms, step_ms } => {
                time_label(*start_ms + i as u64 * *step_ms, *step_ms, 0)
            }
            XAxis::Labels { labels } => labels.get(i).cloned().unwrap_or_default(),
        };
        if text.is_empty() {
            continue;
        }
        let w = c.width(Weight::Regular, LABEL_PX, &text);
        let x = (x_at(i) - w / 2.0).min(c.css_w - PAD - w).max(PAD);
        c.text(Weight::Regular, LABEL_PX, x, bottom - 2.0, &text, axis);
    }
}

/// Bar rows inside `region`, each a label, a value, a track with the bar, and an optional note.
fn draw_bars(c: &mut Canvas, rows: &[BarRow], region: (f32, f32, f32, f32)) {
    let (x0, x1, top, bottom) = region;
    let row_h = 42.0;
    let track = c.tok("chart-grid", 1.0);
    let ink = c.tok("ink", 1.0);
    let ink2 = c.tok("ink-2", 1.0);
    let ink3 = c.tok("ink-3", 1.0);
    let mut y = top;
    for row in rows {
        if y + row_h > bottom {
            break;
        }
        c.text(Weight::Regular, BAR_PX, x0, y + BAR_PX, &row.label, ink2);
        let value_w = c.width(Weight::Semibold, BAR_PX, &row.value);
        c.text(
            Weight::Semibold,
            BAR_PX,
            x1 - value_w,
            y + BAR_PX,
            &row.value,
            ink,
        );
        let bar_y = y + 18.0;
        c.fill_rect(x0, bar_y, x1 - x0, 8.0, track);
        let frac = if row.frac.is_finite() {
            row.frac.clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let fill = c.tone(status_tone(row.status), 1.0);
        c.fill_rect(x0, bar_y, (x1 - x0) * frac, 8.0, fill);
        if !row.note.is_empty() {
            c.text(Weight::Regular, LABEL_PX, x0, y + 36.0, &row.note, ink3);
        }
        y += row_h;
    }
}

/// Tiles inside `region`: a grid of cards, each with its label, value, change, sparkline and note.
fn draw_tiles(c: &mut Canvas, tiles: &[Tile], region: (f32, f32, f32, f32)) {
    let (x0, x1, top, bottom) = region;
    let n = tiles.len();
    if n == 0 {
        return;
    }
    let cols = if n <= 3 {
        n
    } else if n <= 6 {
        3
    } else {
        4
    };
    let rows = n.div_ceil(cols);
    let card_w = (x1 - x0 - GAP * (cols as f32 - 1.0)) / cols as f32;
    let card_h = ((bottom - top - GAP * (rows as f32 - 1.0)) / rows as f32).max(1.0);
    let card = c.tok("sheet-2", 1.0);
    let edge = c.tok("rule", 1.0);
    let ink = c.tok("ink", 1.0);
    let ink3 = c.tok("ink-3", 1.0);
    let ok = c.tok("ok", 1.0);
    let crit = c.tok("crit", 1.0);
    let pad = 12.0;
    for (k, tile) in tiles.iter().enumerate() {
        let x = x0 + (k % cols) as f32 * (card_w + GAP);
        let y = top + (k / cols) as f32 * (card_h + GAP);
        c.fill_rect(x, y, card_w, card_h, card);
        c.stroke_poly(&rect_pts(x, y, card_w, card_h), edge, 1.0, false, true);
        c.text(
            Weight::Regular,
            LABEL_PX,
            x + pad,
            y + 16.0,
            &tile.label,
            ink3,
        );
        c.text(
            Weight::Semibold,
            VALUE_PX,
            x + pad,
            y + 44.0,
            &tile.value,
            ink,
        );
        if let Some(pct) = tile.change_pct.filter(|p| p.is_finite()) {
            let good = (pct > 0.0 && !tile.up_bad) || (pct < 0.0 && tile.up_bad);
            let bad = (pct > 0.0 && tile.up_bad) || (pct < 0.0 && !tile.up_bad);
            let col = if good {
                ok
            } else if bad {
                crit
            } else {
                ink3
            };
            c.text(
                Weight::Regular,
                LABEL_PX,
                x + pad,
                y + 62.0,
                &format!("{pct:+.1} %"),
                col,
            );
        }
        let values: Vec<f64> = tile
            .spark
            .iter()
            .copied()
            .filter(|v| v.is_finite())
            .collect();
        if values.len() >= 2 && card_h > 110.0 {
            let (sx0, sx1) = (x + pad, x + card_w - pad);
            let (sy0, sy1) = (y + 68.0, y + card_h - 30.0);
            let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let span = if hi > lo { hi - lo } else { 1.0 };
            let pts: Vec<(f32, f32)> = values
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let px = sx0 + (sx1 - sx0) * i as f32 / (values.len() - 1) as f32;
                    let py = sy1 - ((v - lo) / span) as f32 * (sy1 - sy0);
                    (px, py)
                })
                .collect();
            let line = c.tone(tile.tone, 1.0);
            c.stroke_poly(&pts, line, 1.5, false, false);
        }
        if !tile.note.is_empty() {
            c.text(
                Weight::Regular,
                LABEL_PX,
                x + pad,
                y + card_h - 12.0,
                &tile.note,
                ink3,
            );
        }
    }
}
