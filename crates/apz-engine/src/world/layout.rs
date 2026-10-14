//! `floor.ts` lines ~123–710: the world's base (sky, ground, backdrop, dust), a machine's board (the
//! rooms on its slots, the robots in and out, the copper they walk on, the atom), and the view's fit.
//! The ring plaza (districts round a dais, with spokes and outskirts) is gone: the board replaces it.

use std::collections::{HashMap, HashSet};
use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI, SQRT_2, TAU};
use std::time::Duration;
use web_time::Instant;

use glam::{Mat4, Quat, Vec3};

use super::board_layout::{
    layout_on_board, request_shared, shared_key_for, shared_pending, trace_graph, BoardDistrict, BoardLayout, BoardParams, Bounds,
    SharedKey, TraceGraph,
};
use std::cell::RefCell;
use super::{AgentInfo, Board, Bot, Colors, District, DistrictLight, Floor, Shared, TracePulses};
use crate::canvas::Canvas;
use crate::entry_scene::{board_key, release_board, take_board};
use crate::controls::FOV_DEG;
use crate::geometry;
use crate::hardware::{self, Model, Params, Pulses, Status};
use crate::outskirts;
use crate::props::{Box2, PropPalette};
use crate::robot::{species_of, Robot};
use crate::rooms::generate::{room_seed, Door};
use crate::rooms::{self, RoomOptions, WALL_MARGIN};
use crate::scene::{Color, Kind, Mat, Material, Obj, Shading, Tex};
use crate::textures::{self, Rng};
use crate::toon;

// ---- Constants (floor.ts 25–40) -----------------------------------------------------------------

pub const PACKET_MS: f64 = 1500.0;

pub const SPEED: f32 = 1.6; // grid units per second; one grid cell is one world unit
pub const MAX_PACKETS: usize = 60;
pub const SLAB_H: f32 = 0.5; // a room's terrace: the height of its flat top above its slot
pub const RAMP: f32 = 1.5; // width of the sloped band around a room's terrace
pub const GRAVITY: f32 = 9.8; // world units per second squared
pub const POLAR: f32 = 0.95; // camera angle from vertical, about 55 degrees: isometric-ish
pub const AZIMUTH: f32 = FRAC_PI_4;

const FIT_FILL: f32 = 0.85;

pub const SOCIAL_NEAR: f32 = 9.5; // rooms this close count as neighbours for a visit or a chat
pub const STROLL_R: f32 = 3.1; // the stroll ring just outside the centre

pub const ROBOT_R: f32 = 0.55; // robot footprint radius for collisions
const DEG: f32 = PI / 180.0;
pub const STEER: [f32; 5] = [0.0, 30.0 * DEG, -30.0 * DEG, 60.0 * DEG, -60.0 * DEG];
pub const DUST_N: usize = 90;

/// The dark ground under a board, well below it.
const BOARD_GROUND_Y: f32 = -4.0;

const HUES: [f32; 10] = [8.0, 28.0, 46.0, 88.0, 140.0, 172.0, 198.0, 222.0, 262.0, 340.0];

#[allow(dead_code)] // read by motion.rs
/// `--ink-2`, the plaque's ink. The web reads it from CSS; `Colors` does not carry it.
const INK2: &str = "#4c544d";

/// The orchestrator: `tier === 'orchestrator'`.
fn is_orchestrator(a: &AgentInfo) -> bool {
    a.tier == "orchestrator"
}

fn children(o: &Obj) -> Vec<Obj> {
    o.get().children.clone()
}

fn prop_palette(c: &Colors) -> PropPalette {
    PropPalette { sheet: c.sheet, rule: c.rule, ink3: c.ink3, signal: c.signal, ok: c.ok, info: c.info }
}

