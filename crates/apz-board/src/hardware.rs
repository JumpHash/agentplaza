//! Schema 1 of the hardware JSON (assets/hardware), as serde types. They mirror the TypeScript types and the
//! validator in apps/web/src/views/floor3d/hardware.ts; `deny_unknown_fields` follows the validator's key lists.
//!
//! The embedded files (see build.rs) are parsed once into a [`Library`]. Nothing here does I/O.

// The schema mirrors every field of the files (the validator's key lists). The geometry reads only some of them;
// the rest (labels, animations, anchors, slots, traces) are parsed so that a file with them still loads.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded.rs"));
}

/// A 3-vector: x, y, z (degrees for rotations).
pub type Vec3 = [f64; 3];

/// A material: a colour, optionally glowing, faded, additive, or unlit.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Material {
    pub color: String,
    pub emissive: Option<String>,
    pub glow: Option<f64>,
    pub opacity: Option<f64>,
    pub additive: Option<bool>,
    pub flat: Option<bool>,
}

/// A part's material: a palette name, or an inline material.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum MatRef {
    Name(String),
    Inline(Material),
}

/// A part's geometry, tagged by `type`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum Geo {
    #[serde(rename = "box")]
    Cube {
        w: f64,
        h: f64,
        d: f64,
    },
    Rbox {
        w: f64,
        h: f64,
        d: f64,
        r: f64,
    },
    Cyl {
        rt: f64,
        rb: f64,
        h: f64,
        seg: Option<u32>,
    },
    Cone {
        r: f64,
        h: f64,
        seg: Option<u32>,
    },
    Sphere {
        r: f64,
    },
    Torus {
        r: f64,
        tube: f64,
        arc: Option<f64>,
    },
    Plane {
        w: f64,
        d: f64,
    },
    Ring {
        ri: f64,
        ro: f64,
    },
    /// A flat outline of [x, z] points, extruded up by h.
    Shape {
        points: Vec<[f64; 2]>,
        h: f64,
    },
}

/// A scale: one number for all axes, or [x, y, z].
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Scale {
    Uniform(f64),
    Xyz(Vec3),
}

