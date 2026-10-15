//! `floor3d/rooms/build.ts`: a generated layout (`gen.rs`) built into a room's meshes, in the room's local frame:
//! the floor at y = 0, the walls outside the inner size, the name board on the back (-z) wall, and every item from
//! its catalogue model. `kit:<name>` is the room kit's builder; `prop:<id>` is the station prop (`robots/props.rs`,
//! built with the robot palette). A prop with no file, or an unknown model, stands in as a placeholder box of its
//! footprint. Shadows follow the web's gates (`cast_policy`).

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, PI};
use std::sync::{Mutex, OnceLock};

use glam::Vec3;

use super::super::geometry;
use super::super::props::Anim;
use super::super::robot::robot_palette;
use super::super::robots::props::{build_prop, prop_registry};
use super::super::scene::{Color, Kind, Mat, Material, Obj};
use super::super::textures::{Rng, canvas_texture};
use super::super::toon;
use super::generate::{
    CatItem, Catalog, Door, Layout, LayoutDoor, LayoutItem, LayoutSpot, Place, layout_hash,
};
use super::kit::{FloorKind, KitPalette, Room, WallKind, floor_texture, make_kit, wall_look};

/// An item casts only from a bounding radius of CAST_RADIUS; a dynamic item (a lit screen) needs DYNAMIC_CAST_RADIUS.
const CAST_RADIUS: f32 = 0.3;
const DYNAMIC_CAST_RADIUS: f32 = 0.6;
/// The plant builder's height at scale 1 (pot base to leaf tip); a catalogue height scales it.
const PLANT_HEIGHT: f32 = 1.07;
/// The name board's text must fit the canvas, with this margin each side.
const SIGN_MARGIN: f32 = 40.0;
/// The ink line's minimum size (toon.ts `outline`).
const OUTLINE_MIN: f32 = 0.12;

/// A built room's meshes (`BuiltLayout` in build.ts). `rooms::build_room` adds the layout and the warning to them.
pub struct BuiltLayout {
    /// Furniture, walls and the name board, in the room's local frame (turn it to face the plaza).
    pub group: Obj,
    /// The floor's top surface material.
    pub floor: Mat,
    /// Monitor materials that light up while the room's agents work.
    pub screens: Vec<Mat>,
    pub animate: Anim,
    /// Where a robot stands to use each item, already in the room's frame (the layout's spots).
    pub spots: Vec<LayoutSpot>,
    /// The inner size the room was built at (x by z).
    pub w: f64,
    pub d: f64,
}

/// `buildLayout(layout, catalog, hue, palette, w, d, s, name)`: the room's meshes for a layout. `w` and `d` are the
/// inner size (x by z); `s` is the half-size the floor texture is drawn for; `name` is the room's name on its board
/// (the role's label). Every number comes from `layout`, so the same layout always builds the same room.
#[allow(clippy::too_many_arguments)]
pub fn build_layout(
    layout: &Layout,
    catalog: &Catalog,
    hue: f32,
    palette: &KitPalette,
    w: f64,
    d: f64,
    s: f32,
    name: &str,
) -> BuiltLayout {
    let g = Obj::group();
    let kit = make_kit(hue, palette);
    let mut room = Room::new(g.clone(), s, hue, kit, Rng::new(layout_hash(layout)));
    build_walls(
        &mut room,
        &layout.shell.walls,
        wall_kind(&layout.shell.wall),
        w as f32,
        d as f32,
        &layout.shell.door,
    );
    for item in &layout.items {
        place_item(&mut room, item, catalog);
    }
    name_board(&mut room, name, d as f32);
    toon::outline(&g, OUTLINE_MIN);

    // Tinted toward the night's blue, so a room's floor reads as lit by the plaza's moonlight.
    let floor = Mat::toon(Color::hex(0xb4bde0));
    floor.get().map = Some(floor_texture(floor_kind(&layout.shell.floor), hue, s));
    let screen = room.k.screen.clone();
    let (glow_ok, glow_info) = (room.k.glow_ok.clone(), room.k.glow_info.clone());
    let (ok, info) = (palette.ok, palette.info);
    let mut anims = std::mem::take(&mut room.anim);
    let animate: Anim = Box::new(move |t: f32| {
        glow_ok.get().color = ok.scale(0.4 + 0.6 * (t * 2.7).sin().max(0.0));
        glow_info.get().color = info.scale(0.4 + 0.6 * (t * 3.9 + 1.0).sin().max(0.0));
        for a in anims.iter_mut() {
            a(t);
        }
    });
    BuiltLayout {
        group: g,
        floor,
        screens: vec![screen],
        animate,
        spots: layout.spots.clone(),
        w,
        d,
    }
}

