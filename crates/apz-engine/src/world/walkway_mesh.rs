//! The gangway and the ramp (`gangway.ts`, `ramp.ts`): a slim bridge at plate height from a plate's
//! edge out to the lift's cab, and a sloped wedge from a terrace edge down to the board. Both are
//! built in world coordinates (y absolute) and shadowed like the rest of the floor.

use crate::geometry::box_geo;
use crate::scene::{Color, Mat, Material, Obj};

use super::Colors;

/// `CAB_BACK` in lift.ts: the cab's back plane sits this far from the shaft's centre.
const CAB_BACK: f32 = -0.12;

const WIDTH: f32 = 0.5; // deck width
const THICK: f32 = 0.06; // deck thickness under its top surface
const RAIL: f32 = 0.04; // rail section
const RAIL_H: f32 = 0.3; // rail height above the deck
const POST: f32 = 0.06; // support post section
const SPAN: f32 = 1.0; // distance between supports

const RAMP_THICK: f32 = 0.06; // the wedge's thickness under its top surface
const RAMP_RAIL_W: f32 = 0.03; // side rail width
const RAMP_RAIL_H: f32 = 0.12; // side rail height above the top surface
const RAMP_GLOW_H: f32 = 0.012; // the light strip on each rail's cap

/// A gangway landing of a plate: the plate edge (ax, az), the shaft's board cell (bx, bz), the
/// outward normal (nx, nz), and the bridge's cells (its first is the start; empty means (ax, az)).
pub struct GangwaySpec {
    pub ax: f32,
    pub az: f32,
    pub bx: f32,
    pub bz: f32,
    pub nx: f32,
    pub nz: f32,
    pub bridge: Vec<[f32; 2]>,
}

/// A gangway: the deck from the plate edge out along the normal to the cab's plate side, its two
/// rails and a support every `SPAN` down to the solid under it (`top_at`, a world point's blocked
/// top; a support never reaches below it). The deck's top is `plate_y`.
///
/// `_pcb_top` and `_colors` are unused: the TS default `topAt` is the PCB top, which the caller's
/// `top_at` replaces, and the gangway's materials are fixed (lift.ts `liftMats`' deck and steel).
pub fn build_gangway(
    spec: &GangwaySpec,
    plate_y: f32,
    _pcb_top: f32,
    _colors: &Colors,
    top_at: &dyn Fn(f32, f32) -> f32,
) -> Obj {
    let deck = Mat::new(Material {
        color: Color::hex(0x7d8aa0),
        emissive: Color::hex(0x1e2636),
        ..Material::default()
    });
    let steel = Mat::new(Material {
        color: Color::hex(0xdfe6f1),
        emissive: Color::hex(0x2a3446),
        ..Material::default()
    });
    let group = Obj::group();
    group.set_name("gangway");

    // The bridge runs from its first cell along the normal to the cab's plate side. Local +z is the normal.
    let [sx, sz] = spec.bridge.first().copied().unwrap_or([spec.ax, spec.az]);
    let (nx, nz) = (spec.nx, spec.nz);
    let len = ((spec.bx - sx) * nx + (spec.bz - sz) * nz + CAB_BACK).max(0.01);
    let bridge = Obj::group();
    bridge
        .set_position(sx, 0.0, sz)
        .set_rotation(0.0, nx.atan2(nz), 0.0);
    group.add(&bridge);

    part(
        &bridge,
        WIDTH,
        THICK,
        len,
        &deck,
        0.0,
        plate_y - THICK / 2.0,
        len / 2.0,
    );
    for side in [-1.0_f32, 1.0] {
        part(
            &bridge,
            RAIL,
            RAIL_H,
            len,
            &steel,
            side * (WIDTH / 2.0 - RAIL / 2.0),
            plate_y + RAIL_H / 2.0,
            len / 2.0,
        );
    }

    // A support every SPAN, from the deck's underside down to the solid it stands on (never below it).
    let bottom = plate_y - THICK;
    let mut s = SPAN / 2.0;
    while s < len {
        let base = top_at(sx + nx * s, sz + nz * s);
        if bottom - base > 0.01 {
            part(
                &bridge,
                POST,
                bottom - base,
                POST,
                &steel,
                0.0,
                (bottom + base) / 2.0,
                s,
            );
        }
        s += SPAN;
    }
    group
}

/// A ramp: a sloped wedge from a terrace edge (`high`) down to the PCB (`low`), with a side rail and
/// a glow strip each way. The group sits at (x, 0, z) with rotation y = `face`, so local +z points
/// out from the terrace; the slope starts at the origin and runs `length` along +z (horizontally).
pub struct RampSpec {
    pub x: f32,
    pub z: f32,
    pub face: f32,
    pub low: f32,
    pub high: f32,
    pub width: f32,
    pub length: f32,
}

