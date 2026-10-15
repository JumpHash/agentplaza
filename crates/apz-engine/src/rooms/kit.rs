//! `floor3d/rooms/kit.ts`: what every district room is built from — the style type, the
//! cartoon materials (`Kit`), floor textures and wall looks, and `Room`'s ready-made pieces.
//! Local frame: floor y = 0, back wall along z = -s, open front (z = +s) toward the plaza, the
//! middle (|x| < 1.3, |z| < 1.3) kept clear for the consoles.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::sync::{Arc, Mutex, OnceLock};

use super::super::canvas::Canvas;
use super::super::geometry;
use super::super::props::{Anim, Box2};
use super::super::scene::{Color, Geo, Kind, Mat, Material, Obj, Tex};
use super::super::textures::{Rng, canvas_texture};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloorKind {
    Planks,
    Parquet,
    Checker,
    Hazard,
    Slate,
    Carpet,
    Blueprint,
    Stars,
    Marble,
    Gravel,
    Tiles,
    Lawn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallKind {
    Panel,
    Brick,
    Hedge,
    Glass,
    Fence,
    Stone,
    None,
}

/// The colours the kit tints from.
#[derive(Debug, Clone, Copy)]
pub struct KitPalette {
    pub sheet: Color,
    pub rule: Color,
    pub ink3: Color,
    pub signal: Color,
    pub ok: Color,
    pub info: Color,
}

/// A cache of toon materials by key (`cached` in kit.ts).
type Cache = Arc<Mutex<HashMap<String, Mat>>>;

/// Cartoon materials, some tinted with the room's hue (`Kit` in kit.ts).
#[derive(Clone)]
pub struct Kit {
    pub wood: Mat,
    pub dark_wood: Mat,
    pub metal: Mat,
    pub dark: Mat,
    pub white: Mat,
    pub paper: Mat,
    pub clay: Mat,
    pub leaf: Mat,
    pub leaf2: Mat,
    pub crate_: Mat,
    pub gold: Mat,
    pub red: Mat,
    pub blue: Mat,
    pub yellow: Mat,
    pub green: Mat,
    pub purple: Mat,
    pub accent: Mat,
    pub accent_soft: Mat,
    pub signal: Mat,
    pub ok: Mat,
    pub info: Mat,
    /// Monitor faces: lit while an agent of the room works.
    pub screen: Mat,
    /// Unlit glowing bits (basic materials).
    pub glow_ok: Mat,
    pub glow_info: Mat,
    pub glow_warm: Mat,
    pub glow_red: Mat,
    /// The kit's own `mat()` cache.
    cache: Cache,
}

impl Kit {
    /// `k.mat(color)`: any other toon colour, cached per kit.
    pub fn mat(&self, color: Color) -> Mat {
        cached(&self.cache, format!("toon:{}", hex_of(color)), || {
            Mat::toon(color)
        })
    }
}

/// `makeKit(hue, palette)`; hue in degrees.
pub fn make_kit(hue: f32, p: &KitPalette) -> Kit {
    let hsl = |s: f32, l: f32| Color::hsl(hue / 360.0, s, l);
    let mat = shared_toon;
    Kit {
        wood: mat(Color::hex(0xd99a5b)),
        dark_wood: mat(Color::hex(0x8a5634)),
        metal: mat(Color::hex(0x8f9bab)),
        dark: mat(Color::hex(0x46505e)),
        white: mat(Color::hex(0xfbf8f1)),
        paper: mat(p.sheet),
        clay: mat(Color::hex(0xd27f55)),
        leaf: mat(Color::hex(0x5fae4e)),
        leaf2: mat(Color::hex(0x3f8a43)),
        crate_: mat(Color::hex(0xe0b36c)),
        gold: mat(Color::hex(0xf4c542)),
        red: mat(Color::hex(0xe5533d)),
        blue: mat(Color::hex(0x4c86d9)),
        yellow: mat(Color::hex(0xf7d154)),
        green: mat(Color::hex(0x56b870)),
        purple: mat(Color::hex(0x9a72d6)),
        accent: mat(hsl(0.55, 0.55)),
        accent_soft: mat(hsl(0.6, 0.78)),
        signal: mat(p.signal),
        ok: mat(p.ok),
        info: mat(p.info),
        screen: Mat::new(Material {
            color: Color::hex(0x3a4658),
            emissive: p.info,
            emissive_intensity: 0.05,
            ..Material::default()
        }),
        glow_ok: Mat::basic(p.ok),
        glow_info: Mat::basic(p.info),
        glow_warm: Mat::basic(Color::hex(0xffd79a)),
        glow_red: Mat::basic(Color::hex(0xff4a3a)),
        cache: Cache::default(),
    }
}

/// The process-wide cache `makeKit` and `wallLook` share, as kit.ts's module-level `cache`.
fn global_cache() -> &'static Cache {
    static GLOBAL: OnceLock<Cache> = OnceLock::new();
    GLOBAL.get_or_init(Cache::default)
}

