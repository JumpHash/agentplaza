//! Walk surfaces (docs/design/walk-surfaces.md): the one list of floors robots may stand on. The layout emits them,
//! the drawn meshes are built from them (mesh.rs), and the nav grid is rasterised from them (grid.rs). Nothing else is
//! support: walls, slabs, furniture and anything not in this list never hold a robot up.
//!
//! This file is the contract. The fields are fixed; add to it only with the owner's agreement.

use serde::{Deserialize, Serialize};

/// What a surface is. `Board` is a hardware top face that robots may use (a PCB, a chip top carrying a plate).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SurfaceKind {
    Board,
    Plate,
    RoomFloor,
    Ramp,
    Stair,
    Bridge,
    Landing,
    Cab,
}

/// A surface's height over (x, z): `y = c + dx * x + dz * z`. Flat when dx = dz = 0. A ramp is a plane.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Plane {
    pub c: f64,
    pub dx: f64,
    pub dz: f64,
}

impl Plane {
    /// A flat surface at height `y`.
    pub fn flat(y: f64) -> Plane {
        Plane { c: y, dx: 0.0, dz: 0.0 }
    }

    /// The height at (x, z).
    pub fn at(&self, x: f64, z: f64) -> f64 {
        self.c + self.dx * x + self.dz * z
    }
}

/// What lies across one side of a surface's polygon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Side {
    /// A wall, a rail or the surface's edge: no walking out.
    Walled,
    /// Walkable to a neighbour surface; the link list says which one, and how.
    Open,
    /// The surface ends in a fall: robots may drop off it (one way, downhill), see the links of kind `Drop`.
    Drop,
}

/// A convex walkable floor piece.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WalkSurface {
    /// Unique on the board; the index into `WalkSet::surfaces` (the audits rely on it).
    pub id: u32,
    pub kind: SurfaceKind,
    /// The convex polygon in (x, z), counter-clockwise seen from above. Side i runs from `poly[i]` to `poly[(i+1) % n]`.
    pub poly: Vec<[f64; 2]>,
    /// The height over the polygon.
    pub height: Plane,
    /// The part or generator that draws this surface (its mesh is built from this record, so the two agree).
    pub owner: String,
    /// One entry per side of `poly`.
    pub sides: Vec<Side>,
}

/// How two surfaces join.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LinkKind {
    /// A step within the step height, two-way.
    Step,
    /// A ramp, two-way.
    Ramp,
    /// A fall, one way: `from` is the high surface, `to` the low one.
    Drop,
    /// A lift: `from` is the bottom landing, `to` the top landing, `via` the cab surface. Two-way along the shaft.
    Lift,
    /// A door or a gangway into a room, two-way.
    Door,
}

/// A join between two surfaces. `at` is the world (x, z) of the join's midpoint, for the rasteriser and the audits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WalkLink {
    pub from: u32,
    pub to: u32,
    pub kind: LinkKind,
    pub via: Option<u32>,
    pub at: [f64; 2],
}

/// The walk list of one board: the surfaces and the links between them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WalkSet {
    pub surfaces: Vec<WalkSurface>,
    pub links: Vec<WalkLink>,
}
