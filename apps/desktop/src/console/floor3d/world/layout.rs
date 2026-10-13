//! `floor.ts` lines ~123–710: district layout by role, the world's base (sky, ground, clouds,
//! plaza, grass and dust), district slabs, rooms and plaques, robots in and out, and the view's fit.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI, SQRT_2, TAU};

use glam::{Mat4, Quat, Vec3};

use super::{AgentInfo, Bot, Colors, District, DistrictLight, Floor, Shared};
use crate::console::floor3d::canvas::Canvas;
use crate::console::floor3d::controls::FOV_DEG;
use crate::console::floor3d::geometry;
use crate::console::floor3d::props::{self, Box2, PropPalette, Site};
use crate::console::floor3d::outskirts;
use crate::console::floor3d::robot::{robot_variant, Robot};
use crate::console::floor3d::rooms::{self, BuiltRoom, KitPalette};
use crate::console::floor3d::scene::{Color, Kind, Mat, Material, Obj, Shading, Tex};
use crate::console::floor3d::textures::{self, Rng};
use crate::console::floor3d::toon;

// ---- Constants (floor.ts 25–40) -----------------------------------------------------------------

#[allow(dead_code)] // read by motion.rs
pub const PACKET_MS: f64 = 1500.0;
#[allow(dead_code)] // read by motion.rs
pub const SPEED: f32 = 1.6; // grid units per second; one grid cell is one world unit
#[allow(dead_code)] // read by motion.rs
pub const MAX_PACKETS: usize = 60;
pub const SLAB_H: f32 = 0.5; // district slab height
pub const DAIS_H: f32 = 0.8; // orchestrator dais height
pub const DAIS_R: f32 = 2.3;
pub const RAMP: f32 = 1.5; // width of the sloped band around a district slab
pub const DAIS_RAMP: f32 = 2.2; // width of the dais's sloped skirt
#[allow(dead_code)] // read by motion.rs
pub const GRAVITY: f32 = 9.8; // world units per second squared
pub const POLAR: f32 = 0.95; // camera angle from vertical, about 55 degrees: isometric-ish
pub const AZIMUTH: f32 = FRAC_PI_4;
/// The share of the stage's half-width and half-height the plaza may fill: 0.85 leaves 7.5 % of
/// the stage clear on each side.
const FIT_FILL: f32 = 0.85;
/// Height the fit allows above the plaza for the robots and their labels, grid units.
const LABEL_HEADROOM: f32 = 4.0;
#[allow(dead_code)] // read by motion.rs
pub const SOCIAL_NEAR: f32 = 9.5; // districts this close count as neighbours for a visit or a chat
#[allow(dead_code)] // read by motion.rs
pub const STROLL_R: f32 = 3.1; // the stroll ring just outside the dais
#[allow(dead_code)] // read by motion.rs
pub const ROBOT_R: f32 = 0.55; // robot footprint radius for collisions
const DEG: f32 = PI / 180.0;
/// Headings tried when a step is blocked, radians.
#[allow(dead_code)] // read by motion.rs
pub const STEER: [f32; 5] = [0.0, 30.0 * DEG, -30.0 * DEG, 60.0 * DEG, -60.0 * DEG];
pub const DUST_N: usize = 90;

/// The district hues: a curated cartoon set (coral, tangerine, butter, lime, mint, teal, sky,
/// periwinkle, lavender, rose).
const HUES: [f32; 10] = [8.0, 28.0, 46.0, 88.0, 140.0, 172.0, 198.0, 222.0, 262.0, 340.0];
/// Gap between two rooms' arcs on the ring.
const GAP: f32 = 2.4;
/// `--ink-2`, the plaque's ink. The web reads it from CSS; `Colors` does not carry it.
const INK2: &str = "#4c544d";

/// The orchestrator: `tier === 'orchestrator'`.
fn is_orchestrator(a: &AgentInfo) -> bool {
    a.tier == "orchestrator"
}

fn top_of(d: &District) -> f32 {
    if d.role == "orchestrator" { DAIS_H } else { SLAB_H }
}

fn children(o: &Obj) -> Vec<Obj> {
    o.get().children.clone()
}

