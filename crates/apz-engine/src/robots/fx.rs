//! Robot effects, as `robots/fx.ts`: small billboards and glow cones from one pool shared by every robot in the
//! scene (sparks, zzz, dots, hearts, keys, steam, notes, bubbles, confetti, ink, trail, glow). A robot spawns them
//! under a bone; they move, grow and fade, then go back to the pool. The desktop has no sprite, so each billboard is
//! a flat quad with a texture drawn in code, turned to the camera's usual view (the floor's isometric angle); notes,
//! bubbles and confetti wobble, and a confetti flake spins in its own plane (its tumble). A trail is left behind:
//! its puffs are placed in the robot's parent frame, so they stay where the robot was as it walks (`Fx::spawn` with
//! the robot's root and a point in that frame). A glow is a cone that hangs from its bone and is not turned. Not
//! built: a glow's floor spot, since the desktop's Obj has no parent pointer and Fx only sees the head bone, so it
//! cannot find the floor under the robot.

use std::f32::consts::{PI, TAU};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use glam::{Mat4, Quat, Vec3};

use super::super::geometry::plane;
use super::super::scene::{Color, Geo, Geometry, Mat, Material, Obj, Shading, Side, Tex, Texture};
use super::super::textures::Rng;
use super::super::world::layout::{AZIMUTH, POLAR};
use super::files::warn_once;

/// The live effects of the whole scene at once: beyond this, new ones are dropped (the budget).
pub const LIVE_MAX: usize = 60;
/// A glow's colour when the caller gives none: the lamp's warm light.
const WARM: u32 = 0xffc46b;

/// The kinds built now. A kind's index is its pool and texture.
const KINDS: [&str; 12] = ["sparks", "zzz", "dots", "hearts", "keys", "steam", "ink", "glow", "notes", "bubbles", "confetti", "trail"];
const TEXTURE_SIZE: usize = 64;
/// A note's colours, one picked per note (fx.ts's tints), and a confetti flake's, and a trail puff's.
const NOTE_TINTS: [u32; 3] = [0x7a5cff, 0xff7ab6, 0x3fb6a8];
const CONFETTI_TINTS: [u32; 5] = [0xff6b8a, 0xffc23d, 0x5bd1b5, 0x8fb8ff, 0xb08bff];
const TRAIL_TINTS: [u32; 1] = [0x2ee6a6];
/// The six spheres of an ink blot: (centre x, centre y, radius) on the texture, as fx.ts.
const INK: [(f32, f32, f32); 6] = [(32.0, 32.0, 13.0), (22.0, 27.0, 11.0), (42.0, 27.0, 11.0), (21.0, 40.0, 10.0), (43.0, 40.0, 10.0), (32.0, 44.0, 11.0)];
/// The three puffs of a steam cloud: (centre x, centre y, radius) on the texture, as fx.ts.
const STEAM: [(f32, f32, f32); 3] = [(24.0, 38.0, 14.0), (41.0, 32.0, 16.0), (33.0, 44.0, 12.0)];

static LIVE: AtomicUsize = AtomicUsize::new(0);
static POOL: Mutex<Vec<Puff>> = Mutex::new(Vec::new());
static TEXTURES: OnceLock<Vec<Tex>> = OnceLock::new();
static QUAD: OnceLock<Geo> = OnceLock::new();
static CONE: OnceLock<Geo> = OnceLock::new();

/// The opacity over a life, as a fraction of the peak (k in 0..1), as fx.ts FADE.
#[derive(Clone, Copy)]
enum Fade {
    /// 1 − k².
    Squared,
    /// Up quickly, then out: min(1, 6k)(1 − k) (steam).
    Puff,
    /// 1 − k (ink).
    Linear,
    /// sin(πk): up and back down (a glow).
    Arc,
    /// Full to 70% of the life, then out: 1 for k < 0.7, else (1 − k) / 0.3 (confetti).
    Hold,
}

