//! `floor3d/textures.ts`: the floor's painted textures (meadow, paving, tiles, wood, metal,
//! polish, water) and the seeded random numbers everything procedural uses.

use std::collections::HashMap;
use std::f32::consts::TAU;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use super::canvas::Canvas;
use super::scene::{next_id, Color, Mat, Tex, Texture};

/// mulberry32, as `textures.ts`'s `rng(seed)`: the same seed gives the same sequence as the web.
#[derive(Debug, Clone)]
pub struct Rng(pub u32);

impl Rng {
    pub fn new(seed: u32) -> Rng {
        Rng(seed)
    }
    /// The next number in [0, 1).
    pub fn next(&mut self) -> f32 {
        // `a = (a + 0x6d2b79f5) >>> 0`
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let mut t = self.0;
        // `t = Math.imul(t ^ (t >>> 15), t | 1)`
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        // `t ^= t + Math.imul(t ^ (t >>> 7), t | 61)`
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        // `((t ^ (t >>> 14)) >>> 0) / 4294967296`
        ((t ^ (t >> 14)) as f64 / 4_294_967_296.0) as f32
    }
}

/// `seedOf(key)`: FNV-1a over the key's UTF-16 code units, as `charCodeAt` reads them.
fn seed_of(key: &str) -> u32 {
    let mut h: u32 = 2_166_136_261;
    for c in key.encode_utf16() {
        h = (h ^ c as u32).wrapping_mul(16_777_619);
    }
    h
}

/// JavaScript's `Math.round` (halves go up), for keys built from it.
fn js_round(x: f32) -> f32 {
    (x + 0.5).floor()
}

