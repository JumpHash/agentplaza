//! The hardware models of the Machines view and the machine's board (`docs/design/hardware-plazas.md`,
//! "One source of truth"). Each model is JSON under `assets/hardware/`, read here exactly as the
//! web reads it: `build` turns a model into a scene graph, `update` drives its tags and
//! animations, `traces` and `Pulses` draw the copper and the current, and `place_atom` puts the
//! plaza's atom at the board's centre.
//!
//! The Machines screen consumes this API next; until then it is unused, hence the module allow.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::f32::consts::{FRAC_PI_2, TAU};
use std::sync::OnceLock;

use glam::{EulerRot, Mat4, Quat, Vec3};
use serde::Deserialize;

use super::geometry;
use super::props::{self, PropPalette};
use super::scene::{self, Color, Mat, Material, Obj, Shading};
use super::textures;
use super::toon::outline;

// ---- Embedded models ------------------------------------------------------------------------

/// Every board and exterior model, in the same order as the web's generated index.
pub static MODELS: &[(&str, &str)] = &[
    ("exterior/workstation", include_str!("../../../../../assets/hardware/exterior/workstation.json")),
    ("exterior/laptop", include_str!("../../../../../assets/hardware/exterior/laptop.json")),
    ("exterior/edge", include_str!("../../../../../assets/hardware/exterior/edge.json")),
    ("exterior/phone", include_str!("../../../../../assets/hardware/exterior/phone.json")),
    ("exterior/server", include_str!("../../../../../assets/hardware/exterior/server.json")),
    ("exterior/apple-laptop", include_str!("../../../../../assets/hardware/exterior/apple-laptop.json")),
    ("exterior/apple-desktop", include_str!("../../../../../assets/hardware/exterior/apple-desktop.json")),
    ("board/workstation", include_str!("../../../../../assets/hardware/board/workstation.json")),
    ("board/laptop", include_str!("../../../../../assets/hardware/board/laptop.json")),
    ("board/edge", include_str!("../../../../../assets/hardware/board/edge.json")),
    ("board/phone", include_str!("../../../../../assets/hardware/board/phone.json")),
    ("board/server", include_str!("../../../../../assets/hardware/board/server.json")),
    ("board/apple-soc", include_str!("../../../../../assets/hardware/board/apple-soc.json")),
];

/// The exterior and board model names for a machine kind and vendor. Apple is its own family:
/// its laptops and desktops share one SoC board. Unknown kinds fall back to `workstation`.
pub fn models_for(kind: &str, vendor: Option<&str>) -> (&'static str, &'static str) {
    if vendor == Some("apple") {
        let exterior = if kind == "laptop" { "exterior/apple-laptop" } else { "exterior/apple-desktop" };
        return (exterior, "board/apple-soc");
    }
    match kind {
        "laptop" => ("exterior/laptop", "board/laptop"),
        "edge" => ("exterior/edge", "board/edge"),
        "phone" => ("exterior/phone", "board/phone"),
        "server" => ("exterior/server", "board/server"),
        _ => ("exterior/workstation", "board/workstation"),
    }
}

/// Reusable part files (`parts/*.json`), referenced by `use: "parts/<name>"`. Empty until the
/// part files land: add each as `("parts/<name>", include_str!(...))`, in the web index's order.
pub static PARTS: &[(&str, &str)] = &[
    ("parts/gpu-pcie", include_str!("../../../../../assets/hardware/parts/gpu-pcie.json")),
    ("parts/dimm", include_str!("../../../../../assets/hardware/parts/dimm.json")),
    ("parts/sodimm", include_str!("../../../../../assets/hardware/parts/sodimm.json")),
    ("parts/m2-ssd", include_str!("../../../../../assets/hardware/parts/m2-ssd.json")),
    ("parts/fan-120", include_str!("../../../../../assets/hardware/parts/fan-120.json")),
    ("parts/heatsink-tower", include_str!("../../../../../assets/hardware/parts/heatsink-tower.json")),
    ("parts/cap-bank", include_str!("../../../../../assets/hardware/parts/cap-bank.json")),
    ("parts/vrm-bank", include_str!("../../../../../assets/hardware/parts/vrm-bank.json")),
    ("parts/nic", include_str!("../../../../../assets/hardware/parts/nic.json")),
    ("parts/usb-c", include_str!("../../../../../assets/hardware/parts/usb-c.json")),
    ("parts/rj45", include_str!("../../../../../assets/hardware/parts/rj45.json")),
];

/// The shared palette (`palette.json`): `{"schema": 1, "colors": {name: material}}`, each entry an
/// inline material (colour, emissive, glow, opacity, flat).
const PALETTE_SRC: &str = include_str!("../../../../../assets/hardware/palette.json");

// ---- Schema ---------------------------------------------------------------------------------

/// A model file. `view` says which of the three kinds it is.
#[derive(Debug, Clone, Deserialize)]
pub struct Model {
    pub schema: u32,
    pub kind: String,
    pub view: View,
    pub size: [f32; 3],
    #[serde(default)]
    pub params: BTreeMap<String, f64>,
    #[serde(default)]
    pub parts: Vec<Part>,
    #[serde(default)]
    pub anchors: Anchors,
    #[serde(default)]
    pub slots: Vec<Slot>,
    #[serde(default)]
    pub traces: Vec<Trace>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum View {
    Exterior,
    Board,
    Part,
}

/// Named points in model space.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Anchors {
    pub label: Option<[f32; 3]>,
    pub port: Option<[f32; 3]>,
    pub camera: Option<Camera>,
    /// Exterior: the part id that animates away on ENTER.
    pub open: Option<String>,
    pub atom: Option<[f32; 3]>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Camera {
    pub at: [f32; 3],
    pub look: [f32; 3],
}

/// One piece of a model. Either `geo` or `use` (a part file), with children transformed by it.
#[derive(Debug, Clone, Deserialize)]
pub struct Part {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub geo: Option<Geo>,
    #[serde(rename = "use", default)]
    pub use_: Option<String>,
    #[serde(default)]
    pub at: Option<[f32; 3]>,
    /// Euler angles in degrees, XYZ order.
    #[serde(default)]
    pub rot: Option<[f32; 3]>,
    #[serde(default)]
    pub scale: Option<Scale>,
    #[serde(default)]
    pub mat: Option<MatSpec>,
    /// The toon ink outline. Default: on for solids, off for flat decals (plane, ring).
    #[serde(default)]
    pub outline: Option<bool>,
    #[serde(default)]
    pub repeat: Option<Repeat>,
    /// `gpus`, `gpus >= n`, `!gpus`: shown only if it holds on the params.
    #[serde(default)]
    pub when: Option<String>,
    #[serde(default)]
    pub anim: Option<Anim>,
    /// `status`, `activity`, `fan` or `screen`.
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub label: Option<Label>,
    #[serde(default)]
    pub children: Vec<Part>,
}

/// A geometry, tagged by `type`. Mirrors the three.js builders the web uses.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Geo {
    Box { w: f32, h: f32, d: f32 },
    Rbox { w: f32, h: f32, d: f32, r: f32 },
    Cyl {
        rt: f32,
        rb: f32,
        h: f32,
        #[serde(default = "seg_default")]
        seg: u32,
    },
    Cone { r: f32, h: f32 },
    Sphere { r: f32 },
    Torus {
        r: f32,
        tube: f32,
        /// Degrees.
        #[serde(default = "full_turn")]
        arc: f32,
    },
    /// Horizontal, facing up.
    Plane { w: f32, d: f32 },
    /// Flat, horizontal.
    Ring { ri: f32, ro: f32 },
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(untagged)]
pub enum Scale {
    Uniform(f32),
    Xyz([f32; 3]),
}

