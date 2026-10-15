//! The navigation overlay (`nav-overlay.ts` on the web, shown with `?nav=1`; `APZ_NAV_OVERLAY=1` in the engine): a debug
//! layer over a board's walkways. The grid's cells by class (blocked red, inflated amber, trace or near cyan, other
//! walkable green; plain PCB cells are left out), the ramp and lift portals, the lift rails and lift fronts, and per robot
//! its path (dots), an amber ring while it waits and a violet ring while it yields. Read-only: it reads the nav world and
//! changes nothing.
//!
//! The cells, portals, rails and fronts are static, built once with the nav. The robots' layer is one dynamic group
//! whose path points and rings are rewritten each frame, so nothing is added to or removed from the scene per robot.

use apz_board::grid::NAV_COST_OPEN;
use apz_board::nav::NavPoint;
use apz_board::nav::world::NavWorld;
use apz_board::navgrid::{NAV_COST_TRACE, NavGrid, PortalKind};
use glam::{Mat4, Vec3};

use crate::geometry::box_geo;
use crate::scene::{Color, Geo, Geometry, Instance, Mat, Material, Obj, Shading, Side, merge};

use super::lift_mesh::along;

/// The cell classes' colours: blocked, inflated, trace or near, walkable (the index `cell_class` returns).
const CELL_COLOURS: [u32; 4] = [0xff4d6d, 0xffb020, 0x4cc9ff, 0x3ee08f];
const RAMP: u32 = 0xff8c1a;
const LIFT: u32 = 0xff3cd6;
const RAIL: u32 = 0x4cc9ff;
const PATH: u32 = 0xcfe3ff;
const AMBER: u32 = 0xffb020;
const VIOLET: u32 = 0xb388ff;

/// Cells sit this far above their surface; portals and lift fronts this far; path dots this far.
const CELL_LIFT: f32 = 0.03;
const PORTAL_LIFT: f32 = 0.08;
const PATH_LIFT: f32 = 0.1;
/// Rings float this far above a robot's surface.
const RING_LIFT: f32 = 1.4;
const RING_INNER: f32 = 0.2;
const RING_OUTER: f32 = 0.26;
const RING_SEGMENTS: usize = 24;
const CELL_OPACITY: f32 = 0.35;
/// The thickness of a portal line or a rail, and the side of a lift front.
const LINE: f32 = 0.05;
const FRONT: f32 = 0.1;
/// A path dot's size, and the spacing of the dots along a path segment.
const DOT: f32 = 0.06;
const DOT_STEP: f32 = 0.12;

/// The class of a cell: an index into `CELL_COLOURS`, or None when the cell is not drawn (a plain PCB cell, or off the walk).
fn cell_class(grid: &NavGrid, idx: usize) -> Option<usize> {
    if grid.blocked[idx] == 1 {
        return Some(0);
    }
    if grid.inflated[idx] == 1 {
        return Some(1);
    }
    if grid.walk[idx] == 0 {
        return None;
    }
    let cost = grid.cost[idx];
    if grid.region[idx] == 0 && cost == NAV_COST_OPEN {
        return None;
    }
    if cost == NAV_COST_TRACE || cost > NAV_COST_OPEN {
        return Some(2);
    }
    Some(3)
}

/// A point over a cell's centre, `lift` above its surface.
fn over(grid: &NavGrid, idx: usize, lift: f32) -> Vec3 {
    let [x, z] = grid.centre(idx);
    Vec3::new(x as f32, grid.y[idx] as f32 + lift, z as f32)
}

/// Flat, upward-facing quads (two triangles each) of one class, merged into one geometry.
#[derive(Default)]
struct Quads {
    positions: Vec<[f32; 3]>,
    indices: Vec<u32>,
}

impl Quads {
    /// A square of side 2 × `half` centred at (cx, y, cz).
    fn add(&mut self, cx: f32, y: f32, cz: f32, half: f32) {
        let base = self.positions.len() as u32;
        self.positions.extend([
            [cx - half, y, cz - half],
            [cx + half, y, cz - half],
            [cx + half, y, cz + half],
            [cx - half, y, cz + half],
        ]);
        self.indices
            .extend([base, base + 2, base + 1, base, base + 3, base + 2]);
    }

