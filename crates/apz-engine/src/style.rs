//! The machines' styles: how the kit's materials, its feature edges and the world around them look. Three styles, each
//! with a light and a dark variant (the console's theme):
//!
//! - [`Style::Ink`] (A, ink and paper): toon-shaded paper and grey bodies, ink edges, one orange for work. The console's
//!   own register; the default.
//! - [`Style::Blueprint`] (C): flat navy faces that hide what is behind them, pale cyan lines, status in amber, green and
//!   red. A technical drawing.
//! - [`Style::Night`] (D, night ops): dark toon bodies, cyan lit edges, strong emissive status lights.

use crate::scene::{Color, Material, Shading};

/// One of the machines' styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Style {
    #[default]
    Ink,
    Blueprint,
    Night,
}

impl Style {
    /// The style a host names: `ink`, `blueprint` or `night` (also the letters `a`, `c` and `d`), any case.
    pub fn parse(name: &str) -> Option<Style> {
        match name.trim().to_ascii_lowercase().as_str() {
            "ink" | "a" => Some(Style::Ink),
            "blueprint" | "c" => Some(Style::Blueprint),
            "night" | "d" => Some(Style::Night),
            _ => None,
        }
    }

    /// The style's name: `ink`, `blueprint` or `night`.
    pub fn name(self) -> &'static str {
        match self {
            Style::Ink => "ink",
            Style::Blueprint => "blueprint",
            Style::Night => "night",
        }
    }
}

/// A machine's state as its status light shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lamp {
    Working,
    Idle,
    Strained,
    Offline,
}

/// What a style looks like in one theme: the kit's materials, the edges, the status lights, and the world's colours.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub style: Style,
    pub dark: bool,
    /// The clear colour behind everything (the sky is not drawn in a style).
    pub background: Color,
    /// The ground plane's colour, and the plinths'.
    pub ground: Color,
    pub plinth: Color,
    /// The feature edges' colour and opacity; None: no edges drawn.
    pub edges: Option<(Color, f32)>,
    /// The status lights: working, idle, strained, offline (in [`Lamp`] order).
    pub lamps: [Color; 4],
    /// The load bars on the plinths: CPU and memory.
    pub cpu: Color,
    pub memory: Color,
    /// Faces are flat (unlit `Shading::Basic`) rather than toon-shaded.
    pub flat: bool,
    /// The kit's textures are drawn (the grille's perforation); off, its materials are plain colour.
    pub textured: bool,
}

impl Palette {
    /// The palette of `style` in the dark (`dark`) or light console theme. Blueprint and Night look the same in both.
    pub fn of(style: Style, dark: bool) -> Palette {
        let c = Color::hex;
        match (style, dark) {
            (Style::Ink, false) => Palette {
                style,
                dark,
                background: c(0xe9ece5),
                ground: c(0xf4f6f1),
                plinth: c(0xf4f6f1),
                edges: Some((c(0x151916), 1.0)),
                lamps: [c(0xe8590c), c(0x2e6a40), c(0x85640e), c(0xa3182b)],
                cpu: c(0xe8590c),
                memory: c(0x4c544d),
                flat: false,
                textured: true,
            },
            (Style::Ink, true) => Palette {
                style,
                dark,
                background: c(0x0a0e19),
                ground: c(0x111726),
                plinth: c(0x1a2236),
                edges: Some((c(0xe3e8f4), 0.9)),
                lamps: [c(0xff7a2e), c(0x5fe0a0), c(0xe2bd5a), c(0xff7c86)],
                cpu: c(0xff7a2e),
                memory: c(0x6cc8ff),
                flat: false,
                textured: true,
            },
            (Style::Blueprint, _) => Palette {
                style,
                dark,
                background: c(0x0b2a4a),
                ground: c(0x0b2a4a),
                plinth: c(0x0e3358),
                edges: Some((c(0xcfeaff), 1.0)),
                lamps: [c(0xffd166), c(0x6be3a4), c(0xff9f43), c(0xff6b7a)],
                cpu: c(0xffd166),
                memory: c(0x7fb2d9),
                flat: true,
                textured: false,
            },
            (Style::Night, _) => Palette {
                style,
                dark,
                background: c(0x05070d),
                ground: c(0x0a1020),
                plinth: c(0x0a1020),
                edges: Some((c(0x38e1ff), 0.45)),
                lamps: [c(0xff7a2e), c(0x2ee6c5), c(0xffd23f), c(0xff4d6d)],
                cpu: c(0xff7a2e),
                memory: c(0x38e1ff),
                flat: false,
                textured: false,
            },
        }
    }