/// A palette name, or an inline material.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum MatSpec {
    Name(String),
    Inline(InlineMat),
}

#[derive(Debug, Clone, Deserialize)]
pub struct InlineMat {
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub emissive: Option<String>,
    #[serde(default)]
    pub glow: f32,
    #[serde(default = "one")]
    pub opacity: f32,
    #[serde(default)]
    pub additive: bool,
    /// Basic (unlit) instead of toon.
    #[serde(default)]
    pub flat: bool,
}

/// Copies of a part. Copy i sits at `step * i`; with `wrap`, at `step * (i % wrap) + row * (i / wrap)`.
#[derive(Debug, Clone, Deserialize)]
pub struct Repeat {
    pub count: Count,
    pub step: [f32; 3],
    pub wrap: Option<u32>,
    pub row: Option<[f32; 3]>,
}

/// A repeat count: a number, or a numeric param's name (`"p_cores"`), or `"p_cores/4"` (the param over 4,
/// rounded up), read at build time.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Count {
    Num(u32),
    Param(String),
}

/// A named count split into its param and divisor: `"p_cores"` is (p_cores, 1), `"p_cores/4"` is (p_cores, 4).
/// None when it is neither a param name nor a param name over a whole number 1 or more.
fn split_count(s: &str) -> Option<(&str, u32)> {
    let (name, div) = match s.split_once('/') {
        Some((name, div)) if !div.starts_with('0') && div.bytes().all(|b| b.is_ascii_digit()) => (name, div.parse::<u32>().ok()?),
        Some(_) => return None,
        None => (s, 1),
    };
    let mut chars = name.chars();
    let first_ok = matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c == '_');
    (first_ok && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')).then_some((name, div))
}

/// Where copy `i` of a repeat sits (see `Repeat`). A `wrap` of 0 is a validation error; it counts as no wrap here.
fn repeat_offset(r: &Repeat, i: u32) -> Vec3 {
    let (col, line) = match r.wrap {
        Some(w) if w >= 1 => (i % w, i / w),
        _ => (i, 0),
    };
    Vec3::from(r.step) * col as f32 + Vec3::from(r.row.unwrap_or([0.0; 3])) * line as f32
}

/// Silkscreen text on a part's top.
#[derive(Debug, Clone, Deserialize)]
pub struct Label {
    pub text: String,
    #[serde(default = "label_size")]
    pub size: f32,
    #[serde(default = "label_color")]
    pub color: String,
}

/// Animations. Each key is optional; a part may carry several.
#[derive(Debug, Clone, Deserialize)]
pub struct Anim {
    pub spin: Option<Spin>,
    pub pulse: Option<Pulse>,
    pub blink: Option<Blink>,
    pub slide: Option<Slide>,
    pub swing: Option<Swing>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Spin {
    #[serde(default = "axis_y")]
    pub axis: String,
    pub rps: f32,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Pulse {
    pub period: f32,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Blink {
    pub period: f32,
    #[serde(default)]
    pub phase: f32,
}

/// Moves to `to` (model space) as the opening runs (`on: "enter"`).
#[derive(Debug, Clone, Deserialize)]
pub struct Slide {
    pub to: [f32; 3],
    #[serde(default = "enter")]
    pub on: String,
}

/// Turns `to` degrees about `axis` as the opening runs.
#[derive(Debug, Clone, Deserialize)]
pub struct Swing {
    #[serde(default = "axis_x")]
    pub axis: String,
    pub to: f32,
    #[serde(default = "enter")]
    pub on: String,
}

/// A surface rooms are built on (board only).
#[derive(Debug, Clone, Deserialize)]
pub struct Slot {
    pub id: String,
    pub on: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub top: f32,
    pub rect: [f32; 4],
    #[serde(default = "one_u32")]
    pub capacity: u32,
}

/// Copper path on the PCB (board only).
#[derive(Debug, Clone, Deserialize)]
pub struct Trace {
    pub path: Vec<[f32; 2]>,
    #[serde(default = "trace_y")]
    pub y: f32,
    #[serde(default = "trace_width")]
    pub width: f32,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
}

fn seg_default() -> u32 {
    24
}
fn full_turn() -> f32 {
    360.0
}
fn one() -> f32 {
    1.0
}
fn one_u32() -> u32 {
    1
}
fn label_size() -> f32 {
    0.6
}
fn label_color() -> String {
    "#e8eadf".to_string()
}
fn axis_y() -> String {
    "y".to_string()
}
fn axis_x() -> String {
    "x".to_string()
}
fn enter() -> String {
    "enter".to_string()
}
fn trace_y() -> f32 {
    0.06
}
fn trace_width() -> f32 {
    0.25
}

// ---- Lookup ---------------------------------------------------------------------------------

fn parse(src: &str) -> Result<Model, String> {
    serde_json::from_str(src).map_err(|e| e.to_string())
}

/// Every embedded file, parsed once.
fn parsed() -> &'static [(&'static str, Result<Model, String>)] {
    static CACHE: OnceLock<Vec<(&'static str, Result<Model, String>)>> = OnceLock::new();
    CACHE.get_or_init(|| MODELS.iter().chain(PARTS.iter()).map(|&(name, src)| (name, parse(src))).collect())
}

/// A model by its name (`"exterior/workstation"`, `"board/laptop"`, `"parts/gpu-pcie"`). `None`
/// when unknown or when its file does not parse (the unit test catches that).
pub fn model(name: &str) -> Option<&'static Model> {
    parsed().iter().find(|(n, _)| *n == name).and_then(|(_, m)| m.as_ref().ok())
}

#[derive(Deserialize)]
struct PaletteFile {
    colors: BTreeMap<String, InlineMat>,
}

/// The palette, parsed once: name to material.
fn palette() -> &'static BTreeMap<String, InlineMat> {
    static PAL: OnceLock<BTreeMap<String, InlineMat>> = OnceLock::new();
    PAL.get_or_init(|| {
        // An unparsable palette is reported by the palette test, not here.
        serde_json::from_str::<PaletteFile>(PALETTE_SRC).map(|f| f.colors).unwrap_or_default()
    })
}