    fn geo(self) -> Geo {
        let normals = vec![[0.0, 1.0, 0.0]; self.positions.len()];
        Geometry::new(self.positions, normals, Vec::new(), self.indices)
    }
}

/// Thin boxes (portal lines, rails, lift fronts) of one colour, merged into one mesh.
#[derive(Default)]
struct Thin(Vec<(Geo, Mat4)>);

impl Thin {
    /// A line from a to b (a zero-length pair draws nothing).
    fn beam(&mut self, a: Vec3, b: Vec3) {
        let len = (b - a).length();
        if len < 1e-4 {
            return;
        }
        self.0.push((box_geo(LINE, len, LINE), along(a, b)));
    }

    /// A lift front marker at `at`.
    fn front(&mut self, at: Vec3) {
        self.0
            .push((box_geo(FRONT, FRONT, FRONT), Mat4::from_translation(at)));
    }

    /// The merged pieces as one basic-shaded mesh of `colour`, or None when there are none.
    fn mesh(&self, colour: u32) -> Option<Obj> {
        if self.0.is_empty() {
            return None;
        }
        let mat = Mat::new(Material {
            shading: Shading::Basic,
            color: Color::hex(colour),
            ..Material::default()
        });
        let m = Obj::mesh(&merge(&self.0), &mat);
        m.set_shadows(false, false).no_outline();
        Some(m)
    }
}

/// The cell layer: one translucent mesh per class, and the number of cells drawn.
fn cell_layer(grid: &NavGrid) -> (Vec<Obj>, usize) {
    let mut parts: Vec<Quads> = CELL_COLOURS.iter().map(|_| Quads::default()).collect();
    let mut drawn = 0;
    for idx in 0..grid.cols * grid.rows {
        let Some(class) = cell_class(grid, idx) else {
            continue;
        };
        let c = over(grid, idx, CELL_LIFT);
        parts[class].add(c.x, c.y, c.z, grid.cell as f32 * 0.5);
        drawn += 1;
    }
    let meshes = parts
        .into_iter()
        .zip(CELL_COLOURS)
        .filter(|(q, _)| !q.indices.is_empty())
        .map(|(q, colour)| {
            let mat = Mat::new(Material {
                shading: Shading::Basic,
                color: Color::hex(colour),
                opacity: CELL_OPACITY,
                transparent: true,
                depth_write: false,
                decal: true,
                side: Side::Double,
                ..Material::default()
            });
            let m = Obj::mesh(&q.geo(), &mat);
            m.set_shadows(false, false).no_outline();
            m
        })
        .collect();
    (meshes, drawn)
}

