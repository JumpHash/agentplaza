//! The robot data formats (schema 1), as the JSON under `assets/robots/` is typed. The reference is
//! `docs/design/robots.md`. `validate.rs` checks a parsed file against the rules here before it is read
//! into these structs (so an error is named by its path), and `files.rs` loads them.

use std::collections::BTreeMap;

use serde::Deserialize;

/// A position or a rotation: `[x, y, z]`.
pub type Vec3 = [f32; 3];

/// The enumerated names the format allows.
pub const LOCOMOTIONS: &[&str] = &["treads", "wheels", "ball", "unicycle", "hover", "fly", "float", "hop", "bounce", "waddle", "walk2", "walk4", "walk6", "scuttle", "slither", "tumble"];
pub const GEO_TYPES: &[&str] = &["box", "rbox", "cyl", "cone", "sphere", "torus", "plane", "ring", "shape"];
/// The materials a part may name: palette colours, the per-robot hues, and the glowing or clear ones.
pub const MATS: &[&str] = &["shell", "shell2", "panel", "paper", "trim", "ink", "signal", "glass", "eye", "light", "thruster", "accent", "clear"];
pub const EYE_SHAPES: &[&str] = &["round", "bar", "visor", "ring", "heart", "arc"];
pub const WAVES: &[&str] = &["sin", "abs", "tri", "saw", "square", "pulse", "hold", "halfsin"];
pub const FX_KINDS: &[&str] = &["sparks", "steam", "bubbles", "zzz", "notes", "hearts", "dots", "keys", "ink", "glow", "trail", "confetti"];
/// The bones every species has: a species without arms still has the arm bones (tentacles, stalks).
pub const REQUIRED_BONES: &[&str] = &["base", "body", "head", "armL", "armR", "gripL", "gripR", "carry"];
pub const PERSONALITY: &[&str] = &["social", "energy", "focus", "curiosity", "play", "tidy"];
pub const GRIPS: &[&str] = &["one", "two", "carry"];

/// A part's material: a name from `MATS`, or an inline material.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum MatRef {
    Name(String),
    Inline(InlineMat),
}

/// An inline material: a colour, and optionally a glow, an opacity and whether it is unlit.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InlineMat {
    pub color: String,
    pub emissive: Option<String>,
    pub glow: Option<f32>,
    pub opacity: Option<f32>,
    pub flat: Option<bool>,
}

/// A part's geometry (the hardware format's geo types, with the robots' segment counts).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Geo {
    #[serde(rename = "type")]
    pub kind: String,
    pub w: Option<f32>,
    pub h: Option<f32>,
    pub d: Option<f32>,
    pub r: Option<f32>,
    pub rt: Option<f32>,
    pub rb: Option<f32>,
    pub ri: Option<f32>,
    pub ro: Option<f32>,
    pub tube: Option<f32>,
    /// Degrees of a torus.
    pub arc: Option<f32>,
    /// Segments: rbox `[n]`, cyl/cone `[radial]`, sphere `[w, h]`, torus `[radial, tubular]`.
    pub seg: Option<Vec<u32>>,
    /// `top` on a sphere: a dome.
    pub half: Option<String>,
    /// A shape's outline, `[x, z]` points.
    pub points: Option<Vec<[f32; 2]>>,
}

/// A uniform scale or one per axis.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(untagged)]
pub enum Scale {
    Uniform(f32),
    Xyz(Vec3),
}

/// One mesh: a geometry at a bone (or at a tool's origin), with a material and a transform.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Part {
    pub bone: Option<String>,
    pub geo: Geo,
    pub mat: MatRef,
    pub at: Option<Vec3>,
    /// Degrees, XYZ, as in the hardware format.
    pub rot: Option<Vec3>,
    pub scale: Option<Scale>,
    /// The ink line: default on, except for an `ink` part; `false` drops it.
    pub outline: Option<bool>,
    /// A human name for the part (the audit and the fixers cite it).
    pub name: Option<String>,
    /// The part is intentionally not mirrored (the symmetry check skips it).
    pub asym: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bone {
    pub parent: String,
    pub at: Vec3,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gait {
    pub speed: f32,
    pub freq: f32,
    pub bob: f32,
    pub sway: f32,
    pub lean: f32,
    pub sideways: bool,
}

impl Gait {
    /// The numeric gait value of a name ("bob"), or None for an unknown name.
    pub fn get(&self, name: &str) -> Option<f32> {
        match name {
            "speed" => Some(self.speed),
            "freq" => Some(self.freq),
            "bob" => Some(self.bob),
            "sway" => Some(self.sway),
            "lean" => Some(self.lean),
            _ => None,
        }
    }
}

/// A channel's number: a value, or `"gait.<name>"` for the species' gait.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Param {
    Num(f32),
    Gait(String),
}