fn cached(cache: &Cache, key: String, make: impl FnOnce() -> Mat) -> Mat {
    let mut map = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    map.entry(key).or_insert_with(make).clone()
}

/// `toonMat({ color })`, shared by sRGB hex.
fn shared_toon(color: Color) -> Mat {
    cached(global_cache(), format!("toon:{}", hex_of(color)), || {
        Mat::toon(color)
    })
}

fn hex_of(c: Color) -> String {
    let [r, g, b] = c.to_srgb_u8();
    format!("{r:02x}{g:02x}{b:02x}")
}

/// A canvas colour: `THREE.Color.getStyle()` of an HSL colour (hue in degrees).
fn css_hsl(hue: f32, s: f32, l: f32) -> String {
    let [r, g, b] = Color::hsl(hue / 360.0, s, l).to_srgb_u8();
    format!("rgb({r}, {g}, {b})")
}

fn rrect(g: &mut Canvas, x: f32, y: f32, w: f32, h: f32, r: f32) {
    g.begin_path();
    g.move_to(x + r, y);
    g.arc_to(x + w, y, x + w, y + h, r);
    g.arc_to(x + w, y + h, x, y + h, r);
    g.arc_to(x, y + h, x, y, r);
    g.arc_to(x, y, x + w, y, r);
    g.close_path();
}

// ---- Floors and walls -----------------------------------------------------------------------