/// A palette entry's base colour.
fn named(name: &str, fallback: u32) -> Color {
    palette()
        .get(name)
        .and_then(|m| m.color.as_deref())
        .and_then(Color::css)
        .unwrap_or(Color::hex(fallback))
}

// ---- Params and `when` ----------------------------------------------------------------------

/// Inputs the model reacts to (`gpus`, ...). Overrides the model's own `params`.
#[derive(Debug, Clone, Default)]
pub struct Params {
    pub values: BTreeMap<String, f64>,
}

impl Params {
    pub fn with(mut self, name: &str, value: f64) -> Self {
        self.values.insert(name.to_string(), value);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Cmp {
    Ge,
    Le,
    Gt,
    Lt,
    Eq,
    Ne,
}

#[derive(Debug, Clone, PartialEq)]
struct When {
    negate: bool,
    name: String,
    cmp: Option<(Cmp, f64)>,
}

/// Parses `name`, `!name`, `name OP number` (OP one of >= <= > < == !=).
fn parse_when(expr: &str) -> Result<When, String> {
    let mut s = expr.trim();
    let negate = match s.strip_prefix('!') {
        Some(rest) => {
            s = rest.trim_start();
            true
        }
        None => false,
    };
    let end = s.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(s.len());
    let name = &s[..end];
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        return Err(format!("'{expr}': expected a param name"));
    }
    let rest = s[end..].trim();
    let cmp = if rest.is_empty() {
        None
    } else {
        const OPS: [(&str, Cmp); 6] =
            [(">=", Cmp::Ge), ("<=", Cmp::Le), ("==", Cmp::Eq), ("!=", Cmp::Ne), (">", Cmp::Gt), ("<", Cmp::Lt)];
        let (op, tail) = OPS
            .iter()
            .find_map(|&(sym, op)| rest.strip_prefix(sym).map(|t| (op, t)))
            .ok_or_else(|| format!("'{expr}': unknown operator"))?;
        let n: f64 = tail.trim().parse().map_err(|_| format!("'{expr}': bad number"))?;
        Some((op, n))
    };
    Ok(When { negate, name: name.to_string(), cmp })
}

/// A missing param reads as 0.
fn eval_when(w: &When, vals: &BTreeMap<String, f64>) -> bool {
    let v = vals.get(&w.name).copied().unwrap_or(0.0);
    let holds = match w.cmp {
        None => v != 0.0,
        Some((op, n)) => match op {
            Cmp::Ge => v >= n,
            Cmp::Le => v <= n,
            Cmp::Gt => v > n,
            Cmp::Lt => v < n,
            Cmp::Eq => v == n,
            Cmp::Ne => v != n,
        },
    };
    holds != w.negate
}

// ---- Validation -----------------------------------------------------------------------------

const SLOT_TYPES: [&str; 11] = ["cpu", "gpu", "memory", "storage", "network", "power", "io", "soc", "modem", "bmc", "npu"];
const TAGS: [&str; 4] = ["status", "activity", "fan", "screen"];

struct Check<'a> {
    name: &'a str,
    params: &'a BTreeMap<String, f64>,
    errs: Vec<String>,
    ids: HashSet<String>,
    pal: &'static BTreeMap<String, InlineMat>,
}

impl Check<'_> {
    fn err(&mut self, msg: String) {
        self.errs.push(format!("{}: {msg}", self.name));
    }

    fn parts(&mut self, list: &[Part]) {
        for p in list {
            if let Some(id) = &p.id {
                if !self.ids.insert(id.clone()) {
                    self.err(format!("duplicate id '{id}'"));
                }
            }
            match &p.mat {
                Some(MatSpec::Name(n)) => {
                    if !self.pal.contains_key(n) {
                        self.err(format!("unknown palette name '{n}'"));
                    }
                }
                Some(MatSpec::Inline(i)) => {
                    for c in [&i.color, &i.emissive].into_iter().flatten() {
                        if Color::css(c).is_none() {
                            self.err(format!("bad colour '{c}'"));
                        }
                    }
                }
                None => {}
            }
            if let Some(name) = &p.use_ {
                if model(name).is_none() {
                    self.err(format!("unknown part '{name}'"));
                }
            }
            if let Some(expr) = &p.when {
                if let Err(e) = parse_when(expr) {
                    self.err(format!("bad when: {e}"));
                }
            }
            if let Some(t) = &p.tag {
                if !TAGS.contains(&t.as_str()) {
                    self.err(format!("unknown tag '{t}'"));
                }
            }
            if let Some(r) = &p.repeat {
                match &r.count {
                    Count::Num(0) => self.err("repeat count must be at least 1".to_string()),
                    Count::Param(s) => match split_count(s) {
                        None => self.err(format!("repeat count '{s}' is not a param name or 'param/n'")),
                        Some((name, _)) if !self.params.contains_key(name) => {
                            self.err(format!("repeat count '{name}' is not a param of the model"))
                        }
                        Some(_) => {}
                    },
                    _ => {}
                }
                if r.wrap == Some(0) {
                    self.err("repeat wrap must be at least 1".to_string());
                }
                match (r.wrap, r.row) {
                    (Some(_), None) => self.err("repeat wrap needs a row [dx, dy, dz]".to_string()),
                    (None, Some(_)) => self.err("repeat row needs a wrap".to_string()),
                    _ => {}
                }
            }
            if let Some(l) = &p.label {
                if Color::css(&l.color).is_none() {
                    self.err(format!("bad label colour '{}'", l.color));
                }
            }
            self.parts(&p.children);
        }
    }
}