/// A repeat's count: a whole number 1..256, or a param's name ("p_cores"), or "p_cores/4".
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Count {
    N(u32),
    Param(String),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repeat {
    pub count: Count,
    pub step: Vec3,
    pub wrap: Option<u32>,
    pub row: Option<Vec3>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Label {
    pub text: String,
    pub size: Option<f64>,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Tag {
    Status,
    Activity,
    Fan,
    Screen,
}

/// Animations. Not used for geometry; their presence matters for how a repeat is built.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anim {
    pub spin: Option<Spin>,
    pub pulse: Option<Pulse>,
    pub blink: Option<Blink>,
    pub slide: Option<Slide>,
    pub swing: Option<Swing>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spin {
    pub axis: String,
    pub rps: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pulse {
    pub period: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Blink {
    pub period: f64,
    pub phase: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slide {
    pub to: Vec3,
    pub on: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Swing {
    pub axis: String,
    pub to: f64,
    pub on: String,
}

/// One part: a geometry or a `use` of another part file, placed by `at`, `rot` and `scale`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Part {
    pub id: Option<String>,
    pub geo: Option<Geo>,
    #[serde(rename = "use")]
    pub use_: Option<String>,
    pub at: Option<Vec3>,
    pub rot: Option<Vec3>,
    pub scale: Option<Scale>,
    pub mat: Option<MatRef>,
    pub outline: Option<bool>,
    pub repeat: Option<Repeat>,
    pub when: Option<String>,
    pub anim: Option<Anim>,
    pub tag: Option<Tag>,
    pub label: Option<Label>,
    pub detail: Option<u8>,
    pub cutaway: Option<bool>,
    /// Fixed by something other than resting on what is under it: it does not rest (no item) but still holds things up.
    pub attached: Option<bool>,
    pub children: Option<Vec<Part>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Camera {
    pub at: Vec3,
    pub look: Vec3,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchors {
    pub label: Option<Vec3>,
    pub port: Option<Vec3>,
    pub camera: Option<Camera>,
    pub open: Option<String>,
    pub atom: Option<Vec3>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slot {
    pub id: String,
    pub on: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub top: f64,
    pub rect: [f64; 4],
    pub capacity: u32,
    pub occluded: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trace {
    pub path: Vec<[f64; 2]>,
    pub y: f64,
    pub width: f64,
    pub from: Option<String>,
    pub to: Option<String>,
}

/// A model file (a board or a part): its size, its default params and its parts.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub schema: u32,
    pub kind: String,
    pub view: String,
    pub size: Vec3,
    pub params: Option<BTreeMap<String, f64>>,
    pub parts: Vec<Part>,
    pub anchors: Option<Anchors>,
    pub slots: Option<Vec<Slot>>,
    pub traces: Option<Vec<Trace>>,
}

/// palette.json: the named materials (the status LEDs among them).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaletteFile {
    pub schema: u32,
    pub colors: BTreeMap<String, Material>,
}

/// The material a part gets when it names none, or names a palette entry that is missing.
pub const DEFAULT_MATERIAL_COLOR: &str = "#9aa3b8";

/// The embedded files, parsed once.
pub struct Library {
    pub palette: BTreeMap<String, Material>,
    models: BTreeMap<String, Model>,
}

impl Library {
    /// The embedded library. Panics if an embedded file does not parse (the crate's tests check every file).
    pub fn get() -> &'static Library {
        static LIB: OnceLock<Library> = OnceLock::new();
        LIB.get_or_init(|| {
            Library::parse().unwrap_or_else(|e| panic!("apz-board: embedded hardware: {e}"))
        })
    }

    fn parse() -> Result<Library, String> {
        let mut palette = None;
        let mut models = BTreeMap::new();
        for (name, text) in embedded::EMBEDDED {
            if *name == "palette" {
                let file: PaletteFile =
                    serde_json::from_str(text).map_err(|e| format!("{name}: {e}"))?;
                palette = Some(file.colors);
            } else {
                let model: Model =
                    serde_json::from_str(text).map_err(|e| format!("{name}: {e}"))?;
                models.insert(name.to_string(), model);
            }
        }
        Ok(Library {
            palette: palette.ok_or("no palette.json")?,
            models,
        })
    }

    /// A model by its embedded name ("board/edge", "parts/gpu-pcie").
    pub fn model(&self, name: &str) -> Option<&Model> {
        self.models.get(name)
    }

    /// The names of the embedded models, in order.
    pub fn model_names(&self) -> impl Iterator<Item = &str> {
        self.models.keys().map(String::as_str)
    }
}

/// Every embedded file's name and source, in order (for the tests).
pub fn embedded_files() -> &'static [(&'static str, &'static str)] {
    embedded::EMBEDDED
}

// ---- The `when` expression -----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Ge,
    Le,
    Eq,
    Ne,
    Gt,
    Lt,
}

/// `name`, `!name`, or `name OP number` (OP one of >= <= == != > <).
#[derive(Debug, Clone, PartialEq)]
pub struct When {
    pub name: String,
    pub negate: bool,
    pub cond: Option<(Op, f64)>,
}

/// Parses a `when` expression as parseWhen does; None when it is not one of the forms above.
pub fn parse_when(expr: &str) -> Option<When> {
    let s = expr.trim();
    let (negate, rest) = match s.strip_prefix('!') {
        Some(r) => (true, r.trim_start()),
        None => (false, s),
    };
    let mut chars = rest.char_indices();
    let (_, first) = chars.next()?;
    if !(first.is_ascii_lowercase() || first == '_') {
        return None;
    }
    let mut end = first.len_utf8();
    for (i, c) in chars {
        if c.is_ascii_alphanumeric() || c == '_' {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    let name = &rest[..end];
    let tail = rest[end..].trim_start();
    if tail.is_empty() {
        return Some(When {
            name: name.to_string(),
            negate,
            cond: None,
        });
    }
    if negate {
        return None;
    }
    let (op, after) = [
        (">=", Op::Ge),
        ("<=", Op::Le),
        ("==", Op::Eq),
        ("!=", Op::Ne),
        (">", Op::Gt),
        ("<", Op::Lt),
    ]
    .iter()
    .find_map(|(t, op)| tail.strip_prefix(t).map(|r| (*op, r)))?;
    let num = after.trim_start();
    if !valid_number(num) {
        return None;
    }
    Some(When {
        name: name.to_string(),
        negate,
        cond: Some((op, num.parse().ok()?)),
    })
}

/// `-?\d+(\.\d+)?` and nothing else.
fn valid_number(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s, None),
    };
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    digits(int) && frac.is_none_or(digits)
}

/// Evaluates a `when` against the params (a missing param is 0), as evalWhen does.
pub fn eval_when(w: &When, params: &BTreeMap<String, f64>) -> bool {
    let v = params.get(&w.name).copied().unwrap_or(0.0);
    if w.negate {
        return v == 0.0;
    }
    match w.cond {
        None => v != 0.0,
        Some((op, n)) => match op {
            Op::Ge => v >= n,
            Op::Le => v <= n,
            Op::Eq => v == n,
            Op::Ne => v != n,
            Op::Gt => v > n,
            Op::Lt => v < n,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn when_evaluates_as_the_ts() {
        let mut p = BTreeMap::new();
        p.insert("gpus".to_string(), 2.0);
        let ev = |s: &str| eval_when(&parse_when(s).expect("parses"), &p);
        assert!(ev("gpus"));
        assert!(!ev("!gpus"));
        assert!(ev("gpus >= 2"));
        assert!(!ev("gpus > 2"));
        assert!(ev("missing == 0"));
        assert!(!ev("missing"));
        assert!(ev("!missing"));
    }
}
