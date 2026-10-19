//! The owner's appearance choice: one global choice the hub stores (`GET`/`PUT /v1/appearance`), which both
//! consoles apply, plus the owner's saved themes. A console may hold a per-device override that wins until cleared.

use serde::{Deserialize, Serialize};

use crate::{BackgroundChoice, Mode, Resolved, ThemeFile, builtin, resolve, skin};

/// System, light or dark.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeChoice {
    #[default]
    System,
    Light,
    Dark,
}

impl ModeChoice {
    /// The mode to draw in, given whether the system is light.
    pub fn mode(self, system_light: bool) -> Mode {
        match self {
            ModeChoice::Light => Mode::Light,
            ModeChoice::Dark => Mode::Dark,
            ModeChoice::System if system_light => Mode::Light,
            ModeChoice::System => Mode::Dark,
        }
    }
}

/// How dense lists and tables are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    Compact,
    #[default]
    Default,
    Comfortable,
}

impl Density {
    /// The factor on a skin's row height.
    pub fn factor(self) -> f32 {
        match self {
            Density::Compact => 0.875,
            Density::Default => 1.0,
            Density::Comfortable => 1.125,
        }
    }
}

/// What the owner chose.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// A built-in or saved theme's id: its palette.
    pub theme: String,
    /// A skin other than the theme's own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skin: Option<String>,
    /// A background other than the theme's own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<BackgroundChoice>,
    pub mode: ModeChoice,
    pub density: Density,
    /// Code and terminal text size in px.
    pub code_size: f32,
    /// Whether the first-run choice between Loom and Graphite was made.
    pub chosen: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance {
            theme: builtin::DEFAULT_THEME.into(),
            skin: None,
            background: None,
            mode: ModeChoice::System,
            density: Density::Default,
            code_size: 13.0,
            chosen: false,
        }
    }
}

/// The hub's appearance document: the choice and the owner's saved themes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Document {
    pub choice: Appearance,
    pub themes: Vec<ThemeFile>,
}

impl Appearance {
    /// Problems with the choice against the saved themes, empty when valid.
    pub fn problems(&self, saved: &[ThemeFile]) -> Vec<String> {
        let mut out = vec![];
        if !builtin::is_builtin(&self.theme) && !saved.iter().any(|t| t.id == self.theme) {
            out.push(format!("unknown theme {:?}", self.theme));
        }
        if let Some(s) = &self.skin
            && skin::skin(s).is_none()
        {
            out.push(format!("unknown skin {s:?}"));
        }
        match &self.background {
            Some(BackgroundChoice::Preset(id)) if crate::background::preset_by_id(id).is_none() => {
                out.push(format!("unknown background preset {id:?}"));
            }
            Some(BackgroundChoice::Spec(b)) => out.extend(b.problems()),
            _ => {}
        }
        if !(9.0..=24.0).contains(&self.code_size) {
            out.push("code size must be 9–24".into());
        }
        out
    }

    /// The theme file the choice names (a saved theme first, then a built-in, then the default).
    pub fn file(&self, saved: &[ThemeFile]) -> ThemeFile {
        saved
            .iter()
            .find(|t| t.id == self.theme)
            .cloned()
            .or_else(|| builtin::builtin(&self.theme))
            .or_else(|| builtin::builtin(builtin::DEFAULT_THEME))
            .expect("the default theme is built in")
    }

    /// Resolves the choice for a system appearance.
    pub fn resolve(&self, saved: &[ThemeFile], system_light: bool) -> Resolved {
        let file = self.file(saved);
        let skin_id = self.skin.clone().unwrap_or_else(|| file.skin.clone());
        let skin = skin::skin(&skin_id)
            .or_else(|| skin::skin(skin::DEFAULT_SKIN))
            .expect("the default skin is built in");
        let mut r = resolve(&file, &skin, self.mode.mode(system_light));
        if let Some(bg) = self.background.as_ref().and_then(BackgroundChoice::resolve) {
            r.background = bg;
        }
        r
    }
}