impl Fade {
    fn value(self, k: f32) -> f32 {
        match self {
            Fade::Squared => 1.0 - k * k,
            Fade::Puff => (6.0 * k).min(1.0) * (1.0 - k),
            Fade::Linear => 1.0 - k,
            Fade::Arc => (PI * k).sin(),
            Fade::Hold => {
                if k < 0.7 {
                    1.0
                } else {
                    (1.0 - k) / 0.3
                }
            }
        }
    }
}

/// The look of a kind, as fx.ts LOOK (the desktop does not spin, tint or sway its pieces).
#[derive(Clone, Copy)]
struct Look {
    /// Seconds alive.
    life: f32,
    /// Size in world units (a cone is scaled by this too).
    size: f32,
    /// The scale over the life, as multiples of `size` (a pop eases out).
    grow: [f32; 2],
    /// Units per second upward (negative falls).
    rise: f32,
    /// A random extra rise, units per second (sparks only).
    rise_jitter: f32,
    /// Random sideways speed, units per second.
    spread: f32,
    /// Peak opacity.
    peak: f32,
    fade: Fade,
    /// The scale eases out (fast at first, then it settles).
    pop: bool,
    /// A glow's cone: not turned to the camera, and drawn with its colour.
    cone: bool,
    additive: bool,
    /// Sideways wobble, units (at 5 rad/s, each piece's own amplitude is 0.5–1.5 of this).
    sway: f32,
    /// Spin rate of a flake in its own plane, rad/s (each piece's own rate is 0.5–1.5 of this, in either direction).
    spin: f32,
    /// Random tints, one per piece; empty when the texture keeps its own colour.
    tints: &'static [u32],
    /// A trail puff: its position is in the robot's parent frame, so it stays where it was left.
    stage: bool,
}

fn look_of(kind: usize) -> Look {
    let base = Look {
        life: 0.5,
        size: 0.14,
        grow: [1.0, 1.0],
        rise: 0.0,
        rise_jitter: 0.0,
        spread: 0.0,
        peak: 1.0,
        fade: Fade::Squared,
        pop: false,
        cone: false,
        additive: false,
        sway: 0.0,
        spin: 0.0,
        tints: &[],
        stage: false,
    };
    match KINDS[kind] {
        "sparks" => Look { life: 0.5, size: 0.14, rise: 0.2, rise_jitter: 0.3, spread: 0.5, ..base },
        "zzz" => Look { life: 1.6, size: 0.24, grow: [0.7, 1.2], rise: 0.32, spread: 0.12, ..base },
        "dots" => Look { life: 1.2, size: 0.26, spread: 0.12, ..base },
        "hearts" => Look { life: 1.5, size: 0.2, rise: 0.45, spread: 0.12, ..base },
        "keys" => Look { life: 0.4, size: 0.14, grow: [0.4, 1.1], rise: 0.25, spread: 0.12, pop: true, ..base },
        "steam" => Look { life: 1.2, size: 0.26, grow: [0.6, 1.9], rise: 0.3, spread: 0.08, peak: 0.85, fade: Fade::Puff, ..base },
        "ink" => Look { life: 1.0, size: 0.42, grow: [0.6, 1.3], rise: 0.04, peak: 0.8, fade: Fade::Linear, ..base },
        // The rise is fx.ts's rise × (0.6 + 0.8·r): a base of 0.6 × rise, plus up to 0.8 × rise at random.
        "notes" => Look { life: 1.4, size: 0.2, grow: [1.0, 1.1], rise: 0.24, rise_jitter: 0.32, spread: 0.1, sway: 0.12, tints: &NOTE_TINTS, ..base },
        "bubbles" => Look { life: 1.4, size: 0.13, grow: [1.0, 1.25], rise: 0.18, rise_jitter: 0.24, spread: 0.05, sway: 0.04, ..base },
        "confetti" => Look {
            life: 1.8,
            size: 0.13,
            rise: -0.3,
            rise_jitter: -0.4,
            spread: 0.8,
            sway: 0.03,
            fade: Fade::Hold,
            spin: 6.0,
            tints: &CONFETTI_TINTS,
            ..base
        },
        "trail" => Look { life: 6.0, size: 0.13, tints: &TRAIL_TINTS, stage: true, ..base },
        _ => Look { life: 1.0, size: 1.0, peak: 0.6, fade: Fade::Arc, cone: true, additive: true, ..base },
    }
}

