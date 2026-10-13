//! The lift (`lift.ts`): a chunky toy elevator on a plate's landing. Four thick pillars on footing plates, a header
//! housing with pulley drums, a cab (an accent deck, railing tubes on its closed sides, a light bar that glows while it
//! travels), two sliding gate leaves at each stop, a call post with a round lamp beside each gate, and a counterweight
//! riding a guide outside the frame. The frame is in the lift's own frame: z = 0 is the front cell's centre, +z out
//! toward the walkway, -z toward the plate. The controller's state arrives through `set_state`.

use std::f32::consts::FRAC_PI_2;

use glam::{Mat4, Quat, Vec3};

use super::Colors;
use crate::geometry::{box_geo, cylinder};
use crate::scene::{merge, Color, Geo, Mat, Material, Obj};
use crate::toon::{outline, toon_mat};

/// The cab's width along the wall (x), and its plane limits in z (the plate edge and the walkway side).
pub const CAB_W: f32 = 1.6;
pub const CAB_BACK: f32 = -0.12;
pub const CAB_FRONT: f32 = 0.37;
/// A stop's gate height above the stop.
pub const GATE_H: f32 = 0.45;

const ZC: f32 = (CAB_BACK + CAB_FRONT) / 2.0; // the shaft's centre line in z
const FOOT: f32 = 0.07; // footing plates under the pillars, the guide and the call posts
const TOP_PAD: f32 = 0.04; // the frame's top above the gate lintel's underside
const HEAD: f32 = 0.2; // the header housing on top of the frame
const PX: f32 = 0.92; // pillar centre x
const PIL: f32 = 0.18; // pillar section
const GATE_W: f32 = 0.8; // one gate leaf
const DECK_LIFT: f32 = 0.01; // the cab's deck sits this far above its stop
const RAIL_R: f32 = 0.03; // cab railing tube radius
const RAIL_H: f32 = 0.34; // cab railing height above the deck
const CANOPY_Y: f32 = 0.42; // the light bar's centre above the deck
const CANOPY_T: f32 = 0.07; // the light bar's thickness
const DECK_T: f32 = 0.12; // cab deck thickness
const CW_X: f32 = 1.25; // the counterweight's guide, outside the frame
const CW_W: f32 = 0.24;
const CW_H: f32 = 0.34;
const CW_D: f32 = 0.2;
const CALL_X: f32 = 1.15; // the call posts stand beside the pillars' outer faces
const CALL_H: f32 = 0.7; // a call post's height above its footing
const CABLE: f32 = 0.03; // cable section

/// Where a lift stands: its landing's x and z, the yaw of its outward normal, and the stop heights (the PCB top and the
/// plate top).
#[derive(Debug, Clone, Copy)]
pub struct LiftSpec {
    pub x: f32,
    pub z: f32,
    pub face: f32,
    pub low: f32,
    pub high: f32,
}

/// One gate: its two leaves (x = -off and +off) at a fixed height and depth.
struct Gate {
    left: Obj,
    right: Obj,
    y: f32,
    z: f32,
}

/// A lift built as scene nodes. `set_state` places the cab, counterweight, cables, gates and lamps.
pub struct LiftMesh {
    pub root: Obj,
    cab: Obj,
    cab_cable: Obj,
    counterweight: Obj,
    cw_cable: Obj,
    gates: Vec<Gate>,
    lamps: Vec<Obj>,
    lamp_idle: Mat,
    lamp_call: Mat,
    lamp_here: Mat,
    bar: Mat,
    bar_base: Color,
    glow: f32,
    low: f32,
    high: f32,
    top: f32,
}

/// The static pieces of one material, merged into one geometry: a box, a square member, a round tube or a disc.
#[derive(Default)]
struct Parts(Vec<(Geo, Mat4)>);

impl Parts {
    fn box_at(&mut self, w: f32, h: f32, d: f32, at: Vec3) {
        self.0.push((box_geo(w, h, d), Mat4::from_translation(at)));
    }

    /// A square member from a to b.
    fn beam(&mut self, a: Vec3, b: Vec3, sec: f32) {
        self.0.push((box_geo(sec, (b - a).length(), sec), along(a, b)));
    }

    /// A round tube from a to b.
    fn tube(&mut self, a: Vec3, b: Vec3, r: f32) {
        self.0.push((cylinder(r, r, (b - a).length(), 12), along(a, b)));
    }

