//! The targets a user test drives (PLAN 19.2, 19.3). A target is what the driver sees (its accessibility tree), what the
//! checker looks at (a screenshot) and what the loop does to it (an action through the target's control channel). The
//! loop (`run.rs`) knows only this trait, so the web, terminal, desktop, simulator and device targets of 8.2–8.5 plug in
//! beside the stand-in without a change to it.
//!
//! The stand-in ([`StandIn`]) is a scripted target: a set of screens, each with its elements, and the links an action on
//! an element follows. Its tree changes on the actions that the loop takes, its screenshot is an SVG of the screen (text,
//! so a text model reads it), and its clock advances by a scripted latency per action, so a timing has something to
//! measure. A seeded bug is a link to the wrong screen, or a latency over a budget.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The roles a driver may act on: what a person can click, type into or tick.
pub const ACTIONABLE: [&str; 4] = ["button", "link", "textbox", "checkbox"];

/// One element of a screen's accessibility tree.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Element {
    /// Unique on the screen: what the driver names in an action.
    pub id: String,
    /// `button`, `link`, `textbox`, `checkbox`, `heading`, `text` and the like.
    pub role: String,
    /// What the element says (its accessible name). An actionable element with no label cannot be found by a person.
    #[serde(default)]
    pub label: String,
    /// What the element holds (a textbox's text).
    #[serde(default)]
    pub value: String,
}

impl Element {
    pub fn actionable(&self) -> bool {
        ACTIONABLE.contains(&self.role.as_str())
    }
}

/// The accessibility tree of a screen. Empty when the target has none (a canvas, a game): the checker looks instead.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tree {
    /// The screen's title, as the driver and the checker name it.
    pub title: String,
    pub elements: Vec<Element>,
}

impl Tree {
    /// Whether the target gave no accessibility tree.
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    /// The elements a driver may act on.
    pub fn actionable(&self) -> impl Iterator<Item = &Element> {
        self.elements.iter().filter(|e| e.actionable())
    }

    /// Whether the screen shows `text`: its title, or some element's label or value, ignoring case.
    pub fn shows(&self, text: &str) -> bool {
        let text = text.to_lowercase();
        self.title.to_lowercase().contains(&text)
            || self.elements.iter().any(|e| {
                e.label.to_lowercase().contains(&text) || e.value.to_lowercase().contains(&text)
            })
    }
}

/// One action of the driver, through the target's channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    Click { element: String },
    Type { element: String, text: String },
    Press { key: String },
}

impl Action {
    /// The action in a line, as the evidence and the report say it.
    pub fn describe(&self) -> String {
        match self {
            Action::Click { element } => format!("click {element}"),
            Action::Type { element, text } => format!("type into {element}: {text}"),
            Action::Press { key } => format!("press {key}"),
        }
    }
}

/// A picture of the target's screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screenshot {
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// A target: the observe and act side of a user test. Its methods are the loop's only contact with it.
pub trait Target {
    /// What kind of target this is, for the report (`stand-in`, `web`, `tui`, …).
    fn kind(&self) -> &str;
    /// The accessibility tree of the screen now.
    fn tree(&mut self) -> Result<Tree, String>;
    /// A picture of the screen now.
    fn screenshot(&mut self) -> Result<Screenshot, String>;
    /// Does `action` through the target's channel. An error is the target failing, not the product: the run stops.
    fn act(&mut self, action: &Action) -> Result<(), String>;
    /// The target's clock in milliseconds, where it has one: a timing is measured against it. `None` when it has none.
    fn clock_ms(&mut self) -> Option<u64> {
        None
    }
}

/// A scripted screen: its elements, and the links an action on an element follows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screen {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub elements: Vec<Element>,
    /// Clicking `element` shows screen `to`.
    #[serde(default)]
    pub on: Vec<Link>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub element: String,
    pub to: String,
}

/// The stand-in target's script: the screens, where it starts, and the latency each action takes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StandIn {
    /// The screen the target starts on.
    pub start: String,
    /// The scripted latency of one action, in milliseconds (the target's clock moves by it).
    #[serde(default)]
    pub latency_ms: u64,
    pub screens: Vec<Screen>,
}

impl StandIn {
    /// A script must make sense before a run starts on it: unique screens, a start that exists, and links to screens and
    /// elements that exist.
    pub fn check(&self) -> Result<(), String> {
        if self.screens.is_empty() {
            return Err("the stand-in target has no screens".into());
        }
        for (i, screen) in self.screens.iter().enumerate() {
            if self.screens[..i].iter().any(|s| s.id == screen.id) {
                return Err(format!("screen {} appears twice", screen.id));
            }
            for link in &screen.on {
                if !screen.elements.iter().any(|e| e.id == link.element) {
                    return Err(format!(
                        "screen {} links from {}, which is not on it",
                        screen.id, link.element
                    ));
                }
                if !self.screens.iter().any(|s| s.id == link.to) {
                    return Err(format!(
                        "screen {} links to {}, which is not a screen",
                        screen.id, link.to
                    ));
                }
            }
        }
        if !self.screens.iter().any(|s| s.id == self.start) {
            return Err(format!(
                "the stand-in starts on {}, which is not a screen",
                self.start
            ));
        }
        Ok(())
    }

