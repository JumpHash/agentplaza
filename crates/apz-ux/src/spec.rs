//! The declared UI checks of a project (PLAN 22.9): its pages, the device sizes they are shown at, the themes and locales, and
//! the visual tolerance. A project declares them in `.plaza/ux.toml`:
//!
//! ```toml
//! [visual]
//! channel_threshold = 8        # steps a channel may move before its pixel counts as changed
//! max_changed_ratio = 0.001    # the share of compared pixels that may change
//! mask = [{ x = 0, y = 0, w = 240, h = 48 }]   # ignored rectangles, in picture pixels
//!
//! [[device]]
//! name = "phone"
//! width = 390
//! height = 844
//!
//! [[page]]
//! name = "checkout"
//! path = "/checkout"
//! devices = ["phone"]
//! themes = ["light", "dark"]   # default: light
//! locales = ["en", "de"]       # default: en
//! ```
//!
//! [`Spec::checks`] expands the pages into one [`Check`] per page, size, theme and locale, against the base address the
//! topology serves the project's pages at.

use serde::Deserialize;

use crate::visual::{Rect, Tolerance};

/// The whole declaration.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    #[serde(default)]
    pub visual: Visual,
    #[serde(default, rename = "device")]
    pub devices: Vec<Device>,
    #[serde(default, rename = "page")]
    pub pages: Vec<Page>,
}

/// The visual tolerance and the ignored areas, for every page.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Visual {
    pub channel_threshold: u8,
    pub max_changed_ratio: f64,
    pub mask: Vec<Rect>,
}

impl Default for Visual {
    fn default() -> Self {
        Visual {
            channel_threshold: 8,
            max_changed_ratio: 0.001,
            mask: Vec::new(),
        }
    }
}

/// A size a page is shown at: the browser's viewport, in CSS pixels.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    pub name: String,
    pub width: u32,
    pub height: u32,
}

/// A page of the project: its path, and the sizes, themes and locales it is checked at.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub name: String,
    pub path: String,
    pub devices: Vec<String>,
    #[serde(default = "light")]
    pub themes: Vec<String>,
    #[serde(default = "english")]
    pub locales: Vec<String>,
}

fn light() -> Vec<String> {
    vec!["light".into()]
}

fn english() -> Vec<String> {
    vec!["en".into()]
}

/// One check: a page at one size, theme and locale, at its address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub page: String,
    pub url: String,
    pub device: Device,
    pub theme: String,
    pub locale: String,
}

impl Check {
    /// The check as people name it: `checkout phone dark de`.
    pub fn label(&self) -> String {
        format!(
            "{} {} {} {}",
            self.page, self.device.name, self.theme, self.locale
        )
    }
}

impl Spec {
    /// Parses and checks a declaration.
    pub fn parse(text: &str) -> Result<Spec, String> {
        let spec: Spec = toml::from_str(text).map_err(|e| format!("the UI checks file: {e}"))?;
        spec.validate()?;
        Ok(spec)
    }

    fn validate(&self) -> Result<(), String> {
        if self.pages.is_empty() {
            return Err("the UI checks declare no page".into());
        }
        let mut names = std::collections::BTreeSet::new();
        for device in &self.devices {
            if device.width == 0 || device.height == 0 {
                return Err(format!("device {} has no size", device.name));
            }
            if !names.insert(device.name.as_str()) {
                return Err(format!("device {} is declared twice", device.name));
            }
        }
        if !(0.0..=1.0).contains(&self.visual.max_changed_ratio) {
            return Err("max_changed_ratio is a share, from 0 to 1".into());
        }
        let mut pages = std::collections::BTreeSet::new();
        for page in &self.pages {
            if !pages.insert(page.name.as_str()) {
                return Err(format!("page {} is declared twice", page.name));
            }
            if !page.path.starts_with('/') {
                return Err(format!("page {}: its path starts with /", page.name));
            }
            if page.devices.is_empty() {
                return Err(format!("page {} names no device", page.name));
            }
            for device in &page.devices {
                if self.device(device).is_none() {
                    return Err(format!(
                        "page {} shows {device}, which no device declares",
                        page.name
                    ));
                }
            }
            for theme in &page.themes {
                if theme != "light" && theme != "dark" {
                    return Err(format!(
                        "page {}: theme {theme} is not light or dark",
                        page.name
                    ));
                }
            }
            if page.locales.is_empty() || page.themes.is_empty() {
                return Err(format!("page {} names no theme or locale", page.name));
            }
        }
        Ok(())
    }

