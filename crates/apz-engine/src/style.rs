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
        let _ = name;
        todo!()
    }

    /// The style's name: `ink`, `blueprint` or `night`.
    pub fn name(self) -> &'static str {
        todo!()
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
        let _ = (style, dark);
        todo!()
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
        let _ = name;
        todo!()
    }

    /// The feature edges' material: `Shading::Line` in the edge colour, transparent when its opacity is below 1; None
    /// when the palette draws no edges.
    pub fn edge_material(&self) -> Option<Material> {
        let _ = (Shading::Line,);
        todo!()
    }
}
