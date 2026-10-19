//! AgentPlaza's themes, written once for both consoles.
//!
//! A theme has three parts that are chosen on their own: a [`skin::Skin`] (shape and type: Loom or Graphite), a
//! palette (every colour token for light and dark) and a [`background::Background`]. A theme file
//! (`.apztheme.json`, [`ThemeFile`]) saves one combination. [`resolve`] turns a file, a skin and a mode into the
//! full token set: missing tokens are derived in OKLCH ([`derive`]), then the contrast pass moves any text or mark
//! that does not reach 4.5:1 or 3:1 on its grounds ([`tokens::CHECKS`]) and reports what it moved.
//!
//! * [`tokens`] — the token list and the contrast rules.
//! * [`color`] — parsing, OKLab/OKLCH, mixing, WCAG contrast, the contrast fix.
//! * [`skin`], [`background`] — the built-in skins and background presets.
//! * [`builtin`] — the built-in themes, read from `apps/shared/themes/*.apztheme.json`.
//! * [`appearance`] — the owner's choice the hub stores (`/v1/appearance`).
//! * [`generate`] — the generated files both consoles read (`themes.gen.json`, `themes.gen.css`).
//! * [`vscode`] — VS Code theme import (JSON with comments, include chains, `.vsix`).

#![forbid(unsafe_code)]

pub mod appearance;
pub mod background;
pub mod builtin;
pub mod color;
pub mod derive;
pub mod generate;
pub mod skin;
pub mod tokens;
pub mod vscode;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use appearance::{Appearance, ModeChoice};
pub use background::Background;
pub use color::Rgba;
pub use skin::Skin;

/// The file format's version.
pub const SCHEMA: u32 = 1;

/// Light or dark.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Light,
    Dark,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Light => "light",
            Mode::Dark => "dark",
        }
    }
}

/// A palette as written in a file: token name → colour text.
pub type PaletteText = BTreeMap<String, String>;

/// Colours for light and/or dark.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Palettes {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light: Option<PaletteText>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dark: Option<PaletteText>,
}

impl Palettes {
    pub fn get(&self, mode: Mode) -> Option<&PaletteText> {
        match mode {
            Mode::Light => self.light.as_ref(),
            Mode::Dark => self.dark.as_ref(),
        }
    }
}

/// A background in a theme file: a preset's id, or a full spec.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BackgroundChoice {
    Preset(String),
    Spec(Background),
}

impl BackgroundChoice {
    pub fn resolve(&self) -> Option<Background> {
        match self {
            BackgroundChoice::Preset(id) => background::preset_by_id(id),
            BackgroundChoice::Spec(b) => Some(b.clone()),
        }
    }
}

/// A `.apztheme.json` file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThemeFile {
    pub schema: u32,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub about: String,
    pub skin: String,
    #[serde(default)]
    pub palette: Palettes,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<BackgroundChoice>,
    /// Applied after derivation, before the contrast pass.
    #[serde(default, skip_serializing_if = "palettes_empty")]
    pub overrides: Palettes,
    /// Where it came from ("vscode: One Dark Pro"), shown in Appearance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

fn palettes_empty(p: &Palettes) -> bool {
    p.light.as_ref().is_none_or(|m| m.is_empty()) && p.dark.as_ref().is_none_or(|m| m.is_empty())
}

impl ThemeFile {
    /// Parses and validates a theme file.
    pub fn parse(text: &str) -> Result<ThemeFile, String> {
        let file: ThemeFile = serde_json::from_str(text).map_err(|e| format!("not a theme file: {e}"))?;
        let problems = file.problems();
        if problems.is_empty() {
            Ok(file)
        } else {
            Err(problems.join("; "))
        }
    }

    /// Problems with the file, empty when it is valid.
    pub fn problems(&self) -> Vec<String> {
        let mut out = vec![];
        if self.schema != SCHEMA {
            out.push(format!("schema {} is not supported (this build reads {SCHEMA})", self.schema));
        }
        if self.id.is_empty()
            || self.id.len() > 64
            || !self.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            out.push("id must be 1–64 characters of a–z, 0–9 and -".into());
        }
        if self.name.trim().is_empty() || self.name.chars().count() > 80 {
            out.push("name must be 1–80 characters".into());
        }
        if skin::skin(&self.skin).is_none() {
            out.push(format!("unknown skin {:?}", self.skin));
        }
        if self.palette.light.is_none() && self.palette.dark.is_none() {
            out.push("a theme needs a light or a dark palette".into());
        }
        for (part, p) in [("palette", &self.palette), ("overrides", &self.overrides)] {
            for (mode, map) in [("light", &p.light), ("dark", &p.dark)] {
                for (name, value) in map.iter().flatten() {
                    if !tokens::is_token(name) {
                        out.push(format!("{part}.{mode}: unknown token {name:?}"));
                    } else if Rgba::parse(value).is_none() {
                        out.push(format!("{part}.{mode}.{name}: {value:?} is not a colour"));
                    }
                }
            }
        }
        match &self.background {
            Some(BackgroundChoice::Preset(id)) if background::preset_by_id(id).is_none() => {
                out.push(format!("unknown background preset {id:?}"));
            }
            Some(BackgroundChoice::Spec(b)) => out.extend(b.problems()),
            _ => {}
        }
        out
    }

    /// The modes this file has colours for.
    pub fn modes(&self) -> Vec<Mode> {
        let mut m = vec![];
        if self.palette.light.is_some() {
            m.push(Mode::Light);
        }
        if self.palette.dark.is_some() {
            m.push(Mode::Dark);
        }
        m
    }
}