    fn device(&self, name: &str) -> Option<&Device> {
        self.devices.iter().find(|d| d.name == name)
    }

    /// The tolerance every page's picture is judged by.
    pub fn tolerance(&self) -> Tolerance {
        Tolerance {
            channel: self.visual.channel_threshold,
            ratio: self.visual.max_changed_ratio,
        }
    }

    /// Every check the declaration names, at `base` (the address the topology serves the project at, without a trailing
    /// slash).
    pub fn checks(&self, base: &str) -> Vec<Check> {
        let base = base.trim_end_matches('/');
        let mut out = Vec::new();
        for page in &self.pages {
            for device in &page.devices {
                let Some(device) = self.device(device) else {
                    continue;
                };
                for theme in &page.themes {
                    for locale in &page.locales {
                        out.push(Check {
                            page: page.name.clone(),
                            url: format!("{base}{}", page.path),
                            device: device.clone(),
                            theme: theme.clone(),
                            locale: locale.clone(),
                        });
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DECLARED: &str = r#"
[visual]
channel_threshold = 6
max_changed_ratio = 0.002
mask = [{ x = 0, y = 0, w = 240, h = 48 }]

[[device]]
name = "phone"
width = 390
height = 844

[[device]]
name = "desktop"
width = 1280
height = 800

[[page]]
name = "checkout"
path = "/checkout"
devices = ["phone", "desktop"]
themes = ["light", "dark"]
locales = ["en", "de"]

[[page]]
name = "home"
path = "/"
devices = ["desktop"]
"#;

    #[test]
    fn a_declaration_expands_to_every_page_size_theme_and_locale() {
        let spec = Spec::parse(DECLARED).unwrap();
        assert_eq!(
            spec.tolerance(),
            Tolerance {
                channel: 6,
                ratio: 0.002
            }
        );
        assert_eq!(spec.visual.mask.len(), 1);
        let checks = spec.checks("http://127.0.0.1:8080/");
        // checkout: 2 sizes x 2 themes x 2 locales; home: one size, light, en.
        assert_eq!(checks.len(), 9);
        let first = &checks[0];
        assert_eq!(first.url, "http://127.0.0.1:8080/checkout");
        assert_eq!(first.label(), "checkout phone light en");
        assert!(
            checks
                .iter()
                .any(|c| c.label() == "checkout desktop dark de")
        );
        let home = checks.last().unwrap();
        assert_eq!(
            (
                home.page.as_str(),
                home.theme.as_str(),
                home.locale.as_str()
            ),
            ("home", "light", "en")
        );
        assert_eq!(home.url, "http://127.0.0.1:8080/");
    }

    #[test]
    fn a_declaration_with_a_page_on_an_undeclared_size_is_refused() {
        let bad = DECLARED.replace("devices = [\"desktop\"]", "devices = [\"tablet\"]");
        let why = Spec::parse(&bad).unwrap_err();
        assert!(why.contains("tablet"), "{why}");
    }

    #[test]
    fn unknown_keys_and_themes_are_refused() {
        assert!(Spec::parse(&DECLARED.replace("max_changed_ratio", "max_ratio")).is_err());
        let sepia = DECLARED.replace("[\"light\", \"dark\"]", "[\"sepia\"]");
        assert!(Spec::parse(&sepia).unwrap_err().contains("sepia"));
        assert!(Spec::parse("").unwrap_err().contains("no page"));
    }

    #[test]
    fn the_defaults_are_light_english_and_a_small_tolerance() {
        let spec = Spec::parse(
            "[[device]]\nname = \"desktop\"\nwidth = 800\nheight = 600\n[[page]]\nname = \"home\"\npath = \"/\"\ndevices = [\"desktop\"]\n",
        )
        .unwrap();
        assert_eq!(
            spec.tolerance(),
            Tolerance {
                channel: 8,
                ratio: 0.001
            }
        );
        let checks = spec.checks("http://h");
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].label(), "home desktop light en");
    }
}