fn kind_index(kind: &str) -> Option<usize> {
    KINDS.iter().position(|k| *k == kind)
}

fn hex(v: u32) -> [f32; 3] {
    [((v >> 16) & 0xff) as f32, ((v >> 8) & 0xff) as f32, (v & 0xff) as f32]
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

/// The distance from `p` to the segment `a`–`b`.
fn segment_distance(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    (p.0 - a.0 - t * dx).hypot(p.1 - a.1 - t * dy)
}

/// Source-over: `src` (colour, coverage) laid over `dst`, both non-premultiplied.
fn over(dst: ([f32; 3], f32), src: ([f32; 3], f32)) -> ([f32; 3], f32) {
    let a = src.1 + dst.1 * (1.0 - src.1);
    if a <= 0.0 {
        return ([0.0; 3], 0.0);
    }
    let c = [0, 1, 2].map(|i| (src.0[i] * src.1 + dst.0[i] * dst.1 * (1.0 - src.1)) / a);
    (c, a)
}

/// Whether (px, py) lies in a rounded rectangle (x, y, w, h, corner radius r).
fn in_rounded(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, r: f32) -> bool {
    let cx = px.clamp(x + r, x + w - r);
    let cy = py.clamp(y + r, y + h - r);
    (px - cx).powi(2) + (py - cy).powi(2) <= r * r
}

/// The parameter t (0..1, clamped) of a two-circle radial gradient at `p`: the circle of centre `f` and radius
/// `r0` grows to centre `c` and radius `r1`; the largest t whose circle passes through `p` (canvas's rule).
fn gradient_t(p: (f32, f32), f: (f32, f32), r0: f32, c: (f32, f32), r1: f32) -> f32 {
    let dot = |a: (f32, f32), b: (f32, f32)| a.0 * b.0 + a.1 * b.1;
    let (cd, pd, dr) = ((c.0 - f.0, c.1 - f.1), (p.0 - f.0, p.1 - f.1), r1 - r0);
    // a t² − 2 b t + cc = 0 for |pd − t cd| = r0 + t dr.
    let a = dot(cd, cd) - dr * dr;
    let b = dot(pd, cd) + r0 * dr;
    let cc = dot(pd, pd) - r0 * r0;
    let disc = b * b - a * cc;
    if disc < 0.0 {
        return 0.0;
    }
    let s = disc.sqrt();
    let best = [(b + s) / a, (b - s) / a].into_iter().filter(|t| r0 + t * dr >= 0.0).fold(f32::MIN, f32::max);
    best.clamp(0.0, 1.0)
}

/// One sphere of an ink blot at `p`: lighter at its top-left, navy-purple, darker at the rim (fx.ts's gradient).
fn ink_shade(p: (f32, f32), cx: f32, cy: f32, r: f32) -> [f32; 3] {
    let t = gradient_t(p, (cx - r * 0.35, cy - r * 0.35), r * 0.1, (cx, cy), r);
    let (c0, c1, c2) = (hex(0x5b4d9c), hex(0x2c2462), hex(0x17123a));
    if t < 0.6 {
        mix(c0, c1, t / 0.6)
    } else {
        mix(c1, c2, (t - 0.6) / 0.4)
    }
}

/// One steam puff at distance `d` from its centre (in radii): light grey-blue, a darker rim, clear at the edge.
fn puff_texel(d: f32) -> ([f32; 3], f32) {
    let (c0, c1, c2) = (hex(0xccd9e7), hex(0xb4c5d8), hex(0x8298b2));
    if d <= 0.7 {
        (mix(c0, c1, d / 0.7), 0.95 + (0.9 - 0.95) * (d / 0.7))
    } else if d <= 0.9 {
        let t = (d - 0.7) / 0.2;
        (mix(c1, c2, t), 0.9 + (0.8 - 0.9) * t)
    } else {
        (c2, 0.8 * (1.0 - (d - 0.9) / 0.1))
    }
}

/// A keycap: a dark ink outline, a cyan cap inset within it, and a small white glint at its top-left.
fn keycap(x: f32, y: f32) -> ([f32; 3], f32) {
    let mut px = ([0.0; 3], 0.0);
    if in_rounded(x, y, 8.0, 10.0, 48.0, 44.0, 10.0) {
        px = over(px, (hex(0x17141f), 1.0));
    }
    if in_rounded(x, y, 12.0, 14.0, 40.0, 36.0, 7.0) {
        px = over(px, (hex(0x33c2ea), 1.0));
    }
    if in_rounded(x, y, 17.0, 19.0, 13.0, 6.0, 3.0) {
        px = over(px, (hex(0xffffff), 0.9));
    }
    px
}

/// One texel of a kind: its colour (sRGB) and coverage (0 to 1). Pixel centres, y down.
fn texel(kind: usize, x: f32, y: f32) -> ([f32; 3], f32) {
    let on = |v: bool| if v { 1.0 } else { 0.0 };
    match KINDS[kind] {
        "sparks" => {
            // A radial glow: white-hot at the centre, amber, fading out at the rim.
            let d = (x - 32.0).hypot(y - 32.0) / 30.0;
            if d < 0.4 {
                (mix(hex(0xfff6c2), hex(0xffc23d), d / 0.4), 1.0)
            } else {
                (hex(0xffc23d), (1.0 - (d - 0.4) / 0.6).max(0.0))
            }
        }
        "zzz" => {
            // A zed: a top bar, a diagonal and a bottom bar.
            let d = [((18.0, 16.0), (46.0, 16.0)), ((46.0, 16.0), (18.0, 48.0)), ((18.0, 48.0), (46.0, 48.0))]
                .iter()
                .map(|(a, b)| segment_distance((x, y), *a, *b))
                .fold(f32::MAX, f32::min);
            (hex(0x8fb8ff), on(d <= 2.5))
        }
        "dots" => (hex(0x5b6475), on([14.0, 32.0, 50.0].iter().any(|cx| (x - cx).hypot(y - 32.0) <= 6.0))),
        "hearts" => {
            // The heart curve (x² + y² − 1)³ − x²y³ ≤ 0, y up.
            let (hx, hy) = ((x - 32.0) / 20.0, (34.0 - y) / 20.0);
            let f = (hx * hx + hy * hy - 1.0).powi(3) - hx * hx * hy.powi(3);
            (hex(0xff6b8a), on(f <= 0.0))
        }
        "keys" => keycap(x, y),
        "steam" => {
            // Three overlapping puffs, each laid over the last.
            let mut px = ([0.0; 3], 0.0);
            for (cx, cy, r) in STEAM {
                let d = (x - cx).hypot(y - cy) / r;
                if d <= 1.0 {
                    px = over(px, puff_texel(d));
                }
            }
            px
        }
        "ink" => {
            // Six opaque spheres; a later one covers an earlier one.
            let mut px = ([0.0; 3], 0.0);
            for (cx, cy, r) in INK {
                if (x - cx).hypot(y - cy) <= r {
                    px = (ink_shade((x, y), cx, cy, r), 1.0);
                }
            }
            px
        }
        "notes" => {
            // A beamed pair of eighth notes, white (the piece's tint colours it): two heads, two stems and a beam.
            let head = |cx: f32, cy: f32| ((x - cx) / 6.5).powi(2) + ((y - cy) / 4.5).powi(2) <= 1.0;
            let stem_l = segment_distance((x, y), (27.5, 46.0), (27.5, 16.0)) <= 1.6;
            let stem_r = segment_distance((x, y), (47.5, 42.0), (47.5, 12.0)) <= 1.6;
            let beam = segment_distance((x, y), (27.5, 16.0), (47.5, 12.0)) <= 2.5;
            ([255.0; 3], on(head(22.0, 46.0) || head(42.0, 42.0) || stem_l || stem_r || beam))
        }
        "bubbles" => {
            // A translucent blue disc with a bright rim and a white glint at its top-left (fx.ts).
            let d = (x - 32.0).hypot(y - 32.0);
            let mut px = ([0.0; 3], 0.0);
            if d <= 22.0 {
                px = over(px, (hex(0xbee6ff), 0.25));
            }
            if (d - 22.0).abs() <= 2.5 {
                px = over(px, (hex(0x9fd8ff), 1.0));
            }
            if (x - 24.0).hypot(y - 24.0) <= 5.0 {
                px = over(px, (hex(0xffffff), 1.0));
            }
            px
        }
        "confetti" => {
            // A white flake, wider than tall; its piece's tint colours it.
            ([255.0; 3], on((10.0..54.0).contains(&x) && (22.0..42.0).contains(&y)))
        }
        "trail" => {
            // A soft dot: a bright core, fading out at the rim (its tint colours it).
            let d = (x - 32.0).hypot(y - 32.0) / 32.0;
            let a = if d <= 0.6 { 1.0 - 0.05 * d / 0.6 } else { (0.95 * (1.0 - (d - 0.6) / 0.4)).max(0.0) };
            ([255.0; 3], a)
        }
        "glow" => {
            // A cone's texture: clear at the apex (the top row, uv v = 1), bright at the base (the bottom row).
            let t = y / TEXTURE_SIZE as f32;
            let a = if t < 0.5 { 0.3 * t / 0.5 } else { 0.3 + 0.6 * (t - 0.5) / 0.5 };
            (hex(0xffffff), a)
        }
        _ => ([0.0; 3], 0.0),
    }
}

/// The texture of each kind, drawn once.
fn textures() -> &'static [Tex] {
    TEXTURES.get_or_init(|| {
        (0..KINDS.len())
            .map(|kind| {
                let n = TEXTURE_SIZE;
                let mut rgba = Vec::with_capacity(n * n * 4);
                for y in 0..n {
                    for x in 0..n {
                        let (rgb, a) = texel(kind, x as f32 + 0.5, y as f32 + 0.5);
                        rgba.extend_from_slice(&[rgb[0] as u8, rgb[1] as u8, rgb[2] as u8, (a * 255.0) as u8]);
                    }
                }
                Arc::new(Texture::new(n as u32, n as u32, rgba))
            })
            .collect()
    })
}