    /// A disc whose axis is z (a pulley drum), centred at `at`.
    fn disc(&mut self, r: f32, depth: f32, at: Vec3) {
        let rot = Quat::from_rotation_x(FRAC_PI_2);
        self.0.push((cylinder(r, r, depth, 20), Mat4::from_rotation_translation(rot, at)));
    }

    fn merge(&self) -> Geo {
        merge(&self.0)
    }
}

/// The transform of a member from a to b: its axis is y, centred on the midpoint.
fn along(a: Vec3, b: Vec3) -> Mat4 {
    let dir = (b - a).normalize();
    Mat4::from_rotation_translation(Quat::from_rotation_arc(Vec3::Y, dir), (a + b) * 0.5)
}

/// One mesh at a local position, shadows on or off.
fn part(parent: &Obj, geo: &Geo, mat: &Mat, at: Vec3, shadow: bool) -> Obj {
    let m = Obj::mesh(geo, mat);
    m.set_position(at.x, at.y, at.z).set_shadows(shadow, shadow);
    parent.add(&m);
    m
}

impl LiftMesh {
    /// A lift on a plate's landing: the frame, the cab at the low stop, the gates, the call lamps, the counterweight.
    pub fn build(spec: &LiftSpec, colors: &Colors) -> LiftMesh {
        let (low, high) = (spec.low, spec.high);
        let top = high + GATE_H + TOP_PAD; // the frame's top; the header sits on it
        let base = low + FOOT; // the pillars stand on their footing plates

        let info = colors.info;
        let signal = colors.signal;
        let ok = colors.ok;
        let mat_steel = toon_mat(Material { color: Color::hex(0xdfe6f1), emissive: Color::hex(0x2a3446), ..Material::default() });
        let mat_cab_deck = toon_mat(Material { color: info, emissive: info, emissive_intensity: 0.15, ..Material::default() });
        let mat_stripe = toon_mat(Material { color: signal, emissive: signal, emissive_intensity: 0.4, ..Material::default() });
        let mat_tower = toon_mat(Material { color: Color::hex(0x8c9cb8), emissive: Color::hex(0x1f2a3e), ..Material::default() });
        let mat_panel = toon_mat(Material { color: Color::hex(0x2d3642), ..Material::default() });
        let mat_gate = toon_mat(Material { color: Color::hex(0xc9d2e0), emissive: Color::hex(0x20283a), ..Material::default() });
        let lamp_idle = Mat::basic(info.scale(0.3));
        let lamp_call = Mat::basic(signal);
        let lamp_here = Mat::basic(ok);
        let bar = Mat::basic(info.scale(0.3));

        let root = Obj::group();
        root.set_position(spec.x, 0.0, spec.z).set_rotation(0.0, spec.face, 0.0);

        // The static frame: pillars, footings, the header, the lintels, the mid rails, the X braces, the guide, the drums.
        let mut tower = Parts::default();
        let mut steel = Parts::default();
        let mut panel = Parts::default();
        for sx in [-1.0f32, 1.0] {
            for z in [CAB_BACK, CAB_FRONT] {
                tower.box_at(PIL, top - base + 0.02, PIL, Vec3::new(sx * PX, (base + top) / 2.0, z));
                tower.box_at(0.2, FOOT + 0.03, 0.2, Vec3::new(sx * PX, low + (FOOT - 0.03) / 2.0, z));
            }
        }
        tower.box_at(2.44, HEAD, 0.7, Vec3::new(0.17, top + HEAD / 2.0, ZC));
        tower.box_at(0.2, FOOT + 0.03, 0.2, Vec3::new(CW_X, low + (FOOT - 0.03) / 2.0, ZC - 0.12));
        for z in [CAB_BACK, CAB_FRONT] {
            steel.box_at(2.0 * PX + PIL - 0.02, 0.06, 0.1, Vec3::new(0.0, top - 0.05, z));
            steel.box_at(2.0 * PX + PIL - 0.02, 0.06, 0.1, Vec3::new(0.0, (low + top) / 2.0, z));
        }
        for sx in [-1.0f32, 1.0] {
            steel.beam(Vec3::new(sx * PX, base, CAB_BACK), Vec3::new(sx * PX, top, CAB_FRONT), 0.07);
            steel.beam(Vec3::new(sx * PX, top, CAB_BACK), Vec3::new(sx * PX, base, CAB_FRONT), 0.07);
        }
        steel.beam(Vec3::new(CW_X, base - 0.01, ZC - 0.12), Vec3::new(CW_X, top + 0.01, ZC - 0.12), 0.07);
        steel.disc(0.085, 0.05, Vec3::new(0.0, top + HEAD / 2.0, ZC + 0.37));
        steel.disc(0.085, 0.05, Vec3::new(CW_X, top + HEAD / 2.0, ZC + 0.37));

        // The call posts: one per stop, beside the pillar away from the cab's centre; a dark housing and a round lamp.
        let mut lamps = Vec::with_capacity(2);
        for st in 0..2usize {
            let b = if st == 0 { low } else { high };
            let x = if st == 0 { -CALL_X } else { CALL_X };
            let z = if st == 0 { CAB_FRONT + 0.16 } else { CAB_BACK - 0.16 };
            let face = if st == 0 { 1.0 } else { -1.0 }; // the lamp looks out to the approach
            let y = b + FOOT + CALL_H;
            tower.box_at(0.2, FOOT + 0.03, 0.2, Vec3::new(x, b + (FOOT - 0.03) / 2.0, z));
            steel.tube(Vec3::new(x, b + FOOT - 0.02, z), Vec3::new(x, y, z), 0.03);
            panel.box_at(0.2, 0.26, 0.06, Vec3::new(x, y, z));
            let lamp = Obj::mesh(&cylinder(0.085, 0.085, 0.04, 20), &lamp_idle);
            lamp.set_quaternion(Quat::from_rotation_x(FRAC_PI_2)).set_position(x, y, z + face * 0.045).set_shadows(false, false);
            root.add(&lamp);
            lamps.push(lamp);
        }
        part(&root, &tower.merge(), &mat_tower, Vec3::ZERO, true);
        part(&root, &steel.merge(), &mat_steel, Vec3::ZERO, true);
        part(&root, &panel.merge(), &mat_panel, Vec3::ZERO, true);

        // The cab: its accent deck (a walkable platform), a signal-coloured lip on the walkway side, railing tubes on the
        // two closed sides, and a light bar across the cab on two posts.
        let cab = Obj::group();
        root.add(&cab);
        let deck = part(&cab, &box_geo(CAB_W, DECK_T, CAB_FRONT - CAB_BACK), &mat_cab_deck, Vec3::new(0.0, -DECK_T / 2.0, ZC), true);
        deck.set_walk();
        part(&cab, &box_geo(CAB_W - 0.02, 0.035, 0.05), &mat_stripe, Vec3::new(0.0, 0.01, CAB_FRONT - 0.02), false);
        let mut rails = Parts::default();
        for sx in [-1.0f32, 1.0] {
            let x = sx * (CAB_W / 2.0 - RAIL_R);
            for z in [CAB_BACK, CAB_FRONT] {
                rails.tube(Vec3::new(x, -0.02, z), Vec3::new(x, RAIL_H, z), RAIL_R);
            }
            rails.tube(Vec3::new(x, RAIL_H, CAB_BACK), Vec3::new(x, RAIL_H, CAB_FRONT), RAIL_R);
            rails.tube(Vec3::new(x, RAIL_H / 2.0, CAB_BACK), Vec3::new(x, RAIL_H / 2.0, CAB_FRONT), RAIL_R);
            rails.tube(Vec3::new(x, RAIL_H, ZC), Vec3::new(x, CANOPY_Y, ZC), RAIL_R); // the light bar's post
        }
        part(&cab, &rails.merge(), &mat_steel, Vec3::ZERO, true);
        part(&cab, &box_geo(CAB_W - 0.1, CANOPY_T, 0.08), &bar, Vec3::new(0.0, CANOPY_Y, ZC), false);

        // The cables and the counterweight: each cable is a unit box scaled to its length in setState.
        let cab_cable = part(&root, &box_geo(CABLE, 1.0, CABLE), &mat_steel, Vec3::new(0.0, 0.0, ZC), false);
        let counterweight = part(&root, &box_geo(CW_W, CW_H, CW_D), &mat_tower, Vec3::new(CW_X, 0.0, ZC), true);
        let cw_cable = part(&root, &box_geo(CABLE, 1.0, CABLE), &mat_steel, Vec3::new(CW_X, 0.0, ZC), false);

        // The gates: one leaf pair per stop, sliding apart as the doors open.
        let mut gate_parts = Parts::default();
        gate_parts.box_at(GATE_W, GATE_H, 0.03, Vec3::ZERO);
        for dy in [-0.15f32, -0.05, 0.05, 0.15] {
            gate_parts.box_at(GATE_W - 0.06, 0.04, 0.05, Vec3::new(0.0, dy, 0.02)); // the slats
        }
        let gate_geo = gate_parts.merge();
        let mut gates = Vec::with_capacity(2);
        for st in 0..2usize {
            let y = (if st == 0 { low } else { high }) - 0.02 + GATE_H / 2.0; // its foot sinks into the board or the plate
            let z = if st == 0 { CAB_FRONT } else { CAB_BACK };
            let off = GATE_W / 2.0 + 0.01;
            let left = part(&root, &gate_geo, &mat_gate, Vec3::new(-off, y, z), true);
            let right = part(&root, &gate_geo, &mat_gate, Vec3::new(off, y, z), true);
            gates.push(Gate { left, right, y, z });
        }

        let mut lift = LiftMesh {
            root,
            cab,
            cab_cable,
            counterweight,
            cw_cable,
            gates,
            lamps,
            lamp_idle,
            lamp_call,
            lamp_here,
            bar,
            bar_base: info,
            glow: 0.3,
            low,
            high,
            top,
        };
        lift.set_state(low, 0.0, 0, false, [0, 0]);
        outline(&lift.root, 0.0);
        lift
    }