/// A colour as CSS for the canvas.
fn css(c: Color) -> String {
    let [r, g, b] = c.to_srgb_u8();
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// `agentHue(cid)` and `roleHue(role)`: degrees.
pub fn agent_hue(cid: u64) -> f32 {
    ((cid as f64 * 137.508 + 20.0) % 360.0) as f32
}

pub fn role_hue(role: &str) -> f32 {
    let mut hash: u32 = 0;
    let mut buf = [0u16; 2];
    for ch in role.chars() {
        let code = ch.encode_utf16(&mut buf)[0] as u32;
        hash = hash.wrapping_mul(31).wrapping_add(code);
    }
    HUES[(hash % HUES.len() as u32) as usize]
}

/// The shared pad, packet, halo and token geometry and materials (floor.ts mount).
pub(super) fn shared(colors: &Colors) -> Shared {
    // Halos: transparent, additive, opacity 0.35, no depth write.
    let halo = |color: Color| {
        Mat::new(Material {
            shading: Shading::Basic,
            color,
            transparent: true,
            opacity: 0.35,
            depth_write: false,
            additive: true,
            ..Material::default()
        })
    };
    Shared {
        pad_geo: geometry::circle(0.42, 28, 0.0, TAU),
        packet_geo: geometry::sphere(0.13, 14, 10),
        halo_geo: geometry::sphere(0.34, 14, 10),
        token_geo: geometry::box_geo(0.2, 0.2, 0.2),
        accent: Mat::basic(colors.signal),
        ok: Mat::basic(colors.ok),
        accent_halo: halo(colors.signal),
        ok_halo: halo(colors.ok),
        token: Mat::basic(colors.signal),
    }
}

/// Sky, stars, the ground, the backdrop and dust, added to `floor.root`. The Machines view keeps the
/// ground; a machine's board stands on the dark ground (`show_board`). The fog and lights are
/// `World`'s defaults, which are floor.ts's values.
pub(super) fn base(floor: &mut Floor) {

    // Night: a deep blue zenith banded down to a violet horizon glow, stars and a ringed planet.
    floor.root.add(&toon::sky(Color::hex(0x05071a), Color::hex(0x3a2275), Color::hex(0x090c18)));

    let sky = toon::starfield(900, 330.0, &mut Rng::new(31));
    floor.stars = sky.group;
    floor.planet = sky.planet;
    floor.stars.set_dynamic(true); // turns every frame
    floor.root.add(&floor.stars);

    // Ground: one smooth sheet out to the horizon.
    let ground_map = textures::ground();
    let ground_mat = toon::toon_mat(Material { map: Some(ground_map), ..Material::default() });
    let ground = Obj::mesh(&geometry::plane(500.0, 500.0, 1, 1), &ground_mat);
    ground.set_rotation(-FRAC_PI_2, 0.0, 0.0).set_position(0.0, -0.01, 0.0).set_shadows(false, true);
    floor.root.add(&ground);
    floor.ground = ground;

    // The dark ground a board stands above.
    let dark_mat = toon::toon_mat(Material { color: Color::hex(0x0b0f1e), ..Material::default() });
    let board_ground = Obj::mesh(&geometry::plane(500.0, 500.0, 1, 1), &dark_mat);
    board_ground.set_rotation(-FRAC_PI_2, 0.0, 0.0).set_position(0.0, BOARD_GROUND_Y, 0.0).set_shadows(false, true);
    floor.board_ground = board_ground;

    floor.backdrop = outskirts::build_backdrop();
    floor.root.add(&floor.backdrop);
    floor.root.add(&floor.district_group);

    // Dust: sparse motes drifting in the air. Seeds: per mote x, y, z and rise speed.
    let mut rnd = Rng::new(17);
    floor.dust_seeds = (0..DUST_N)
        .map(|_| {
            let a = rnd.next() * TAU;
            let r = rnd.next().sqrt() * 9.0;
            let y = rnd.next() * 4.2;
            let rise = 0.04 + rnd.next() * 0.05;
            [a.cos() * r, y, a.sin() * r, rise]
        })
        .collect();
    let dust_mat = Mat::new(Material {
        shading: Shading::Points { size: 0.08 },
        color: Color::hex(0x7fe3ff),
        transparent: true,
        opacity: 0.8,
        additive: true,
        depth_write: false,
        ..Material::default()
    });
    floor.dust = Obj::points(vec![[0.0; 3]; DUST_N], &dust_mat);
    floor.dust.set_dynamic(true);
    floor.root.add(&floor.dust);
}

/// `plaqueTexture(text, orch)`: a cream plate with the role's name, outlined in the signal colour
/// for the orchestrator and the rule colour for the rest.
fn plaque_texture(text: &str, orch: bool, c: &Colors) -> Tex {
    let mut cv = Canvas::new(512, 128);
    cv.begin_path();
    cv.move_to(24.0, 4.0);
    cv.arc_to(508.0, 4.0, 508.0, 124.0, 20.0);
    cv.arc_to(508.0, 124.0, 4.0, 124.0, 20.0);
    cv.arc_to(4.0, 124.0, 4.0, 4.0, 20.0);
    cv.arc_to(4.0, 4.0, 508.0, 4.0, 20.0);
    cv.close_path();
    cv.set_fill(&css(c.sheet));
    cv.fill();
    cv.set_stroke(&css(if orch { c.signal } else { c.rule }));
    cv.stroke();
    cv.set_fill(INK2);
    cv.set_text_align("center");
    cv.set_text_baseline("middle");
    let face = "500 48px ui-monospace, SFMono-Regular, Menlo, monospace";
    cv.set_font(face);
    let w = cv.measure_text(text);
    if w > 460.0 {
        cv.set_font(&format!("500 {}px ui-monospace, SFMono-Regular, Menlo, monospace", ((48.0 * 460.0) / w).floor()));
    }
    cv.fill_text(text, 256.0, 66.0);
    cv.to_texture()
}

/// `Box3.setFromObject(part)` in `group`'s frame: each mesh's local bounds with its eight corners
/// carried by the node's world matrix (three's `expandByObject`). None when the part has no geometry.
fn bounds_of(group: &Obj, part: &Obj) -> Option<(Vec3, Vec3)> {
    let mut out: Option<(Vec3, Vec3)> = None;
    part.traverse(&mut |o: &Obj| {
        let geo = match &o.get().kind {
            Kind::Mesh { geo, .. } | Kind::Instanced { geo, .. } => geo.clone(),
            _ => return,
        };
        if geo.positions.is_empty() {
            return;
        }
        let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        for p in &geo.positions {
            lo = lo.min(Vec3::from(*p));
            hi = hi.max(Vec3::from(*p));
        }
        let m = group.world_matrix_of(o).unwrap_or(Mat4::IDENTITY);
        for i in 0..8u32 {
            let corner = Vec3::new(
                if i & 1 == 0 { lo.x } else { hi.x },
                if i & 2 == 0 { lo.y } else { hi.y },
                if i & 4 == 0 { lo.z } else { hi.z },
            );
            let w = m.transform_point3(corner);
            out = Some(match out {
                None => (w, w),
                Some((a, b)) => (a.min(w), b.max(w)),
            });
        }
    });
    out
}

/// A district's place on the board, what seeds and fits its room: the machine's id (`''` without a board's machine),
/// the district's index in the layout, its desks (the homes that point at it, at least one) and the board's copper.
struct RoomSite<'a> {
    /// The layout's turn and door for the room, when it has them (the shared layout's).
    plan: Option<(f32, Door)>,
    machine: &'a str,
    index: usize,
    agents: usize,
    graph: &'a TraceGraph,
}