/// A floor kind by its layout name; an unknown name is the web's default, tiles.
fn floor_kind(name: &str) -> FloorKind {
    match name {
        "planks" => FloorKind::Planks,
        "parquet" => FloorKind::Parquet,
        "checker" => FloorKind::Checker,
        "hazard" => FloorKind::Hazard,
        "slate" => FloorKind::Slate,
        "carpet" => FloorKind::Carpet,
        "blueprint" => FloorKind::Blueprint,
        "stars" => FloorKind::Stars,
        "marble" => FloorKind::Marble,
        "gravel" => FloorKind::Gravel,
        "lawn" => FloorKind::Lawn,
        _ => FloorKind::Tiles,
    }
}

/// A wall kind by its layout name. `none`, and any unknown name, has no wall (wallLook returns null).
fn wall_kind(name: &str) -> Option<WallKind> {
    match name {
        "panel" => Some(WallKind::Panel),
        "brick" => Some(WallKind::Brick),
        "hedge" => Some(WallKind::Hedge),
        "glass" => Some(WallKind::Glass),
        "fence" => Some(WallKind::Fence),
        "stone" => Some(WallKind::Stone),
        _ => None,
    }
}

/// The walls on the sides the layout names. A wall sits outside the inner size: its inner face is at the edge.
/// The door side has two stubs with the opening (`door.width`, centred at 0) between them, and a thin lintel over the
/// opening (not on hedges and glass), so no wall runs across the door (build.ts `buildWalls`).
fn build_walls(
    room: &mut Room,
    sides: &[Door],
    kind: Option<WallKind>,
    w: f32,
    d: f32,
    door: &LayoutDoor,
) {
    let Some(kind) = kind else { return };
    let Some((mat, h, t)) = wall_look(kind, room.hue) else {
        return;
    };
    if sides.is_empty() {
        return;
    }
    // A cap rail in the darker wood (the cartoon trim), except on the hedge and the glass.
    let cap = kind != WallKind::Hedge && kind != WallKind::Glass;
    let dark = room.k.dark_wood.clone();
    let seg = |room: &mut Room, x: f32, z: f32, len: f32, rot: f32| {
        let it = room.item(x, z, rot);
        room.box_(&it, len, h, t, &mat, 0.0, 0.0, 0.0);
        if cap {
            room.box_(&it, len + 0.04, 0.06, t + 0.06, &dark, 0.0, h, 0.0);
        }
    };
    let hw = door.width as f32 / 2.0;
    for side in sides {
        // The north and south walls run the full outer width (they cover the corners); east and west the inner depth.
        // `fixed` is the wall's centre line, `outer` its half length, and `at` maps a coordinate along the wall to (x, z).
        let (ns, fixed, rot, outer) = match side {
            Door::N => (true, -d / 2.0 - t / 2.0, 0.0, w / 2.0 + t),
            Door::S => (true, d / 2.0 + t / 2.0, 0.0, w / 2.0 + t),
            Door::E => (false, w / 2.0 + t / 2.0, FRAC_PI_2, d / 2.0),
            Door::W => (false, -w / 2.0 - t / 2.0, FRAC_PI_2, d / 2.0),
        };
        let at = |along: f32| if ns { (along, fixed) } else { (fixed, along) };
        if *side != door.side {
            let (x, z) = at(0.0);
            seg(room, x, z, 2.0 * outer, rot);
            continue;
        }
        // The door side: a stub from each outer end to the opening's edge, then a lintel over the opening.
        for (a, b) in [(-outer, -hw), (hw, outer)] {
            let len = b - a;
            if len <= 0.0 {
                continue;
            }
            let (x, z) = at((a + b) / 2.0);
            seg(room, x, z, len, rot);
        }
        if kind != WallKind::Hedge && kind != WallKind::Glass && door.width > 0.0 {
            let (x, z) = at(0.0);
            let it = room.item(x, z, rot);
            room.box_(&it, door.width as f32, 0.2, t, &mat, 0.0, h - 0.2, 0.0);
        }
    }
}