/// The unit quad every billboard shares (its own size is its node's scale), facing +z.
fn quad() -> &'static Geo {
    QUAD.get_or_init(|| plane(1.0, 1.0, 1, 1))
}

/// The glow's cone, shared: three's `ConeGeometry(0.22, 1.1, 14, 1, true)` translated by −0.55 in y, so the apex
/// sits at the origin and the open base at y = −1.1 (no cap). uv v is 1 at the apex and 0 at the base.
fn cone() -> &'static Geo {
    CONE.get_or_init(|| {
        let (r, h, seg) = (0.22_f32, 1.1_f32, 14_usize);
        let slope = r / h;
        let mut positions = Vec::with_capacity(2 * (seg + 1));
        let mut normals = Vec::with_capacity(2 * (seg + 1));
        let mut uvs = Vec::with_capacity(2 * (seg + 1));
        for row in 0..2 {
            let (radius, y, v) = if row == 0 { (0.0, 0.0, 1.0) } else { (r, -h, 0.0) };
            for x in 0..=seg {
                let u = x as f32 / seg as f32;
                let (s, c) = (u * TAU).sin_cos();
                positions.push([radius * s, y, radius * c]);
                normals.push(Vec3::new(s, slope, c).normalize().to_array());
                uvs.push([u, v]);
            }
        }
        let mut indices = Vec::with_capacity(seg * 6);
        for x in 0..seg {
            let (a, b, c, d) = (x as u32, (seg + 1 + x) as u32, (seg + 2 + x) as u32, (x + 1) as u32);
            indices.extend_from_slice(&[a, b, d, b, c, d]);
        }
        Geometry::new(positions, normals, uvs, indices)
    })
}