/// `doorSide(cx, cz, turn, graph)`: the side of a room that faces the trace point nearest it, in the room's turned
/// frame (n = -z, e = +x, s = +z, w = -x). With no trace point, or the room on one, the door is 's'.
fn door_side(cx: f32, cz: f32, turn: f32, graph: &TraceGraph) -> Door {
    let (cx, cz, turn) = (f64::from(cx), f64::from(cz), f64::from(turn));
    let mut best: Option<([f64; 2], f64)> = None;
    for p in &graph.nodes {
        let p = [f64::from(p[0]), f64::from(p[1])];
        let dist = (p[0] - cx).hypot(p[1] - cz);
        if best.is_none_or(|(_, b)| dist < b) {
            best = Some((p, dist));
        }
    }
    let Some((p, dist)) = best else { return Door::S };
    if dist < 1e-9 {
        return Door::S;
    }
    let (wx, wz) = (p[0] - cx, p[1] - cz);
    let (c, s) = (turn.cos(), turn.sin());
    let (lx, lz) = (wx * c - wz * s, wx * s + wz * c);
    if lx.abs() > lz.abs() {
        if lx > 0.0 { Door::E } else { Door::W }
    } else if lz > 0.0 {
        Door::S
    } else {
        Door::N
    }
}

/// `districtMesh(d)` on a board: a room's terrace, its flat top at `y` (the slot's top) and its sloped
/// band running down to the PCB at `base`, so it stands on the chip; its floor, room, plaque and
/// outline. Adds the room's footprints, monitors and animation to `floor`.
/// A roster whose shared layout is computing, for the Floor that asked: `poll_shared_layout` adopts it once the cache
/// has it. The Floor struct (mod.rs) has no field for it, so it lives per thread here; `owner` is the Floor's address,
/// so another Floor on the thread never adopts it.
struct PendingShared {
    owner: usize,
    sync_key: String,
    key: SharedKey,
    roster: Vec<AgentInfo>,
}

thread_local! {
    static PENDING_SHARED: RefCell<Option<PendingShared>> = const { RefCell::new(None) };
}

fn queue_shared(pending: Option<PendingShared>) {
    PENDING_SHARED.with(|p| *p.borrow_mut() = pending);
}

/// The rooms of a board still to build: `step_rooms` builds one district per step.
pub(super) struct RoomsJob {
    base: f32,
    items: Vec<BoardDistrict>,
    desks: Vec<usize>,
    graph: TraceGraph,
    machine: String,
    next: usize,
}

fn district_mesh(floor: &mut Floor, d: &District, y: f32, base: f32, site: &RoomSite) -> Obj {
    let c = floor.colors;
    let g = Obj::group();
    let orch = d.role == "orchestrator";
    let top_y = y + SLAB_H;
    let height = (top_y - base).max(SLAB_H);
    let body = if d.round {
        geometry::cylinder(d.r, d.r + RAMP, height, 48)
    } else {
        let square = geometry::cylinder(d.r * SQRT_2, (d.r + RAMP) * SQRT_2, height, 4);
        geometry::transformed(&square, Mat4::from_rotation_y(FRAC_PI_4))
    };
    let slab = Obj::mesh(&body, &textures::slab_side(d.hue));
    slab.set_position(d.cx, base + height / 2.0, d.cy).set_shadows(true, true);
    g.add(&slab);
    // Neon in the district's hue along the top edge and round the foot on the PCB.
    let neon = Mat::basic(Color::hsl(d.hue / 360.0, 0.95, 0.62));
    for (rr, ny) in [(d.r, top_y + 0.01), (d.r + RAMP, base + 0.03)] {
        let ring = if d.round {
            geometry::torus(rr, 0.045, 6, 72, TAU)
        } else {
            geometry::torus(rr * SQRT_2, 0.045, 6, 4, TAU)
        };
        let edge = Obj::mesh(&ring, &neon);
        edge.set_rotation(FRAC_PI_2, 0.0, if d.round { 0.0 } else { FRAC_PI_4 }).set_position(d.cx, ny, d.cy);
        edge.get().no_outline = true;
        g.add(&edge);
    }

    // The room faces the centre: square rooms turn in quarter turns so they stay square to their slab.
    // The shared layout gives the turn and the door (its aisle side); the engine's own layout faces the centre.
    let facing = (-d.cx).atan2(-d.cy);
    let turn = match site.plan {
        Some((turn, _)) => turn,
        None if d.round => facing,
        None => ((facing / FRAC_PI_2) + 0.5).floor() * FRAC_PI_2,
    };
    // The room's seed (machine, role, district index) and walkway side: the trace point nearest the room, in the
    // room's turned frame. Its inner size is the cell less the wall margin. A cell below the role's minimum is built
    // at its size and warned (`rooms::build_room`), not enlarged: the board's slot sizes need raising for those roles.
    // TODO: room.spots (where each robot stands in this room) are not stored on the district yet; the behaviour engine reads them.
    let seed = room_seed(site.machine, &d.role, site.index);
    let door = site.plan.map_or_else(|| door_side(d.cx, d.cy, turn, site.graph), |(_, door)| door);
    let inner = 2.0 * f64::from(d.r) - WALL_MARGIN;
    let options = RoomOptions { seed, door, w: inner, d: inner, agents: Some(site.agents) };
    let built = rooms::build_room(&d.role, d.r, d.hue, &rooms::kit_palette(&c), &options);
    // The floor covers the cell, or the built room where that is larger (its walls stand just outside the inner size).
    let top_geo = if d.round {
        geometry::circle(d.r, 48, 0.0, TAU)
    } else {
        let cell = f64::from(2.0 * d.r);
        geometry::plane(cell.max(built.w + WALL_MARGIN) as f32, cell.max(built.d + WALL_MARGIN) as f32, 1, 1)
    };
    let top = Obj::mesh(&top_geo, &built.floor);
    top.set_rotation(-FRAC_PI_2, 0.0, turn).set_position(d.cx, top_y + 0.004, d.cy).set_shadows(false, true);
    g.add(&top);
    floor.last_room = Some((turn, built.spots.clone(), built.layout.items.clone()));
    let room = built.group;
    room.set_position(d.cx, top_y, d.cy).set_rotation(0.0, turn, 0.0);
    g.add(&room);
    for part in children(&room) {
        // Rugs and floor markings (`userData.walk`) do not block the floor.
        if part.get().walk {
            continue;
        }
        let Some((lo, hi)) = bounds_of(&room, &part) else { continue };
        if lo.y > SLAB_H + 2.2 {
            continue; // overhead pieces do not block the floor
        }
        floor.prop_boxes.push(Box2 { x0: lo.x, x1: hi.x, z0: lo.z, z1: hi.z });
    }
    floor.lit_by_role.insert(d.role.clone(), DistrictLight { screens: built.screens, lit: false });
    floor.prop_anim.push(built.animate);

    // Role plaque lies flat on the floor at the room's open front, readable from the centre.
    let plaque_mat = Mat::new(Material {
        shading: Shading::Basic,
        map: Some(plaque_texture(&d.role.to_uppercase(), orch, &c)),
        transparent: true,
        depth_write: false,
        ..Material::default()
    });
    let plaque = Obj::mesh(&geometry::plane(2.6, 0.65, 1, 1), &plaque_mat);
    let holder = Obj::group();
    holder.set_position(d.cx, top_y + 0.012, d.cy).set_rotation(0.0, (-d.cx).atan2(-d.cy), 0.0);
    plaque.set_rotation(-FRAC_PI_2, 0.0, 0.0).set_position(0.0, 0.0, d.r - 0.5);
    holder.add(&plaque);
    g.add(&holder);
    toon::outline(&g, 0.12);
    g
}