/// A room's floor texture; one texture tile covers two world units.
pub fn floor_texture(kind: FloorKind, hue: f32, size: f32) -> Tex {
    let c = |s: f32, l: f32| css_hsl(hue, s, l);
    let key = format!("floor:{kind:?}:{}", hue.round() as i32);
    canvas_texture(&key, 256, 256, 1.0, |g, w, h, r| match kind {
        FloorKind::Planks | FloorKind::Parquet => {
            let planks = kind == FloorKind::Planks;
            let n: usize = if planks { 5 } else { 8 };
            let cols: usize = if planks { 2 } else { 4 };
            let ph = h / n as f32;
            for i in 0..n {
                for j in 0..cols {
                    let pw = w / cols as f32;
                    let off = if planks {
                        (i % 2) as f32 * pw * 0.5
                    } else {
                        0.0
                    };
                    let shift = if off != 0.0 { pw } else { 0.0 };
                    let hue_v = 28.0 + r.next() * 8.0;
                    let sat = 50.0 + r.next() * 12.0;
                    let light = if planks {
                        56.0 + r.next() * 8.0
                    } else {
                        44.0 + r.next() * 10.0
                    };
                    let x0 = j as f32 * pw + off;
                    let y0 = i as f32 * ph;
                    g.set_fill(&format!("hsl({hue_v}, {sat}%, {light}%)"));
                    g.fill_rect(x0 - shift, y0, pw, ph);
                    g.fill_rect(x0, y0, pw, ph);
                    g.set_fill("rgba(70,38,16,0.7)");
                    g.fill_rect(x0, y0, 2.0, ph);
                }
                g.set_fill("rgba(70,38,16,0.75)");
                g.fill_rect(0.0, i as f32 * ph, w, 2.0);
                g.set_fill("rgba(255,255,255,0.16)");
                g.fill_rect(0.0, i as f32 * ph + 4.0, w, 4.0);
            }
        }
        FloorKind::Checker => {
            let n = 4;
            let t = w / n as f32;
            for i in 0..n {
                for j in 0..n {
                    let fill = if (i + j) % 2 == 1 {
                        "#f6f3ec".to_string()
                    } else {
                        c(0.45, 0.62)
                    };
                    g.set_fill(&fill);
                    g.fill_rect(i as f32 * t, j as f32 * t, t, t);
                }
            }
        }
        FloorKind::Hazard => {
            g.set_fill("#9aa3ae");
            g.fill_rect(0.0, 0.0, w, h);
            for i in 0..6 {
                for j in 0..6 {
                    g.set_fill("rgba(60,66,76,0.35)");
                    g.begin_path();
                    g.arc(
                        i as f32 * 44.0 + 20.0,
                        j as f32 * 44.0 + 20.0,
                        4.0,
                        0.0,
                        TAU,
                        false,
                    );
                    g.fill();
                }
            }
            g.set_fill("#f2c230");
            g.fill_rect(0.0, 0.0, w, 26.0);
            g.set_fill("#2b2420");
            let mut x = -26.0_f32;
            while x < w {
                g.begin_path();
                g.move_to(x, 26.0);
                g.line_to(x + 13.0, 0.0);
                g.line_to(x + 26.0, 0.0);
                g.line_to(x + 13.0, 26.0);
                g.fill();
                x += 26.0;
            }
        }
        FloorKind::Slate | FloorKind::Marble => {
            let slate = kind == FloorKind::Slate;
            g.set_fill(if slate { "#4d5866" } else { "#f3efe6" });
            g.fill_rect(0.0, 0.0, w, h);
            let n = 2usize;
            let t = w / n as f32;
            for i in 0..n {
                for j in 0..n {
                    let (fi, fj) = (i as f32, j as f32);
                    rrect(g, fi * t + 3.0, fj * t + 3.0, t - 6.0, t - 6.0, 4.0);
                    let fill = if slate {
                        format!("hsl(214, 14%, {}%)", 30.0 + r.next() * 8.0)
                    } else {
                        format!("hsl(40, 30%, {}%)", 92.0 + r.next() * 4.0)
                    };
                    g.set_fill(&fill);
                    g.fill();
                    g.set_stroke(if slate {
                        "rgba(20,24,30,0.6)"
                    } else {
                        "rgba(150,130,100,0.5)"
                    });
                    g.set_line_width(2.0);
                    g.stroke();
                    for _ in 0..3 {
                        let stroke = if slate {
                            "rgba(255,255,255,0.08)".to_string()
                        } else {
                            c(0.25, 0.75)
                        };
                        g.set_stroke(&stroke);
                        g.set_line_width(1.5);
                        g.begin_path();
                        let (mx, my) = (fi * t + r.next() * t, fj * t + r.next() * t);
                        g.move_to(mx, my);
                        let cx = fi * t + r.next() * t;
                        let cy = fj * t + r.next() * t;
                        let x = fi * t + r.next() * t;
                        let y = fj * t + r.next() * t;
                        g.quadratic_curve_to(cx, cy, x, y);
                        g.stroke();
                    }
                }
            }
        }
        FloorKind::Carpet => {
            g.set_fill(&c(0.45, 0.5));
            g.fill_rect(0.0, 0.0, w, h);
            g.set_stroke(&c(0.5, 0.62));
            g.set_line_width(6.0);
            for i in 0..4 {
                for j in 0..4 {
                    g.begin_path();
                    g.arc(
                        i as f32 * 64.0 + 32.0,
                        j as f32 * 64.0 + 32.0,
                        16.0,
                        0.0,
                        TAU,
                        false,
                    );
                    g.stroke();
                }
            }
        }
        FloorKind::Blueprint => {
            g.set_fill("#2f6db5");
            g.fill_rect(0.0, 0.0, w, h);
            g.set_stroke("rgba(255,255,255,0.25)");
            g.set_line_width(1.0);
            let mut x = 0.0_f32;
            while x <= w {
                g.begin_path();
                g.move_to(x, 0.0);
                g.line_to(x, h);
                g.stroke();
                g.begin_path();
                g.move_to(0.0, x);
                g.line_to(w, x);
                g.stroke();
                x += 16.0;
            }
            g.set_stroke("rgba(255,255,255,0.75)");
            g.set_line_width(3.0);
            g.stroke_rect(40.0, 40.0, 110.0, 70.0);
            g.begin_path();
            g.arc(190.0, 180.0, 36.0, 0.0, TAU, false);
            g.stroke();
            g.begin_path();
            g.move_to(40.0, 200.0);
            g.line_to(130.0, 200.0);
            g.line_to(130.0, 150.0);
            g.stroke();
        }
        FloorKind::Stars => {
            g.set_fill("#26305e");
            g.fill_rect(0.0, 0.0, w, h);
            for _ in 0..70 {
                let fill = if r.next() < 0.2 {
                    "#ffe9a8"
                } else {
                    "rgba(255,255,255,0.85)"
                };
                g.set_fill(fill);
                let x = r.next() * w;
                let y = r.next() * h;
                let s = 1.0 + r.next() * 2.5;
                g.begin_path();
                g.move_to(x, y - s * 2.0);
                g.line_to(x + s * 0.6, y);
                g.line_to(x, y + s * 2.0);
                g.line_to(x - s * 0.6, y);
                g.close_path();
                g.fill();
            }
        }
        FloorKind::Gravel | FloorKind::Lawn => {
            let gravel = kind == FloorKind::Gravel;
            g.set_fill(if gravel { "#c9b99a" } else { "#7fc05a" });
            g.fill_rect(0.0, 0.0, w, h);
            for _ in 0..260 {
                let fill = if gravel {
                    format!("hsla(35, 18%, {}%, 0.9)", 55.0 + r.next() * 30.0)
                } else {
                    let hue_v = 90.0 + r.next() * 20.0;
                    let light = 40.0 + r.next() * 20.0;
                    format!("hsla({hue_v}, 45%, {light}%, 0.6)")
                };
                g.set_fill(&fill);
                g.begin_path();
                let x = r.next() * w;
                let y = r.next() * h;
                let rx = 2.0 + r.next() * 4.0;
                let ry = 1.5 + r.next() * 3.0;
                let rot = r.next() * 3.0;
                g.ellipse(x, y, rx, ry, rot, 0.0, TAU, false);
                g.fill();
            }
        }
        FloorKind::Tiles => {
            g.set_fill(&c(0.28, 0.66));
            g.fill_rect(0.0, 0.0, w, h);
            let n = 4usize;
            let t = w / n as f32;
            for i in 0..n {
                for j in 0..n {
                    rrect(
                        g,
                        i as f32 * t + 3.0,
                        j as f32 * t + 3.0,
                        t - 6.0,
                        t - 6.0,
                        6.0,
                    );
                    g.set_fill(&c(0.5, if (i + j) % 2 == 1 { 0.84 } else { 0.8 }));
                    g.fill();
                }
            }
        }
    })
    .with_repeat(size, size)
}