    /// The colour of a lamp state.
    pub fn lamp(&self, lamp: Lamp) -> Color {
        self.lamps[lamp as usize]
    }

    /// The material of the kit material `name` (see `tools/apz-kit`: graphite, graphite_dark, aluminium,
    /// aluminium_dark, polymer, bezel, bay, grille, interior, shroud, blade, glass, rubber, status, led, signal, screen,
    /// plate) in this palette. Unknown names take polymer's. `status` lights in the working colour (the fleet recolours
    /// it per machine), `led` in the idle colour, `signal` in the working colour, all emissive; `glass` is transparent
    /// (opacity 0.22, no depth write) in the toon styles and invisible in Blueprint. Shading is `Toon`, or `Basic` when
    /// the palette is flat.
    pub fn material(&self, name: &str) -> Material {
        let shading = if self.flat {
            Shading::Basic
        } else {
            Shading::Toon
        };
        let emissive_intensity = if self.style == Style::Night { 2.0 } else { 1.0 };
        // Flat shading draws no emissive: a flat palette's lights take their colour as the base colour too.
        let lit = |lamp: Lamp| Material {
            shading,
            color: if self.flat {
                self.lamp(lamp)
            } else {
                Color::BLACK
            },
            emissive: self.lamp(lamp),
            emissive_intensity,
            ..Material::default()
        };
        match name {
            "status" => lit(Lamp::Working),
            "led" => lit(Lamp::Idle),
            "signal" => lit(Lamp::Working),
            "glass" => match self.style {
                Style::Blueprint => Material {
                    shading: Shading::Basic,
                    color: Color::hex(0x0e3358),
                    visible: false,
                    ..Material::default()
                },
                _ => Material {
                    shading: Shading::Toon,
                    color: Color::hex(glass_hex(self.style, self.dark)),
                    transparent: true,
                    opacity: 0.22,
                    depth_write: false,
                    ..Material::default()
                },
            },
            _ => Material {
                shading,
                color: Color::hex(face_hex(self.style, self.dark, name)),
                ..Material::default()
            },
        }
    }

    /// The feature edges' material: `Shading::Line` in the edge colour, transparent when its opacity is below 1; None
    /// when the palette draws no edges.
    pub fn edge_material(&self) -> Option<Material> {
        self.edges.map(|(color, opacity)| Material {
            shading: Shading::Line,
            color,
            transparent: opacity < 1.0,
            opacity,
            depth_write: false,
            ..Material::default()
        })
    }
}

/// The face materials of the ink light theme (name, sRGB hex).
const INK_LIGHT: [(&str, u32); 14] = [
    ("graphite", 0xd5dad1),
    ("graphite_dark", 0xc2c8bd),
    ("aluminium", 0xf4f6f1),
    ("aluminium_dark", 0xb9bfb4),
    ("polymer", 0x7d867e),
    ("bezel", 0x9aa197),
    ("bay", 0xe2e6dd),
    ("grille", 0xc7cfc3),
    ("interior", 0x4c544d),
    ("shroud", 0xdbe1d7),
    ("blade", 0x7d867e),
    ("rubber", 0x4c544d),
    ("screen", 0xf4f6f1),
    ("plate", 0xf4f6f1),
];

/// The face materials of the ink dark theme.
const INK_DARK: [(&str, u32); 14] = [
    ("graphite", 0x3b4867),
    ("graphite_dark", 0x2b3550),
    ("aluminium", 0xd9dee6),
    ("aluminium_dark", 0x9aa2b0),
    ("polymer", 0x1c2029),
    ("bezel", 0x12151d),
    ("bay", 0x252b3a),
    ("grille", 0x1a1f2b),
    ("interior", 0x0a0d14),
    ("shroud", 0x4a5572),
    ("blade", 0x2a303d),
    ("rubber", 0x0d0f14),
    ("screen", 0x0b1020),
    ("plate", 0x0c0f16),
];

/// The face materials of Blueprint: flat navy, the screen and plate a shade deeper.
const BLUEPRINT: [(&str, u32); 14] = [
    ("graphite", 0x0e3358),
    ("graphite_dark", 0x0e3358),
    ("aluminium", 0x0e3358),
    ("aluminium_dark", 0x0e3358),
    ("polymer", 0x0e3358),
    ("bezel", 0x0e3358),
    ("bay", 0x0e3358),
    ("grille", 0x0e3358),
    ("interior", 0x0e3358),
    ("shroud", 0x0e3358),
    ("blade", 0x0e3358),
    ("rubber", 0x0e3358),
    ("screen", 0x0b2a4a),
    ("plate", 0x0b2a4a),
];