/// The copper's pulses: one set per trace, with the slots its ends join.
fn trace_pulses(model: &Model) -> Vec<TracePulses> {
    let mut one = model.clone();
    let mut out = Vec::new();
    for t in &model.traces {
        if t.path.len() < 2 {
            continue;
        }
        one.traces = vec![t.clone()];
        out.push(TracePulses { pulses: Pulses::new(&one), from: t.from.clone(), to: t.to.clone() });
    }
    out
}

/// Each slot's emissive materials, with their rest intensity: a slot's glow scales them.
fn glowing_parts(built: &hardware::Built, model: &Model) -> HashMap<String, Vec<(Mat, f32)>> {
    let mut out = HashMap::new();
    for slot in &model.slots {
        let Some(part) = built.parts.get(&slot.on) else { continue };
        let mut mats = Vec::new();
        part.traverse(&mut |o: &Obj| {
            let mat = match &o.get().kind {
                Kind::Mesh { mat, .. } => Some(mat.clone()),
                _ => None,
            };
            let Some(mat) = mat else { return };
            let glowing = {
                let m = mat.get();
                (m.emissive != Color::BLACK).then_some(m.emissive_intensity)
            };
            if let Some(base) = glowing {
                o.set_dynamic(true); // its glow changes every frame
                mats.push((mat, base));
            }
        });
        if !mats.is_empty() {
            out.insert(slot.id.clone(), mats);
        }
    }
    out
}

/// `createBot(agent, district, hx, hy)`: a robot, its console pad and its task token.
fn create_bot(floor: &Floor, agent: &AgentInfo, district: usize, hx: f32, hy: f32) -> Bot {
    let d = &floor.districts[district];
    let orch = is_orchestrator(agent);
    let robot = Robot::with_variant(d.hue, orch, &floor.robot_palette, agent_hue(agent.cid), species_of(agent.species.as_deref(), agent.cid));
    robot.group.traverse(&mut |o: &Obj| {
        o.set_shadows(true, true);
    });
    floor.root.add(&robot.group);

    let pad_mat = Mat::new(Material {
        shading: Shading::Basic,
        color: floor.colors.ink3,
        transparent: true,
        opacity: 0.6,
        depth_write: false,
        ..Material::default()
    });
    let pad = Obj::mesh(&floor.shared.pad_geo, &pad_mat);
    pad.set_dynamic(true).set_rotation(-FRAC_PI_2, 0.0, 0.0).set_position(hx, floor.top_at(district) + 0.02, hy);
    floor.root.add(&pad);

    let token = Obj::mesh(&floor.shared.token_geo, &floor.shared.token);
    token.set_dynamic(true).set_visible(false);
    floor.root.add(&token);

    let mut rnd = Rng::new(agent.cid as u32 ^ 0x9e37_79b9);
    let now = floor.now_ms;
    Bot {
        agent: agent.clone(),
        district,
        home: (hx, hy),
        x: hx,
        y: hy,
        tx: hx,
        ty: hy,
        heading: 0.0,
        moving: false,
        py: 0.0,
        vy: 0.0,
        state: None,
        next_wander: now + (rnd.next() * 3000.0) as f64,
        next_social: now + 4000.0 + (rnd.next() * 10000.0) as f64,
        social: None,
        activity: "idle".into(),
        robot,
        robot_key: format!("{}|{}", d.hue, orch),
        tilt: Quat::IDENTITY,
        pad,
        pad_mat,
        token,
        highlighted: false,
        route: Vec::new(),
        route_goal: None,
    }
}