/// The wall material, height and thickness for a wall kind (`None` for `WallKind::None`).
pub fn wall_look(kind: WallKind, hue: f32) -> Option<(Mat, f32, f32)> {
    let hsl = |s: f32, l: f32| Color::hsl(hue / 360.0, s, l);
    let cache = global_cache();
    let look = match kind {
        WallKind::Panel => (
            cached(cache, format!("wall:panel:{hue}"), || {
                Mat::toon(hsl(0.4, 0.8))
            }),
            1.1,
            0.14,
        ),
        WallKind::Brick => (
            cached(cache, "wall:brick".to_string(), || {
                Mat::toon(Color::hex(0xc8684a))
            }),
            1.0,
            0.2,
        ),
        WallKind::Stone => (
            cached(cache, "wall:stone".to_string(), || {
                Mat::toon(Color::hex(0xd9d1c2))
            }),
            0.9,
            0.22,
        ),
        WallKind::Hedge => (
            cached(cache, "wall:hedge".to_string(), || {
                Mat::toon(Color::hex(0x4f9a45))
            }),
            0.8,
            0.35,
        ),
        WallKind::Glass => (
            cached(cache, "wall:glass".to_string(), || {
                Mat::new(Material {
                    color: Color::hex(0xbfe6f2),
                    transparent: true,
                    opacity: 0.55,
                    ..Material::default()
                })
            }),
            1.2,
            0.06,
        ),
        WallKind::Fence => (
            cached(cache, "wall:fence".to_string(), || {
                Mat::toon(Color::hex(0xf2e6cf))
            }),
            0.6,
            0.08,
        ),
        WallKind::None => return None,
    };
    Some(look)
}

/// A room being built (`Room` in kit.ts). `g` is its group; pieces are added under it.
pub struct Room {
    pub g: Obj,
    pub s: f32,
    pub hue: f32,
    pub k: Kit,
    pub rand: Rng,
    pub anim: Vec<Anim>,
    /// Each item's ground footprint in the room's local frame.
    pub boxes: Vec<Box2>,
}

/// A mesh standing at (x, y, z) under `parent`, receiving shadows (kit.ts `put`). It casts unless it is `thin`
/// (kit.ts `thinPart`: a trim, a decal or a thin rod never casts); an item's own cast gate is applied on top of it
/// by build.rs (`cast_policy`).
fn put(parent: &Obj, geo: &Geo, mat: &Mat, x: f32, y: f32, z: f32, thin: bool) -> Obj {
    let m = Obj::mesh(geo, mat);
    m.set_position(x, y, z);
    m.set_shadows(!thin, true);
    parent.add(&m);
    m
}

impl Room {
    pub fn new(g: Obj, s: f32, hue: f32, k: Kit, rand: Rng) -> Room {
        Room {
            g,
            s,
            hue,
            k,
            rand,
            anim: Vec::new(),
            boxes: Vec::new(),
        }
    }

    /// Records an item's footprint: a `hw` × `hd` rectangle centred on (x, z), turned by `rot_y`.
    fn footprint(&mut self, x: f32, z: f32, rot_y: f32, hw: f32, hd: f32) {
        let (s, c) = rot_y.sin_cos();
        let ax = c.abs() * hw + s.abs() * hd;
        let az = s.abs() * hw + c.abs() * hd;
        self.boxes.push(Box2 {
            x0: x - ax,
            x1: x + ax,
            z0: z - az,
            z1: z + az,
        });
    }

