//! Skins: shape and type. A skin sets radii, row height, how section labels are set, card and popover shadows and
//! how primary actions are drawn. Colours come from the palette; a skin never names one, except to say which
//! palette token its primary action takes.

use serde::{Deserialize, Serialize};

/// One drop shadow layer: offset y, blur, and the alpha of black under it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shadow {
    pub y: f32,
    pub blur: f32,
    pub alpha: f32,
}

/// Shadows for one mode.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shadows {
    pub card: Vec<Shadow>,
    pub pop: Vec<Shadow>,
}

/// How section labels, table heads and field labels are set.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Label {
    /// Set in the mono face.
    pub mono: bool,
    /// Set in capitals.
    pub upper: bool,
    pub size: f32,
    pub weight: u16,
    /// Letter spacing in em.
    pub tracking: f32,
}

/// Which palette tokens a primary action is drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionFrom {
    /// Ink fill, sheet text (Loom: orange stays for the owner's own marks).
    Ink,
    /// Signal fill, on-signal text (Graphite).
    Signal,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Skin {
    pub id: String,
    pub name: String,
    /// One line for the Appearance card.
    pub about: String,
    pub font: String,
    pub font_mono: String,
    /// Base text size in px.
    pub text: f32,
    pub radius: f32,
    pub radius_lg: f32,
    /// A list row's height.
    pub row: f32,
    pub label: Label,
    pub action: ActionFrom,
    pub light: Shadows,
    pub dark: Shadows,
    /// The default palette (a built-in theme id) and background preset for this skin.
    pub palette: String,
    pub background: String,
}

fn s(y: f32, blur: f32, alpha: f32) -> Shadow {
    Shadow { y, blur, alpha }
}

/// The built-in skins.
pub fn skins() -> Vec<Skin> {
    vec![
        Skin {
            id: "loom".into(),
            name: "Loom".into(),
            about: "Warm paper, mono capital labels, ink for actions".into(),
            font: "Geist".into(),
            font_mono: "Geist Mono".into(),
            text: 13.5,
            radius: 8.0,
            radius_lg: 12.0,
            row: 32.0,
            label: Label {
                mono: true,
                upper: true,
                size: 11.0,
                weight: 500,
                tracking: 0.08,
            },
            action: ActionFrom::Ink,
            light: Shadows {
                card: vec![s(1.0, 1.0, 0.04)],
                pop: vec![s(2.0, 6.0, 0.06), s(16.0, 40.0, 0.16)],
            },
            dark: Shadows {
                card: vec![s(1.0, 1.0, 0.30)],
                pop: vec![s(2.0, 6.0, 0.40), s(16.0, 40.0, 0.55)],
            },
            palette: "loom-clay".into(),
            background: "loom-paper".into(),
        },
        Skin {
            id: "graphite".into(),
            name: "Graphite".into(),
            about: "Cool neutrals, sans labels, cobalt for actions".into(),
            font: "Geist".into(),
            font_mono: "Geist Mono".into(),
            text: 13.5,
            radius: 8.0,
            radius_lg: 12.0,
            row: 32.0,
            label: Label {
                mono: false,
                upper: false,
                size: 12.0,
                weight: 500,
                tracking: 0.0,
            },
            action: ActionFrom::Signal,
            light: Shadows {
                card: vec![],
                pop: vec![s(12.0, 32.0, 0.16)],
            },
            dark: Shadows {
                card: vec![],
                pop: vec![s(12.0, 32.0, 0.55)],
            },
            palette: "graphite-cobalt".into(),
            background: "graphite-grid".into(),
        },
    ]
}

/// The built-in skin with this id.
pub fn skin(id: &str) -> Option<Skin> {
    skins().into_iter().find(|s| s.id == id)
}

/// The skin a new hub starts in.
pub const DEFAULT_SKIN: &str = "loom";
