//! The board core: the hardware boards of AgentPlaza, the parts that rest on them, and the top surfaces they rest on.
//!
//! One Rust crate for both targets: the desktop links it natively, and the web loads it as WebAssembly. It does no
//! I/O at run time (the assets are embedded by build.rs from `assets/hardware`), starts no threads, and uses f64 math
//! throughout, so the same input gives the same output on both sides.
//!
//! What it computes, from a board's JSON and its parts:
//!
//! - Items: each resting part (a solid that is not attached, or a group that is not attached) with its world box and
//!   the height of its highest upward face, in the order the web's traversal visits them (ground.ts groundAudit).
//! - The support field: the highest upward face under a point (ground.ts topBelow), so a plate can rest on a board.
//! - The part count: the number of part keys the web's `Built.parts` map holds for the board.
//!
//! Float32 and labels, as the web's three.js build stores them:
//!
//! - Vertex positions are rounded to f32 (three.js keeps them in a Float32Array), and so are the elements of every
//!   instance matrix. Each generated coordinate is rounded before any further arithmetic, so the boxes and the
//!   support heights match the web's to the last bit of f32 storage.
//! - A label is a flat plane 4 * size wide, `size` deep (default 0.6), raised to top + 0.02 above its part, where top
//!   is the highest vertex of the part (for a `use` group, the target's `size[1]`). Labels are solids: they hold things
//!   up and their boxes count in a group's box. They are not items. They are built only when the web has a document
//!   (the web's test fixture stubs one), so the fixture includes them.
//!
//! Not in this crate yet: the layout, walk surfaces, navigation and motion (see docs/design/apz-board.md), and
//! rendering, which each app keeps.

pub mod audit;
pub mod field;
pub mod grid;
pub mod layout;
pub mod mesh;
pub mod plan;
pub mod walk;

mod geom;
mod hardware;
mod scene;
mod support;

use std::collections::BTreeMap;

pub use hardware::embedded_files;
use hardware::Library;
use support::Support;

/// An axis-aligned box in world units (the board's root frame): min and max corners.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Aabb {
    /// The box with nothing in it: every union with it is the other box.
    pub fn empty() -> Aabb {
        Aabb { min: [f64::INFINITY; 3], max: [f64::NEG_INFINITY; 3] }
    }

    /// Whether no point has been included.
    pub fn is_empty(&self) -> bool {
        self.min[0] > self.max[0] || self.min[1] > self.max[1] || self.min[2] > self.max[2]
    }

    /// Grows the box to contain a point.
    pub fn include(&mut self, p: [f64; 3]) {
        for (k, &c) in p.iter().enumerate() {
            self.min[k] = self.min[k].min(c);
            self.max[k] = self.max[k].max(c);
        }
    }

    /// The smallest box containing both.
    pub fn union(&self, o: &Aabb) -> Aabb {
        let mut r = *self;
        for k in 0..3 {
            r.min[k] = r.min[k].min(o.min[k]);
            r.max[k] = r.max[k].max(o.max[k]);
        }
        r
    }
}

/// A resting item of a board: a part (or a group of parts) that stands on what is under it.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// The part's id ("" when it has none); a repeat's copies are "id#i" (the copies of an instanced repeat).
    pub name: String,
    /// The item's box in the board's root frame.
    pub bounds: Aabb,
    /// The height of its highest upward face, or None when it has none (a glow, a light, a box with no solid top).
    pub top: Option<f64>,
}

/// A built board: its items, the support field, and the part count. Build one with [`Board::from_kind`].
#[derive(Debug, Clone)]
pub struct Board {
    items: Vec<Item>,
    support: Support,
    part_count: usize,
    bounds: Aabb,
}

impl Board {
    /// The board of a kind ("edge", "laptop", ...). The kind's file is `board/<kind>`. The given params override the
    /// model's own (as buildModelSteps does: `{...model.params, ...params}`).
    ///
    /// Panics with a message naming the kind when no board file has that kind.
    pub fn from_kind(kind: &str, params: &BTreeMap<String, f64>) -> Board {
        let lib = Library::get();
        let name = format!("board/{kind}");
        let model = lib.model(&name).unwrap_or_else(|| {
            let known: Vec<&str> = lib.model_names().filter(|n| n.starts_with("board/")).collect();
            panic!("apz-board: no board kind \"{kind}\" (known: {})", known.join(", "))
        });
        let scene = scene::build(lib, model, params);
        let mut bounds = Aabb::empty();
        for it in &scene.items {
            bounds = bounds.union(&it.bounds);
        }
        if bounds.is_empty() {
            bounds = Aabb { min: [0.0; 3], max: [0.0; 3] };
        }
        Board { items: scene.items, support: scene.support, part_count: scene.part_count, bounds }
    }

    /// The resting items, in the web's traversal order.
    pub fn parts(&self) -> &[Item] {
        &self.items
    }

    /// The union of the items' boxes (a zero box when there are no items).
    pub fn bounds(&self) -> Aabb {
        self.bounds
    }

    /// The height of the highest upward face at (x, z), in the board's root frame; None for the ground or no support.
    pub fn support_top(&self, x: f64, z: f64) -> Option<f64> {
        self.support.top(x, z)
    }

    /// The height of the highest upward face at (x, z) at or below `y` (the web's topBelow), 0 for none. For the layout.
    pub(crate) fn support_below(&self, x: f64, z: f64, y: f64) -> f64 {
        self.support.top_below(x, z, y).unwrap_or(0.0)
    }

    /// The number of distinct part keys the web's `Built.parts` map holds for this board.
    pub fn part_count(&self) -> usize {
        self.part_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_kind_panics_with_its_name() {
        let r = std::panic::catch_unwind(|| Board::from_kind("no-such-board", &BTreeMap::new()));
        assert!(r.is_err());
    }

    #[test]
    fn aabb_union_and_empty() {
        let mut b = Aabb::empty();
        assert!(b.is_empty());
        b.include([1.0, 2.0, 3.0]);
        b.include([-1.0, 0.0, 5.0]);
        assert_eq!(b.min, [-1.0, 0.0, 3.0]);
        assert_eq!(b.max, [1.0, 2.0, 5.0]);
        assert_eq!(b.union(&Aabb::empty()), b);
    }
}
