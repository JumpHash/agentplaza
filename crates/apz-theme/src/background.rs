//! Background styles: a small spec that one shader draws (`apz-engine`'s `backdrop.wgsl`). Presets bundle a style
//! with its colours; every preset can be opened and changed. Colours are palette token names unless set as hex.

use serde::{Deserialize, Serialize};

use crate::Tokens;
use crate::color::Rgba;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pattern {
    #[default]
    None,
    Dots,
    Weave,
    Grid,
    Blueprint,
    Contours,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Motion {
    #[default]
    Still,
    Slow,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ornament {
    #[default]
    None,
    Threads,
    Orbits,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Behind console pages only.
    #[default]
    Pages,
    /// Behind pages and the IDE.
    All,
}

/// A background spec, as stored in a theme file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Background {
    pub id: String,
    pub name: String,
    pub pattern: Pattern,
    /// The pattern's cell size in px.
    pub scale: f32,
    /// Up to three colour fields: token names (`signal`) or hex.
    pub fields: Vec<String>,
    /// How strongly the fields show, 0..=1.
    pub strength: f32,
    pub motion: Motion,
    /// The pattern lights under the pointer.
    pub pointer: bool,
    pub ornament: Ornament,
    /// Film grain, 0..=0.08.
    pub grain: f32,
    pub scope: Scope,
}

impl Default for Background {
    fn default() -> Self {
        Background {
            id: "plain".into(),
            name: "Plain".into(),
            pattern: Pattern::None,
            scale: 22.0,
            fields: vec![],
            strength: 0.0,
            motion: Motion::Still,
            pointer: false,
            ornament: Ornament::None,
            grain: 0.0,
            scope: Scope::Pages,
        }
    }
}

fn preset(
    id: &str,
    name: &str,
    pattern: Pattern,
    scale: f32,
    fields: &[&str],
    strength: f32,
    motion: Motion,
    pointer: bool,
    ornament: Ornament,
    grain: f32,
) -> Background {
    Background {
        id: id.into(),
        name: name.into(),
        pattern,
        scale,
        fields: fields.iter().map(|f| (*f).to_string()).collect(),
        strength,
        motion,
        pointer,
        ornament,
        grain,
        scope: Scope::Pages,
    }
}

/// The built-in presets, in the Appearance screen's order.
pub fn presets() -> Vec<Background> {
    use Motion::*;
    use Ornament as O;
    use Pattern as P;
    vec![
        preset("loom-paper", "Paper", P::Dots, 22.0, &["signal", "warn", "info"], 0.8, Slow, true, O::Threads, 0.045),
        preset("loom-linen", "Linen", P::Weave, 22.0, &["signal"], 0.45, Still, false, O::None, 0.03),
        preset("graphite-grid", "Grid", P::Grid, 32.0, &["signal", "review", "info"], 0.55, Slow, true, O::Orbits, 0.03),
        preset("graphite-blueprint", "Blueprint", P::Blueprint, 24.0, &["info"], 0.3, Still, true, O::None, 0.0),
        preset("contours", "Contours", P::Contours, 48.0, &["signal", "info"], 0.4, Slow, false, O::None, 0.02),
        Background::default(),
    ]
}

/// The preset with this id.
pub fn preset_by_id(id: &str) -> Option<Background> {
    presets().into_iter().find(|b| b.id == id)
}

impl Background {
    /// Problems with the spec, empty when it is valid.
    pub fn problems(&self) -> Vec<String> {
        let mut out = vec![];
        if self.fields.len() > 3 {
            out.push("a background has at most three colour fields".into());
        }
        for f in &self.fields {
            if !crate::tokens::is_token(f) && Rgba::parse(f).is_none() {
                out.push(format!("background field {f:?} is neither a token nor a colour"));
            }
        }
        if !(4.0..=256.0).contains(&self.scale) {
            out.push("background scale must be 4–256 px".into());
        }
        if !(0.0..=1.0).contains(&self.strength) {
            out.push("background strength must be 0–1".into());
        }
        if !(0.0..=0.08).contains(&self.grain) {
            out.push("background grain must be 0–0.08".into());
        }
        out
    }

    /// The field colours, resolved against a palette (opaque). Missing fields are the ground.
    pub fn field_colors(&self, tokens: &Tokens) -> [Rgba; 3] {
        let ground = tokens.get("paper");
        let pick = |i: usize| {
            self.fields
                .get(i)
                .and_then(|f| {
                    if crate::tokens::is_token(f) {
                        Some(tokens.get(f))
                    } else {
                        Rgba::parse(f).map(|c| c.over(ground))
                    }
                })
                .unwrap_or(ground)
        };
        [pick(0), pick(1), pick(2)]
    }
}
