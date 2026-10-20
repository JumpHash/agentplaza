//! The backdrop's maths: the uniform block one shader (`backdrop.wgsl`, here) draws a background spec from, and a
//! CPU reference of the same picture. `apz-engine` runs the shader on the desktop (wgpu) and the web console runs
//! it in the browser (WebGPU, or WebGL2 through the GLSL the build translates); the desktop's tiny-skia fallback
//! and the golden tests use [`render_cpu`]. Keep the two in step: a change to one is a change to the other.

use crate::background::{Motion, Ornament, Pattern};
use crate::{Background, Rgba, Tokens};

/// The frame rate a moving backdrop is drawn at, at most.
pub const MAX_FPS: f32 = 30.0;

/// The css px over which the grid pattern fades out from the top of the window.
const TOP_FADE: f32 = 480.0;

/// The radius of the pointer light, in css px (the prototype's lit patch is 520 across).
const LIGHT_RADIUS: f32 = 260.0;

/// How far the grain's noise strays from mid grey (the shader's `GRAIN_SPREAD`).
const GRAIN_SPREAD: f32 = 0.35;

/// The sidebar's width in css px: the second colour field sits by it (the prototype's `--side-w`).
pub const SIDE_W: f32 = 288.0;

/// The shader's uniform block. It mirrors `Uniforms` in backdrop.wgsl field for field, and holds only vec4s so that
/// the layout is the same on every backend, WebGL2 included.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Uniforms {
    /// Width and height of the target in device px, the scale (device px per css px) and the time in seconds.
    pub size: [f32; 4],
    /// The pointer x and y in css px, whether a pointer is present (0 or 1), and the sidebar's width in css px.
    pub pointer: [f32; 4],
    /// The ground colour (the token `panel`).
    pub ground: [f32; 4],
    /// The pattern colour (the token `pattern`; `amb-line`, with its alpha, for the grid).
    pub pattern: [f32; 4],
    /// The pointer light's colour (the token `signal`).
    pub signal: [f32; 4],
    /// The first colour field; a missing field is the ground.
    pub field0: [f32; 4],
    /// The second colour field.
    pub field1: [f32; 4],
    /// The third colour field.
    pub field2: [f32; 4],
    /// The pattern id (0 none, 1 dots, 2 weave, 3 grid, 4 blueprint, 5 contours), the cell in device px, the field
    /// strength and the grain amplitude.
    pub params: [f32; 4],
    /// The ornament id (0 none, 1 threads, 2 orbits; not drawn by the shader), whether motion is on (0 or 1), whether
    /// the pointer light is on (0 or 1), and the top fade in css px.
    pub params2: [f32; 4],
}

impl Uniforms {
    /// The block as 40 floats in field order, the bytes the shader reads.
    pub fn floats(&self) -> [f32; 40] {
        let mut out = [0.0; 40];
        for (i, v) in [
            self.size,
            self.pointer,
            self.ground,
            self.pattern,
            self.signal,
            self.field0,
            self.field1,
            self.field2,
            self.params,
            self.params2,
        ]
        .iter()
        .enumerate()
        {
            out[i * 4..i * 4 + 4].copy_from_slice(v);
        }
        out
    }
}

/// The shader, written once for every renderer.
pub const WGSL: &str = include_str!("backdrop.wgsl");

/// The uniforms for a background spec on a palette. `width` and `height` are the target in device px, `scale` is
/// device px per css px, `time` is in seconds, and `pointer` is in css px when a pointer is over the window.
pub fn uniforms(
    bg: &Background,
    tokens: &Tokens,
    width: f32,
    height: f32,
    scale: f32,
    time: f32,
    pointer: Option<(f32, f32)>,
) -> Uniforms {
    let scale = scale.max(0.1);
    let [f0, f1, f2] = bg.field_colors(tokens);
    let (px, py, on) = match pointer {
        Some((x, y)) => (x, y, 1.0),
        None => (0.0, 0.0, 0.0),
    };
    let ground = tokens.get("panel");
    let dark = 0.2126 * ground.r + 0.7152 * ground.g + 0.0722 * ground.b < 0.5;
    let strength = if dark {
        bg.strength_dark.unwrap_or(bg.strength)
    } else {
        bg.strength
    };
    let grain = if dark {
        bg.grain_dark.unwrap_or(bg.grain)
    } else {
        bg.grain
    };
    // The grid draws in `amb-line` with its alpha; every other pattern in the opaque `pattern`.
    let ink = if bg.pattern == Pattern::Grid {
        let c = tokens.get("amb-line");
        [c.r, c.g, c.b, c.a]
    } else {
        opaque(tokens.get("pattern"))
    };
    let pattern = match bg.pattern {
        Pattern::None => 0.0,
        Pattern::Dots => 1.0,
        Pattern::Weave => 2.0,
        Pattern::Grid => 3.0,
        Pattern::Blueprint => 4.0,
        Pattern::Contours => 5.0,
    };
    let ornament = match bg.ornament {
        Ornament::None => 0.0,
        Ornament::Threads => 1.0,
        Ornament::Orbits => 2.0,
    };
    Uniforms {
        size: [width, height, scale, time],
        pointer: [px, py, on, SIDE_W],
        ground: opaque(ground),
        pattern: ink,
        signal: opaque(tokens.get("signal")),
        field0: opaque(f0),
        field1: opaque(f1),
        field2: opaque(f2),
        params: [pattern, bg.scale * scale, strength, grain],
        params2: [
            ornament,
            flag(bg.motion == Motion::Slow),
            flag(bg.pointer),
            TOP_FADE,
        ],
    }
}