/// The face materials of Night.
const NIGHT: [(&str, u32); 14] = [
    ("graphite", 0x1e2539),
    ("graphite_dark", 0x161c2c),
    ("aluminium", 0x8a93a6),
    ("aluminium_dark", 0x5b6478),
    ("polymer", 0x0d1018),
    ("bezel", 0x080b12),
    ("bay", 0x141b2e),
    ("grille", 0x0f1420),
    ("interior", 0x05070b),
    ("shroud", 0x2a3350),
    ("blade", 0x161a24),
    ("rubber", 0x07080c),
    ("screen", 0x05070d),
    ("plate", 0x080b12),
];

/// The face table of a style in one theme.
fn face_table(style: Style, dark: bool) -> &'static [(&'static str, u32)] {
    match (style, dark) {
        (Style::Ink, false) => &INK_LIGHT[..],
        (Style::Ink, true) => &INK_DARK[..],
        (Style::Blueprint, _) => &BLUEPRINT[..],
        (Style::Night, _) => &NIGHT[..],
    }
}

/// The hex colour of face material `name`; unknown names take polymer's.
fn face_hex(style: Style, dark: bool, name: &str) -> u32 {
    let table = face_table(style, dark);
    let find = |key: &str| table.iter().find(|(n, _)| *n == key).map(|(_, hex)| *hex);
    find(name).or_else(|| find("polymer")).unwrap_or(0)
}

/// The hex colour of the glass in a style's theme (the toon styles only).
fn glass_hex(style: Style, dark: bool) -> u32 {
    match (style, dark) {
        (Style::Ink, false) => 0xdbe1d7,
        (Style::Ink, true) => 0x9db6dc,
        (Style::Night, _) => 0x2a3350,
        (Style::Blueprint, _) => 0x0e3358,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STYLES: [Style; 3] = [Style::Ink, Style::Blueprint, Style::Night];

    const NAMES: [&str; 18] = [
        "graphite",
        "graphite_dark",
        "aluminium",
        "aluminium_dark",
        "polymer",
        "bezel",
        "bay",
        "grille",
        "interior",
        "shroud",
        "blade",
        "glass",
        "rubber",
        "status",
        "led",
        "signal",
        "screen",
        "plate",
    ];

    #[test]
    fn names_round_trip() {
        for style in STYLES {
            assert_eq!(Style::parse(style.name()), Some(style));
        }
    }

    #[test]
    fn letters_case_and_spaces_parse() {
        assert_eq!(Style::parse("a"), Some(Style::Ink));
        assert_eq!(Style::parse("C"), Some(Style::Blueprint));
        assert_eq!(Style::parse(" d "), Some(Style::Night));
        assert_eq!(Style::parse("INK"), Some(Style::Ink));
        assert_eq!(Style::parse(" Blueprint\n"), Some(Style::Blueprint));
        assert_eq!(Style::parse("b"), None);
        assert_eq!(Style::parse(""), None);
    }

    #[test]
    fn every_material_has_the_expected_shading() {
        for style in STYLES {
            for dark in [false, true] {
                let palette = Palette::of(style, dark);
                for name in NAMES {
                    let m = palette.material(name);
                    match name {
                        "glass" => assert!(m.transparent || !m.visible, "{style:?} glass is solid"),
                        "status" | "led" | "signal" => {
                            assert!(m.emissive != Color::BLACK, "{style:?} {name} does not glow");
                        }
                        _ => {
                            assert_eq!(
                                matches!(m.shading, Shading::Basic),
                                style == Style::Blueprint,
                                "{style:?} {name} has the wrong shading"
                            );
                            assert!(matches!(m.shading, Shading::Basic | Shading::Toon));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn unknown_names_take_polymer() {
        let palette = Palette::of(Style::Ink, true);
        assert_eq!(
            palette.material("nope").color,
            palette.material("polymer").color
        );
    }

    #[test]
    fn edges_are_lines_in_every_style() {
        for style in STYLES {
            for dark in [false, true] {
                let m = Palette::of(style, dark)
                    .edge_material()
                    .expect("every style draws edges");
                assert!(matches!(m.shading, Shading::Line));
            }
        }
    }

    #[test]
    fn ink_themes_differ_and_blueprint_and_night_match() {
        assert_ne!(
            Palette::of(Style::Ink, false).background,
            Palette::of(Style::Ink, true).background
        );
        let blueprint_dark = Palette::of(Style::Blueprint, true);
        assert_eq!(
            Palette {
                dark: false,
                ..blueprint_dark.clone()
            },
            Palette::of(Style::Blueprint, false)
        );
        let night_dark = Palette::of(Style::Night, true);
        assert_eq!(
            Palette {
                dark: false,
                ..night_dark
            },
            Palette::of(Style::Night, false)
        );
    }
}