    /// `item(x, z, rotY)`: a furniture group at (x, z), with its own footprint.
    pub fn item(&mut self, x: f32, z: f32, rot_y: f32) -> Obj {
        let it = Obj::group();
        it.set_position(x, 0.0, z);
        it.set_rotation(0.0, rot_y, 0.0);
        self.g.add(&it);
        it
    }

    /// `box(parent, w, h, d, mat, x, y, z)`: a box standing on y (its base at y).
    #[allow(clippy::too_many_arguments)]
    pub fn box_(
        &mut self,
        parent: &Obj,
        w: f32,
        h: f32,
        d: f32,
        mat: &Mat,
        x: f32,
        y: f32,
        z: f32,
    ) -> Obj {
        // thinPart: a box thinner than 0.02 in any side never casts.
        let thin = w.min(h).min(d) < 0.02;
        put(
            parent,
            &geometry::box_geo(w, h, d),
            mat,
            x,
            y + h / 2.0,
            z,
            thin,
        )
    }

    /// `cyl(parent, rTop, rBottom, h, mat, x, y, z, seg)`: a cylinder standing on y.
    #[allow(clippy::too_many_arguments)]
    pub fn cyl(
        &mut self,
        parent: &Obj,
        r_top: f32,
        r_bottom: f32,
        h: f32,
        mat: &Mat,
        x: f32,
        y: f32,
        z: f32,
        seg: u32,
    ) -> Obj {
        // thinPart: a cylinder of radius under 0.05 never casts (the rods and the posts).
        let thin = r_top.max(r_bottom) < 0.05;
        put(
            parent,
            &geometry::cylinder(r_top, r_bottom, h, seg),
            mat,
            x,
            y + h / 2.0,
            z,
            thin,
        )
    }

    /// `ball(parent, r, mat, x, y, z)`: a sphere centred at (x, y, z).
    pub fn ball(&mut self, parent: &Obj, r: f32, mat: &Mat, x: f32, y: f32, z: f32) -> Obj {
        // thinPart: a sphere under 0.02 across never casts.
        let thin = 2.0 * r < 0.02;
        put(parent, &geometry::sphere(r, 16, 12), mat, x, y, z, thin)
    }

    /// `cone(parent, r, h, mat, x, y, z, seg)`: a cone standing on y. Never thin: three's ConeGeometry has no
    /// radiusTop, so kit.ts's cylinder rule never marks it (its `thinPart` is false).
    #[allow(clippy::too_many_arguments)]
    pub fn cone(
        &mut self,
        parent: &Obj,
        r: f32,
        h: f32,
        mat: &Mat,
        x: f32,
        y: f32,
        z: f32,
        seg: u32,
    ) -> Obj {
        put(
            parent,
            &geometry::cone(r, h, seg),
            mat,
            x,
            y + h / 2.0,
            z,
            false,
        )
    }

    /// `rug(w, d, mat, x, z, round)`: a flat rug or floor marking; walkable, no footprint.
    pub fn rug(&mut self, w: f32, d: f32, mat: &Mat, x: f32, z: f32, round: bool) -> Obj {
        let geo = if round {
            geometry::circle(w / 2.0, 40, 0.0, TAU)
        } else {
            geometry::plane(w, d, 1, 1)
        };
        let m = Obj::mesh(&geo, mat);
        m.set_rotation(-FRAC_PI_2, 0.0, 0.0);
        m.set_position(x, 0.012, z);
        m.set_shadows(false, true);
        m.no_outline();
        m.set_walk();
        self.g.add(&m);
        m
    }

    /// `desk(x, z, rotY)`: a desk with a monitor (it lights up while the room's agents work)
    /// and a chair.
    pub fn desk(&mut self, x: f32, z: f32, rot_y: f32) -> Obj {
        let it = self.item(x, z, rot_y);
        let k = self.k.clone();
        self.box_(&it, 1.0, 0.06, 0.55, &k.wood, 0.0, 0.5, 0.0);
        self.box_(&it, 0.08, 0.5, 0.5, &k.wood, -0.42, 0.0, 0.0);
        self.box_(&it, 0.08, 0.5, 0.5, &k.wood, 0.42, 0.0, 0.0);
        self.box_(&it, 0.3, 0.02, 0.18, &k.metal, 0.0, 0.56, -0.12);
        self.box_(&it, 0.07, 0.18, 0.05, &k.metal, 0.0, 0.58, -0.12);
        // The monitor lights up while agents work (its emissive is animated).
        self.box_(&it, 0.62, 0.38, 0.04, &k.screen, 0.0, 0.74, -0.14)
            .set_dynamic(true);
        self.chair(0.0, 0.55, PI, Some(&it));
        self.footprint(x, z, rot_y, 0.5, 0.275);
        it
    }