/// The billboard's orientation: its face turned to the camera's usual view direction (the floor's isometric
/// angle, the eye's direction from the target).
fn facing() -> Quat {
    let (sp, cp) = (POLAR.sin(), POLAR.cos());
    Quat::from_rotation_arc(Vec3::Z, Vec3::new(sp * AZIMUTH.sin(), cp, sp * AZIMUTH.cos()))
}

/// A pooled effect: its quad or cone and its own material (its opacity and colour are its own).
struct Puff {
    kind: usize,
    obj: Obj,
    mat: Mat,
}

fn make(kind: usize) -> Puff {
    let look = look_of(kind);
    let mat = Mat::new(Material {
        shading: Shading::Basic,
        map: Some(Arc::clone(&textures()[kind])),
        transparent: true,
        depth_write: false,
        additive: look.additive,
        side: if look.cone { Side::Double } else { Side::Front },
        ..Material::default()
    });
    let obj = if look.cone {
        Obj::mesh(cone(), &mat)
    } else {
        let obj = Obj::mesh(quad(), &mat);
        obj.set_quaternion(facing());
        obj
    };
    Puff { kind, obj, mat }
}

fn take(kind: usize) -> Puff {
    let pooled = POOL.lock().ok().and_then(|mut pool| {
        let at = pool.iter().rposition(|p| p.kind == kind)?;
        Some(pool.swap_remove(at))
    });
    pooled.unwrap_or_else(|| make(kind))
}