/// The rules of the spec's schema: unknown palette names (once the palette is embedded),
/// duplicate ids, `use` of an unknown part, slots on missing parts, a bad `when`, unknown tags.
/// An empty list means the model is valid.
pub fn validate(model: &Model, name: &str) -> Vec<String> {
    let mut c = Check { name, params: &model.params, errs: Vec::new(), ids: HashSet::new(), pal: palette() };
    if model.schema != 1 {
        c.err(format!("schema {} (expected 1)", model.schema));
    }
    c.parts(&model.parts);
    for s in &model.slots {
        if !c.ids.contains(&s.on) {
            c.err(format!("slot '{}' sits on missing part '{}'", s.id, s.on));
        }
        if !SLOT_TYPES.contains(&s.kind.as_str()) {
            c.err(format!("slot '{}': unknown type '{}'", s.id, s.kind));
        }
    }
    if let Some(open) = &model.anchors.open {
        if !c.ids.contains(open) {
            c.err(format!("anchors.open: missing part '{open}'"));
        }
    }
    for t in &model.traces {
        if t.path.len() < 2 {
            c.err("trace with fewer than two points".to_string());
        }
    }
    c.errs
}

// ---- Materials and geometry -----------------------------------------------------------------

/// `flat` lays a plane or ring horizontal, facing up (three's `rotateX(-PI/2)`).
fn flat() -> Mat4 {
    Mat4::from_rotation_x(-FRAC_PI_2)
}

fn geometry_of(g: &Geo) -> scene::Geo {
    match *g {
        Geo::Box { w, h, d } => geometry::box_geo(w, h, d),
        Geo::Rbox { w, h, d, r } => geometry::rounded_box(w, h, d, 3, r),
        Geo::Cyl { rt, rb, h, seg } => geometry::cylinder_full(rt, rb, h, seg, 1, false, 0.0, TAU),
        Geo::Cone { r, h } => geometry::cone(r, h, 24),
        Geo::Sphere { r } => geometry::sphere(r, 24, 16),
        Geo::Torus { r, tube, arc } => geometry::torus(r, tube, 8, 64, arc.to_radians()),
        Geo::Plane { w, d } => geometry::transformed(&geometry::plane(w, d, 1, 1), flat()),
        Geo::Ring { ri, ro } => geometry::transformed(&geometry::ring(ri, ro, 48, 1, 0.0, TAU), flat()),
    }
}

/// The height of a part's top above its origin, where a label sits.
fn top_of(g: Option<&Geo>) -> f32 {
    match g {
        Some(Geo::Box { h, .. } | Geo::Rbox { h, .. } | Geo::Cyl { h, .. } | Geo::Cone { h, .. }) => h / 2.0,
        Some(Geo::Sphere { r }) => *r,
        _ => 0.0,
    }
}

/// A part's material spec with palette names resolved to their entry.
fn resolve(spec: Option<&MatSpec>) -> Option<&InlineMat> {
    match spec {
        None => None,
        Some(MatSpec::Name(n)) => palette().get(n),
        Some(MatSpec::Inline(i)) => Some(i),
    }
}

fn material(spec: Option<&MatSpec>) -> Mat {
    let fallback = Color::hex(0x8a8f98);
    let m = match resolve(spec) {
        None => Material { color: fallback, ..Material::default() },
        Some(i) => {
            let opacity = i.opacity.clamp(0.0, 1.0);
            Material {
                shading: if i.flat { Shading::Basic } else { Shading::Toon },
                color: i.color.as_deref().and_then(Color::css).unwrap_or(fallback),
                emissive: i.emissive.as_deref().and_then(Color::css).unwrap_or(Color::BLACK),
                emissive_intensity: i.glow,
                opacity,
                transparent: opacity < 1.0 || i.additive,
                additive: i.additive,
                depth_write: !i.additive,
                ..Material::default()
            }
        }
    };
    Mat::new(m)
}

fn glow_of(spec: Option<&MatSpec>) -> f32 {
    resolve(spec).map_or(0.0, |i| i.glow)
}

/// Silkscreen text: a flat plane on the part's top, its texture drawn on the canvas.
fn label_node(l: &Label, top: f32) -> Obj {
    const PX: f32 = 64.0;
    let width = l.text.chars().count().max(1) as f32 * l.size * 0.62;
    let tw = (width * PX).clamp(64.0, 2048.0) as u32;
    let th = (l.size * PX).clamp(32.0, 512.0) as u32;
    let key = format!("hw-label:{}:{tw}x{th}", l.text);
    let text = l.text.clone();
    let tex = textures::canvas_texture(&key, tw, th, 1.0, move |c, w, h, _| {
        c.set_font(&format!("bold {}px sans-serif", (h * 0.8) as u32));
        c.set_fill("#ffffff");
        c.set_text_align("center");
        c.set_text_baseline("middle");
        c.fill_text(&text, w * 0.5, h * 0.5);
    });
    let mat = Mat::new(Material {
        shading: Shading::Basic,
        color: Color::css(&l.color).unwrap_or(Color::WHITE),
        map: Some(tex),
        transparent: true,
        depth_write: false,
        ..Material::default()
    });
    let node = Obj::mesh(&geometry::transformed(&geometry::plane(width, l.size, 1, 1), flat()), &mat);
    node.set_position(0.0, top + 0.02, 0.0).no_outline();
    node
}

// ---- Building -------------------------------------------------------------------------------

const OUTLINE_MIN: f32 = 0.1;
const MAX_USE_DEPTH: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tag {
    Status,
    Activity,
    Fan,
    Screen,
}

fn tag_of(s: &str) -> Option<Tag> {
    match s {
        "status" => Some(Tag::Status),
        "activity" => Some(Tag::Activity),
        "fan" => Some(Tag::Fan),
        "screen" => Some(Tag::Screen),
        _ => None,
    }
}

fn axis_of(s: &str) -> Vec3 {
    match s {
        "x" => Vec3::X,
        "z" => Vec3::Z,
        _ => Vec3::Y,
    }
}

/// A part that animates or carries a tag: its node, material and rest pose.
struct Live {
    obj: Obj,
    mat: Option<Mat>,
    tag: Option<Tag>,
    /// The material's glow, the base of every emissive change.
    glow: f32,
    rest: Vec3,
    rest_q: Quat,
    spin_axis: Vec3,
    spin_rps: f32,
    pulse: Option<f32>,
    blink: Option<(f32, f32)>,
    /// Target position, and whether it follows the opening (`on: "enter"`).
    slide: Option<(Vec3, bool)>,
    /// Axis, angle in radians at full open, and whether it follows the opening.
    swing: Option<(Vec3, f32, bool)>,
    angle: f32,
}

/// Machine state, as the Machines view reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Online,
    Draining,
    Offline,
    Revoked,
}

/// Per-frame inputs: the machine's status, how busy its agents are (0..1), and how far it is
/// opened (0..1, driven by ENTER).
#[derive(Debug, Clone, Copy, Default)]
pub struct State {
    pub status: Status,
    pub activity: f32,
    pub open: f32,
}