/// The portal lines (ramps and lifts), the lift rails and the lift fronts.
fn portal_layer(grid: &NavGrid) -> Vec<Obj> {
    let (mut ramps, mut lifts, mut rails, mut fronts) = (
        Thin::default(),
        Thin::default(),
        Thin::default(),
        Thin::default(),
    );
    for p in &grid.portals {
        let (a, b) = (over(grid, p.a, PORTAL_LIFT), over(grid, p.b, PORTAL_LIFT));
        match p.kind {
            PortalKind::Ramp => ramps.beam(a, b),
            PortalKind::Lift => lifts.beam(a, b),
        }
    }
    for l in &grid.lifts {
        rails.beam(
            Vec3::new(l.x as f32, l.low as f32, l.z as f32),
            Vec3::new(l.x as f32, l.high as f32, l.z as f32),
        );
        fronts.front(over(grid, l.front, PORTAL_LIFT));
    }
    [
        ramps.mesh(RAMP),
        lifts.mesh(LIFT),
        rails.mesh(RAIL),
        fronts.mesh(LIFT),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// A robot's path as dots: its points, and dots between them every `DOT_STEP`.
fn path_dots(pts: &[NavPoint]) -> Vec<[f32; 3]> {
    let mut out = Vec::new();
    let mut prev: Option<Vec3> = None;
    for p in pts {
        let at = Vec3::new(p.x as f32, p.y as f32 + PATH_LIFT, p.z as f32);
        match prev {
            Some(a) => {
                let n = ((at - a).length() / DOT_STEP).ceil().max(1.0) as usize;
                for k in 1..=n {
                    out.push((a + (at - a) * (k as f32 / n as f32)).to_array());
                }
            }
            None => out.push(at.to_array()),
        }
        prev = Some(at);
    }
    out
}

/// A flat ring on the floor (its axis is y), from RING_INNER to RING_OUTER.
fn ring_geo() -> Geo {
    let (mut positions, mut indices) = (Vec::new(), Vec::new());
    for k in 0..=RING_SEGMENTS {
        let (s, c) = (k as f32 / RING_SEGMENTS as f32 * std::f32::consts::TAU).sin_cos();
        positions.push([c * RING_OUTER, 0.0, s * RING_OUTER]);
        positions.push([c * RING_INNER, 0.0, s * RING_INNER]);
    }
    for k in 0..RING_SEGMENTS as u32 {
        let o = 2 * k;
        indices.extend([o, o + 2, o + 1, o + 1, o + 2, o + 3]);
    }
    let normals = vec![[0.0, 1.0, 0.0]; positions.len()];
    Geometry::new(positions, normals, Vec::new(), indices)
}

/// The overlay of one board's nav: its static layer under `root`, and the robots' dynamic layer under it.
pub struct NavOverlay {
    pub root: Obj,
    /// The cells drawn (the walkable and blocked cells; plain PCB cells are not).
    pub cells: usize,
    paths: Obj,
    waits: Obj,
    yields: Obj,
}

impl NavOverlay {
    /// The overlay of a nav grid: the cells, portals, rails and lift fronts, and the robots' layer (empty until `update`).
    pub fn new(grid: &NavGrid) -> NavOverlay {
        let root = Obj::group();
        root.set_name("nav_overlay").no_outline();
        let (cells, drawn) = cell_layer(grid);
        for m in cells.iter().chain(portal_layer(grid).iter()) {
            root.add(m);
        }

        let robots = Obj::group();
        robots.set_name("nav_robots").set_dynamic(true).no_outline();
        let path_mat = Mat::new(Material {
            shading: Shading::Points { size: DOT },
            color: Color::hex(PATH),
            ..Material::default()
        });
        let paths = Obj::points(Vec::new(), &path_mat);
        let ring = ring_geo();
        let amber = Mat::new(Material {
            shading: Shading::Basic,
            color: Color::hex(AMBER),
            side: Side::Double,
            ..Material::default()
        });
        let violet = Mat::new(Material {
            shading: Shading::Basic,
            color: Color::hex(VIOLET),
            side: Side::Double,
            ..Material::default()
        });
        let waits = Obj::instanced(&ring, &amber, Vec::new());
        let yields = Obj::instanced(&ring, &violet, Vec::new());
        for o in [&paths, &waits, &yields] {
            o.set_dynamic(true).no_outline();
            robots.add(o);
        }
        root.add(&robots);
        NavOverlay {
            root,
            cells: drawn,
            paths,
            waits,
            yields,
        }
    }

    /// Per frame: every robot's path as dots, and a ring above each robot that waits (amber) or yields (violet).
    pub fn update(&mut self, world: &NavWorld) {
        let mut dots = Vec::new();
        let (mut waits, mut yields) = (Vec::new(), Vec::new());
        for id in world.robot_ids() {
            if let Some(pts) = world.path(id) {
                dots.extend(path_dots(&pts));
            }
            let pose = world.pose(id);
            let ring = Instance {
                matrix: Mat4::from_translation(Vec3::new(
                    pose.x as f32,
                    pose.y as f32 + RING_LIFT,
                    pose.z as f32,
                )),
                color: Color::WHITE,
            };
            if pose.waiting {
                waits.push(ring);
            } else if pose.yielding {
                yields.push(ring);
            }
        }
        self.paths.set_points(dots);
        self.waits.set_instances(waits);
        self.yields.set_instances(yields);
    }
}