    /// `chair(x, z, rotY, parent?)`: a chair, its seat facing +z. With a `parent` it becomes
    /// part of that item (no footprint of its own).
    pub fn chair(&mut self, x: f32, z: f32, rot_y: f32, parent: Option<&Obj>) -> Obj {
        let it = match parent {
            Some(p) => {
                let it = Obj::group();
                it.set_position(x, 0.0, z);
                it.set_rotation(0.0, rot_y, 0.0);
                p.add(&it);
                it
            }
            None => self.item(x, z, rot_y),
        };
        let k = self.k.clone();
        self.box_(&it, 0.42, 0.06, 0.42, &k.accent, 0.0, 0.36, 0.0);
        self.box_(&it, 0.42, 0.42, 0.06, &k.accent, 0.0, 0.42, -0.18);
        for (cx, cz) in [(-0.17, -0.17), (0.17, -0.17), (-0.17, 0.17), (0.17, 0.17)] {
            self.cyl(&it, 0.025, 0.025, 0.36, &k.metal, cx, 0.0, cz, 6);
        }
        if parent.is_none() {
            self.footprint(x, z, rot_y, 0.21, 0.21);
        }
        it
    }

    /// `shelf(x, z, rotY, w, h)`: a bookshelf against a wall, its books facing +z.
    pub fn shelf(&mut self, x: f32, z: f32, rot_y: f32, w: f32, h: f32) -> Obj {
        let it = self.item(x, z, rot_y);
        let k = self.k.clone();
        let spines = [
            k.red.clone(),
            k.blue.clone(),
            k.yellow.clone(),
            k.green.clone(),
            k.purple.clone(),
        ];
        self.box_(&it, w, h, 0.4, &k.dark_wood, 0.0, 0.0, 0.0);
        let rows = (h / 0.42).round().max(2.0) as u32;
        for row in 0..rows {
            let y = 0.1 + (row as f32 * (h - 0.2)) / rows as f32;
            let mut bx = -w / 2.0 + 0.1;
            while bx < w / 2.0 - 0.14 {
                let bw = 0.06 + self.rand.next() * 0.06;
                let bh = 0.2 + self.rand.next() * 0.1;
                let idx = (self.rand.next() * spines.len() as f32).floor() as usize;
                self.box_(&it, bw, bh, 0.26, &spines[idx], bx + bw / 2.0, y, 0.04);
                bx += bw + 0.012;
            }
        }
        self.footprint(x, z, rot_y, w / 2.0, 0.22);
        it
    }

    /// `plant(x, z, scale)`: a potted plant.
    pub fn plant(&mut self, x: f32, z: f32, scale: f32) -> Obj {
        let it = self.item(x, z, 0.0);
        let k = self.k.clone();
        self.cyl(
            &it,
            0.22 * scale,
            0.16 * scale,
            0.3 * scale,
            &k.clay,
            0.0,
            0.0,
            0.0,
            14,
        );
        self.ball(&it, 0.34 * scale, &k.leaf, 0.0, 0.5 * scale, 0.0);
        self.cone(
            &it,
            0.22 * scale,
            0.45 * scale,
            &k.leaf2,
            0.0,
            0.62 * scale,
            0.0,
            10,
        );
        self.footprint(x, z, 0.0, 0.22 * scale, 0.22 * scale);
        it
    }

    /// `crate(x, z, size, rotY)`: a wooden crate, `size` on a side.
    pub fn crate_(&mut self, x: f32, z: f32, size: f32, rot_y: f32) -> Obj {
        let it = self.item(x, z, rot_y);
        let k = self.k.clone();
        self.box_(&it, size, size, size, &k.crate_, 0.0, 0.0, 0.0);
        self.box_(
            &it,
            size * 1.02,
            size * 0.12,
            size * 1.02,
            &k.dark_wood,
            0.0,
            size * 0.44,
            0.0,
        );
        self.footprint(x, z, rot_y, size / 2.0, size / 2.0);
        it
    }