fn status_color(s: Status) -> Color {
    match s {
        Status::Online => named("led-ok", 0x4fa36a),
        Status::Draining => named("led-warn", 0xe0a030),
        Status::Offline => named("led-off", 0x4a4f5c),
        Status::Revoked => named("led-crit", 0xe8603c),
    }
}

/// A built model: its scene root, its parts by id, and the state that `update` drives.
pub struct Built {
    pub root: Obj,
    /// Every built part with an id (a repeated part maps to its group of copies).
    pub parts: HashMap<String, Obj>,
    live: Vec<Live>,
    t: f32,
}

struct Builder {
    vals: BTreeMap<String, f64>,
    parts: HashMap<String, Obj>,
    live: Vec<Live>,
    depth: u32,
}

impl Builder {
    fn add_parts(&mut self, parent: &Obj, list: &[Part]) {
        for p in list {
            if let Some(expr) = &p.when {
                // Validated models only: an unparsable `when` hides the part.
                let shown = parse_when(expr).map(|w| eval_when(&w, &self.vals)).unwrap_or(false);
                if !shown {
                    continue;
                }
            }
            let node = self.part(p);
            parent.add(&node);
        }
    }

    fn part(&mut self, p: &Part) -> Obj {
        let at = Vec3::from(p.at.unwrap_or([0.0; 3]));
        let node = match &p.repeat {
            Some(rep) => {
                let group = Obj::group();
                // A named count reads its param now; a missing or negative one repeats nothing. "p/n" is the param over n,
                // rounded up. At most 64 copies, as in the browser loader.
                let n = match &rep.count {
                    Count::Num(n) => *n,
                    Count::Param(s) => split_count(s)
                        .and_then(|(name, div)| self.vals.get(name).map(|v| (v.round().max(0.0) as u32).div_ceil(div)))
                        .unwrap_or(0),
                };
                for i in 0..n.min(64) {
                    let copy = self.node(p, at + repeat_offset(rep, i));
                    group.add(&copy);
                }
                group
            }
            None => self.node(p, at),
        };
        if let Some(id) = &p.id {
            self.parts.insert(id.clone(), node.clone());
        }
        node
    }

    /// One copy of a part at `at`: its geometry, transform, outline, children, label, and its
    /// animation record when it has a tag or an animation.
    fn node(&mut self, p: &Part, at: Vec3) -> Obj {
        let flat_geo = matches!(p.geo, Some(Geo::Plane { .. } | Geo::Ring { .. }));
        let mat = p.geo.as_ref().map(|_| material(p.mat.as_ref()));
        let obj = match (&p.geo, &mat) {
            (Some(g), Some(m)) => Obj::mesh(&geometry_of(g), m),
            _ => Obj::group(),
        };
        let rot = p.rot.unwrap_or([0.0; 3]);
        let rest_q = Quat::from_euler(EulerRot::XYZ, rot[0].to_radians(), rot[1].to_radians(), rot[2].to_radians());
        obj.set_position(at.x, at.y, at.z).set_quaternion(rest_q);
        match p.scale {
            Some(Scale::Uniform(s)) => {
                obj.set_scalar(s);
            }
            Some(Scale::Xyz([x, y, z])) => {
                obj.set_scale(x, y, z);
            }
            None => {}
        }
        if !p.outline.unwrap_or(!flat_geo) {
            obj.no_outline();
        }

        if let Some(name) = &p.use_ {
            if self.depth < MAX_USE_DEPTH {
                if let Some(m) = model(name) {
                    self.depth += 1;
                    self.add_parts(&obj, &m.parts);
                    self.depth -= 1;
                }
            }
        }
        self.add_parts(&obj, &p.children);
        if let Some(label) = &p.label {
            obj.add(&label_node(label, top_of(p.geo.as_ref())));
        }

        let tag = p.tag.as_deref().and_then(tag_of);
        if tag.is_some() || p.anim.is_some() {
            let anim = p.anim.as_ref();
            let spin = anim.and_then(|a| a.spin.as_ref());
            self.live.push(Live {
                obj: obj.clone(),
                mat,
                tag,
                glow: glow_of(p.mat.as_ref()),
                rest: at,
                rest_q,
                spin_axis: spin.map(|s| axis_of(&s.axis)).unwrap_or(Vec3::Y),
                spin_rps: spin.map(|s| s.rps).unwrap_or(0.0),
                pulse: anim.and_then(|a| a.pulse).map(|pu| pu.period.max(0.01)),
                blink: anim.and_then(|a| a.blink).map(|b| (b.period.max(0.01), b.phase)),
                slide: anim.and_then(|a| a.slide.as_ref()).map(|s| (Vec3::from(s.to), s.on == "enter")),
                swing: anim
                    .and_then(|a| a.swing.as_ref())
                    .map(|s| (axis_of(&s.axis), s.to.to_radians(), s.on == "enter")),
                angle: 0.0,
            });
        }
        obj
    }
}

/// Builds a model's scene. `params` override the model's own `params` (a missing param is 0).
/// Parts are toon-shaded with ink outlines unless `flat`; `repeat`, `when`, `use`, `children`,
/// `scale`, `rot` and `label` are applied as the schema says.
pub fn build(model: &Model, params: &Params) -> Built {
    let mut vals = model.params.clone();
    vals.extend(params.values.iter().map(|(k, v)| (k.clone(), *v)));
    let mut b = Builder { vals, parts: HashMap::new(), live: Vec::new(), depth: 0 };
    let root = Obj::group();
    b.add_parts(&root, &model.parts);
    outline(&root, OUTLINE_MIN);
    if !b.live.is_empty() {
        // Animated and tagged parts change after the scene is built: never bake them.
        root.set_dynamic(true);
    }
    Built { root, parts: b.parts, live: b.live, t: 0.0 }
}