pub fn build_ramp(spec: &RampSpec, colors: &Colors) -> Obj {
    let length = spec.length.max(0.01);
    let rise = spec.high - spec.low;
    let slope_len = length.hypot(rise); // the wedge's length along its own slope
    let deck = Mat::toon(Color::hex(0x46505e));
    let steel = Mat::toon(Color::hex(0x8f9bab));
    let glow = Mat::basic(colors.info);

    let group = Obj::group();
    group.set_name("ramp");
    group
        .set_position(spec.x, 0.0, spec.z)
        .set_rotation(0.0, spec.face, 0.0);
    // The slope frame: its origin on the terrace-level top line, tilted about x so +z drops to the PCB.
    let slope = Obj::group();
    slope
        .set_position(0.0, spec.high, 0.0)
        .set_rotation(rise.atan2(length), 0.0, 0.0);
    group.add(&slope);

    part(
        &slope,
        spec.width,
        RAMP_THICK,
        slope_len,
        &deck,
        0.0,
        -RAMP_THICK / 2.0,
        slope_len / 2.0,
    );
    let rail_x = spec.width / 2.0 - RAMP_RAIL_W / 2.0;
    for side in [-1.0_f32, 1.0] {
        part(
            &slope,
            RAMP_RAIL_W,
            RAMP_RAIL_H,
            slope_len,
            &steel,
            side * rail_x,
            RAMP_RAIL_H / 2.0,
            slope_len / 2.0,
        );
        part(
            &slope,
            RAMP_RAIL_W,
            RAMP_GLOW_H,
            slope_len,
            &glow,
            side * rail_x,
            RAMP_RAIL_H + RAMP_GLOW_H / 2.0,
            slope_len / 2.0,
        );
    }
    group
}

/// The ramp from point `a` (on the plate, at `a_y`) down to point `b` (on the board, at `b_y`): the
/// face points along a -> b, and the length is their horizontal distance. Width is the TS default, 0.6.
pub fn ramp_between(a: [f32; 2], a_y: f32, b: [f32; 2], b_y: f32) -> RampSpec {
    let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
    RampSpec {
        x: a[0],
        z: a[1],
        face: dx.atan2(dz),
        low: b_y,
        high: a_y,
        width: 0.6,
        length: dx.hypot(dz),
    }
}

/// A box mesh with its shadows on, at (x, y, z) under `parent`.
#[allow(clippy::too_many_arguments)]
fn part(parent: &Obj, w: f32, h: f32, d: f32, mat: &Mat, x: f32, y: f32, z: f32) {
    let geo = box_geo(w, h, d);
    let m = Obj::mesh(&geo, mat);
    m.set_position(x, y, z).set_shadows(true, true);
    parent.add(&m);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square_bridge() -> GangwaySpec {
        // A bridge along +z from the plate edge (0, 0) to the shaft's cell (0, 4).
        GangwaySpec {
            ax: 0.0,
            az: 0.0,
            bx: 0.0,
            bz: 4.0,
            nx: 0.0,
            nz: 1.0,
            bridge: vec![[0.0, 0.0], [0.0, 1.0], [0.0, 2.0], [0.0, 3.0]],
        }
    }

    /// The bridge group: the gangway's first child.
    fn bridge_of(gw: &Obj) -> Obj {
        let node = gw.get();
        let bridge = node.children[0].clone();
        bridge
    }

    #[test]
    fn gangway_deck_length_matches_the_formula() {
        let gw = build_gangway(&square_bridge(), 0.5, 0.0, &Colors::default(), &|_, _| 0.0);
        let deck = {
            let bridge = bridge_of(&gw);
            let node = bridge.get();
            let deck = node.children[0].clone();
            deck
        };
        // len = (bz - sz) * nz + CAB_BACK; the deck is centred at len / 2 along the bridge.
        let len = 4.0 + CAB_BACK;
        assert!((deck.position().z - len / 2.0).abs() < 1e-5);
    }

    #[test]
    fn gangway_posts_are_skipped_when_the_plate_is_near_the_ground() {
        // Deck underside at -0.01 over ground at 0: within 0.01, so no posts (deck and two rails only).
        let low = build_gangway(&square_bridge(), 0.05, 0.0, &Colors::default(), &|_, _| 0.0);
        assert_eq!(bridge_of(&low).get().children.len(), 3);
        // A plate a metre up: posts at 0.5, 1.5, 2.5 and 3.5 (the length is 3.88).
        let high = build_gangway(&square_bridge(), 1.0, 0.0, &Colors::default(), &|_, _| 0.0);
        assert_eq!(bridge_of(&high).get().children.len(), 3 + 4);
    }

    #[test]
    fn ramp_between_faces_the_board_and_spans_the_horizontal_distance() {
        let spec = ramp_between([1.0, 2.0], 0.5, [4.0, 6.0], 0.0);
        assert!((spec.face - 3.0_f32.atan2(4.0)).abs() < 1e-6);
        assert!((spec.length - 5.0).abs() < 1e-6);
        assert_eq!((spec.x, spec.z), (1.0, 2.0));
        assert_eq!((spec.high, spec.low), (0.5, 0.0));
    }
}