/// The room's name on a painted board at the middle of the back (-z) wall, always kept.
fn name_board(room: &mut Room, name: &str, d: f32) {
    let tex = canvas_texture(&format!("roomsign:{name}"), 512, 128, 1.0, |g, w, h, _| {
        g.set_fill("#fff6e0");
        g.fill_rect(0.0, 0.0, w, h);
        g.set_stroke("#2b2420");
        g.set_line_width(10.0);
        g.stroke_rect(5.0, 5.0, w - 10.0, h - 10.0);
        g.set_fill("#2b2420");
        g.set_text_align("center");
        g.set_text_baseline("middle");
        let font =
            |px: f32| format!("700 {px}px ui-rounded, \"Comic Sans MS\", system-ui, sans-serif");
        g.set_font(&font(64.0));
        let measured = g.measure_text(name);
        if measured > w - SIGN_MARGIN {
            g.set_font(&font((64.0 * (w - SIGN_MARGIN) / measured).floor()));
        }
        g.fill_text(name, w / 2.0, h / 2.0 + 4.0);
    });
    let it = room.item(0.0, -d / 2.0 + 0.3, 0.0);
    // The posts are thin rods: they never cast (kit `cyl` thinPart).
    let dark = room.k.dark_wood.clone();
    room.cyl(&it, 0.04, 0.04, 1.5, &dark, -0.8, 0.0, 0.0, 8);
    room.cyl(&it, 0.04, 0.04, 1.5, &dark, 0.8, 0.0, 0.0, 8);
    // The board's wood body keeps casting; its painted face is a plane a hair proud of the body's front.
    let wood = room.k.wood.clone();
    let body = Obj::mesh(&geometry::box_geo(1.9, 0.48, 0.06), &wood);
    body.set_position(0.0, 1.42, 0.02).set_shadows(true, false);
    it.add(&body);
    let face = Mat::toon(Color::WHITE);
    face.get().map = Some(tex);
    let plane = Obj::mesh(&geometry::plane(1.9, 0.48, 1, 1), &face);
    plane
        .set_position(0.0, 1.42, 0.052)
        .set_shadows(false, false);
    it.add(&plane);
}

/// One layout item, placed at its (x, y, z) and turned by its rot (degrees about y).
fn place_item(room: &mut Room, item: &LayoutItem, catalog: &Catalog) {
    let cat = catalog.items.get(&item.id);
    let size = cat.map_or([0.5, 0.5, 0.5], |c| c.size);
    if cat.and_then(|c| c.layer.as_deref()) == Some("floor") {
        // Floor layer (rugs): a flat rug lifted a hair above the floor and polygon-offset (a decal), never cast.
        let mat = rug_mat(&room.k.accent_soft);
        room.rug(
            size[0] as f32,
            size[2] as f32,
            &mat,
            item.x as f32,
            item.z as f32,
            false,
        );
        return;
    }
    let rot = item.rot as f32 * PI / 180.0;
    let model = cat.map_or("", |c| c.model.as_str());
    let obj = match model.strip_prefix("prop:") {
        Some(id) => prop_object(room, id, item, rot, size),
        None => kit_object(
            room,
            model.strip_prefix("kit:").unwrap_or(model),
            item,
            cat,
            rot,
        ),
    };
    let p = obj.position();
    obj.set_position(p.x, item.y as f32, p.z);
    let place = cat.map(|c| c.place);
    // A surface item (a monitor) lights its screen while the room works: it is dynamic, so it is never baked.
    if place == Some(Place::Surface) {
        obj.set_dynamic(true);
    }
    cast_policy(&obj, place == Some(Place::Wallmount));
}

/// A kit model by its builder's name. An unknown name stands in as a placeholder box of the catalogue size.
fn kit_object(
    room: &mut Room,
    name: &str,
    item: &LayoutItem,
    cat: Option<&CatItem>,
    rot: f32,
) -> Obj {
    let (x, z) = (item.x as f32, item.z as f32);
    let [w, h, depth] = cat.map_or([0.5, 0.5, 0.5], |c| c.size).map(|v| v as f32);
    match name {
        "deskTop" => room.desk_top(x, z, rot),
        "chair" => room.chair(x, z, rot, None),
        "monitor" => room.monitor(x, z, rot, item.y as f32),
        "shelf" => room.shelf(x, z, rot, w, h),
        "plant" => room.plant(x, z, h / PLANT_HEIGHT),
        "lamp" => room.lamp(x, z),
        "poster" => room.poster(x, z, rot),
        "clock" => room.clock(x, z, rot),
        "sofa" => room.sofa(x, z, rot),
        "bin" => room.bin(x, z),
        "coatStand" => room.coat_stand(x, z),
        "smallCrate" => room.small_crate(x, z, rot),
        "safe" => room.safe(x, z, rot),
        _ => room.prop_placeholder(x, z, rot, w, h, depth, name),
    }
}

