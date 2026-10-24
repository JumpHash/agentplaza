//! A page as the checks read it: its accessibility tree (roles and names, as the browser computes them) and the facts of its
//! elements that the rules need (their text, colours, boxes and overflow). Serialised, it is also the fixture a rule is
//! tested over.

use serde::{Deserialize, Serialize};

/// One node of the accessibility tree, in the tree's order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AxNode {
    /// The element the node is (its key in [`Element::key`]); empty for a node that is no element.
    pub key: String,
    /// The accessible role (`button`, `link`, `img`, `StaticText`, `generic`, …).
    pub role: String,
    /// The accessible name the browser computed from the page (its text, its label, its alternative text).
    pub name: String,
    /// Whether the browser leaves the node out of the tree a screen reader reads.
    pub ignored: bool,
}

/// The facts of one element the rules read. Colours are opaque: a translucent background is composited by the probe, and
/// a background the probe cannot tell (an image or a gradient behind the text) is `None`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Element {
    pub key: String,
    /// The element's tag name, in lower case.
    pub tag: String,
    /// The element's `id` attribute; empty when it has none.
    pub id: String,
    /// The text the element holds itself (its own text nodes, not its children's).
    pub text: String,
    /// The `tabindex` attribute, when it is set.
    pub tabindex: Option<i64>,
    /// Whether a person can move focus to it with the keyboard.
    pub focusable: bool,
    /// Whether it is rendered with a box (not `display: none`, not `visibility: hidden`, not empty).
    pub visible: bool,
    /// Its border box in CSS pixels of the page: x, y, width, height.
    pub rect: [f64; 4],
    /// The text's colour, opaque, 0 to 255 per channel.
    pub fg: Option<[f64; 3]>,
    /// The colour behind the text, opaque; `None` when it cannot be told.
    pub bg: Option<[f64; 3]>,
    pub font_px: f64,
    /// Whether the text is bold (weight 700 or more).
    pub bold: bool,
    /// The computed `overflow-x`.
    pub overflow_x: String,
    /// The element's scroll width and its client width: a scroll width past the client width is content outside its box.
    pub scroll_width: f64,
    pub client_width: f64,
}

/// A page's snapshot: its language, its accessibility tree and the elements the tree names.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    /// The document's language (`lang` on the root element); empty when it declares none.
    pub lang: String,
    pub nodes: Vec<AxNode>,
    pub elements: Vec<Element>,
}

impl Snapshot {
    /// Whether the tree names `key` as ignored (hidden from assistive technology).
    pub fn ignored(&self, key: &str) -> bool {
        !key.is_empty() && self.nodes.iter().any(|n| n.key == key && n.ignored)
    }
}