/// A finished effect leaves its parent and goes back to the pool.
fn give_back(parent: &Obj, puff: Puff) {
    parent.remove(&puff.obj);
    LIVE.fetch_sub(1, Ordering::Relaxed);
    if let Ok(mut pool) = POOL.lock() {
        pool.push(puff);
    }
}

struct Item {
    puff: Puff,
    parent: Obj,
    look: Look,
    age: f32,
    vx: f32,
    vy: f32,
    /// The wobble's amplitude and phase, and the flake's spin rate (rad/s), each its own.
    sway: f32,
    phase: f32,
    spin: f32,
    /// Where it was spawned: under the parent, or (a trail) in the parent's parent frame.
    at: Vec3,
}

/// A robot's live effects.
pub struct Fx {
    items: Vec<Item>,
    rng: Rng,
}

impl Fx {
    pub fn new(seed: u32) -> Fx {
        Fx { items: Vec::new(), rng: Rng::new(seed) }
    }

    /// Spawns one effect under `parent` at `at`, in the warm light's colour (a glow's colour only). False when the
    /// kind is not built or the scene is full. A trail is the exception: `parent` is the robot's root and `at` is in
    /// the root's parent frame, where the puff is left.
    pub fn spawn(&mut self, kind: &str, parent: &Obj, at: Vec3) -> bool {
        self.spawn_colored(kind, parent, at, WARM)
    }