    /// `lamp(x, z)`: a floor lamp with a warm, gently flickering light.
    pub fn lamp(&mut self, x: f32, z: f32) -> Obj {
        let it = self.item(x, z, 0.0);
        let k = self.k.clone();
        self.cyl(&it, 0.035, 0.05, 1.7, &k.metal, 0.0, 0.0, 0.0, 8);
        self.cone(&it, 0.24, 0.26, &k.accent_soft, 0.0, 1.62, 0.0, 16);
        self.ball(&it, 0.1, &k.glow_warm, 0.0, 1.66, 0.0);
        let light = Obj::point_light(Color::hex(0xffd79a), 0.5, 5.0, 2.0);
        light.set_position(0.0, 1.55, 0.0);
        it.add(&light);
        let phase = self.rand.next() * 10.0;
        self.anim.push(Box::new(move |t: f32| {
            let mut node = light.get();
            if let Kind::PointLight { intensity, .. } = &mut node.kind {
                *intensity = 0.5 * (1.0 + 0.04 * (t * 7.1 + phase).sin());
            }
        }));
        self.footprint(x, z, 0.0, 0.05, 0.05);
        it
    }

    // ---- Builders for the room generator (rooms/gen.ts). Each item's origin is its floor centre (y = 0);
    // a builder keeps the old pieces above as they are. No footprint: the web's new builders record none.

    /// `deskTop(x, z, rotY)`: a desk's table only, top at y = 0.56, footprint 1.0 x 0.55 (no chair, no monitor).
    pub fn desk_top(&mut self, x: f32, z: f32, rot_y: f32) -> Obj {
        let it = self.item(x, z, rot_y);
        let k = self.k.clone();
        self.box_(&it, 1.0, 0.06, 0.55, &k.wood, 0.0, 0.5, 0.0);
        self.box_(&it, 0.08, 0.5, 0.5, &k.wood, -0.42, 0.0, 0.0);
        self.box_(&it, 0.08, 0.5, 0.5, &k.wood, 0.42, 0.0, 0.0);
        it
    }

    /// `monitor(x, z, rotY, y)`: a monitor standing on a desk. Its origin is the desk top (`y` is the host's top);
    /// the screen lights up while agents work (dynamic, so it is never baked). Footprint 0.62 x 0.18, height 0.56.
    pub fn monitor(&mut self, x: f32, z: f32, rot_y: f32, y: f32) -> Obj {
        let it = self.item(x, z, rot_y);
        it.set_position(x, y, z);
        let k = self.k.clone();
        self.box_(&it, 0.3, 0.02, 0.18, &k.metal, 0.0, 0.0, 0.0);
        self.box_(&it, 0.07, 0.18, 0.05, &k.metal, 0.0, 0.02, 0.0);
        self.box_(&it, 0.62, 0.38, 0.04, &k.screen, 0.0, 0.18, -0.02)
            .set_dynamic(true);
        it
    }

    /// `poster(x, z, rotY)`: a framed picture on a wall, 0.5 wide, 0.6 high, 0.1 deep; its bottom is the group's y = 0.
    pub fn poster(&mut self, x: f32, z: f32, rot_y: f32) -> Obj {
        let it = self.item(x, z, rot_y);
        let k = self.k.clone();
        self.box_(&it, 0.5, 0.04, 0.1, &k.dark_wood, 0.0, 0.0, 0.0);
        self.box_(&it, 0.5, 0.04, 0.1, &k.dark_wood, 0.0, 0.56, 0.0);
        self.box_(&it, 0.04, 0.52, 0.1, &k.dark_wood, -0.23, 0.04, 0.0);
        self.box_(&it, 0.04, 0.52, 0.1, &k.dark_wood, 0.23, 0.04, 0.0);
        self.box_(&it, 0.42, 0.52, 0.02, &k.paper, 0.0, 0.04, 0.0);
        self.box_(&it, 0.2, 0.24, 0.01, &k.accent, -0.07, 0.2, 0.015);
        let sun = self.cyl(&it, 0.07, 0.07, 0.01, &k.signal, 0.0, 0.0, 0.0, 16);
        sun.set_rotation(FRAC_PI_2, 0.0, 0.0)
            .set_position(0.1, 0.38, 0.015);
        self.box_(&it, 0.3, 0.03, 0.01, &k.dark, 0.0, 0.14, 0.015);
        self.box_(&it, 0.2, 0.03, 0.01, &k.dark, -0.05, 0.08, 0.015);
        it
    }

    /// `clock(x, z, rotY)`: a round wall clock, a face 0.36 across and 0.08 deep; its bottom is the group's y = 0.
    pub fn clock(&mut self, x: f32, z: f32, rot_y: f32) -> Obj {
        let it = self.item(x, z, rot_y);
        let k = self.k.clone();
        let rim = self.cyl(&it, 0.18, 0.18, 0.08, &k.wood, 0.0, 0.0, 0.0, 24);
        rim.set_rotation(FRAC_PI_2, 0.0, 0.0)
            .set_position(0.0, 0.18, 0.0);
        let face = self.cyl(&it, 0.14, 0.14, 0.012, &k.white, 0.0, 0.0, 0.0, 24);
        face.set_rotation(FRAC_PI_2, 0.0, 0.0)
            .set_position(0.0, 0.18, 0.035);
        self.box_(&it, 0.012, 0.13, 0.003, &k.dark, 0.0, 0.18, 0.0425);
        self.box_(&it, 0.09, 0.012, 0.003, &k.dark, 0.045, 0.174, 0.0425);
        it
    }