/// One procedural channel: `value = base + amp × wave(2π·freq·t + phase)`, or `rate` rad/s accumulated.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Channel {
    pub bone: String,
    pub prop: String,
    pub base: Option<Param>,
    pub amp: Option<Param>,
    /// Hz.
    pub freq: Option<Param>,
    pub phase: Option<f32>,
    pub wave: Option<String>,
    /// For square and pulse: the high fraction of each cycle.
    pub duty: Option<f32>,
    /// `in-out`: the amplitude follows a sine envelope over the clip's `dur`.
    pub env: Option<String>,
    /// Set the value at once, without the blend.
    pub snap: Option<bool>,
    /// rad/s, accumulated while the clip plays.
    pub rate: Option<f32>,
}

/// An effect a clip spawns while it plays, at an anchor.
#[derive(Debug, Clone, Deserialize)]
pub struct FxSpec {
    pub kind: String,
    pub at: String,
    /// Seconds between spawns (default 0.6).
    pub every: Option<f32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Clip {
    #[serde(rename = "loop")]
    pub looped: Option<bool>,
    /// Seconds, for a one-shot.
    pub dur: Option<f32>,
    pub channels: Vec<Channel>,
    #[serde(default)]
    pub fx: Vec<FxSpec>,
}

/// A glowing eye: a glass socket (unless `glass` is false) and a pupil that blinks and takes the state colour.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Eye {
    pub bone: String,
    pub at: Vec3,
    /// The glass socket's radius (default 0.1).
    pub r: Option<f32>,
    pub pupil: f32,
    pub shape: String,
    pub glass: Option<bool>,
    /// The pupil's stretch in y (1 = round).
    pub stretch: Option<f32>,
    /// The eye is intentionally not mirrored (the symmetry check skips it).
    pub asym: Option<bool>,
}

/// A bone that turns in a state: rad/s about an axis.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spin {
    pub bone: String,
    pub axis: String,
    pub walk: f32,
    pub idle: f32,
    pub offline: Option<f32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quirk {
    pub clip: String,
    pub weight: f32,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Personality {
    pub social: u8,
    pub energy: u8,
    pub focus: u8,
    pub curiosity: u8,
    pub play: u8,
    pub tidy: u8,
}

/// Extra parts only the orchestrator wears.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Orchestrator {
    pub parts: Vec<Part>,
}

/// One species (`species/<id>.json`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Species {
    pub schema: u32,
    pub id: String,
    /// The nickname shown in the agent card.
    pub name: String,
    pub blurb: String,
    pub accent: String,
    pub height: f32,
    /// The label anchor's height above the root.
    pub label_top: f32,
    pub locomotion: String,
    pub gait: Gait,
    pub bones: BTreeMap<String, Bone>,
    pub parts: Vec<Part>,
    pub eyes: Vec<Eye>,
    pub spin: Vec<Spin>,
    pub feet: Vec<String>,
    pub floats: bool,
    pub personality: Personality,
    pub quirks: Vec<Quirk>,
    pub clips: BTreeMap<String, Clip>,
    /// A shared clip id → this species' clip id that replaces it.
    pub overrides: BTreeMap<String, String>,
    pub orchestrator: Option<Orchestrator>,
}

/// One held tool or carried item (`tools/<id>.json`): parts at its origin, held at the grip.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    pub schema: u32,
    pub id: String,
    pub name: String,
    pub grip: String,
    /// How a one-hand tool sits in the fist (its parts hang along −y): `forward` (default), `down`, `up`.
    #[serde(default)]
    pub aim: Option<String>,
    pub parts: Vec<Part>,
    pub tip: Vec3,
}

/// The lists, in order: the species (the deal), the tools and the props.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Index {
    pub schema: u32,
    pub species: Vec<String>,
    pub tools: Vec<String>,
    pub props: Vec<String>,
}

/// The fixed robot colours (`palette.json`), as hex strings.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Palette {
    pub ink: String,
    pub sheet: String,
    pub paper: String,
    pub rule: String,
    pub signal: String,
    pub ok: String,
    pub info: String,
    #[serde(rename = "inkMuted")]
    pub ink_muted: String,
}

/// The shared clips (`clips.json`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ClipsFile {
    pub schema: u32,
    pub clips: BTreeMap<String, Clip>,
}
