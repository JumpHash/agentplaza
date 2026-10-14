//! The room lab's debug overlay (`APZ_ROOM_DEBUG=1`), as the web's room-lab.ts `overlay`: the grid (the catalogue's cell,
//! 0.25 by default), the cells a solid (red) or a walk item (amber) holds, the reservations (clear zones yellow; the door
//! and the spine blue), the spots (dots coloured by their clip) and a red marker over each item or spot a violation names.
//! Drawn in the room's local frame, at the floor's height.

use apz_board::rooms::generate::{Catalog, Door, Layout, Place, fnv1a32};

use crate::geometry;
use crate::scene::{Color, Mat, Material, Obj, Shading};

/// The grid's cell when the catalogue names none.
const CELL_DEFAULT: f64 = 0.25;
const SOLID: u32 = 0xff3b3b;
const WALK: u32 = 0xffb020;
const DOOR: u32 = 0x3b82ff;
const CLEAR: u32 = 0xffe14d;
const MARKER: u32 = 0xff1744;
const GRID: u32 = 0x1f2937;
const EPS: f64 = 1e-9;

/// The layout of one room and what the overlay shows of it.
pub struct Overlay<'a> {
    pub layout: &'a Layout,
    pub violations: &'a [String],
    pub catalog: &'a Catalog,
    /// The inner size (x by z) and the walkway side.
    pub w: f64,
    pub d: f64,
    pub door: Door,
}

/// An axis-aligned rectangle on the floor.
#[derive(Clone, Copy)]
struct Rect {
    x0: f64,
    x1: f64,
    z0: f64,
    z1: f64,
}

/// A rot-0 offset turned by `rot` degrees, as the verifier maps it.
fn turn(x: f64, z: f64, rot: f64) -> (f64, f64) {
    match rot.round() as i64 {
        90 => (z, -x),
        180 => (-x, -z),
        270 => (-z, x),
        _ => (x, z),
    }
}

/// The axis-aligned box of a rot-0 rectangle turned by `rot` and moved to (cx, cz).
fn box_of(x0: f64, x1: f64, z0: f64, z1: f64, cx: f64, cz: f64, rot: f64) -> Rect {
    let pts = [
        turn(x0, z0, rot),
        turn(x1, z0, rot),
        turn(x0, z1, rot),
        turn(x1, z1, rot),
    ];
    let xs = pts.iter().map(|p| p.0 + cx);
    let zs = pts.iter().map(|p| p.1 + cz);
    Rect {
        x0: xs.clone().fold(f64::INFINITY, f64::min),
        x1: xs.fold(f64::NEG_INFINITY, f64::max),
        z0: zs.clone().fold(f64::INFINITY, f64::min),
        z1: zs.fold(f64::NEG_INFINITY, f64::max),
    }
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.x0 < b.x1 - EPS && a.x1 > b.x0 + EPS && a.z0 < b.z1 - EPS && a.z1 > b.z0 + EPS
}

fn centre_in(a: Rect, b: Rect) -> bool {
    let (cx, cz) = ((a.x0 + a.x1) / 2.0, (a.z0 + a.z1) / 2.0);
    cx >= b.x0 - EPS && cx <= b.x1 + EPS && cz >= b.z0 - EPS && cz <= b.z1 + EPS
}