/// A full, resolved token set. Every token in [`tokens::TOKENS`] is present.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Tokens(pub BTreeMap<String, Rgba>);

impl Tokens {
    /// The token's colour; magenta for a name that is not there, so a mistake shows.
    pub fn get(&self, name: &str) -> Rgba {
        self.0.get(name).copied().unwrap_or(Rgba::rgb(1.0, 0.0, 1.0))
    }

    pub fn set(&mut self, name: &str, c: Rgba) {
        self.0.insert(name.to_string(), c);
    }

    pub fn has(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }
}

/// One colour the contrast pass moved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Adjusted {
    pub token: String,
    pub from: Rgba,
    pub to: Rgba,
    /// The ground it failed on, and the ratio it had there.
    pub against: String,
    pub ratio: f32,
    pub min: f32,
}

/// What resolving did.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    /// Tokens taken from the file.
    pub taken: usize,
    /// Tokens derived.
    pub derived: usize,
    pub adjusted: Vec<Adjusted>,
    /// Checks that still fail after the pass (a colour that cannot reach the ratio on all its grounds).
    pub failing: Vec<Adjusted>,
}

impl Report {
    /// "41 colours taken, 19 derived, 3 adjusted for contrast".
    pub fn summary(&self) -> String {
        let plural = |n: usize, one: &str, many: &str| if n == 1 { one.to_string() } else { many.to_string() };
        format!(
            "{} {} taken, {} derived, {} adjusted for contrast",
            self.taken,
            plural(self.taken, "colour", "colours"),
            self.derived,
            self.adjusted.len()
        )
    }
}

/// A theme resolved for one skin and mode: what a console applies.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Resolved {
    pub id: String,
    pub name: String,
    pub skin: Skin,
    /// The mode the tokens are for. A file with one palette resolves in that mode whatever was asked.
    pub mode: Mode,
    pub tokens: Tokens,
    pub background: Background,
    pub report: Report,
}

/// Resolves `file` for `skin` in `mode`: the file's palette, the derived rest, the overrides, then the contrast
/// pass. A file without a palette for `mode` resolves in the mode it has.
pub fn resolve(file: &ThemeFile, skin: &Skin, mode: Mode) -> Resolved {
    let mode = if file.palette.get(mode).is_some() {
        mode
    } else if file.palette.get(Mode::Light).is_some() {
        Mode::Light
    } else {
        Mode::Dark
    };
    let mut given = BTreeMap::new();
    for (name, value) in file.palette.get(mode).into_iter().flatten() {
        if let (true, Some(c)) = (tokens::is_token(name), Rgba::parse(value)) {
            given.insert(name.clone(), c);
        }
    }
    let taken = given.keys().filter(|k| !derive::SKIN_TOKENS.contains(&k.as_str())).count();
    let mut toks = derive::derive(&given, skin, mode);
    for (name, value) in file.overrides.get(mode).into_iter().flatten() {
        if let (true, Some(c)) = (tokens::is_token(name), Rgba::parse(value)) {
            toks.set(name, c.over(toks.get("paper")));
        }
    }
    let (adjusted, failing) = contrast_pass(&mut toks);
    let background = file
        .background
        .as_ref()
        .and_then(BackgroundChoice::resolve)
        .or_else(|| background::preset_by_id(&skin.background))
        .unwrap_or_default();
    Resolved {
        id: file.id.clone(),
        name: file.name.clone(),
        skin: skin.clone(),
        mode,
        tokens: toks,
        background,
        report: Report {
            taken,
            derived: tokens::TOKENS.len() - taken,
            adjusted,
            failing,
        },
    }
}

/// Runs [`tokens::CHECKS`] over `toks`, moving each failing foreground to the nearest passing colour. Returns
/// what moved and what still fails.
pub fn contrast_pass(toks: &mut Tokens) -> (Vec<Adjusted>, Vec<Adjusted>) {
    let mut adjusted = vec![];
    for check in tokens::CHECKS {
        let fg = toks.get(check.fg);
        let grounds: Vec<Rgba> = check.on.iter().map(|g| toks.get(g)).collect();
        if let Some(to) = color::fix_contrast(fg, &grounds, check.min) {
            let (against, ratio) = worst_ground(fg, check.on, toks);
            adjusted.push(Adjusted {
                token: check.fg.into(),
                from: fg,
                to,
                against,
                ratio,
                min: check.min,
            });
            toks.set(check.fg, to);
        }
    }
    let failing = check_all(toks);
    (adjusted, failing)
}

fn worst_ground(fg: Rgba, on: &[&str], toks: &Tokens) -> (String, f32) {
    on.iter()
        .map(|g| ((*g).to_string(), fg.quantized().contrast(toks.get(g))))
        .fold((String::new(), f32::INFINITY), |a, b| if b.1 < a.1 { b } else { a })
}

/// Every check that fails on `toks` as they stand.
pub fn check_all(toks: &Tokens) -> Vec<Adjusted> {
    let mut out = vec![];
    for check in tokens::CHECKS {
        let fg = toks.get(check.fg);
        let (against, ratio) = worst_ground(fg, check.on, toks);
        if ratio < check.min {
            out.push(Adjusted {
                token: check.fg.into(),
                from: fg,
                to: fg,
                against,
                ratio,
                min: check.min,
            });
        }
    }
    out
}