    /// As `spawn`, with a glow's colour (hex, sRGB); the other kinds ignore it (a tinted piece takes its own tint).
    pub fn spawn_colored(&mut self, kind: &str, parent: &Obj, at: Vec3, color: u32) -> bool {
        let Some(k) = kind_index(kind) else {
            warn_once(&format!("effect \"{kind}\" is not built yet (a no-op)"));
            return false;
        };
        if LIVE.load(Ordering::Relaxed) >= LIVE_MAX {
            return false;
        }
        let look = look_of(k);
        let puff = take(k);
        let vx = (self.rng.next() - 0.5) * look.spread;
        let mut vy = look.rise;
        if look.rise_jitter != 0.0 {
            vy += look.rise_jitter * self.rng.next();
        }
        let sway = look.sway * (0.5 + self.rng.next());
        let phase = self.rng.next() * TAU;
        let spin = if look.spin > 0.0 { (if self.rng.next() < 0.5 { -1.0 } else { 1.0 }) * look.spin * (0.5 + self.rng.next()) } else { 0.0 };
        // A glow takes the caller's colour; a piece with tints takes one of them at random; the rest keep white.
        let rgb = if look.cone {
            Some(color)
        } else if !look.tints.is_empty() {
            let n = look.tints.len();
            Some(look.tints[((self.rng.next() * n as f32) as usize).min(n - 1)])
        } else {
            None
        };
        puff.mat.get().color = rgb.map_or(Color::WHITE, Color::hex);
        puff.mat.get().opacity = look.peak * look.fade.value(0.0);
        puff.obj.set_position(at.x, at.y, at.z).set_scalar(look.size);
        parent.add(&puff.obj);
        self.items.push(Item { puff, parent: parent.clone(), look, age: 0.0, vx, vy, sway, phase, spin, at });
        LIVE.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// Advances every live effect: it moves, grows (a pop eases out), fades; and goes back to the pool when done.
    /// `frame` is the local matrix of the robot's root, the parent of each piece: a trail's puffs are placed in the
    /// root's parent frame, so they are re-placed here through the root's current transform and stay where they were.
    pub fn update(&mut self, dt: f32, frame: Mat4) {
        let (scale, root_rot, _) = frame.to_scale_rotation_translation();
        let inverse = frame.inverse();
        let face = facing();
        let mut i = 0;
        while i < self.items.len() {
            let it = &mut self.items[i];
            it.age += dt;
            if it.age >= it.look.life {
                let done = self.items.swap_remove(i);
                give_back(&done.parent, done.puff);
                continue;
            }
            let look = it.look;
            let k = it.age / look.life;
            let e = if look.pop { 1.0 - (1.0 - k).powi(3) } else { k };
            let s = look.size * (look.grow[0] + (look.grow[1] - look.grow[0]) * e);
            let obj = &it.puff.obj;
            if look.stage {
                let p = inverse.transform_point3(it.at);
                obj.set_position(p.x, p.y, p.z).set_scalar(s / scale.x.max(1e-6)).set_quaternion(root_rot.inverse() * face);
            } else {
                let wobble = it.sway * (5.0 * it.age + it.phase).sin();
                obj.set_position(it.at.x + it.vx * it.age + wobble, it.at.y + it.vy * it.age, it.at.z).set_scalar(s);
                if !look.cone {
                    let tumble = if it.spin != 0.0 { it.spin * it.age + it.phase } else { 0.0 };
                    obj.set_quaternion(face * Quat::from_rotation_z(tumble));
                }
            }
            it.puff.mat.get().opacity = look.peak * look.fade.value(k);
            i += 1;
        }
    }

    /// Every live effect goes back to the pool (a robot's end, or a change of species).
    pub fn clear(&mut self) {
        for it in self.items.drain(..) {
            give_back(&it.parent, it.puff);
        }
    }
}

impl Drop for Fx {
    fn drop(&mut self) {
        self.clear();
    }
}

/// A seed for one robot's effects (from its id): each robot's sprinkle differs.
pub fn seed(n: u64) -> u32 {
    (n.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pool and the live count are shared, so the tests here run one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The children under `parent` (not counting itself).
    fn children_of(parent: &Obj) -> usize {
        let mut n = 0;
        parent.traverse(&mut |_: &Obj| n += 1);
        n - 1
    }

    fn in_pool(kind: &str) -> bool {
        let k = kind_index(kind).expect("a built kind");
        POOL.lock().unwrap().iter().any(|p| p.kind == k)
    }

    #[test]
    fn each_new_kind_spawns_moves_and_returns_to_the_pool() {
        let _serial = serial();
        for kind in ["notes", "bubbles", "confetti", "trail"] {
            let root = Obj::group();
            let mut fx = Fx::new(11);
            assert!(fx.spawn(kind, &root, Vec3::new(0.0, 0.5, 0.0)), "{kind} spawns");
            assert_eq!(fx.items.len(), 1, "{kind}");
            assert_eq!(children_of(&root), 1, "{kind} is under its parent");
            let start = fx.items[0].puff.obj.position();
            fx.update(0.3, Mat4::IDENTITY);
            let now = fx.items[0].puff.obj.position();
            if kind == "trail" {
                assert_eq!(now, start, "a trail stays where it was left");
            } else {
                assert!(now.distance(start) > 1e-3, "{kind} moves");
            }
            fx.update(6.5, Mat4::IDENTITY); // past every life (a trail's is 6 s)
            assert!(fx.items.is_empty(), "{kind} is done");
            assert_eq!(children_of(&root), 0, "{kind} leaves its parent");
            assert!(in_pool(kind), "{kind} goes back to the pool");
        }
    }

    #[test]
    fn notes_rise_and_confetti_falls() {
        let _serial = serial();
        let root = Obj::group();
        let mut fx = Fx::new(3);
        assert!(fx.spawn("notes", &root, Vec3::ZERO));
        assert!(fx.spawn("confetti", &root, Vec3::ZERO));
        fx.update(0.2, Mat4::IDENTITY);
        assert!(fx.items[0].puff.obj.position().y > 0.0, "a note rises");
        assert!(fx.items[1].puff.obj.position().y < 0.0, "a flake falls");
        fx.clear();
    }

    #[test]
    fn confetti_and_notes_take_one_of_their_tints() {
        let _serial = serial();
        let root = Obj::group();
        let mut fx = Fx::new(9);
        for _ in 0..20 {
            assert!(fx.spawn("confetti", &root, Vec3::ZERO));
        }
        let colors: Vec<Color> = fx.items.iter().map(|it| it.puff.mat.get().color).collect();
        assert!(colors.iter().all(|c| CONFETTI_TINTS.iter().any(|t| *c == Color::hex(*t))));
        assert!(colors.iter().any(|c| *c != colors[0]), "the flakes differ in colour");
        fx.clear();
        assert!(fx.spawn("notes", &root, Vec3::ZERO));
        let note = fx.items[0].puff.mat.get().color;
        assert!(NOTE_TINTS.iter().any(|t| note == Color::hex(*t)));
        fx.clear();
    }

    #[test]
    fn a_trail_puff_stays_where_the_robot_left_it() {
        let _serial = serial();
        let root = Obj::group();
        root.set_position(3.0, 0.0, 0.0).set_scalar(2.0);
        let mut fx = Fx::new(5);
        let left = Vec3::new(1.0, 0.0, 2.0);
        assert!(fx.spawn("trail", &root, left));
        // The robot walks on and turns: the puff stays in the frame it was left in, at its own size.
        root.set_position(-4.0, 0.0, 1.0).set_rotation(0.0, 1.2, 0.0);
        let frame = root.local_matrix();
        fx.update(0.1, frame);
        let puff = &fx.items[0].puff.obj;
        let world = frame.transform_point3(puff.position());
        assert!(world.distance(left) < 1e-4, "the trail is at {world:?}");
        let (world_scale, _, _) = (frame * puff.local_matrix()).to_scale_rotation_translation();
        assert!((world_scale.x - 0.13).abs() < 1e-4, "the trail keeps its size: {}", world_scale.x);
        fx.clear();
    }

    #[test]
    fn the_new_textures_are_drawn() {
        let at = |kind: &str, x: f32, y: f32| texel(kind_index(kind).expect("a built kind"), x, y).1;
        assert_eq!(at("notes", 22.0, 46.0), 1.0, "a note's head");
        assert_eq!(at("notes", 2.0, 2.0), 0.0);
        assert_eq!(at("bubbles", 32.0, 10.0), 1.0, "a bubble's rim");
        assert_eq!(at("bubbles", 2.0, 2.0), 0.0);
        assert_eq!(at("confetti", 32.0, 32.0), 1.0);
        assert_eq!(at("confetti", 2.0, 2.0), 0.0);
        assert_eq!(at("trail", 32.0, 32.0), 1.0, "a trail's core");
        assert_eq!(at("trail", 2.0, 2.0), 0.0);
    }

    #[test]
    fn a_confetti_fade_holds_then_goes_out() {
        assert_eq!(Fade::Hold.value(0.5), 1.0);
        assert!((Fade::Hold.value(0.85) - 0.5).abs() < 1e-5);
    }
}