/// A station prop from its file, built with the robot palette at the floor's centre; a placeholder without one.
fn prop_object(room: &mut Room, id: &str, item: &LayoutItem, rot: f32, size: [f64; 3]) -> Obj {
    let (x, z) = (item.x as f32, item.z as f32);
    let Some(prop) = prop_registry().props.get(id) else {
        let [w, h, d] = size.map(|v| v as f32);
        return room.prop_placeholder(x, z, rot, w, h, d, &format!("prop:{id}"));
    };
    let built = build_prop(prop, &robot_palette(), room.hue);
    built
        .group
        .set_position(x, item.y as f32, z)
        .set_rotation(0.0, rot, 0.0);
    room.g.add(&built.group);
    prop_thin_pass(&built.group);
    built.group
}

/// A prop's meshes are merged per bone in the web (a BufferGeometry), so its `thinPart` is the bounding-box rule
/// alone: a mesh under 0.02 in any side never casts.
fn prop_thin_pass(group: &Obj) {
    group.traverse(&mut |o: &Obj| {
        let thin = match &o.get().kind {
            Kind::Mesh { geo, .. } => {
                bounds(&geo.positions).is_some_and(|(lo, hi)| (hi - lo).min_element() < 0.02)
            }
            _ => return,
        };
        o.get().cast_shadow = !thin;
    });
}

/// The item's cast gate (build.ts `castPolicy`): its bounding radius, the box of its meshes in the item's own frame
/// (its turn included), must reach CAST_RADIUS, or DYNAMIC_CAST_RADIUS when an item is dynamic, and a wall-mounted
/// piece never casts. A mesh keeps its own thinness (set when it was made), so it casts only if both allow it.
fn cast_policy(obj: &Obj, wallmount: bool) {
    let mut meshes = Vec::new();
    let mut dynamic = false;
    obj.traverse(&mut |o: &Obj| {
        let n = o.get();
        dynamic |= n.dynamic;
        if matches!(n.kind, Kind::Mesh { .. }) {
            meshes.push(o.clone());
        }
    });
    let radius = mesh_box(obj, &meshes).map_or(0.0, |(lo, hi)| (hi - lo).length() / 2.0);
    let cast = !wallmount && radius >= CAST_RADIUS && (!dynamic || radius >= DYNAMIC_CAST_RADIUS);
    for m in &meshes {
        m.get().cast_shadow &= cast;
    }
}

/// The box of the given meshes (under `obj`) in `obj`'s parent frame: `obj`'s own position and turn are included, as
/// the web's `setFromObject(obj)` has them.
fn mesh_box(obj: &Obj, meshes: &[Obj]) -> Option<(Vec3, Vec3)> {
    let mut all: Option<(Vec3, Vec3)> = None;
    for m in meshes {
        let Some(world) = obj.world_matrix_of(m) else {
            continue;
        };
        let local = match &m.get().kind {
            Kind::Mesh { geo, .. } => bounds(&geo.positions),
            _ => None,
        };
        let Some(local) = local else { continue };
        for corner in corners(local) {
            let p = world.transform_point3(corner);
            all = Some(match all {
                None => (p, p),
                Some((lo, hi)) => (lo.min(p), hi.max(p)),
            });
        }
    }
    all
}

/// The min and max corners of a point set; None when it is empty.
fn bounds(points: &[[f32; 3]]) -> Option<(Vec3, Vec3)> {
    let mut it = points.iter().map(|p| Vec3::from_array(*p));
    let first = it.next()?;
    Some(it.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p))))
}

/// The eight corners of a box.
fn corners((lo, hi): (Vec3, Vec3)) -> [Vec3; 8] {
    std::array::from_fn(|i| {
        Vec3::new(
            if i & 1 == 0 { lo.x } else { hi.x },
            if i & 2 == 0 { lo.y } else { hi.y },
            if i & 4 == 0 { lo.z } else { hi.z },
        )
    })
}

/// A rug's material with a depth bias (the decal flag: drawn as the hardware decals are, so it wins over the floor
/// it lies on). One per source material, cached (the web caches per source material too).
fn rug_mat(source: &Mat) -> Mat {
    static CACHE: OnceLock<Mutex<HashMap<u64, Mat>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = cache.lock().unwrap_or_else(|e| e.into_inner());
    map.entry(source.id())
        .or_insert_with(|| {
            Mat::new(Material {
                decal: true,
                ..source.snapshot()
            })
        })
        .clone()
}