/// `removeBot(b)`: takes the robot, pad and token out of the scene.
fn remove_bot(root: &Obj, bot: &Bot) {
    root.remove(&bot.robot.group);
    root.remove(&bot.pad);
    root.remove(&bot.token);
}

impl Floor {
    /// The height of the surface a robot in district `idx` stands on: its room's top.
    pub fn top_at(&self, idx: usize) -> f32 {
        self.board.as_ref().and_then(|b| b.layout.districts.get(idx)).map_or(0.0, |bd| bd.y + SLAB_H)
    }

    /// Shows a machine's board in the Machine view: its model built at the origin with `params`, the
    /// copper and its pulses, the atom over the centre, and the dark ground under it. Takes out the
    /// previous board, its robots and rooms; `sync` places the rooms and robots.
    pub fn show_board(&mut self, key: String, model: &'static Model, params: Params, status: Status) {
        self.clear_board();
        // The built parts are the machine's own (its id, model and params): a machine shown again takes them from the cache.
        let cache_key = board_key(&self.machine_id, &key, &params);
        let built = take_board(&mut self.boards, &cache_key, || hardware::build(model, &params));
        let obj = Obj::group();
        obj.add(&built.root);
        obj.add(&hardware::traces(model));
        // The atom's wrap is the child `place_atom` appends: its node is kept for the ENTER's glow.
        let before = obj.get().children.len();
        let atom = hardware::place_atom(model, &obj, &prop_palette(&self.colors));
        let atom_node = obj.get().children.get(before).cloned();
        let pulses = trace_pulses(model);
        for tp in &pulses {
            obj.add(&tp.pulses.obj);
        }
        // Only the parts that move are dynamic (the built model's animated parts, the pulses and the
        // atom, set in `hardware`): the rest of the board is baked with the statics.
        let glow = glowing_parts(&built, model);
        let graph = trace_graph(model);
        // Empty until `sync` lays out the roster (it always follows, so the empty layout is never read).
        let layout = BoardLayout {
            districts: Vec::new(),
            homes: HashMap::new(),
            atom: [0.0; 3],
            bounds: Bounds { cx: 0.0, cz: 0.0, w: 0.0, d: 0.0 },
            pcb_top: 0.0,
            plates: Vec::new(),
            shared: None,
        };

        self.root.remove(&self.ground);
        self.root.add(&self.board_ground);
        self.root.add(&obj);
        self.board = Some(Board {
            key,
            cache_key,
            model,
            params,
            status,
            obj,
            built,
            atom,
            atom_node,
            pulses,
            glow,
            level: HashMap::new(),
            layout,
            graph,
            sig: String::new(),
        });
        self.statics_dirty = true;
        self.fitted = false;
    }

    /// The board's navigation for its layout: built from the shared layout (its walkways under the board's group), or
    /// none for the engine's own layout. The robots join it on their next step.
    fn rebuild_nav(&mut self) {
        if let (Some(old), Some(board)) = (self.nav.take(), self.board.as_ref()) {
            board.obj.remove(&old.root);
        }
        let Some(board) = self.board.as_ref() else { return };
        let Some((shared, gpus)) = board.layout.shared.clone() else { return };
        let nav = super::nav::BoardNav::new(&shared, gpus, &self.machine_id, &self.colors);
        board.obj.add(&nav.root);
        self.statics_dirty = true;
        self.nav = Some(nav);
    }

    /// The work for the board's layout: its board spots, the rooms' stations as the rooms build (`step_rooms`), and the
    /// nav's reach when the board has a nav.
    fn reset_work(&mut self) {
        let Some(board) = self.board.as_ref() else { return };
        let params = board.params.clone();
        let (model, graph, layout) = (board.model, board.graph.clone(), board.layout.clone());
        self.work = super::work::RobotWork::new();
        self.work.set_board(model, &params, &graph, &layout);
        self.work.begin_rooms();
        if let Some(nav) = self.nav.as_ref() {
            self.work.set_reach(nav.reach());
        }
    }

    /// Takes the board, its robots and rooms out of the scene.
    fn clear_board(&mut self) {
        self.rooms_job = None;
        self.nav = None;
        self.root.remove(&self.board_ground);
        if let Some(old) = self.board.take() {
            self.root.remove(&old.obj);
            // The built parts leave the scene with the board and go back to the cache (not dropped).
            old.obj.remove(&old.built.root);
            release_board(&mut self.boards, &old.cache_key, old.built);
        }
        for bot in self.bots.drain(..) {
            remove_bot(&self.root, &bot);
        }
        for child in children(&self.district_group) {
            self.district_group.remove(&child);
        }
        self.districts.clear();
        self.lit_by_role.clear();
        self.prop_anim.clear();
        self.prop_boxes.clear();
        self.hovered = None;
        self.selected = None;
    }

