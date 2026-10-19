//! Colours: parsing, hex output, compositing, OKLab/OKLCH, mixing and WCAG contrast.
//!
//! Every colour is sRGB with straight alpha, each channel in 0..=1. OKLab is Björn Ottosson's (2020); mixing and
//! lightness steps happen there so a tint keeps its hue. Contrast is WCAG 2.x relative luminance.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An sRGB colour with straight alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

/// A colour in OKLCH: lightness 0..=1, chroma (0..~0.4), hue in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Oklch {
    pub l: f32,
    pub c: f32,
    pub h: f32,
}

impl Rgba {
    pub const WHITE: Rgba = Rgba::rgb(1.0, 1.0, 1.0);
    pub const BLACK: Rgba = Rgba::rgb(0.0, 0.0, 0.0);

    pub const fn rgb(r: f32, g: f32, b: f32) -> Rgba {
        Rgba { r, g, b, a: 1.0 }
    }

    /// From 8-bit channels.
    pub fn rgb8(r: u8, g: u8, b: u8) -> Rgba {
        Rgba::rgb(
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0,
        )
    }

    /// Parses `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb(r g b)`, `rgb(r, g, b)` and `rgba(r, g, b, a)`
    /// (channels 0–255 or percentages, alpha 0–1 or a percentage). Case and surrounding space are ignored.
    pub fn parse(text: &str) -> Option<Rgba> {
        let t = text.trim();
        if let Some(hex) = t.strip_prefix('#') {
            return parse_hex(hex);
        }
        let lower = t.to_ascii_lowercase();
        let inner = lower
            .strip_prefix("rgba(")
            .or_else(|| lower.strip_prefix("rgb("))?
            .strip_suffix(')')?;
        let parts: Vec<&str> = inner
            .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
            .filter(|p| !p.is_empty())
            .collect();
        if parts.len() != 3 && parts.len() != 4 {
            return None;
        }
        // A channel or alpha that is not a finite number (`nan`, `inf`) is refused, so no colour holds a NaN.
        let channel = |p: &str| -> Option<f32> {
            let v = match p.strip_suffix('%') {
                Some(pct) => pct.parse::<f32>().ok()? / 100.0,
                None => p.parse::<f32>().ok()? / 255.0,
            };
            v.is_finite().then(|| v.clamp(0.0, 1.0))
        };
        let alpha = |p: &str| -> Option<f32> {
            let v = match p.strip_suffix('%') {
                Some(pct) => pct.parse::<f32>().ok()? / 100.0,
                None => p.parse::<f32>().ok()?,
            };
            v.is_finite().then(|| v.clamp(0.0, 1.0))
        };
        Some(Rgba {
            r: channel(parts[0])?,
            g: channel(parts[1])?,
            b: channel(parts[2])?,
            a: if parts.len() == 4 {
                alpha(parts[3])?
            } else {
                1.0
            },
        })
    }

    /// `#rrggbb`, or `#rrggbbaa` when not opaque. Lower case.
    pub fn to_hex(self) -> String {
        let [r, g, b, a] = self.to_u8();
        if a == 255 {
            format!("#{r:02x}{g:02x}{b:02x}")
        } else {
            format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
        }
    }

    /// The four channels as bytes (rounded, clamped).
    pub fn to_u8(self) -> [u8; 4] {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        [q(self.r), q(self.g), q(self.b), q(self.a)]
    }

    /// The colour as the bytes would give it back: what a console shows after `to_hex`.
    pub fn quantized(self) -> Rgba {
        let [r, g, b, a] = self.to_u8();
        Rgba {
            a: f32::from(a) / 255.0,
            ..Rgba::rgb8(r, g, b)
        }
    }

    /// The same colour with another alpha.
    pub fn with_alpha(self, a: f32) -> Rgba {
        Rgba { a, ..self }
    }

    /// This colour painted over an opaque `ground` (sRGB "over", as browsers composite).
    pub fn over(self, ground: Rgba) -> Rgba {
        let a = self.a.clamp(0.0, 1.0);
        Rgba::rgb(
            self.r * a + ground.r * (1.0 - a),
            self.g * a + ground.g * (1.0 - a),
            self.b * a + ground.b * (1.0 - a),
        )
    }