impl Built {
    /// Advances the tags and animations by `dt` seconds.
    ///
    /// Tags: `status` LEDs take the status colour; `activity` LEDs blink while the machine is busy
    /// (`activity > 0`); `fan` spins when online, faster with `activity`; `screen` glows with
    /// activity. Anims: `spin`, `pulse`, `blink`, and `slide` / `swing` follow `open`.
    pub fn update(&mut self, dt: f32, state: &State) {
        self.t += dt;
        let t = self.t;
        let open = state.open.clamp(0.0, 1.0);
        let activity = state.activity.clamp(0.0, 1.0);
        let online = state.status == Status::Online;
        let busy = activity > 0.0;
        let led_ok = named("led-ok", 0x4fa36a);
        let led_off = named("led-off", 0x4a4f5c);

        for l in self.live.iter_mut() {
            if let Some((to, on_enter)) = l.slide {
                let k = if on_enter { open } else { 0.0 };
                let p = l.rest.lerp(to, k);
                l.obj.set_position(p.x, p.y, p.z);
            }
            let mut q = l.rest_q;
            if let Some((axis, to, on_enter)) = l.swing {
                let k = if on_enter { open } else { 0.0 };
                q *= Quat::from_axis_angle(axis, to * k);
            }
            let rps = match l.tag {
                Some(Tag::Fan) if online => 2.0 * (1.0 + 2.0 * activity),
                Some(Tag::Fan) => 0.0,
                _ => l.spin_rps,
            };
            if rps != 0.0 {
                l.angle = (l.angle + rps * TAU * dt) % TAU;
                q *= Quat::from_axis_angle(l.spin_axis, l.angle);
            }
            l.obj.set_quaternion(q);

            let Some(mat) = &l.mat else { continue };
            let mut m = mat.get();
            if let Some(period) = l.pulse {
                m.emissive_intensity = l.glow.max(0.6) * (0.5 + 0.5 * (TAU * t / period).sin());
            }
            if let Some((period, phase)) = l.blink {
                let on = ((t + phase) % period) < period * 0.5;
                m.emissive_intensity = if on { l.glow } else { l.glow * 0.1 };
            }
            match l.tag {
                Some(Tag::Status) => {
                    let c = status_color(state.status);
                    m.color = c;
                    m.emissive = c;
                    m.emissive_intensity = if l.glow > 0.0 { l.glow } else { 1.0 };
                }
                Some(Tag::Activity) => {
                    let on = busy && (t % 0.8) < 0.4;
                    let c = if on { led_ok } else { led_off };
                    m.color = c;
                    m.emissive = c;
                    m.emissive_intensity = if on { 1.5 } else { 0.1 };
                }
                Some(Tag::Screen) => m.emissive_intensity = l.glow * (0.5 + 0.5 * activity),
                _ => {}
            }
        }
    }
}

// ---- Traces and pulses ----------------------------------------------------------------------

/// The copper of every trace: one merged mesh of a thin strip per segment, lying at the trace's
/// `y`. Joints overlap by half a width at each end.
pub fn traces(model: &Model) -> Obj {
    let group = Obj::group();
    let mut pieces: Vec<(scene::Geo, Mat4)> = Vec::new();
    for tr in &model.traces {
        for w in tr.path.windows(2) {
            let (a, b) = (w[0], w[1]);
            let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
            let len = (dx * dx + dz * dz).sqrt();
            if len <= 1e-6 {
                continue;
            }
            let mid = Vec3::new((a[0] + b[0]) * 0.5, tr.y, (a[1] + b[1]) * 0.5);
            let rot = Mat4::from_rotation_y((-dz).atan2(dx));
            let strip = geometry::box_geo(len + tr.width, 0.04, tr.width);
            pieces.push((strip, Mat4::from_translation(mid) * rot));
        }
    }
    if pieces.is_empty() {
        return group;
    }
    let copper = Mat::new(Material { color: named("copper", 0xb87333), ..Material::default() });
    group.add(&Obj::mesh(&scene::merge(&pieces), &copper));
    group
}

fn path_len(path: &[[f32; 2]]) -> f32 {
    path.windows(2).map(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt()).sum()
}

/// The point `d` units along a polyline (clamped to its end).
fn point_at(path: &[[f32; 2]], mut d: f32) -> [f32; 2] {
    for w in path.windows(2) {
        let (a, b) = (w[0], w[1]);
        let l = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        if d <= l {
            let t = if l > 0.0 { d / l } else { 0.0 };
            return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
        }
        d -= l;
    }
    path.last().copied().unwrap_or([0.0, 0.0])
}

struct Run {
    path: Vec<[f32; 2]>,
    y: f32,
    len: f32,
    phase: f32,
    speed: f32,
    /// Index among this trace's pulses, and how many it has: pulses light up with activity.
    k: usize,
    n: usize,
    sphere: Obj,
}

/// Current pulses: small additive spheres running along the traces. More of them are visible as
/// the machine gets busier (`update`'s `activity`).
pub struct Pulses {
    /// The group holding the spheres; add it to the scene.
    pub obj: Obj,
    runs: Vec<Run>,
}

impl Pulses {
    pub fn new(model: &Model) -> Pulses {
        let obj = Obj::group();
        obj.set_dynamic(true);
        let mat = Mat::new(Material {
            shading: Shading::Basic,
            color: Color::hex(0x7fe8ff),
            transparent: true,
            opacity: 0.9,
            additive: true,
            depth_write: false,
            ..Material::default()
        });
        let geo = geometry::sphere(0.14, 8, 6);
        let mut runs = Vec::new();
        for (i, tr) in model.traces.iter().enumerate() {
            let len = path_len(&tr.path);
            if len <= 0.0 {
                continue;
            }
            let n = ((len / 8.0).ceil() as usize).clamp(1, 4);
            for k in 0..n {
                let sphere = Obj::mesh(&geo, &mat);
                obj.add(&sphere);
                runs.push(Run {
                    path: tr.path.clone(),
                    y: tr.y,
                    len,
                    phase: k as f32 / n as f32,
                    speed: 4.0 + (i % 3) as f32,
                    k,
                    n,
                    sphere,
                });
            }
        }
        Pulses { obj, runs }
    }

    /// Moves the pulses by `dt` seconds; `activity` (0..1) sets how many show.
    pub fn update(&mut self, dt: f32, activity: f32) {
        let a = activity.clamp(0.0, 1.0);
        for r in self.runs.iter_mut() {
            r.phase = (r.phase + r.speed * dt / r.len).fract();
            let [x, z] = point_at(&r.path, r.phase * r.len);
            r.sphere.set_position(x, r.y, z);
            r.sphere.set_visible((r.k as f32) < a * r.n as f32);
        }
    }
}

// ---- The atom -------------------------------------------------------------------------------

/// The rest height of the atom inside `props::build_plaza` (its globe's y).
const ATOM_REST_Y: f32 = 4.2;

/// Places the plaza's atom at `anchors.atom` under `root`: scale 0.15 on an exterior, 1.0 on a
/// board. The atom is the same one the plaza draws (props.rs has no standalone atom builder, so it
/// is taken from `build_plaza`, which adds it last). Returns its animation, which the caller must
/// call every frame with the elapsed time. `None` when the model has no atom anchor, or is a part.
pub fn place_atom(model: &Model, root: &Obj, palette: &PropPalette) -> Option<props::Anim> {
    let at = model.anchors.atom?;
    let scale = match model.view {
        View::Exterior => 0.15,
        View::Board => 1.0,
        View::Part => return None,
    };
    let (plaza, anim) = props::build_plaza(palette);
    let globe = plaza.get().children.last().cloned()?;
    plaza.remove(&globe);
    let wrap = Obj::group();
    wrap.set_scalar(scale).set_position(at[0], at[1] - ATOM_REST_Y * scale, at[2]);
    wrap.add(&globe);
    root.add(&wrap);
    Some(anim)
}