    /// Starts the rooms of the board's current layout (`buildWorld`): the old ones come out now, and
    /// `step_rooms` builds the new ones, a district at a time.
    fn start_rooms(&mut self) {
        let Some(board) = self.board.as_ref() else { return };
        // Each district's desks: the homes that point at it, at least one.
        let mut desks = vec![0usize; board.layout.districts.len()];
        for &(idx, _, _) in board.layout.homes.values() {
            if let Some(n) = desks.get_mut(idx) {
                *n += 1;
            }
        }
        let job = RoomsJob {
            base: board.layout.pcb_top,
            items: board.layout.districts.clone(),
            desks,
            graph: board.graph.clone(),
            machine: self.machine_id.clone(),
            next: 0,
        };
        for child in children(&self.district_group) {
            self.district_group.remove(&child);
        }
        self.lit_by_role.clear();
        self.prop_anim.clear();
        self.prop_boxes.clear();
        self.rooms_job = (!job.items.is_empty()).then_some(job);
        self.statics_dirty = true; // the rooms changed: re-bake the statics
    }

    /// Builds the queued rooms a district at a time until `budget` is spent (one district at least). Returns
    /// whether rooms are still queued. The web's warm queue spends its frame budget on its jobs the same way.
    /// Also the per-frame hook of the shared layout: a finished one is adopted here (`poll_shared_layout`), and while
    /// one computes the rooms count as pending, so the frame keeps calling this.
    pub fn step_rooms(&mut self, budget: Duration) -> bool {
        self.poll_shared_layout();
        let t0 = Instant::now();
        let Some(mut job) = self.rooms_job.take() else { return self.rooms_pending() };
        loop {
            let index = job.next;
            let bd = &job.items[index];
            let site = RoomSite { plan: bd.plan, machine: &job.machine, index, agents: job.desks[index].max(1), graph: &job.graph };
            let g = district_mesh(self, &bd.district, bd.y, job.base, &site);
            if let Some((turn, spots, items)) = self.last_room.take() {
                self.work.add_room(bd, turn, &spots, &items);
            }
            self.district_group.add(&g);
            self.statics_dirty = true;
            job.next += 1;
            if job.next == job.items.len() {
                return self.rooms_pending();
            }
            if t0.elapsed() >= budget {
                break;
            }
        }
        self.rooms_job = Some(job);
        true
    }

    /// Builds every queued room now.
    pub fn finish_rooms(&mut self) {
        self.step_rooms(Duration::MAX);
    }

    /// Whether rooms are still queued (`step_rooms` builds them): work every frame should advance.
    pub fn rooms_pending(&self) -> bool {
        self.rooms_job.is_some()
    }

    /// Whether the shared (apz-board) layout for this floor is still computing off the frame. Nothing to do per frame
    /// but poll for it (`step_rooms` does), so the host keeps ticking at the ambient rate, not the active one.
    pub fn shared_waiting(&self) -> bool {
        PENDING_SHARED.with(|p| p.borrow().as_ref().is_some_and(|q| q.owner == self.owner()))
    }

    /// Keeps the board in step with the agents on the machine: one room per role on the board's
    /// slots, rebuilt when the layout changes, and a robot per agent. Every room is built here.
    pub fn sync(&mut self, list: &[AgentInfo]) {
        self.begin_sync(list);
        self.finish_rooms();
    }

    /// The address that owns this floor's pending shared layout (see `PendingShared`).
    fn owner(&self) -> usize {
        self as *const Self as usize
    }

    /// The layout for a roster the board has not cached: the shared one when it is ready, else the engine's own now
    /// (`layout_on_board`), with the shared one queued for `poll_shared_layout`. Nothing waits on the shared layout.
    fn fresh_layout(&self, list: &[AgentInfo], model: &Model, bp: &BoardParams, sync_key: &str) -> BoardLayout {
        let key = shared_key_for(list, model, bp);
        if let Some(shared) = request_shared(&key) {
            queue_shared(None);
            return shared;
        }
        let pending = shared_pending(&key).then(|| PendingShared {
            owner: self.owner(),
            sync_key: sync_key.to_string(),
            key,
            roster: list.to_vec(),
        });
        queue_shared(pending);
        layout_on_board(list, model, bp)
    }

    /// Adopts this floor's shared layout once the cache has it: the roster is synced again on it, which rebuilds the
    /// rooms through `start_rooms`. A pending one the cache failed on is dropped, and the engine's own layout stays.
    fn poll_shared_layout(&mut self) {
        let owner = self.owner();
        let Some(pending) = PENDING_SHARED.with(|p| {
            let mut p = p.borrow_mut();
            if p.as_ref().is_some_and(|q| q.owner == owner) {
                p.take()
            } else {
                None
            }
        }) else {
            return;
        };
        if let Some(shared) = request_shared(&pending.key) {
            self.layout_cache = Some((pending.sync_key, shared));
            self.begin_sync(&pending.roster);
        } else if shared_pending(&pending.key) {
            queue_shared(Some(pending));
        }
    }