    /// WCAG relative luminance of the opaque colour.
    pub fn luminance(self) -> f32 {
        let lin = |c: f32| {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }

    /// WCAG contrast ratio between two opaque colours (1..=21).
    pub fn contrast(self, other: Rgba) -> f32 {
        let (a, b) = (self.luminance(), other.luminance());
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// Whether the colour reads as light (closer to white than black by luminance).
    pub fn is_light(self) -> bool {
        self.contrast(Rgba::BLACK) >= self.contrast(Rgba::WHITE)
    }

    pub fn to_oklab(self) -> [f32; 3] {
        let lin = |c: f32| {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        let (r, g, b) = (lin(self.r), lin(self.g), lin(self.b));
        let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
        let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
        let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
        [
            0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
            1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
            0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
        ]
    }

    /// From OKLab; out-of-gamut channels are left unclamped (see [`in_gamut`]).
    pub fn from_oklab(lab: [f32; 3], a: f32) -> Rgba {
        let [ll, aa, bb] = lab;
        let l = (ll + 0.396_337_78 * aa + 0.215_803_76 * bb).powi(3);
        let m = (ll - 0.105_561_346 * aa - 0.063_854_17 * bb).powi(3);
        let s = (ll - 0.089_484_18 * aa - 1.291_485_5 * bb).powi(3);
        let r = 4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s;
        let g = -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s;
        let b = -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s;
        let enc = |c: f32| {
            if c <= 0.003_130_8 {
                12.92 * c
            } else {
                1.055 * c.max(0.0).powf(1.0 / 2.4) - 0.055
            }
        };
        Rgba {
            r: enc(r),
            g: enc(g),
            b: enc(b),
            a,
        }
    }

    pub fn to_oklch(self) -> Oklch {
        let [l, a, b] = self.to_oklab();
        let c = (a * a + b * b).sqrt();
        let mut h = b.atan2(a).to_degrees();
        if h < 0.0 {
            h += 360.0;
        }
        Oklch { l, c, h }
    }

    /// From OKLCH, reducing chroma until the colour fits sRGB.
    pub fn from_oklch(o: Oklch, alpha: f32) -> Rgba {
        let mut c = o.c.max(0.0);
        loop {
            let rad = o.h.to_radians();
            let col = Rgba::from_oklab([o.l.clamp(0.0, 1.0), c * rad.cos(), c * rad.sin()], alpha);
            if in_gamut(col) || c <= 0.0005 {
                return clamp(col);
            }
            c *= 0.94;
        }
    }

    /// Mixes toward `other` by `t` (0 = self, 1 = other) in OKLab, as CSS `color-mix(in oklab, …)` does.
    pub fn mix(self, other: Rgba, t: f32) -> Rgba {
        let (p, q) = (self.to_oklab(), other.to_oklab());
        let lerp = |a: f32, b: f32| a + (b - a) * t;
        clamp(Rgba::from_oklab(
            [lerp(p[0], q[0]), lerp(p[1], q[1]), lerp(p[2], q[2])],
            lerp(self.a, other.a),
        ))
    }

    /// The colour at another OKLCH lightness, same hue and chroma where sRGB allows.
    pub fn with_lightness(self, l: f32) -> Rgba {
        let o = self.to_oklch();
        Rgba::from_oklch(Oklch { l, ..o }, self.a)
    }

    /// The same lightness and chroma at another hue.
    pub fn with_hue(self, h: f32) -> Rgba {
        let o = self.to_oklch();
        Rgba::from_oklch(Oklch { h, ..o }, self.a)
    }
}

fn parse_hex(hex: &str) -> Option<Rgba> {
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |i: usize, n: usize| u8::from_str_radix(&hex[i..i + n], 16).ok();
    let (r, g, b, a) = match hex.len() {
        3 | 4 => {
            let d = |i| digit(i, 1).map(|v| v * 17);
            (
                d(0)?,
                d(1)?,
                d(2)?,
                if hex.len() == 4 { d(3)? } else { 255 },
            )
        }
        6 | 8 => {
            let d = |i| digit(i, 2);
            (
                d(0)?,
                d(2)?,
                d(4)?,
                if hex.len() == 8 { d(6)? } else { 255 },
            )
        }
        _ => return None,
    };
    Some(Rgba {
        a: f32::from(a) / 255.0,
        ..Rgba::rgb8(r, g, b)
    })
}

/// Whether every channel is within sRGB (with a little slack for float error).
pub fn in_gamut(c: Rgba) -> bool {
    let ok = |v: f32| (-0.0005..=1.0005).contains(&v);
    ok(c.r) && ok(c.g) && ok(c.b)
}

fn clamp(c: Rgba) -> Rgba {
    Rgba {
        r: c.r.clamp(0.0, 1.0),
        g: c.g.clamp(0.0, 1.0),
        b: c.b.clamp(0.0, 1.0),
        a: c.a.clamp(0.0, 1.0),
    }
}

/// The nearest colour to `fg` (by OKLCH lightness, hue kept) that reaches `min` contrast against every ground.
/// `None` when `fg` already passes, or when no colour the search tries is better than `fg`. Grounds are opaque.
/// The search moves away from the grounds' average luminance; if that direction cannot reach `min` it tries the
/// other one, and returns the best it found.
pub fn fix_contrast(fg: Rgba, grounds: &[Rgba], min: f32) -> Option<Rgba> {
    let passes = |c: Rgba| grounds.iter().all(|g| c.quantized().contrast(*g) >= min);
    if grounds.is_empty() || passes(fg) {
        return None;
    }
    let worst = |c: Rgba| {
        grounds
            .iter()
            .map(|g| c.quantized().contrast(*g))
            .fold(f32::INFINITY, f32::min)
    };
    let light_ground =
        grounds.iter().map(|g| g.luminance()).sum::<f32>() / grounds.len() as f32 > 0.18;
    let start = fg.to_oklch();
    let mut best = fg;
    for dir in if light_ground {
        [-1.0f32, 1.0]
    } else {
        [1.0, -1.0]
    } {
        let mut l = start.l;
        for _ in 0..400 {
            l += dir * 0.0025;
            if !(0.0..=1.0).contains(&l) {
                break;
            }
            let c = Rgba::from_oklch(Oklch { l, ..start }, fg.a).quantized();
            if passes(c) {
                return Some(c);
            }
            if worst(c) > worst(best) {
                best = c;
            }
        }
    }
    (best != fg).then_some(best)
}

impl Serialize for Rgba {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Rgba {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Rgba, D::Error> {
        let text = String::deserialize(d)?;
        Rgba::parse(&text).ok_or_else(|| serde::de::Error::custom(format!("not a colour: {text}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints_hex() {
        assert_eq!(Rgba::parse("#F6F4EE").unwrap().to_hex(), "#f6f4ee");
        assert_eq!(Rgba::parse("#abc").unwrap().to_hex(), "#aabbcc");
        assert_eq!(Rgba::parse("#00000080").unwrap().to_hex(), "#00000080");
        assert_eq!(
            Rgba::parse("rgba(40, 30, 15, .5)").unwrap().to_hex(),
            "#281e0f80"
        );
        assert_eq!(Rgba::parse("rgb(255 0 0)").unwrap().to_hex(), "#ff0000");
        assert!(Rgba::parse("#ggg").is_none());
        assert!(Rgba::parse("red").is_none());
    }

    #[test]
    fn oklab_round_trips() {
        for hex in [
            "#f6f4ee", "#d0602c", "#2f5ff0", "#121110", "#5ccf8c", "#000000", "#ffffff",
        ] {
            let c = Rgba::parse(hex).unwrap();
            let back = Rgba::from_oklab(c.to_oklab(), 1.0);
            assert_eq!(back.to_hex(), hex, "{hex}");
            let back = Rgba::from_oklch(c.to_oklch(), 1.0);
            assert_eq!(back.to_hex(), hex, "{hex} via lch");
        }
    }

    #[test]
    fn contrast_matches_wcag() {
        assert!((Rgba::BLACK.contrast(Rgba::WHITE) - 21.0).abs() < 0.01);
        let c = Rgba::parse("#767676").unwrap().contrast(Rgba::WHITE);
        assert!((c - 4.54).abs() < 0.01, "{c}");
    }

    #[test]
    fn fix_reaches_the_target_and_keeps_hue() {
        let fg = Rgba::parse("#6e695e").unwrap();
        let hover = Rgba::parse("#ece8df").unwrap();
        assert!(fg.contrast(hover) < 4.5);
        let fixed = fix_contrast(fg, &[hover], 4.5).unwrap();
        assert!(fixed.contrast(hover) >= 4.5);
        assert!((fixed.to_oklch().h - fg.to_oklch().h).abs() < 6.0);
        assert!(fix_contrast(fixed, &[hover], 4.5).is_none());
    }

    #[test]
    fn alpha_composites_over_the_ground() {
        let half = Rgba::parse("#00000080").unwrap();
        assert_eq!(half.over(Rgba::WHITE).to_hex(), "#7f7f7f");
    }

    #[test]
    fn non_finite_channels_are_refused() {
        assert!(Rgba::parse("rgb(nan 0 0)").is_none());
        assert!(Rgba::parse("rgb(0, inf, 0)").is_none());
        assert!(Rgba::parse("rgba(0, 0, 0, nan)").is_none());
        assert!(Rgba::parse("rgba(0, 0, 0, infinity)").is_none());
        assert!(Rgba::parse("rgb(1e39 0 0)").is_none());
        assert_eq!(Rgba::parse("rgb(255 0 0)").unwrap().to_hex(), "#ff0000");
    }

    #[test]
    fn a_fix_that_improves_nothing_is_not_reported() {
        // White on black is already as far apart as colours get, so no lightness step improves it.
        assert!(fix_contrast(Rgba::WHITE, &[Rgba::BLACK], 30.0).is_none());
    }
}