    /// The controller's state: `cab_y` the cab floor height, `door_t` 0..1 the gates' opening at stop `stop` (0 low,
    /// 1 high; the other stop stays shut), `travelling` whether the cab is moving, `calls` the robots waiting per stop.
    pub fn set_state(&mut self, cab_y: f32, door_t: f32, stop: usize, travelling: bool, calls: [usize; 2]) {
        let top = self.top;
        self.cab.set_position(0.0, cab_y + DECK_LIFT, 0.0);

        let bar_y = cab_y + DECK_LIFT + CANOPY_Y + CANOPY_T / 2.0;
        let cab_len = (top + 0.02 - (bar_y - 0.01)).max(0.01); // sunk 0.01 into the bar and the header
        self.cab_cable.set_position(0.0, bar_y - 0.01 + cab_len / 2.0, ZC).set_scale(1.0, cab_len, 1.0);

        // The counterweight runs opposite the cab, never below the board.
        let cw_y = self.low + FOOT + 0.02 + CW_H / 2.0 + (self.high - cab_y);
        self.counterweight.set_position(CW_X, cw_y, ZC);
        let cw_top = cw_y + CW_H / 2.0;
        let cw_len = (top + 0.02 - (cw_top - 0.01)).max(0.01);
        self.cw_cable.set_position(CW_X, cw_top - 0.01 + cw_len / 2.0, ZC).set_scale(1.0, cw_len, 1.0);

        let target = if travelling { 1.0 } else { 0.3 };
        self.glow += (target - self.glow) * 0.15;
        self.bar.get().color = self.bar_base.scale(self.glow);

        for st in 0..2usize {
            let open = if stop == st { door_t } else { 0.0 };
            let off = GATE_W / 2.0 + 0.01 + open * GATE_W;
            let gate = &self.gates[st];
            gate.left.set_position(-off, gate.y, gate.z);
            gate.right.set_position(off, gate.y, gate.z);
            let here = stop == st && !travelling;
            let mat = if here {
                &self.lamp_here
            } else if calls[st] > 0 {
                &self.lamp_call
            } else {
                &self.lamp_idle
            };
            self.lamps[st].set_material(mat);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lift() -> LiftMesh {
        LiftMesh::build(&LiftSpec { x: 0.0, z: 0.0, face: 0.0, low: 0.4, high: 2.4 }, &Colors::default())
    }

    #[test]
    fn the_cab_follows_the_state() {
        let mut m = lift();
        let mut count = 0;
        m.root.traverse(&mut |_: &Obj| count += 1);
        assert!(count > 10, "the lift has its frame, cab, gates and lamps");

        m.set_state(1.3, 0.0, 0, false, [0, 0]);
        assert!((m.cab.position().y - (1.3 + DECK_LIFT)).abs() < 1e-5);
        m.set_state(2.4, 0.0, 1, false, [0, 0]);
        assert!((m.cab.position().y - (2.4 + DECK_LIFT)).abs() < 1e-5);
    }

    #[test]
    fn a_gate_leaf_moves_out_as_the_door_opens() {
        let mut m = lift();
        m.set_state(0.4, 0.0, 0, false, [0, 0]);
        let shut = m.gates[0].left.position().x;
        m.set_state(0.4, 1.0, 0, false, [0, 0]);
        let open = m.gates[0].left.position().x;
        assert!(open < shut - 0.5, "the left leaf slides out past its shut place");
        // The other stop stays shut.
        assert!((m.gates[1].left.position().x + GATE_W / 2.0 + 0.01).abs() < 1e-5);
    }
}