    /// `sync` without the rooms: the layout and the robots now, the rooms queued for `step_rooms`.
    /// The layout is cached per board and roster: a sync that finds the same ones reuses it.
    pub fn begin_sync(&mut self, list: &[AgentInfo]) {
        let Some(board) = self.board.as_ref() else { return };
        let bp = BoardParams { params: board.params.clone() };
        let model = board.model;
        let key = format!("{}|{:?}", board.key, list);
        let cached = self.layout_cache.as_ref().filter(|(k, _)| *k == key).map(|(_, layout)| layout.clone());
        let layout = match cached {
            Some(layout) => layout,
            None => {
                let layout = self.fresh_layout(list, model, &bp, &key);
                self.layout_cache = Some((key, layout.clone()));
                layout
            }
        };
        let mut homes: Vec<_> = layout.homes.iter().map(|(cid, h)| (*cid, *h)).collect();
        homes.sort_by_key(|(cid, _)| *cid);
        // The machine's id seeds the rooms: a change of machine rebuilds them even when the layout is the same.
        let sig = format!("{:?}|{:?}|{}", layout.districts, homes, self.machine_id);
        self.districts = layout.districts.iter().map(|bd| bd.district.clone()).collect();
        let homes = layout.homes.clone();
        let Some(board) = self.board.as_mut() else { return };
        let changed = board.sig != sig;
        board.sig = sig;
        board.layout = layout;
        if changed {
            self.start_rooms();
            self.rebuild_nav();
            self.reset_work();
        }

        let mut seen = HashSet::new();
        for agent in list {
            let Some(&(idx, hx, hy)) = homes.get(&agent.cid) else { continue };
            seen.insert(agent.cid);
            let key = format!("{}|{}", self.districts[idx].hue, is_orchestrator(agent));
            let i = match self.bots.iter().position(|b| b.agent.cid == agent.cid) {
                Some(i) if self.bots[i].robot_key == key => i,
                Some(i) => {
                    let old = self.bots.remove(i);
                    remove_bot(&self.root, &old);
                    let bot = create_bot(self, agent, idx, hx, hy);
                    self.bots.insert(i, bot);
                    i
                }
                None => {
                    let bot = create_bot(self, agent, idx, hx, hy);
                    self.bots.push(bot);
                    self.bots.len() - 1
                }
            };
            let top = self.top_at(idx);
            let bot = &mut self.bots[i];
            bot.agent = agent.clone();
            bot.district = idx;
            bot.home = (hx, hy);
            bot.pad.set_position(hx, top + 0.02, hy);
        }

        let mut k = 0;
        while k < self.bots.len() {
            if seen.contains(&self.bots[k].agent.cid) {
                k += 1;
            } else {
                let old = self.bots.remove(k);
                remove_bot(&self.root, &old);
            }
        }
        if let Some(c) = self.selected {
            if !self.bots.iter().any(|b| b.agent.cid == c) {
                self.selected = None;
            }
        }
        if let Some(c) = self.hovered {
            if !self.bots.iter().any(|b| b.agent.cid == c) {
                self.hovered = None;
            }
        }
    }

    /// The points the fit frames, world units: the board's footprint on the PCB and at the tallest
    /// room's top; each room's footprint with its ramp, on the PCB and at its top; the atom; and each
    /// robot's home up to the height of its label.
    pub(super) fn fit_points(&self) -> Vec<Vec3> {
        let Some(board) = &self.board else { return vec![Vec3::ZERO] };
        let pcb = board.layout.pcb_top;
        let bounds = board.layout.bounds;
        let mut top = pcb;
        let mut pts = Vec::new();
        for bd in &board.layout.districts {
            let t = bd.y + SLAB_H;
            top = top.max(t);
            for (x, z) in footprint(&bd.district) {
                pts.push(Vec3::new(x, pcb, z));
                pts.push(Vec3::new(x, t, z));
            }
        }
        let (x0, x1) = (bounds.cx - bounds.w / 2.0, bounds.cx + bounds.w / 2.0);
        let (z0, z1) = (bounds.cz - bounds.d / 2.0, bounds.cz + bounds.d / 2.0);
        for (x, z) in [(x0, z0), (x1, z0), (x1, z1), (x0, z1)] {
            pts.push(Vec3::new(x, pcb, z));
            pts.push(Vec3::new(x, top, z));
        }
        let [ax, ay, az] = board.layout.atom;
        pts.push(Vec3::new(ax, ay + 1.0, az));
        // A robot stands about 1.5 units tall, its label just above it.
        const ROBOT_AND_LABEL: f32 = 2.0;
        for bot in &self.bots {
            let (x, z) = bot.home;
            pts.push(Vec3::new(x, self.top_at(bot.district) + ROBOT_AND_LABEL, z));
        }
        pts
    }