fn prop_palette(c: &Colors) -> PropPalette {
    PropPalette { sheet: c.sheet, rule: c.rule, ink3: c.ink3, signal: c.signal, ok: c.ok, info: c.info }
}

/// The rooms keep their own light paper (documents, signs) under the night lighting.
fn kit_palette(c: &Colors) -> KitPalette {
    KitPalette { sheet: Color::hex(0xe9edf7), rule: Color::hex(0xa9b4cc), ink3: c.ink3, signal: c.signal, ok: c.ok, info: c.info }
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

/// `layoutDistricts(list)`: districts by role around the central dais, and each agent's home:
/// cid → (district index, x, y).
pub fn layout_districts(list: &[AgentInfo]) -> (Vec<District>, HashMap<u64, (usize, f32, f32)>) {
    let mut groups: BTreeMap<String, Vec<&AgentInfo>> = BTreeMap::new();
    for a in list {
        let key = if is_orchestrator(a) { String::new() } else { a.role.clone() };
        groups.entry(key).or_default().push(a);
    }
    let mut districts = Vec::new();
    let mut homes = HashMap::new();
    if let Some(centre) = groups.get("") {
        let d = District { role: "orchestrator".into(), cx: 0.0, cy: 0.0, r: 2.0, hue: 25.0, round: true };
        place(&mut districts, &mut homes, d, centre);
    }
    let roles: Vec<String> = groups.keys().filter(|k| !k.is_empty()).cloned().collect();
    // Each room is its style's size (bigger when crowded). Rooms share the ring by width: each gets
    // an arc as wide as itself plus a gap, and the ring is as large as the arcs need.
    let size_of = |role: &str| -> f32 {
        let n = groups.get(role).map_or(0, |m| m.len()) as f32;
        rooms::room_style(role).size.max(1.5 * n.sqrt().ceil().max(1.0) + 1.5)
    };
    let widths: Vec<f32> = roles.iter().map(|r| 2.0 * (size_of(r) + RAMP) + GAP).collect();
    let total: f32 = widths.iter().sum();
    let ring_r = (10.0 + RAMP).max(total / TAU);
    let mut arc = 0.0;
    for (i, role) in roles.iter().enumerate() {
        let members = &groups[role.as_str()];
        let angle = ((arc + widths[i] / 2.0) / total) * TAU - FRAC_PI_2;
        arc += widths[i];
        let r = size_of(role);
        let style = rooms::room_style(role);
        // A square room's corners reach further out than its side: push it out so it clears its neighbours.
        let out = if style.round { ring_r } else { ring_r + r * 0.3 };
        let d = District {
            role: role.clone(),
            cx: angle.cos() * out,
            cy: angle.sin() * out,
            r,
            hue: role_hue(role),
            round: style.round,
        };
        place(&mut districts, &mut homes, d, members);
    }
    (districts, homes)
}

/// Adds a district and gives its members their console slots (3 apart, by cid).
fn place(districts: &mut Vec<District>, homes: &mut HashMap<u64, (usize, f32, f32)>, d: District, members: &[&AgentInfo]) {
    let side = (members.len() as f32).sqrt().ceil().max(1.0) as usize;
    let mut sorted = members.to_vec();
    sorted.sort_by_key(|a| a.cid);
    let span = 1.5 * (side as f32 - 1.0); // homes sit 3 apart
    for (i, a) in sorted.iter().enumerate() {
        let gx = d.cx - span + (i % side) as f32 * 3.0;
        let gy = d.cy - span + (i / side) as f32 * 3.0;
        homes.insert(a.cid, (districts.len(), gx, gy));
    }
    districts.push(d);
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

/// Sky, ground, clouds, plaza and dust, added to `floor.root`. The fog and lights are `World`'s
/// defaults, which are floor.ts's values.
pub(super) fn base(floor: &mut Floor) {
    let c = floor.colors;

    // Night: a deep blue zenith banded down to a violet horizon glow, stars and a ringed planet.
    floor.root.add(&toon::sky(Color::hex(0x05071a), Color::hex(0x3a2275), Color::hex(0x090c18)));

    let sky = toon::starfield(900, 330.0, &mut Rng::new(31));
    floor.stars = sky.group;
    floor.planet = sky.planet;
    floor.stars.set_dynamic(true); // turns every frame
    floor.root.add(&floor.stars);

    // Ground: one smooth sheet out to the horizon, lighter round the plaza.
    let ground_map = textures::ground();
    let ground_mat = toon::toon_mat(Material { map: Some(ground_map), ..Material::default() });
    let ground = Obj::mesh(&geometry::plane(500.0, 500.0, 1, 1), &ground_mat);
    ground.set_rotation(-FRAC_PI_2, 0.0, 0.0).set_position(0.0, -0.01, 0.0).set_shadows(false, true);
    floor.root.add(&ground);
    floor.root.add(&outskirts::build_backdrop());

    floor.root.add(&floor.district_group).add(&floor.path_group);

    let (plaza, anim) = props::build_plaza(&prop_palette(&c));
    floor.plaza = plaza;
    floor.plaza_anim = anim;
    floor.plaza.set_dynamic(true); // the xenon atom and its rings animate
    floor.root.add(&floor.plaza);

    // Dust: sparse motes drifting over the plaza. Seeds: per mote x, y, z and rise speed.
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
    cv.set_line_width(6.0);
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

/// `districtMesh(d)`: a district's slab (or the dais), its floor, room, plaque and outline.
/// Adds the room's footprints, monitors and animation to `floor`.
fn district_mesh(floor: &mut Floor, d: &District) -> Obj {
    let c = floor.colors;
    let g = Obj::group();
    let orch = d.role == "orchestrator";
    if orch {
        let dais_mat = toon::toon_mat(Material { color: Color::hex(0x4a5690), ..Material::default() });
        let dais = Obj::mesh(&geometry::cylinder(DAIS_R, DAIS_R + DAIS_RAMP, DAIS_H, 48), &dais_mat);
        dais.set_position(d.cx, DAIS_H / 2.0, d.cy).set_shadows(true, true);
        let rim = Obj::mesh(&geometry::torus(DAIS_R, 0.05, 8, 64, TAU), &Mat::basic(c.signal));
        rim.set_rotation(FRAC_PI_2, 0.0, 0.0).set_position(d.cx, DAIS_H + 0.01, d.cy);
        // A cyan ring of light round the foot of the skirt.
        let foot = Obj::mesh(&geometry::torus(DAIS_R + DAIS_RAMP, 0.06, 8, 96, TAU), &Mat::basic(c.info));
        foot.set_rotation(FRAC_PI_2, 0.0, 0.0).set_position(d.cx, 0.04, d.cy);
        foot.get().no_outline = true;
        g.add(&dais).add(&rim).add(&foot);
    } else {
        // A terrace: the flat top plus a sloped band all round, so robots can walk on and off; its
        // floor is the room's own, laid as a separate surface so the texture runs square to the room.
        let body = if d.round {
            geometry::cylinder(d.r, d.r + RAMP, SLAB_H, 48)
        } else {
            let square = geometry::cylinder(d.r * SQRT_2, (d.r + RAMP) * SQRT_2, SLAB_H, 4);
            geometry::transformed(&square, Mat4::from_rotation_y(FRAC_PI_4))
        };
        let slab = Obj::mesh(&body, &textures::slab_side(d.hue));
        slab.set_position(d.cx, SLAB_H / 2.0, d.cy).set_shadows(true, true);
        g.add(&slab);
        // Neon in the district's hue along the slab's top edge and round its foot.
        let neon = Mat::basic(Color::hsl(d.hue / 360.0, 0.95, 0.62));
        for (rr, y) in [(d.r, SLAB_H + 0.01), (d.r + RAMP, 0.03)] {
            let ring = if d.round {
                geometry::torus(rr, 0.045, 6, 72, TAU)
            } else {
                geometry::torus(rr * SQRT_2, 0.045, 6, 4, TAU)
            };
            let edge = Obj::mesh(&ring, &neon);
            edge.set_rotation(FRAC_PI_2, 0.0, if d.round { 0.0 } else { FRAC_PI_4 }).set_position(d.cx, y, d.cy);
            edge.get().no_outline = true;
            g.add(&edge);
        }

        // The room faces the plaza: square rooms turn in quarter turns so they stay square to their slab.
        let facing = (-d.cx).atan2(-d.cy);
        let turn = if d.round { facing } else { ((facing / FRAC_PI_2) + 0.5).floor() * FRAC_PI_2 };
        let BuiltRoom { group: room, floor: room_floor, screens, animate } =
            rooms::build_room(&d.role, d.r, d.hue, &kit_palette(&c));
        let top_geo = if d.round {
            geometry::circle(d.r, 48, 0.0, TAU)
        } else {
            geometry::plane(2.0 * d.r, 2.0 * d.r, 1, 1)
        };
        let top = Obj::mesh(&top_geo, &room_floor);
        top.set_rotation(-FRAC_PI_2, 0.0, turn).set_position(d.cx, SLAB_H + 0.004, d.cy).set_shadows(false, true);
        g.add(&top);
        room.set_position(d.cx, SLAB_H, d.cy).set_rotation(0.0, turn, 0.0);
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
        floor.lit_by_role.insert(d.role.clone(), DistrictLight { screens, lit: false });
        floor.prop_anim.push(animate);
    }

    // Role plaque lies flat on the back edge of the slab (the dais for the orchestrator).
    let plaque_mat = Mat::new(Material {
        shading: Shading::Basic,
        map: Some(plaque_texture(&d.role.to_uppercase(), orch, &c)),
        transparent: true,
        depth_write: false,
        ..Material::default()
    });
    let plaque = Obj::mesh(&geometry::plane(2.6, 0.65, 1, 1), &plaque_mat);
    if orch {
        plaque.set_rotation(-FRAC_PI_2, 0.0, 0.0).set_position(d.cx, top_of(d) + 0.01, d.cy - 1.6);
        g.add(&plaque);
    } else {
        // Flat on the floor at the room's open front, readable from the plaza.
        let holder = Obj::group();
        holder.set_position(d.cx, top_of(d) + 0.012, d.cy).set_rotation(0.0, (-d.cx).atan2(-d.cy), 0.0);
        plaque.set_rotation(-FRAC_PI_2, 0.0, 0.0).set_position(0.0, 0.0, d.r - 0.5);
        holder.add(&plaque);
        g.add(&holder);
    }
    toon::outline(&g, 0.12);
    g
}

/// Districts and their paths, rebuilt whenever the layout changes (`buildWorld`).
fn build_world(floor: &mut Floor) {
    let ds = floor.districts.clone();
    for child in children(&floor.district_group) {
        floor.district_group.remove(&child);
    }
    floor.lit_by_role.clear();
    floor.prop_anim.clear();
    floor.prop_boxes.clear();
    for d in &ds {
        let g = district_mesh(floor, d);
        floor.district_group.add(&g);
    }

    for child in children(&floor.path_group) {
        floor.path_group.remove(&child);
    }
    let pp = prop_palette(&floor.colors);
    let from: Vec<Vec3> = ds.iter().map(|d| Vec3::new(d.cx, 0.0, d.cy)).collect();
    floor.path_group.add(&props::build_paths(&from, &pp));
    let bearings: Vec<f32> = ds.iter().map(|d| d.cy.atan2(d.cx)).collect();
    let fountain = props::build_plaza_furniture(&pp, &bearings);
    fountain.group.set_dynamic(true); // the water animates
    floor.path_group.add(&fountain.group);
    floor.prop_anim.push(fountain.animate);
    floor.prop_boxes.extend(fountain.boxes);
    let sites: Vec<Site> = ds.iter().map(|d| Site { cx: d.cx, cy: d.cy, r: d.r }).collect();
    let (scatter, scatter_boxes) = outskirts::build_scatter(&sites, &pp);
    floor.path_group.add(&scatter);
    floor.prop_boxes.extend(scatter_boxes);

    floor.statics_dirty = true; // the world was rebuilt: re-bake the statics
}

/// `createBot(agent, district, hx, hy)`: a robot, its console pad and its task token.
fn create_bot(floor: &Floor, agent: &AgentInfo, district: usize, hx: f32, hy: f32) -> Bot {
    let d = &floor.districts[district];
    let orch = is_orchestrator(agent);
    let robot = Robot::with_variant(d.hue, orch, &floor.robot_palette, agent_hue(agent.cid), robot_variant(agent.cid));
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
    pad.set_dynamic(true).set_rotation(-FRAC_PI_2, 0.0, 0.0).set_position(hx, top_of(d) + 0.02, hy);
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
    }
}

/// `removeBot(b)`: takes the robot, pad and token out of the scene.
fn remove_bot(root: &Obj, bot: &Bot) {
    root.remove(&bot.robot.group);
    root.remove(&bot.pad);
    root.remove(&bot.token);
}

impl Floor {
    /// Keeps the floor in step with the agents: rebuilds the districts when the roles change
    /// (`districtSig`), adds and removes robots, updates names, states and labels' data.
    pub fn sync(&mut self, list: &[AgentInfo]) {
        let (districts, homes) = layout_districts(list);
        self.districts = districts;
        let sig = format!("{:?}", self.districts);
        if sig != self.district_sig {
            self.district_sig = sig;
            build_world(self);
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
            let top = top_of(&self.districts[idx]);
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

    /// The radius of the disc about the origin that holds every district's slab and its ramp, the
    /// orchestrator's dais, and the robots' labels above them (`LABEL_HEADROOM`, grid units). The
    /// view no longer fits this sphere; it stays as the bound the fit test checks against.
    #[allow(dead_code)] // read by the fit test only
    pub fn plaza_radius(&self) -> f32 {
        let mut reach = DAIS_R + DAIS_RAMP;
        for d in &self.districts {
            // A square's farthest corner from the origin; a round room's rim.
            let far = if d.round {
                d.cx.hypot(d.cy) + d.r
            } else {
                (d.cx.abs() + d.r).hypot(d.cy.abs() + d.r)
            };
            reach = reach.max(far + RAMP);
        }
        reach.hypot(LABEL_HEADROOM)
    }

    /// The points the fit frames, world units: each district's footprint with its ramp, on the
    /// ground and at its top; the dais's rim on the ground and at its top; and each robot's home
    /// up to the height of its label. The outskirts are left out.
    pub(super) fn fit_points(&self) -> Vec<Vec3> {
        let mut pts = Vec::new();
        for d in &self.districts {
            let top = top_of(d);
            for (x, z) in footprint(d) {
                pts.push(Vec3::new(x, 0.0, z));
                pts.push(Vec3::new(x, top, z));
            }
        }
        for (x, z) in rim(0.0, 0.0, DAIS_R + DAIS_RAMP) {
            pts.push(Vec3::new(x, 0.0, z));
            pts.push(Vec3::new(x, DAIS_H, z));
        }
        // A robot stands about 1.5 units tall, its label just above it.
        const ROBOT_AND_LABEL: f32 = 2.0;
        for bot in &self.bots {
            let (x, z) = bot.home;
            pts.push(Vec3::new(x, DAIS_H + ROBOT_AND_LABEL, z));
        }
        pts
    }

    /// `resetView()`: the isometric-ish view that frames the plaza in an `aspect` (width / height)
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
        // The near side of the plaza projects larger, so a target at the origin leaves the plaza low
        // on the stage. Each pass solves the distance, then moves the target across and up to the
        // middle of the points' extent on the stage; a few passes settle it.
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

/// Ground points around a district's footprint, its ramp included: a square's corners, or a round
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
        AgentInfo { cid, name: format!("agent {cid}"), role: role.into(), status: "idle".into(), tier: if role == "orchestrator" { "orchestrator".into() } else { "worker".into() }, model: String::new() }
    }

    #[test]
    fn layout_districts_two_roles_four_homes() {
        let list = [agent(1, "coder"), agent(2, "coder"), agent(3, "writer"), agent(4, "writer")];
        let (districts, homes) = layout_districts(&list);
        assert_eq!(districts.len(), 2);
        assert_eq!(homes.len(), 4);
    }

    #[test]
    fn layout_sync_creates_four_bots() {
        let list = [agent(1, "coder"), agent(2, "coder"), agent(3, "writer"), agent(4, "writer")];
        let mut floor = Floor::new();
        floor.sync(&list);
        assert_eq!(floor.bots.len(), 4);
    }
}