/// `c.getStyle()`: the colour as a CSS `rgb()` string.
fn css_of(c: Color) -> String {
    let [r, g, b] = c.to_srgb_u8();
    format!("rgb({r},{g},{b})")
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn texture_cache() -> &'static Mutex<HashMap<String, Tex>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Tex>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn material_cache() -> &'static Mutex<HashMap<String, Mat>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Mat>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A repeat-wrapped canvas texture, drawn once per key and cached.
/// `draw` gets the canvas, its size and an `Rng` seeded from the key.
pub fn canvas_texture(key: &str, w: u32, h: u32, repeat: f32, draw: impl FnOnce(&mut Canvas, f32, f32, &mut Rng)) -> Tex {
    let hit = lock(texture_cache()).get(key).cloned();
    if let Some(hit) = hit {
        return hit;
    }
    let mut canvas = Canvas::new(w, h);
    draw(&mut canvas, w as f32, h as f32, &mut Rng::new(seed_of(key)));
    let base = canvas.to_texture();
    // repeat-wrapped, repeat × repeat, sRGB, as the TS sets on the CanvasTexture.
    let tex: Tex = Arc::new(Texture { id: next_id(), repeat: [repeat, repeat], wrap: true, srgb: true, ..(*base).clone() });
    lock(texture_cache()).insert(key.to_string(), tex.clone());
    tex
}

/// Rounded rectangle path.
fn rrect(g: &mut Canvas, x: f32, y: f32, w: f32, h: f32, radius: f32) {
    g.begin_path();
    g.move_to(x + radius, y);
    g.arc_to(x + w, y, x + w, y + h, radius);
    g.arc_to(x + w, y + h, x, y + h, radius);
    g.arc_to(x, y + h, x, y, radius);
    g.arc_to(x, y, x + w, y, radius);
    g.close_path();
}

/// The ground: one smooth sheet under the whole floor, lighter where the plaza stands and fading
/// darker toward the horizon, with a few faint survey rings round the centre. Not repeated: it
/// spans the ground plane once, centred on the plaza.
pub fn ground() -> Tex {
    canvas_texture("ground", 1024, 1024, 1.0, |g, w, h, _| {
        g.set_fill_radial(w / 2.0, h / 2.0, 0.0, w / 2.0, h / 2.0, w / 2.0, &[(0.0, "#3d4680"), (0.12, "#353d72"), (0.35, "#272d58"), (1.0, "#161a36")]);
        g.fill_rect(0.0, 0.0, w, h);
        g.set_stroke("rgba(160,180,255,0.07)");
        g.set_line_width(2.0);
        for r in [44.0_f32, 66.0, 92.0, 124.0] {
            g.begin_path();
            g.arc(w / 2.0, h / 2.0, r, 0.0, TAU, false);
            g.stroke();
        }
    })
}

/// Cobblestones: rounded stones, each with a highlight and an inked edge, set in dark grout.
pub fn paving(tint: Color, repeat: f32) -> Tex {
    canvas_texture(&format!("paving:{}:{}", css_of(tint), repeat), 256, 256, repeat, move |g, w, h, r| {
        let rows: i32 = 5;
        let row_h = h / rows as f32;
        let stone_w = w / 4.0;
        g.set_fill(&css_of(tint.scale(0.55)));
        g.fill_rect(0.0, 0.0, w, h);
        for row in 0..rows {
            let shift = (row % 2) as f32 * (stone_w / 2.0);
            for col in -1..=4 {
                let x = col as f32 * stone_w + shift + 3.0 + r.next() * 2.0;
                let y = row as f32 * row_h + 3.0 + r.next() * 2.0;
                let sw = stone_w - 7.0;
                let sh = row_h - 7.0;
                rrect(g, x, y, sw, sh, 9.0);
                g.set_fill(&css_of(tint.scale(0.92 + r.next() * 0.14)));
                g.fill();
                g.set_line_width(2.0);
                g.set_stroke("rgba(43,36,32,0.55)");
                g.stroke();
                rrect(g, x + 5.0, y + 4.0, sw * 0.45, 5.0, 2.5);
                g.set_fill("rgba(255,255,255,0.45)");
                g.fill();
            }
        }
    })
}

/// A district's floor: big glazed tiles in its role hue, a highlight on each and inked grout.
pub fn tiles(hue: f32, repeat: f32) -> Tex {
    canvas_texture(&format!("tiles:{}:{}", js_round(hue), repeat), 256, 256, repeat, move |g, w, h, r| {
        let grout = Color::hsl(hue / 360.0, 0.28, 0.66);
        let base = Color::hsl(hue / 360.0, 0.5, 0.84);
        g.set_fill(&css_of(grout));
        g.fill_rect(0.0, 0.0, w, h);
        let n: usize = 4;
        let t = w / n as f32;
        for j in 0..n {
            for i in 0..n {
                let (fi, fj) = (i as f32, j as f32);
                let alt = if (i + j) % 2 == 0 { 1.0 } else { 0.9 };
                rrect(g, fi * t + 3.0, fj * t + 3.0, t - 6.0, t - 6.0, 6.0);
                g.set_fill(&css_of(base.scale(alt * (0.97 + r.next() * 0.06))));
                g.fill();
                g.begin_path();
                g.move_to(fi * t + 10.0, fj * t + 14.0);
                g.line_to(fi * t + 22.0, fj * t + 9.0);
                g.set_stroke("rgba(255,255,255,0.6)");
                g.set_line_width(3.0);
                g.set_line_cap("round");
                g.stroke();
            }
        }
    })
}

/// Cartoon planks: warm, flat bands with inked seams and a few knots.
pub fn wood() -> Tex {
    canvas_texture("wood", 256, 256, 1.0, |g, w, h, r| {
        let planks: usize = 4;
        let ph = h / planks as f32;
        for p in 0..planks {
            let top = p as f32 * ph;
            let hue = 28.0 + r.next() * 6.0;
            let sat = 55.0 + r.next() * 10.0;
            let light = 50.0 + r.next() * 8.0;
            g.set_fill(&format!("hsl({hue}, {sat}%, {light}%)"));
            g.fill_rect(0.0, top, w, ph);
            g.set_fill("rgba(255,255,255,0.18)");
            g.fill_rect(0.0, top + 4.0, w, 5.0);
            for _ in 0..2 {
                g.set_stroke("rgba(90,50,20,0.45)");
                g.set_line_width(2.0);
                g.begin_path();
                let x = r.next() * w;
                let rx = 7.0 + r.next() * 5.0;
                g.ellipse(x, top + ph / 2.0, rx, 3.0, 0.0, 0.0, TAU, false);
                g.stroke();
            }
            g.set_fill("rgba(60,32,14,0.8)");
            g.fill_rect(0.0, top, w, 3.0);
        }
    })
}

/// Painted metal: a flat cool grey with one bright stripe.
pub fn metal() -> Tex {
    canvas_texture("metal", 64, 64, 1.0, |g, w, h, _| {
        g.set_fill("#8f9bab");
        g.fill_rect(0.0, 0.0, w, h);
        g.set_fill("rgba(255,255,255,0.35)");
        g.fill_rect(0.0, h * 0.2, w, h * 0.12);
    })
}

/// Polished stone: the tint, a little warmer, with a few soft cartoon sparkles.
pub fn polish(tint: Color) -> Tex {
    canvas_texture(&format!("polish:{}", css_of(tint)), 256, 256, 2.0, move |g, w, h, r| {
        g.set_fill(&css_of(tint));
        g.fill_rect(0.0, 0.0, w, h);
        for _ in 0..14 {
            g.set_fill("rgba(255,255,255,0.35)");
            let x = r.next() * w;
            let y = r.next() * h;
            let sw = 18.0 + r.next() * 30.0;
            rrect(g, x, y, sw, 5.0, 2.5);
            g.fill();
        }
        for _ in 0..10 {
            g.set_fill("rgba(120,100,80,0.12)");
            g.begin_path();
            let x = r.next() * w;
            let y = r.next() * h;
            let rad = 8.0 + r.next() * 14.0;
            g.arc(x, y, rad, 0.0, TAU, false);
            g.fill();
        }
    })
}

/// Water: flat cartoon teal with white wave squiggles. Scrolled over time by the fountain.
pub fn water() -> Tex {
    canvas_texture("water", 256, 256, 2.0, |g, w, h, r| {
        g.set_fill("#3cc0d6");
        g.fill_rect(0.0, 0.0, w, h);
        g.set_fill("#5fd3e3");
        for _ in 0..10 {
            g.begin_path();
            let x = r.next() * w;
            let y = r.next() * h;
            let rx = 30.0 + r.next() * 30.0;
            let ry = 12.0 + r.next() * 10.0;
            g.ellipse(x, y, rx, ry, 0.0, 0.0, TAU, false);
            g.fill();
        }
        g.set_line_cap("round");
        for _ in 0..26 {
            let x = r.next() * w;
            let y = r.next() * h;
            g.set_stroke("rgba(255,255,255,0.85)");
            g.set_line_width(3.0);
            g.begin_path();
            g.move_to(x, y);
            g.quadratic_curve_to(x + 8.0, y - 6.0, x + 16.0, y);
            g.quadratic_curve_to(x + 24.0, y + 6.0, x + 32.0, y);
            g.stroke();
        }
    })
}

/// A slab's sides: a dark hull band in its district hue, under the slab's neon rim.
pub fn slab_side(hue: f32) -> Mat {
    let key = format!("side:{}", js_round(hue));
    let hit = lock(material_cache()).get(&key).cloned();
    if let Some(hit) = hit {
        return hit;
    }
    let m = Mat::toon(Color::hsl(hue / 360.0, 0.3, 0.2));
    lock(material_cache()).insert(key, m.clone());
    m
}

/// The cache `canvas_texture` keeps (exposed for tests).
pub fn cached() -> usize {
    lock(texture_cache()).len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_mulberry32() {
        // Independent reference: the same steps with the 32-bit multiply done in u64.
        fn imul(a: u32, b: u32) -> u32 {
            ((a as u64 * b as u64) & 0xffff_ffff) as u32
        }
        let mut a: u32 = 31;
        let mut want = Vec::new();
        for _ in 0..3 {
            a = a.wrapping_add(0x6d2b_79f5);
            let mut t = a;
            t = imul(t ^ (t >> 15), t | 1);
            t ^= t.wrapping_add(imul(t ^ (t >> 7), t | 61));
            want.push(((t ^ (t >> 14)) as f64 / 4_294_967_296.0) as f32);
        }
        let mut r = Rng::new(31);
        for w in want {
            assert_eq!(r.next(), w);
        }
    }

    #[test]
    fn canvas_texture_same_key_same_arc() {
        let a = canvas_texture("test:same-key", 8, 8, 1.0, |_, _, _, _| {});
        let b = canvas_texture("test:same-key", 8, 8, 1.0, |_, _, _, _| {});
        assert!(std::sync::Arc::ptr_eq(&a, &b));
        assert!(cached() >= 1);
    }

    #[test]
    fn ground_is_a_texture() {
        let t = ground();
        assert_eq!(t.repeat, [1.0, 1.0]);
        assert!(t.wrap);
    }
}
