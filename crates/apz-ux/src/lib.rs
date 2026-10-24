//! UI and UX checks (PLAN 22.8, 22.9): a page at each declared size, theme and locale, judged three ways.
//!
//! - [`visual`]: the screenshot against the previous release's, pixel by pixel, with a per-channel threshold, an ignore mask
//!   and a tolerance on the share of changed pixels; the diff picture is the evidence.
//! - [`audit`]: axe-style rules over the page's accessibility tree and its elements: names on controls and images,
//!   contrast of text against its background, focus order, duplicate ids and the document's language.
//! - [`layout`]: text whose content is wider than its box (`scrollWidth` past `clientWidth`).
//!
//! The page is read through [`probe::Devtools`], the DevTools commands a browser answers ([`check::run`] drives it at one
//! size, theme and locale). Its findings are [`Issue`]s whose [`Kind`] is `visual`, `accessibility` or `layout`: the
//! kinds the review graph reads. Pure apart from the commands it sends, so the rules are tested over fixtures and the
//! pipeline over a scripted browser; the real Chrome test lives with the web target (apid's `user_test`).

use serde::Serialize;

pub mod audit;
pub mod check;
pub mod layout;
pub mod probe;
pub mod spec;
pub mod tree;
pub mod visual;

/// What an issue is about: the review graph's finding kinds for UI and UX (PLAN 22.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Visual,
    Accessibility,
    Layout,
}

impl Kind {
    /// The kind as the review graph names it.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Visual => "visual",
            Kind::Accessibility => "accessibility",
            Kind::Layout => "layout",
        }
    }
}

/// One thing a check found on one page at one size, theme and locale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Issue {
    pub kind: Kind,
    /// The rule that failed, as its name (`control-name`, `color-contrast`, `pixel-diff`, …).
    pub rule: &'static str,
    /// The element it is about (its key in the snapshot), or the check's label for a whole-page finding.
    pub element: String,
    pub message: String,
}

impl Issue {
    pub fn new(
        kind: Kind,
        rule: &'static str,
        element: impl Into<String>,
        message: impl Into<String>,
    ) -> Issue {
        Issue {
            kind,
            rule,
            element: element.into(),
            message: message.into(),
        }
    }
}