/// The numbers a violation names after `pre` (at a word start), each followed by `tail`.
fn numbers_after(text: &str, pre: &str, tail: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find(pre) {
        let start = from + at;
        let digits_at = start + pre.len();
        let word_start = pre.starts_with(' ')
            || text[..start]
                .chars()
                .next_back()
                .map_or(true, |c| !(c.is_alphanumeric() || c == '_'));
        let digits: String = text[digits_at..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if word_start && !digits.is_empty() && text[digits_at + digits.len()..].starts_with(tail) {
            if let Ok(n) = digits.parse() {
                out.push(n);
            }
        }
        from = digits_at;
    }
    out
}

/// The item and spot numbers the violations name: `item N` / `items N`, ` and N (` (the second item of a pair), and
/// `spot N`.
fn named(violations: &[String]) -> (Vec<usize>, Vec<usize>) {
    let (mut items, mut spots) = (Vec::new(), Vec::new());
    for v in violations {
        items.extend(numbers_after(v, "item ", ""));
        items.extend(numbers_after(v, "items ", ""));
        items.extend(numbers_after(v, " and ", " ("));
        spots.extend(numbers_after(v, "spot ", ""));
    }
    items.sort_unstable();
    items.dedup();
    spots.sort_unstable();
    spots.dedup();
    (items, spots)
}

/// The materials of one overlay: one per state colour, a marker, the grid's lines and one per spot clip.
struct Kit {
    solid: Mat,
    walk: Mat,
    door: Mat,
    clear: Mat,
    marker: Mat,
    grid: Mat,
    spots: std::collections::BTreeMap<String, Mat>,
}

fn flat(color: u32, opacity: f32) -> Mat {
    let transparent = opacity < 1.0;
    Mat::new(Material {
        shading: Shading::Basic,
        color: Color::hex(color),
        opacity,
        transparent,
        depth_write: !transparent,
        fog: false,
        decal: transparent,
        ..Material::default()
    })
}

impl Kit {
    fn new() -> Kit {
        Kit {
            solid: flat(SOLID, 0.5),
            walk: flat(WALK, 0.5),
            door: flat(DOOR, 0.5),
            clear: flat(CLEAR, 0.5),
            marker: flat(MARKER, 1.0),
            grid: flat(GRID, 0.25),
            spots: Default::default(),
        }
    }

    /// The dot of a spot: its clip's hue (as the web's spotMaterial), white when the spot has no clip.
    fn spot(&mut self, clip: Option<&str>) -> Mat {
        let key = clip.unwrap_or("").to_string();
        self.spots
            .entry(key)
            .or_insert_with(|| match clip {
                None => flat(0xffffff, 1.0),
                Some(c) => {
                    let hue = (fnv1a32(c.as_bytes()) % 360) as f32 / 360.0;
                    Mat::new(Material {
                        shading: Shading::Basic,
                        color: Color::hsl(hue, 0.85, 0.5),
                        ..Material::default()
                    })
                }
            })
            .clone()
    }
}

/// A flat square or strip on the floor (a plane laid down), at (x, y, z).
fn flat_at(geo: &crate::scene::Geo, mat: &Mat, x: f32, y: f32, z: f32) -> Obj {
    let m = Obj::mesh(geo, mat);
    m.set_rotation(-std::f32::consts::FRAC_PI_2, 0.0, 0.0)
        .set_position(x, y, z)
        .set_shadows(false, false);
    m
}

/// The overlay of one room, in the room's local frame (the caller places it where the room stands).
pub fn overlay(o: &Overlay) -> Obj {
    let (w, d) = (o.w, o.d);
    let cell = o.catalog.cell.unwrap_or(CELL_DEFAULT);
    let cols = (w / cell).floor() as usize;
    let rows = (d / cell).floor() as usize;
    let (gx, gz) = (-w / 2.0, -d / 2.0);
    let cell_box = |i: usize, j: usize| Rect {
        x0: gx + i as f64 * cell,
        x1: gx + (i + 1) as f64 * cell,
        z0: gz + j as f64 * cell,
        z1: gz + (j + 1) as f64 * cell,
    };
    let cells: Vec<(usize, usize)> = (0..rows)
        .flat_map(|j| (0..cols).map(move |i| (i, j)))
        .collect();
    let rects: Vec<Rect> = cells.iter().map(|&(i, j)| cell_box(i, j)).collect();

    // Occupancy: 2 = a blocking solid, 1 = a walk item. Reservations: 1 = a clear zone, 2 = the door or the spine.
    let mut occ = vec![0u8; rects.len()];
    let mut res = vec![0u8; rects.len()];
    for it in &o.layout.items {
        let Some(cat) = o.catalog.items.get(&it.id) else {
            continue;
        };
        let [cw, _, cd] = cat.size;
        let (hw, hd) = (cw / 2.0, cd / 2.0);
        let floor_layer = cat.layer.as_deref() == Some("floor");
        let solid = cat.place != Place::Surface && !floor_layer && cat.place != Place::Wallmount;
        if solid {
            let body = box_of(-hw, hw, -hd, hd, it.x, it.z, it.rot);
            for (k, c) in rects.iter().enumerate() {
                if overlaps(*c, body) {
                    occ[k] = if cat.walk { occ[k].max(1) } else { 2 };
                }
            }
        }
        let (front, side) = cat.clear.map_or((0.0, 0.0), |c| (c.front, c.side));
        if front > 0.0 && !floor_layer {
            let zone = box_of(-hw - side, hw + side, hd, hd + front, it.x, it.z, it.rot);
            for (k, c) in rects.iter().enumerate() {
                if overlaps(*c, zone) && res[k] == 0 {
                    res[k] = 1;
                }
            }
        }
    }
    let width = ((if matches!(o.door, Door::N | Door::S) {
        w
    } else {
        d
    }) - 0.5)
        .min(1.0);
    let door_box = match o.door {
        Door::N => Rect {
            x0: -width / 2.0,
            x1: width / 2.0,
            z0: -d / 2.0,
            z1: -d / 2.0 + 1.0,
        },
        Door::S => Rect {
            x0: -width / 2.0,
            x1: width / 2.0,
            z0: d / 2.0 - 1.0,
            z1: d / 2.0,
        },
        Door::E => Rect {
            x0: w / 2.0 - 1.0,
            x1: w / 2.0,
            z0: -width / 2.0,
            z1: width / 2.0,
        },
        Door::W => Rect {
            x0: -w / 2.0,
            x1: -w / 2.0 + 1.0,
            z0: -width / 2.0,
            z1: width / 2.0,
        },
    };
    let spine_box = match o.door {
        Door::N => Rect {
            x0: -0.25,
            x1: 0.25,
            z0: -d / 2.0,
            z1: 0.0,
        },
        Door::S => Rect {
            x0: -0.25,
            x1: 0.25,
            z0: 0.0,
            z1: d / 2.0,
        },
        Door::E => Rect {
            x0: 0.0,
            x1: w / 2.0,
            z0: -0.25,
            z1: 0.25,
        },
        Door::W => Rect {
            x0: -w / 2.0,
            x1: 0.0,
            z0: -0.25,
            z1: 0.25,
        },
    };
    for (k, c) in rects.iter().enumerate() {
        if centre_in(*c, door_box) || centre_in(*c, spine_box) {
            res[k] = 2;
        }
    }

    let mut kit = Kit::new();
    let group = Obj::group();
    // The grid lines (0.025 above the floor), then one tinted square per cell that carries a state.
    let line_across = geometry::plane(cols as f32 * cell as f32, 0.02, 1, 1);
    let line_along = geometry::plane(0.02, rows as f32 * cell as f32, 1, 1);
    for j in 0..=rows {
        let z = gz + j as f64 * cell;
        group.add(&flat_at(
            &line_across,
            &kit.grid,
            (gx + cols as f64 * cell / 2.0) as f32,
            0.025,
            z as f32,
        ));
    }
    for i in 0..=cols {
        let x = gx + i as f64 * cell;
        group.add(&flat_at(
            &line_along,
            &kit.grid,
            x as f32,
            0.025,
            (gz + rows as f64 * cell / 2.0) as f32,
        ));
    }
    let tile = geometry::plane(cell as f32 * 0.94, cell as f32 * 0.94, 1, 1);
    for (k, c) in rects.iter().enumerate() {
        let mat = match (occ[k], res[k]) {
            (2, _) => kit.solid.clone(),
            (_, 2) => kit.door.clone(),
            (1, _) => kit.walk.clone(),
            (_, 1) => kit.clear.clone(),
            _ => continue,
        };
        let (cx, cz) = ((c.x0 + c.x1) / 2.0, (c.z0 + c.z1) / 2.0);
        group.add(&flat_at(&tile, &mat, cx as f32, 0.03, cz as f32));
    }

    // The spots, a dot each, coloured by clip.
    let dot = geometry::sphere(0.04, 8, 6);
    for sp in &o.layout.spots {
        let m = Obj::mesh(&dot, &kit.spot(sp.clip.as_deref()));
        m.set_position(sp.x as f32, 0.05, sp.z as f32)
            .set_shadows(false, false);
        group.add(&m);
    }

    // The violations: a red marker and its pin over each item or spot they name.
    let (item_marks, spot_marks) = named(o.violations);
    let ball = geometry::sphere(0.09, 10, 8);
    let pin_geo = |top: f32| geometry::box_geo(0.02, top, 0.02);
    let pin = |group: &Obj, x: f32, y: f32, z: f32, top: f32| {
        let b = Obj::mesh(&ball, &kit.marker);
        b.set_position(x, top, z).set_shadows(false, false);
        group.add(&b);
        let stick = Obj::mesh(&pin_geo(top - y), &kit.marker);
        stick
            .set_position(x, (top + y) / 2.0, z)
            .set_shadows(false, false);
        group.add(&stick);
    };
    for n in item_marks {
        if let Some(it) = o.layout.items.get(n) {
            pin(
                &group,
                it.x as f32,
                it.y as f32,
                it.z as f32,
                it.y as f32 + 1.1,
            );
        }
    }
    for n in spot_marks {
        if let Some(sp) = o.layout.spots.get(n) {
            pin(&group, sp.x as f32, 0.0, sp.z as f32, 0.9);
        }
    }
    group
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn violations_name_their_items_and_spots() {
        let v = vec![
            "item 3 (desk) overlaps item 5 (chair)".to_string(),
            "items 7 and 9 (desk, chair) block the door".to_string(),
            "spot 4 has no clip".to_string(),
            "reitem 8 is not a word".to_string(),
        ];
        let (items, spots) = named(&v);
        assert_eq!(items, vec![3, 5, 7, 9]);
        assert_eq!(spots, vec![4]);
    }

    #[test]
    fn turns_follow_the_verifier() {
        assert_eq!(turn(1.0, 2.0, 90.0), (2.0, -1.0));
        assert_eq!(turn(1.0, 2.0, 180.0), (-1.0, -2.0));
        assert_eq!(turn(1.0, 2.0, 270.0), (-2.0, 1.0));
        assert_eq!(turn(1.0, 2.0, 0.0), (1.0, 2.0));
    }
}