    /// `resetView()`: the isometric-ish view that frames the board in an `aspect` (width / height)
    /// stage. The eye keeps its angles (`POLAR`, `AZIMUTH`) and the distance is solved so that each
    /// fit point sits within `FIT_FILL` of the stage's half-width and half-height. A point `v` from
    /// the target is `dist - v·dir` deep, so it needs `dist >= v·dir + max(|across| / (FIT_FILL ·
    /// tan_h), |up| / (FIT_FILL · tan_v))`; the nearest distance is the largest such bound.
    pub fn reset_view(&mut self, aspect: f32) {
        // three's setFromSphericalCoords(1, POLAR, AZIMUTH): the unit direction from target to eye.
        let dir = Vec3::new(POLAR.sin() * AZIMUTH.sin(), POLAR.cos(), POLAR.sin() * AZIMUTH.cos());
        let right = Vec3::Y.cross(dir).normalize();
        let up = dir.cross(right);
        let tan_v = (FOV_DEG / 2.0).to_radians().tan();
        let tan_h = tan_v * aspect.max(1e-3);
        let points = self.fit_points();
        let min_dist = self.orbit.min_distance;
        let solve = |target: Vec3| {
            points.iter().fold(min_dist, |far, p| {
                let v = *p - target;
                let across = v.dot(right).abs() / (FIT_FILL * tan_h);
                let lift = v.dot(up).abs() / (FIT_FILL * tan_v);
                far.max(v.dot(dir) + across.max(lift))
            })
        };
        // The near side of the board projects larger, so a target at the origin leaves it low on the
        // stage. Each pass solves the distance, then moves the target across and up to the middle of
        // the points' extent on the stage; a few passes settle it.
        let mut target = Vec3::new(0.0, 0.4, 0.0);
        for _ in 0..4 {
            let dist = solve(target);
            let (mut x0, mut x1, mut y0, mut y1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
            for p in &points {
                let v = *p - target;
                let depth = dist - v.dot(dir);
                let nx = v.dot(right) / (depth * tan_h);
                let ny = v.dot(up) / (depth * tan_v);
                x0 = x0.min(nx);
                x1 = x1.max(nx);
                y0 = y0.min(ny);
                y1 = y1.max(ny);
            }
            target += right * ((x0 + x1) / 2.0 * dist * tan_h) + up * ((y0 + y1) / 2.0 * dist * tan_v);
        }
        let dist = solve(target);
        self.orbit.look(target + dir * dist, target);
    }
}

/// Ground points around a room's footprint, its ramp included: a square's corners, or a round
/// room's rim.
fn footprint(d: &District) -> Vec<(f32, f32)> {
    let reach = d.r + RAMP;
    if d.round {
        rim(d.cx, d.cy, reach)
    } else {
        vec![(d.cx - reach, d.cy - reach), (d.cx + reach, d.cy - reach), (d.cx + reach, d.cy + reach), (d.cx - reach, d.cy + reach)]
    }
}

/// Points on a circle of `reach` about (`cx`, `cy`): the vertices of a polygon that circumscribes the
/// circle, so framing them frames the whole circle.
fn rim(cx: f32, cy: f32, reach: f32) -> Vec<(f32, f32)> {
    const N: usize = 24;
    let r = reach / (PI / N as f32).cos();
    (0..N)
        .map(|i| {
            let a = TAU * i as f32 / N as f32;
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(cid: u64, role: &str) -> AgentInfo {
        AgentInfo { cid, name: format!("agent {cid}"), role: role.into(), status: "idle".into(), tier: if role == "orchestrator" { "orchestrator".into() } else { "worker".into() }, model: String::new(), species: None }
    }

    #[test]
    fn sync_puts_a_room_per_role_and_a_robot_per_agent_on_the_board() {
        let model = hardware::model("board/edge").expect("the edge board parses");
        let list = [agent(1, "coder"), agent(2, "coder"), agent(3, "writer"), agent(4, "writer")];
        let mut floor = Floor::new();
        floor.show_board("test".into(), model, Params::default().with("gpus", 1.0), Status::Online);
        floor.sync(&list);
        assert_eq!(floor.districts.len(), 2, "one room per role");
        assert_eq!(floor.bots.len(), 4, "one robot per agent");
        assert!(floor.districts.iter().all(|d| d.r > 0.0));
        assert!(floor.bots.iter().all(|b| b.py == 0.0), "robots start on the PCB, not on the ground");
    }

    #[test]
    fn a_board_drops_the_ring_and_keeps_its_own_ground() {
        let model = hardware::model("board/workstation").expect("the workstation board parses");
        let mut floor = Floor::new();
        floor.show_board("test".into(), model, Params::default(), Status::Online);
        assert!(floor.board.is_some());
        let once = floor.root.get().children.len();
        floor.show_board("again".into(), model, Params::default(), Status::Online);
        assert_eq!(floor.root.get().children.len(), once, "a second board replaces the first, it does not add to the scene");
    }

    #[test]
    fn showing_a_machines_board_again_takes_its_built_parts_from_the_cache() {
        let model = hardware::model("board/workstation").expect("the workstation board parses");
        let mut floor = Floor::new();
        floor.machine_id = "m-1".into();
        floor.show_board("first".into(), model, Params::default(), Status::Online);
        let first = floor.board.as_ref().expect("a board").built.root.clone();
        floor.show_board("other".into(), model, Params::default().with("gpus", 1.0), Status::Online);
        floor.show_board("first".into(), model, Params::default(), Status::Online);
        let again = floor.board.as_ref().expect("a board").built.root.clone();
        assert!(first == again, "the second showing takes the built parts back from the cache, it does not build them again");
    }

    #[test]
    fn the_door_faces_the_nearest_trace_point_in_the_rooms_turned_frame() {
        let graph = |nodes: Vec<[f32; 2]>| TraceGraph { nodes, edges: Vec::new() };
        // No copper, or the room on a trace point: 's' (doorSide's default).
        assert_eq!(door_side(0.0, 0.0, 0.0, &graph(Vec::new())), Door::S);
        assert_eq!(door_side(1.0, 1.0, 0.0, &graph(vec![[1.0, 1.0]])), Door::S);
        // A point to the +z side (south), to +x (east), to -z (north) and to -x (west), unturned.
        assert_eq!(door_side(0.0, 0.0, 0.0, &graph(vec![[0.0, 5.0]])), Door::S);
        assert_eq!(door_side(0.0, 0.0, 0.0, &graph(vec![[5.0, 0.0]])), Door::E);
        assert_eq!(door_side(0.0, 0.0, 0.0, &graph(vec![[0.0, -5.0]])), Door::N);
        assert_eq!(door_side(0.0, 0.0, 0.0, &graph(vec![[-5.0, 0.0]])), Door::W);
        // Turned a quarter (the room faces the centre), a south point is seen on the west side of the room.
        assert_eq!(door_side(0.0, 0.0, FRAC_PI_2, &graph(vec![[0.0, 5.0]])), Door::W);
        // The nearest point wins; on a tie the first one does.
        assert_eq!(door_side(0.0, 0.0, 0.0, &graph(vec![[0.0, -2.0], [3.0, 0.0]])), Door::N);
        assert_eq!(door_side(0.0, 0.0, 0.0, &graph(vec![[2.0, 0.0], [0.0, 2.0]])), Door::E);
    }
}