    /// A fresh target on the start screen.
    pub fn open(&self) -> Stand {
        Stand {
            script: self.clone(),
            at: self.start.clone(),
            values: BTreeMap::new(),
            clock: 0,
        }
    }
}

/// A running stand-in: its script, the screen it is on, what is typed into it, and its clock.
pub struct Stand {
    script: StandIn,
    at: String,
    values: BTreeMap<String, String>,
    clock: u64,
}

impl Stand {
    fn screen(&self) -> &Screen {
        self.script
            .screens
            .iter()
            .find(|s| s.id == self.at)
            .expect("the stand-in is on a screen of its script")
    }
}

impl Target for Stand {
    fn kind(&self) -> &str {
        "stand-in"
    }

    fn tree(&mut self) -> Result<Tree, String> {
        let screen = self.screen();
        let elements = screen
            .elements
            .iter()
            .map(|e| Element {
                value: self
                    .values
                    .get(&e.id)
                    .cloned()
                    .unwrap_or_else(|| e.value.clone()),
                ..e.clone()
            })
            .collect();
        Ok(Tree {
            title: screen.title.clone(),
            elements,
        })
    }

    fn screenshot(&mut self) -> Result<Screenshot, String> {
        let tree = self.tree()?;
        let mut svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\"><title>{}</title>",
            escape(&tree.title)
        );
        for (row, e) in tree.elements.iter().enumerate() {
            svg.push_str(&format!(
                "<text x=\"8\" y=\"{}\">[{}] {} {}</text>",
                24 + row * 20,
                escape(&e.role),
                escape(&e.label),
                escape(&e.value)
            ));
        }
        svg.push_str("</svg>");
        Ok(Screenshot {
            mime: "image/svg+xml".into(),
            bytes: svg.into_bytes(),
        })
    }

    fn act(&mut self, action: &Action) -> Result<(), String> {
        self.clock += self.script.latency_ms;
        match action {
            Action::Click { element } => {
                let screen = self.screen();
                if !screen.elements.iter().any(|e| &e.id == element) {
                    return Err(format!("no element {element} on {}", screen.id));
                }
                let next = screen
                    .on
                    .iter()
                    .find(|l| &l.element == element)
                    .map(|l| l.to.clone());
                if let Some(to) = next {
                    self.at = to;
                }
                Ok(())
            }
            Action::Type { element, text } => {
                let screen = self.screen();
                match screen.elements.iter().find(|e| &e.id == element) {
                    Some(e) if e.role == "textbox" => {
                        let key = element.clone();
                        let value = text.clone();
                        self.values.insert(key, value);
                        Ok(())
                    }
                    Some(_) => Err(format!("{element} is not a textbox")),
                    None => Err(format!("no element {element} on {}", screen.id)),
                }
            }
            Action::Press { .. } => Ok(()),
        }
    }

    fn clock_ms(&mut self) -> Option<u64> {
        Some(self.clock)
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(id: &str, role: &str, label: &str) -> Element {
        Element {
            id: id.into(),
            role: role.into(),
            label: label.into(),
            value: String::new(),
        }
    }

    pub fn shop() -> StandIn {
        StandIn {
            start: "cart".into(),
            latency_ms: 300,
            screens: vec![
                Screen {
                    id: "cart".into(),
                    title: "Cart".into(),
                    elements: vec![el("pay", "button", "Pay"), el("note", "textbox", "")],
                    on: vec![Link {
                        element: "pay".into(),
                        to: "receipt".into(),
                    }],
                },
                Screen {
                    id: "receipt".into(),
                    title: "Receipt".into(),
                    elements: vec![el("home", "link", "Back to shop")],
                    on: vec![],
                },
            ],
        }
    }

    #[test]
    fn a_click_moves_the_screen_and_the_clock_moves_by_the_latency() {
        let mut stand = shop().open();
        assert_eq!(stand.tree().unwrap().title, "Cart");
        assert!(stand.tree().unwrap().shows("pay"));
        stand
            .act(&Action::Click {
                element: "pay".into(),
            })
            .unwrap();
        let tree = stand.tree().unwrap();
        assert_eq!(tree.title, "Receipt");
        assert!(tree.shows("Back to shop"));
        assert_eq!(stand.clock_ms(), Some(300));
        let shot = stand.screenshot().unwrap();
        assert_eq!(shot.mime, "image/svg+xml");
        assert!(String::from_utf8(shot.bytes).unwrap().contains("Receipt"));
    }

    #[test]
    fn a_typed_value_shows_in_the_tree_and_a_bad_element_is_refused() {
        let mut stand = shop().open();
        stand
            .act(&Action::Type {
                element: "note".into(),
                text: "no ice".into(),
            })
            .unwrap();
        assert!(stand.tree().unwrap().shows("no ice"));
        assert!(
            stand
                .act(&Action::Click {
                    element: "nowhere".into()
                })
                .is_err()
        );
        assert!(
            stand
                .act(&Action::Type {
                    element: "pay".into(),
                    text: "x".into()
                })
                .is_err()
        );
    }

    #[test]
    fn a_script_that_links_nowhere_is_refused() {
        assert!(shop().check().is_ok());
        let mut bad = shop();
        bad.screens[0].on[0].to = "gone".into();
        assert!(bad.check().unwrap_err().contains("not a screen"));
        let mut bad = shop();
        bad.start = "void".into();
        assert!(bad.check().unwrap_err().contains("starts on"));
    }
}