/// Whether the backdrop moves: the motion is slow, or the pointer light is on. Whether a pointer is actually over
/// the window is the host's to check.
pub fn animates(bg: &Background) -> bool {
    bg.motion == Motion::Slow || bg.pointer
}

/// The CPU picture of `u`, RGBA8, row-major, `width` by `height` pixels. The maths follow backdrop.wgsl exactly.
pub fn render_cpu(u: &Uniforms, width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let c = shade(u, [x as f32 + 0.5, y as f32 + 0.5]);
            for ch in c {
                out.push((ch.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
            out.push(255);
        }
    }
    out
}

/// The colour at device pixel `p`, the same steps as `fs_main` in backdrop.wgsl.
fn shade(u: &Uniforms, p: [f32; 2]) -> [f32; 3] {
    let size = [u.size[0], u.size[1]];
    let s = u.size[2];
    let t = u.size[3];
    let mot = u.params2[1];
    let lw = s.max(1.0);
    let pc = [p[0] / s, p[1] / s];
    let (w, h) = (size[0] / s, size[1] / s);

    let mut col = rgb(u.ground);
    let strength = u.params[2];
    for (i, field) in [u.field0, u.field1, u.field2].into_iter().enumerate() {
        let a = strength * field_alpha(pc, i, w, h, u.pointer[3], t, mot);
        col = mix(col, rgb(field), a);
    }

    let kind = (u.params[0] + 0.5).floor() as i32;
    if kind != 0 {
        let mut ink = rgb(u.pattern);
        if u.pointer[2] > 0.5 && u.params2[2] > 0.5 {
            let pp = [u.pointer[0] * s, u.pointer[1] * s];
            let light = 1.0 - smoothstep(0.0, LIGHT_RADIUS * s, dist(p, pp));
            ink = mix(ink, rgb(u.signal), 0.7 * light);
        }
        col = mix(
            col,
            ink,
            pattern_cover(p, kind, u.params[1], lw, s, size) * u.pattern[3],
        );
    }

    let anim = mot.max(u.pointer[2] * u.params2[2]);
    let seed = if anim > 0.5 { (t * 12.0).floor() } else { 0.0 };
    let mut n = hash01(
        (p[0] / s).floor() as u32,
        (p[1] / s).floor() as u32,
        seed.max(0.0) as u32,
    );
    let g = u.ground;
    if 0.2126 * g[0] + 0.7152 * g[1] + 0.0722 * g[2] < 0.5 {
        n = 1.0 - n;
    }
    let n = 0.5 + (n - 0.5) * GRAIN_SPREAD;
    let col = mix(col, [n, n, n], u.params[3]);
    [
        col[0].clamp(0.0, 1.0),
        col[1].clamp(0.0, 1.0),
        col[2].clamp(0.0, 1.0),
    ]
}

/// Ease in and out, as the shader's `ease`.
fn ease(x: f32) -> f32 {
    x * x * (3.0 - 2.0 * x)
}

/// How far along its drift a field is at `t`: there and back over twice `period`, eased.
fn drift(t: f32, period: f32) -> f32 {
    let u = (t / period) % 2.0;
    let tri = if u < 1.0 { u } else { 2.0 - u };
    ease(tri.clamp(0.0, 1.0))
}

/// The alpha of colour field `idx` at `p` (css px), as the shader's `field_alpha`: the prototype's `.amb-f1..3`.
fn field_alpha(p: [f32; 2], idx: usize, w: f32, h: f32, side: f32, t: f32, mot: f32) -> f32 {
    let v = w.max(h) * 0.01;
    let (c, r, pct, to, period) = match idx {
        0 => (
            [w - 12.0 * v, -2.0 * v],
            32.0 * v,
            0.24,
            [-16.0 * v, 9.0 * v, 1.12],
            46.0,
        ),
        1 => (
            [side + 14.0 * v, -4.0 * v],
            26.0 * v,
            0.18,
            [20.0 * v, 7.0 * v, 0.88],
            61.0,
        ),
        _ => (
            [w - 22.0 * v, h + 14.0 * v],
            36.0 * v,
            0.15,
            [-22.0 * v, -10.0 * v, 1.08],
            73.0,
        ),
    };
    let k = mot * drift(t, period);
    let c = [c[0] + to[0] * k, c[1] + to[1] * k];
    let r = r * (1.0 + (to[2] - 1.0) * k);
    pct * (1.0 - dist(p, c) / r).clamp(0.0, 1.0)
}

/// Coverage of the pattern at `p`, 0..1, for pattern `kind` (1 to 5).
fn pattern_cover(p: [f32; 2], kind: i32, cell: f32, lw: f32, s: f32, size: [f32; 2]) -> f32 {
    match kind {
        1 => {
            let c = [
                ((p[0] / cell).floor() + 0.5) * cell,
                ((p[1] / cell).floor() + 0.5) * cell,
            ];
            ((1.2 - dist(p, c) / s) / 0.2).clamp(0.0, 1.0)
        }
        2 => {
            let h = line_cov(line_dist(p[1], cell), lw) * 0.5;
            let v = line_cov(line_dist(p[0], cell), lw) * 0.5;
            let cx = (p[0] / cell).floor() as i32;
            let cy = (p[1] / cell).floor() as i32;
            if ((cx + cy) & 1) == 0 {
                h.max(v * (1.0 - h * 2.0))
            } else {
                v.max(h * (1.0 - v * 2.0))
            }
        }
        3 => {
            let q = [p[0] / s, p[1] / s];
            let cs = cell / s;
            let g = if q[0] % cs < 1.0 || q[1] % cs < 1.0 {
                1.0
            } else {
                0.0
            };
            let (wc, hc) = (size[0] / s, size[1] / s);
            let e =
                (((q[0] - 0.62 * wc) / (1.3 * wc)).powi(2) + (q[1] / (0.95 * hc)).powi(2)).sqrt();
            let mask = 1.0 - ((e - 0.18) / (0.72 - 0.18)).clamp(0.0, 1.0);
            g * mask
        }
        4 => {
            let fine_cell = cell * 0.25;
            let fine = line_cov(line_dist(p[0], fine_cell), lw)
                .max(line_cov(line_dist(p[1], fine_cell), lw));
            let major =
                line_cov(line_dist(p[0], cell), lw).max(line_cov(line_dist(p[1], cell), lw));
            (fine * 0.35).max(major * 0.6)
        }
        5 => {
            let q = [p[0] / cell, p[1] / cell];
            let level = (f32::sin(q[0] * 0.37)
                + f32::sin(q[1] * 0.29)
                + f32::sin((q[0] + q[1]) * 0.19)
                + f32::sin((q[0] - q[1]) * 0.23))
                * 0.125
                + 0.5;
            let x = level * 10.0;
            let t = x - x.floor();
            0.6 * (1.0 - t.min(1.0 - t) / 0.05).clamp(0.0, 1.0)
        }
        _ => 0.0,
    }
}

/// Coverage of a line centred on distance `d` from it, `w` device px wide.
fn line_cov(d: f32, w: f32) -> f32 {
    (w * 0.5 + 0.5 - d).clamp(0.0, 1.0)
}

/// The nearest multiple of `cell` to `v`.
fn nearest(v: f32, cell: f32) -> f32 {
    (v / cell + 0.5).floor() * cell
}

/// The distance from `v` to the nearest multiple of `cell`.
fn line_dist(v: f32, cell: f32) -> f32 {
    (v - nearest(v, cell)).abs()
}

/// The shader's hash of an integer pixel coordinate and a frame seed, 0..1.
fn hash01(x: u32, y: u32, s: u32) -> f32 {
    let mut h =
        (x.wrapping_mul(0x27d4_eb2d) ^ y.wrapping_mul(0x1656_67b1)) ^ s.wrapping_mul(0x9e37_79b9);
    h = (h ^ (h >> 15)).wrapping_mul(0x2c1b_3c6d);
    h = (h ^ (h >> 12)).wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65535.0
}

/// The GLSL-style smoothstep.
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The distance between two points.
fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Linear mix of two colours.
fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// The first three components of a uniform colour.
fn rgb(c: [f32; 4]) -> [f32; 3] {
    [c[0], c[1], c[2]]
}

/// A palette colour as a uniform colour, opaque.
fn opaque(c: Rgba) -> [f32; 4] {
    [c.r, c.g, c.b, 1.0]
}

/// 0 or 1 as a uniform value.
fn flag(on: bool) -> f32 {
    if on { 1.0 } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clay() -> Tokens {
        let file = crate::builtin::builtin("loom-clay").expect("the loom-clay theme");
        let skin = crate::skin::skin("loom").expect("the loom skin");
        crate::resolve(&file, &skin, crate::Mode::Light).tokens
    }

    fn paper() -> Background {
        crate::background::preset_by_id("loom-paper").expect("the loom-paper preset")
    }

    fn fnv(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
        })
    }

    fn pixel(img: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * width + x) * 4) as usize;
        [img[i], img[i + 1], img[i + 2], img[i + 3]]
    }

    #[test]
    fn loom_paper_uniforms_carry_the_palette_and_the_spec() {
        let tokens = clay();
        let u = uniforms(&paper(), &tokens, 64.0, 48.0, 2.0, 1.5, Some((10.0, 20.0)));
        let ground = tokens.get("panel");
        assert_eq!(u.ground[0], ground.r);
        assert_eq!(u.ground[1], ground.g);
        assert_eq!(u.ground[2], ground.b);
        assert_eq!(u.size, [64.0, 48.0, 2.0, 1.5]);
        assert_eq!(u.params[0], 1.0, "dots");
        assert_eq!(u.params[1], 44.0, "a 22 css px cell at scale 2");
        assert_eq!(u.params2[0], 1.0, "threads");
        assert_eq!(u.params2[1], 1.0, "slow motion");
        assert_eq!(u.params2[2], 1.0, "the pointer light");
        assert_eq!(u.pointer[2], 1.0, "a pointer is present");
        assert!(animates(&paper()));
        assert!(!animates(&Background::default()));
        assert_eq!(std::mem::size_of::<Uniforms>(), 160, "ten vec4s");
    }

    #[test]
    fn far_from_a_field_the_picture_is_the_ground() {
        let tokens = clay();
        let mut bg = paper();
        bg.pattern = Pattern::None;
        bg.grain = 0.0;
        bg.pointer = false;
        bg.motion = Motion::Still;
        // One field only: with three, every pixel of a 64 by 48 picture is near one of them.
        bg.fields.truncate(1);
        let u = uniforms(&bg, &tokens, 64.0, 48.0, 1.0, 0.0, None);
        let img = render_cpu(&u, 64, 48);
        let g = tokens.get("panel");
        let want = [g.r, g.g, g.b].map(|c| (c * 255.0).round() as i32);
        let got = pixel(&img, 64, 63, 47);
        for ch in 0..3 {
            assert!(
                (i32::from(got[ch]) - want[ch]).abs() <= 6,
                "channel {ch}: got {got:?}, ground {want:?}"
            );
        }
    }

    #[test]
    fn dots_darken_a_cell_centre() {
        let tokens = clay();
        let mut bg = paper();
        bg.fields.clear();
        bg.grain = 0.0;
        bg.pointer = false;
        bg.motion = Motion::Still;
        let u = uniforms(&bg, &tokens, 64.0, 48.0, 1.0, 0.0, None);
        let img = render_cpu(&u, 64, 48);
        let sum = |p: [u8; 4]| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]);
        let corner = pixel(&img, 64, 11, 11);
        let away = pixel(&img, 64, 22, 22);
        assert!(
            sum(corner) < sum(away),
            "the corner {corner:?} should be darker than {away:?}"
        );
    }

    #[test]
    fn the_picture_is_deterministic() {
        let tokens = clay();
        let u = uniforms(&paper(), &tokens, 96.0, 64.0, 1.0, 0.0, None);
        let first = render_cpu(&u, 96, 64);
        assert_eq!(first.len(), 96 * 64 * 4);
        assert_eq!(fnv(&first), fnv(&render_cpu(&u, 96, 64)));
    }
}