    /// `sofa(x, z, rotY)`: a two-seat sofa, 1.6 wide, 0.8 high, 0.7 deep, its seat facing +z; centred on the origin.
    pub fn sofa(&mut self, x: f32, z: f32, rot_y: f32) -> Obj {
        let it = self.item(x, z, rot_y);
        let k = self.k.clone();
        self.box_(&it, 1.2, 0.22, 0.7, &k.accent, 0.0, 0.0, 0.0);
        self.box_(&it, 1.6, 0.58, 0.18, &k.accent, 0.0, 0.22, -0.26);
        for sx in [-0.71, 0.71] {
            self.box_(&it, 0.18, 0.44, 0.52, &k.accent, sx, 0.0, 0.09);
        }
        for sx in [-0.31, 0.31] {
            self.box_(&it, 0.6, 0.14, 0.52, &k.accent_soft, sx, 0.22, 0.09);
        }
        it
    }

    /// `bin(x, z)`: a waste bin, 0.3 x 0.6 x 0.3, with a lid-coloured band on top.
    pub fn bin(&mut self, x: f32, z: f32) -> Obj {
        let it = self.item(x, z, 0.0);
        let k = self.k.clone();
        self.box_(&it, 0.3, 0.54, 0.3, &k.metal, 0.0, 0.0, 0.0);
        self.box_(&it, 0.3, 0.06, 0.3, &k.accent, 0.0, 0.54, 0.0);
        it
    }

    /// `coatStand(x, z)`: a round foot 0.4 across, a pole and three hooks; 1.8 high.
    pub fn coat_stand(&mut self, x: f32, z: f32) -> Obj {
        let it = self.item(x, z, 0.0);
        let k = self.k.clone();
        self.cyl(&it, 0.2, 0.2, 0.04, &k.dark, 0.0, 0.0, 0.0, 20);
        self.cyl(&it, 0.025, 0.025, 1.68, &k.dark_wood, 0.0, 0.04, 0.0, 8);
        self.ball(&it, 0.06, &k.dark, 0.0, 1.74, 0.0);
        for i in 0..3 {
            let a = i as f32 * TAU / 3.0;
            let hook = self.box_(&it, 0.18, 0.03, 0.03, &k.dark_wood, 0.0, 0.0, 0.0);
            hook.set_position(a.sin() * 0.09, 1.5, a.cos() * 0.09)
                .set_rotation(0.0, a - FRAC_PI_2, 0.0);
        }
        it
    }

    /// `safe(x, z, rotY)`: a wall safe, dark metal 0.7 x 0.9 x 0.6, a gold dial and a handle on the front (+z).
    pub fn safe(&mut self, x: f32, z: f32, rot_y: f32) -> Obj {
        let it = self.item(x, z, rot_y);
        let k = self.k.clone();
        self.box_(&it, 0.7, 0.9, 0.58, &k.dark, 0.0, 0.0, 0.0);
        let dial = self.cyl(&it, 0.1, 0.1, 0.02, &k.gold, 0.0, 0.0, 0.0, 20);
        dial.set_rotation(FRAC_PI_2, 0.0, 0.0)
            .set_position(0.0, 0.5, 0.3);
        self.box_(&it, 0.04, 0.12, 0.03, &k.metal, 0.22, 0.4, 0.29);
        it
    }

    /// `smallCrate(x, z, rotY)`: the room's 0.5 cube crate.
    pub fn small_crate(&mut self, x: f32, z: f32, rot_y: f32) -> Obj {
        self.crate_(x, z, 0.5, rot_y)
    }

    /// `propPlaceholder(x, z, rotY, w, h, d, label)`: stands in for a station prop with no file (or an unknown
    /// model): a plain box of its footprint and height, in a neutral colour.
    #[allow(clippy::too_many_arguments)]
    pub fn prop_placeholder(
        &mut self,
        x: f32,
        z: f32,
        rot_y: f32,
        w: f32,
        h: f32,
        d: f32,
        label: &str,
    ) -> Obj {
        let it = self.item(x, z, rot_y);
        it.set_name(&format!("prop:{label}"));
        let neutral = self.k.mat(Color::hex(0xb9bfc9));
        self.box_(&it, w, h, d, &neutral, 0.0, 0.0, 0.0);
        it
    }
}