// ---- Tests ----------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn plaza_palette() -> PropPalette {
        PropPalette {
            sheet: Color::hex(0xf5efe6),
            rule: Color::hex(0xc9bfae),
            ink3: Color::hex(0x6b6357),
            signal: Color::hex(0xe8603c),
            ok: Color::hex(0x4fa36a),
            info: Color::hex(0x3c7fd6),
        }
    }

    /// Exercises every schema feature: repeat, when, a tag with spin, a label, a swing and a
    /// pulse, a trace, a slot, and inline materials only (no palette names).
    const SAMPLE: &str = r##"{
      "schema": 1, "kind": "workstation", "view": "board", "size": [40, 2, 30],
      "params": { "gpus": 1, "p_cores": 2 },
      "parts": [
        { "id": "pcb", "geo": { "type": "box", "w": 40, "h": 0.4, "d": 30 }, "at": [0, 0.2, 0],
          "mat": { "color": "#1f6b3a" } },
        { "id": "gpu0", "geo": { "type": "rbox", "w": 10, "h": 2, "d": 4, "r": 0.2 }, "at": [10, 1.2, 0],
          "rot": [0, 0, 90], "scale": 1.5, "mat": { "color": "#333333", "glow": 1, "flat": true },
          "tag": "fan", "anim": { "spin": { "axis": "y", "rps": 2 } }, "label": { "text": "GPU0" } },
        { "id": "dimm", "geo": { "type": "box", "w": 0.6, "h": 1, "d": 0.1 }, "at": [-6, 1, -4],
          "repeat": { "count": 4, "step": [1, 0, 0] }, "tag": "status",
          "mat": { "color": "#333333", "emissive": "#4fa36a", "glow": 1, "additive": true } },
        { "id": "door", "geo": { "type": "sphere", "r": 0.5 }, "at": [0, 2, 0],
          "anim": { "swing": { "axis": "x", "to": -110, "on": "enter" }, "pulse": { "period": 1.6 },
                    "blink": { "period": 0.8 } } },
        { "id": "core", "geo": { "type": "box", "w": 0.5, "h": 0.5, "d": 0.5 }, "at": [0, 1, 6],
          "repeat": { "count": "p_cores", "step": [1, 0, 0] }, "when": "gpu_cores >= 0" },
        { "id": "gpu-extra", "geo": { "type": "plane", "w": 2, "d": 2 }, "at": [0, 0.5, 0],
          "when": "gpus >= 2", "mat": { "color": "#ffffff" } }
      ],
      "anchors": { "camera": { "at": [0, 40, 30], "look": [0, 0, 0] }, "atom": [0, 4, 0], "open": "door" },
      "slots": [ { "id": "cpu0", "on": "pcb", "type": "cpu", "top": 0.4, "rect": [-4, -4, 8, 8], "capacity": 4 } ],
      "traces": [ { "path": [[0, 0], [5, 0], [5, 5]], "y": 0.06, "width": 0.25 } ]
    }"##;

    #[test]
    fn every_embedded_model_parses_and_validates() {
        for (name, src) in MODELS.iter().chain(PARTS.iter()) {
            let m: Model = parse(src).unwrap_or_else(|e| panic!("{name}: {e}"));
            let errs = validate(&m, name);
            assert!(errs.is_empty(), "{errs:?}");
        }
    }

    #[test]
    fn embedded_models_are_found_by_name() {
        for (name, _) in MODELS {
            assert!(model(name).is_some(), "{name} does not resolve");
        }
        assert!(model("exterior/nope").is_none());
    }

    #[test]
    fn palette_source_parses() {
        let pal = serde_json::from_str::<PaletteFile>(PALETTE_SRC).expect("palette.json parses");
        assert!(pal.colors.contains_key("pcb") && pal.colors.contains_key("led-ok"));
        for (name, m) in &pal.colors {
            for c in [&m.color, &m.emissive].into_iter().flatten() {
                assert!(Color::css(c).is_some(), "palette {name}: bad colour {c}");
            }
        }
    }

    #[test]
    fn when_expressions() {
        let vals = BTreeMap::from([("gpus".to_string(), 1.0)]);
        let eval = |s: &str| eval_when(&parse_when(s).unwrap(), &vals);
        assert!(eval("gpus"));
        assert!(!eval("!gpus"));
        assert!(eval("gpus >= 1"));
        assert!(!eval("gpus >= 2"));
        assert!(eval("gpus < 2"));
        assert!(!eval("missing"));
        assert!(parse_when("gpus >> 1").is_err());
        assert!(parse_when("").is_err());
        assert!(parse_when("1x").is_err());
    }

    #[test]
    fn when_accepts_any_numeric_param() {
        let vals = BTreeMap::from([("gpu_cores".to_string(), 32.0), ("memory_gib".to_string(), 16.0)]);
        let eval = |s: &str| eval_when(&parse_when(s).unwrap(), &vals);
        assert!(eval("gpu_cores >= 30"));
        assert!(!eval("memory_gib >= 32"));
    }

    #[test]
    fn models_for_picks_the_family() {
        assert_eq!(models_for("laptop", Some("apple")), ("exterior/apple-laptop", "board/apple-soc"));
        assert_eq!(models_for("workstation", Some("apple")), ("exterior/apple-desktop", "board/apple-soc"));
        assert_eq!(models_for("server", None), ("exterior/server", "board/server"));
        assert_eq!(models_for("laptop", Some("intel")), ("exterior/laptop", "board/laptop"));
        assert_eq!(models_for("toaster", None), ("exterior/workstation", "board/workstation"));
        for kind in ["workstation", "laptop", "edge", "phone", "server"] {
            let (e, b) = models_for(kind, None);
            assert!(model(e).is_some() && model(b).is_some(), "{kind}");
        }
        let (e, b) = models_for("laptop", Some("apple"));
        assert!(model(e).is_some() && model(b).is_some());
        let (e, b) = models_for("workstation", Some("apple"));
        assert!(model(e).is_some() && model(b).is_some());
    }

    #[test]
    fn validate_reports_the_schema_rules() {
        let bad = r##"{
          "schema": 1, "kind": "x", "view": "board", "size": [1, 1, 1],
          "parts": [
            { "id": "a", "geo": { "type": "box", "w": 1, "h": 1, "d": 1 } },
            { "id": "a", "geo": { "type": "box", "w": 1, "h": 1, "d": 1 }, "use": "parts/none" },
            { "id": "b", "when": "gpus >>", "tag": "nope" }
          ],
          "slots": [ { "id": "s", "on": "ghost", "type": "cpu", "top": 1, "rect": [0, 0, 1, 1] } ],
          "traces": [ { "path": [[0, 0]] } ]
        }"##;
        let m: Model = parse(bad).unwrap();
        let errs = validate(&m, "bad");
        for needle in ["duplicate id", "unknown part", "bad when", "unknown tag", "missing part", "two points"] {
            assert!(errs.iter().any(|e| e.contains(needle)), "no '{needle}' in {errs:?}");
        }
    }

    #[test]
    fn repeat_wrap_lays_copies_in_rows() {
        // The same numbers as the browser loader's test: count 10, wrap 4, step [1, 0, 0], row [0, 0, 1].
        let r = Repeat { count: Count::Num(10), step: [1.0, 0.0, 0.0], wrap: Some(4), row: Some([0.0, 0.0, 1.0]) };
        let offsets: Vec<[f32; 3]> = (0..10).map(|i| repeat_offset(&r, i).to_array()).collect();
        let expected = [
            [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [3.0, 0.0, 0.0], [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0], [2.0, 0.0, 1.0], [3.0, 0.0, 1.0], [0.0, 0.0, 2.0], [1.0, 0.0, 2.0],
        ];
        assert_eq!(offsets, expected);
        // Without a wrap the copies stay in one line.
        let line = Repeat { count: Count::Num(3), step: [2.0, 0.0, 0.0], wrap: None, row: None };
        let line_offsets: Vec<[f32; 3]> = (0..3).map(|i| repeat_offset(&line, i).to_array()).collect();
        assert_eq!(line_offsets, [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [4.0, 0.0, 0.0]]);
    }

    #[test]
    fn repeat_counts_name_a_param_or_its_share() {
        assert_eq!(split_count("p_cores"), Some(("p_cores", 1)));
        assert_eq!(split_count("p_cores/4"), Some(("p_cores", 4)));
        for bad in ["p/0", "p/", "p/+3", "p/04", "p/x", "9p", "p-x", ""] {
            assert_eq!(split_count(bad), None, "{bad}");
        }
    }

    #[test]
    fn repeat_wrap_row_and_count_are_checked_and_built() {
        let src = r##"{
          "schema": 1, "kind": "x", "view": "part", "size": [1, 1, 1], "params": { "n": 4 },
          "parts": [
            { "id": "a", "geo": { "type": "box", "w": 1, "h": 1, "d": 1 }, "repeat": { "count": "n/2", "step": [1, 0, 0], "wrap": 2 } },
            { "id": "b", "geo": { "type": "box", "w": 1, "h": 1, "d": 1 }, "repeat": { "count": 2, "step": [1, 0, 0], "row": [0, 0, 1] } },
            { "id": "c", "geo": { "type": "box", "w": 1, "h": 1, "d": 1 }, "repeat": { "count": "n/x", "step": [1, 0, 0], "wrap": 0, "row": [0, 0, 1] } }
          ]
        }"##;
        let m: Model = parse(src).unwrap();
        let errs = validate(&m, "repeats");
        for needle in ["needs a row", "row needs a wrap", "param name or 'param/n'", "wrap must be at least 1"] {
            assert!(errs.iter().any(|e| e.contains(needle)), "no '{needle}' in {errs:?}");
        }
        // n = 4 over 2 is two copies of "a"; a bad count builds nothing and a bad wrap does not panic.
        let built = build(&m, &Params::default());
        assert_eq!(built.parts.get("a").unwrap().get().children.len(), 2);
        assert_eq!(built.parts.get("b").unwrap().get().children.len(), 2);
    }

    #[test]
    fn builds_every_embedded_model() {
        let pal = plaza_palette();
        for (name, _) in MODELS {
            let m = model(name).unwrap();
            let mut built = build(m, &Params::default());
            assert!(!built.root.get().children.is_empty() || m.parts.is_empty(), "{name} built empty");
            let _ = place_atom(m, &built.root, &pal);
            built.update(0.016, &State::default());
            let _ = traces(m);
            let mut pulses = Pulses::new(m);
            pulses.update(0.016, 1.0);
        }
    }

    #[test]
    fn sample_builds_with_params_and_animates() {
        let m: Model = parse(SAMPLE).unwrap();
        let errs = validate(&m, "sample");
        assert!(errs.is_empty(), "{errs:?}");

        // The `gpus >= 2` part is hidden with the default gpus = 1.
        let one = build(&m, &Params::default());
        assert!(one.parts.contains_key("gpu0") && one.parts.contains_key("dimm"));
        assert!(!one.parts.contains_key("gpu-extra"));

        // A named repeat count reads the model's param: p_cores = 2 gives two copies.
        let core = one.parts.get("core").unwrap().clone();
        assert_eq!(core.get().children.len(), 2);

        let mut two = build(&m, &Params::default().with("gpus", 2.0));
        assert!(two.parts.contains_key("gpu-extra"));
        assert!(!two.live.is_empty());
        // The swing turns the door by its `to` angle (110 degrees) as the opening runs.
        let door = two.parts.get("door").unwrap().clone();
        two.update(0.0, &State { open: 0.0, ..State::default() });
        let closed = door.get().quaternion.unwrap_or(Quat::IDENTITY);
        two.update(0.0, &State { open: 1.0, ..State::default() });
        let open_q = door.get().quaternion.unwrap_or(Quat::IDENTITY);
        assert!(closed.angle_between(open_q) > 1.5, "the swing did not turn the door");

        for state in [
            State { status: Status::Online, activity: 0.5, open: 1.0 },
            State { status: Status::Revoked, activity: 0.0, open: 0.3 },
            State { status: Status::Offline, activity: 1.0, open: 0.0 },
        ] {
            two.update(0.5, &state);
        }

        let t = traces(&m);
        assert!(!t.get().children.is_empty());
    }

    #[test]
    fn point_at_walks_the_polyline() {
        let path = [[0.0, 0.0], [4.0, 0.0], [4.0, 3.0]];
        assert!((path_len(&path) - 7.0).abs() < 1e-5);
        assert_eq!(point_at(&path, 2.0), [2.0, 0.0]);
        assert_eq!(point_at(&path, 6.0), [4.0, 2.0]);
        assert_eq!(point_at(&path, 99.0), [4.0, 3.0]);
    }
}
