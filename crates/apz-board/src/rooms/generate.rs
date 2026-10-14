//! The room generator, v2 (groups with a purpose): a line-by-line port of the web's `rooms/gen.ts`, with the
//! algorithm in `docs/design/rooms.md` ("v2 algorithm details" is binding). The same input and the same data give
//! the same layout, bit for bit: every float expression below is written in the order `gen.ts` runs it, and the
//! golden fixtures and the oracle dumps prove both apps agree. Nothing here touches the scene: the output is plain
//! data that `build.rs` turns into meshes.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The catalogue, the role templates and the precomputed minimum sizes: the same JSON the web loads.
const CATALOG_JSON: &str = include_str!("../../../../assets/rooms/catalog.json");
const ROLES_JSON: &str = include_str!("../../../../assets/rooms/roles.json");
const MIN_SIZE_JSON: &str = include_str!("../../../../assets/rooms/minsize.json");

const CELL_DEFAULT: f64 = 0.25;
const INSET: f64 = 0.05;
const WALLMOUNT_DEPTH: f64 = 0.1;
/// Metres in front of a wall-mounted item that must stay clear of tall solids.
const MOUNT_VIEW: f64 = 0.6;
/// Half-width margin of the view strip, on each side of the mount.
const MOUNT_VIEW_SIDE: f64 = 0.1;
const CLEAR_MIN_ACCESS: f64 = 0.25;
const EPS: f64 = 1e-9;
const COPLANAR_TOL: f64 = 1e-3;
const MAX_FAILED_TRIES: usize = 20;
const ON_SURFACE_OFFSET: f64 = 0.05;
const DOOR_NEAR: f64 = 0.6;

// ---- the PRNG, the hashes, the canonical JSON ---------------------------------------------------

/// mulberry32: one generator per room. Every step is u32 arithmetic, as the JS `Math.imul`, `>>>` and `^`.
#[derive(Clone, Debug)]
pub struct Rng {
    a: u32,
}

impl Rng {
    pub fn new(seed: u32) -> Self {
        Rng { a: seed }
    }

    /// The next draw in [0, 1).
    pub fn next_f64(&mut self) -> f64 {
        self.a = self.a.wrapping_add(0x6D2B_79F5);
        let mut t = self.a;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }
}

/// `pick(list)` = `list[floor(r() * len)]`; the draw is taken even for an empty list (where the web gives
/// `undefined`, and the template's "" here).
fn pick_str(list: &[String], rng: &mut Rng) -> String {
    let r = rng.next_f64();
    if list.is_empty() {
        String::new()
    } else {
        list[(r * list.len() as f64).floor() as usize].clone()
    }
}

/// FNV-1a, 32 bit, over the UTF-8 bytes.
pub fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for &b in bytes {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// The seed a floor gives a room: `fnv1a32(machineId + "/" + role + "/" + roomIndex)`.
pub fn room_seed(machine_id: &str, role: &str, room_index: usize) -> u32 {
    fnv1a32(format!("{machine_id}/{role}/{room_index}").as_bytes())
}

/// `Math.round(v * 1000) / 1000`, with `-0` as `0`. JS `Math.round` rounds halves toward +infinity, so this is
/// `floor` plus one when the fraction is at least a half.
pub fn round_num(v: f64) -> f64 {
    let y = v * 1000.0;
    let f = y.floor();
    let r = if y - f >= 0.5 { f + 1.0 } else { f };
    let out = r / 1000.0;
    if out == 0.0 {
        0.0
    } else {
        out
    }
}

/// A rounded number as the canonical JSON writes it (`String(roundNum(v))`): the shortest decimal, `-0` as `0`.
fn num(v: f64) -> String {
    let r = round_num(v);
    if r == 0.0 {
        return "0".to_string();
    }
    format!("{r}")
}

fn json_str(s: &str) -> String {
    serde_json::to_string(s).expect("a string always serialises")
}

/// The canonical JSON of any JSON value (`canonicalJson` in gen.ts): object keys sorted, no spaces, numbers
/// rounded like `roundNum` (so `-0` is `0` and no trailing zeros), strings JSON-escaped. Generic over the value, so
/// a key added to a layout is hashed without any change here.
pub fn canonical_value(v: &Value) -> String {
    let mut out = String::new();
    write_canonical(v, &mut out);
    out
}

fn write_canonical(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => match n.as_f64() {
            Some(f) => out.push_str(&num(f)),
            None => out.push_str("null"),
        },
        Value::String(s) => out.push_str(&json_str(s)),
        Value::Array(items) => {
            out.push('[');
            for (k, item) in items.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (k, key) in keys.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                out.push_str(&json_str(key));
                out.push(':');
                write_canonical(&map[key.as_str()], out);
            }
            out.push('}');
        }
    }
}

// ---- the data: the catalogue and the role templates ----------------------------------------------

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Door {
    N,
    E,
    S,
    W,
}

impl Door {
    pub fn as_str(self) -> &'static str {
        match self {
            Door::N => "n",
            Door::E => "e",
            Door::S => "s",
            Door::W => "w",
        }
    }

    pub fn parse(s: &str) -> Option<Door> {
        match s {
            "n" => Some(Door::N),
            "e" => Some(Door::E),
            "s" => Some(Door::S),
            "w" => Some(Door::W),
            _ => None,
        }
    }
}

const SIDES: [Door; 4] = [Door::N, Door::E, Door::S, Door::W];

/// The rotation that turns a wall item's front (+z) to the room centre, per wall (`SIDE_ROT`).
fn side_rot(d: Door) -> f64 {
    match d {
        Door::N => 0.0,
        Door::E => 270.0,
        Door::S => 180.0,
        Door::W => 90.0,
    }
}

/// An item's `place`. Unknown values are kept (`Other`) rather than failing the parse: the web treats any place
/// other than `surface` and `wallmount` as a solid (or a floor layer, see `layer`).
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Place {
    Wall,
    Corner,
    Center,
    Free,
    Surface,
    Wallmount,
    #[serde(other)]
    Other,
}

/// A group's anchor rule (`Anchor` in gen.ts). Unknown rules behave as `center` for the candidates but still
/// ban the middle region (the web's `rule !== 'center'`).
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Anchor {
    Wall,
    Corner,
    Center,
    Door,
    Wallmount,
    #[serde(other)]
    Other,
}

#[derive(Deserialize, Clone, Debug)]
pub struct CatSpot {
    pub at: [f64; 2],
    pub face: f64,
    #[serde(default)]
    pub clip: Option<String>,
}

#[derive(Deserialize, Clone, Copy, Debug, Default)]
pub struct Clear {
    #[serde(default)]
    pub front: f64,
    #[serde(default)]
    pub side: f64,
}

#[derive(Deserialize, Clone, Debug)]
pub struct CatItem {
    /// The model: `kit:<name>`, `prop:<id>` or `part:<id>`.
    pub model: String,
    /// Footprint w (x) by h (height) by d (z) at rot 0.
    pub size: [f64; 3],
    pub place: Place,
    #[serde(default)]
    pub face: Option<String>,
    #[serde(default)]
    pub clear: Option<Clear>,
    #[serde(default)]
    pub spots: Vec<CatSpot>,
    #[serde(default)]
    pub walk: bool,
    /// `"floor"` for rugs: never occupies cells, y = 0.
    #[serde(default)]
    pub layer: Option<String>,
    #[serde(default)]
    pub mount: Option<f64>,
}

/// One entry of a catalogue group: a catalogue item id, or `$station` (the template's station).
#[derive(Deserialize, Clone, Debug)]
pub struct CatGroupItem {
    pub id: String,
    /// Relation to the anchor (absent for the first item).
    #[serde(default)]
    pub rel: Option<String>,
    #[serde(default)]
    pub gap: Option<f64>,
    /// `"anchor"`: the item faces the anchor.
    #[serde(default)]
    pub face: Option<String>,
    /// `[lo, hi]`: the number of copies, drawn once per group instance.
    #[serde(default)]
    pub count: Option<[i64; 2]>,
    #[serde(default)]
    pub optional: bool,
}

#[derive(Deserialize, Clone, Debug)]
pub struct CatGroup {
    pub anchor: Anchor,
    pub items: Vec<CatGroupItem>,
}

#[derive(Deserialize, Debug)]
pub struct Catalog {
    #[serde(default)]
    pub cell: Option<f64>,
    pub items: BTreeMap<String, CatItem>,
    #[serde(default)]
    pub groups: BTreeMap<String, CatGroup>,
}

/// A template entry naming a group: `station` replaces `$station`; `per: "agent"` repeats it per desk.
#[derive(Deserialize, Clone, Debug)]
pub struct RoleGroupRef {
    pub group: String,
    #[serde(default)]
    pub station: Option<String>,
    #[serde(default)]
    pub per: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct RolePool {
    pub group: String,
    #[serde(default)]
    pub w: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct RoleShell {
    pub floor: Vec<String>,
    pub wall: Vec<String>,
    pub open: f64,
}

#[derive(Deserialize, Clone, Debug)]
pub struct RoleTemplate {
    #[serde(default)]
    pub family: Option<String>,
    /// `"wall"` (the default) or `"room"`: where desk-sets anchor (see `anchor_rules_of`).
    #[serde(default)]
    pub desk_facing: Option<String>,
    pub shell: RoleShell,
    /// Placed first; a failure is a drop (a verifier violation).
    #[serde(default)]
    pub required: Vec<RoleGroupRef>,
    /// Placed after required, in order, if they fit; a failure is a skip, never a violation.
    #[serde(default)]
    pub preferred: Vec<RoleGroupRef>,
    #[serde(default)]
    pub pool: Vec<RolePool>,
    pub density: [f64; 2],
}

#[derive(Deserialize, Debug)]
pub struct Roles {
    pub roles: BTreeMap<String, RoleTemplate>,
    #[serde(default)]
    pub families: BTreeMap<String, RoleTemplate>,
}

/// The shipped data, parsed once.
pub struct Data {
    pub catalog: Catalog,
    pub roles: Roles,
}

pub fn data() -> &'static Data {
    static DATA: OnceLock<Data> = OnceLock::new();
    DATA.get_or_init(|| Data {
        catalog: serde_json::from_str(CATALOG_JSON).expect("assets/rooms/catalog.json parses"),
        roles: serde_json::from_str(ROLES_JSON).expect("assets/rooms/roles.json parses"),
    })
}

/// The role ids in roles.json's own order (the web's `Object.keys(ROLES.roles)`): the room lab lines them up in it.
pub fn role_order() -> &'static [String] {
    use serde::de::{Deserializer, IgnoredAny, MapAccess, Visitor};
    struct Keys;
    impl<'de> Visitor<'de> for Keys {
        type Value = Vec<String>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a map of roles")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Vec<String>, A::Error> {
            let mut keys = Vec::new();
            while let Some((k, IgnoredAny)) = map.next_entry::<String, IgnoredAny>()? {
                keys.push(k);
            }
            Ok(keys)
        }
    }
    struct InOrder(Vec<String>);
    impl<'de> Deserialize<'de> for InOrder {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<InOrder, D::Error> {
            d.deserialize_map(Keys).map(InOrder)
        }
    }
    #[derive(Deserialize)]
    struct File {
        roles: InOrder,
    }
    static ORDER: OnceLock<Vec<String>> = OnceLock::new();
    ORDER.get_or_init(|| serde_json::from_str::<File>(ROLES_JSON).expect("assets/rooms/roles.json parses").roles.0)
}

/// The template of a role: its own entry, else the family "other" (`templateFor` in gen.ts).
pub fn template_for<'r>(role: &str, roles: &'r Roles) -> Option<&'r RoleTemplate> {
    roles.roles.get(role).or_else(|| roles.families.get("other"))
}

/// The precomputed minimum sizes: role -> the minSize for 1..6 agents (`null` = none up to 8).
pub fn min_size_table() -> &'static BTreeMap<String, Vec<Option<f64>>> {
    static TABLE: OnceLock<BTreeMap<String, Vec<Option<f64>>>> = OnceLock::new();
    TABLE.get_or_init(|| serde_json::from_str(MIN_SIZE_JSON).expect("assets/rooms/minsize.json parses"))
}

/// The smallest square side (0.5 steps from 2.5 to 8) at which the required groups of `role` always place, for
/// `agents` desks: the table's value when it holds this role and count, otherwise computed (`minSize` in gen.ts).
/// `f64::INFINITY` when no side up to 8 qualifies.
pub fn min_size(role: &str, agents: usize, data: &Data) -> f64 {
    if let Some(row) = min_size_table().get(role) {
        if let Some(v) = agents.checked_sub(1).and_then(|i| row.get(i)) {
            return v.unwrap_or(f64::INFINITY);
        }
    }
    compute_min_size(role, agents, data)
}

/// minSize by generation (`computeMinSize` in gen.ts): every seed 1..=500 and each of the four doors must drop
/// no required group.
pub fn compute_min_size(role: &str, agents: usize, data: &Data) -> f64 {
    for k in 0..12 {
        let side = 2.5 + 0.5 * f64::from(k);
        let fits = SIDES.iter().all(|&door| {
            (1..=500_u32).all(|seed| {
                let input = Input { role: role.to_string(), w: side, d: side, door, seed, agents: Some(agents) };
                generate(&input, data).dropped.is_empty()
            })
        });
        if fits {
            return side;
        }
    }
    f64::INFINITY
}

// ---- the input and the output -----------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Input {
    pub role: String,
    /// Inner size in units (x by z).
    pub w: f64,
    pub d: f64,
    /// The side facing the nearest walkway: n = -z, e = +x, s = +z, w = -x.
    pub door: Door,
    pub seed: u32,
    /// Desk count for per-agent entries; default `clamp(floor(w * d / 6), 1, 4)`.
    pub agents: Option<usize>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct LayoutDoor {
    pub side: Door,
    pub at: f64,
    pub width: f64,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct LayoutShell {
    pub floor: String,
    pub wall: String,
    pub walls: Vec<Door>,
    pub door: LayoutDoor,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct LayoutItem {
    pub n: usize,
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub rot: f64,
    pub host: Option<usize>,
    /// The group instance (into `Layout::groups`); the first item with a given `g` is that group's anchor.
    pub g: usize,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct LayoutSpot {
    pub item: usize,
    pub x: f64,
    pub z: f64,
    pub face: f64,
    pub clip: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Layout {
    pub shell: LayoutShell,
    pub items: Vec<LayoutItem>,
    pub spots: Vec<LayoutSpot>,
    /// `groups[g]` = the group id of instance g, in creation order.
    pub groups: Vec<String>,
    /// Required group ids that could not be placed (a verifier violation), in order of failure.
    pub dropped: Vec<String>,
    /// Preferred group ids that did not fit (not a violation), in order of failure.
    pub skipped: Vec<String>,
}

/// The canonical JSON of the layout (every key of the serialised layout, so a new field is hashed too).
pub fn canonical_json(l: &Layout) -> String {
    canonical_value(&serde_json::to_value(l).expect("a layout always serialises"))
}

/// FNV-1a 32 of the layout's canonical JSON: the layout hash the golden fixtures pin.
pub fn layout_hash(l: &Layout) -> u32 {
    fnv1a32(canonical_json(l).as_bytes())
}

// ---- geometry ------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Solid,
    Surface,
    Floor,
    Wall,
}

fn kind_of(cat: &CatItem) -> Kind {
    if cat.place == Place::Surface {
        return Kind::Surface;
    }
    if cat.layer.as_deref() == Some("floor") {
        return Kind::Floor;
    }
    if cat.place == Place::Wallmount {
        return Kind::Wall;
    }
    Kind::Solid
}

/// Rotate a local (x, z) by rot degrees about y (exact for 0/90/180/270): `rotPt` in gen.ts.
fn rot_pt(x: f64, z: f64, rot: f64) -> (f64, f64) {
    let r = ((rot % 360.0) + 360.0) % 360.0;
    if r == 90.0 {
        (z, -x)
    } else if r == 180.0 {
        (-x, -z)
    } else if r == 270.0 {
        (-z, x)
    } else {
        (x, z)
    }
}

/// The rot (0/90/180/270) whose front `(sin rot, cos rot)` best faces the room centre; ties take the first.
fn facing(cx: f64, cz: f64) -> f64 {
    // dot(front, toCentre) with toCentre = (-cx, -cz), for rot 0, 90, 180, 270.
    let dots = [-cz, -cx, cz, cx];
    let mut best = 0;
    for k in 1..4 {
        if dots[k] > dots[best] {
            best = k;
        }
    }
    best as f64 * 90.0
}

/// Index range of cells whose open square overlaps the open interval [lo, hi] (`idxRange`).
fn idx_range(lo: f64, hi: f64, half: f64, cell: f64, n: i64) -> (i64, i64) {
    let a = (((lo + half) / cell + EPS).floor().max(0.0)) as i64;
    let b = (((hi + half) / cell - EPS).ceil() as i64 - 1).min(n - 1);
    (a, b)
}

/// The extent of rotated points: (min x, max x, min z, max z), as the web's running min and max.
fn extent(pts: &[(f64, f64)]) -> (f64, f64, f64, f64) {
    let (mut mnx, mut mxx, mut mnz, mut mxz) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
    for &(px, pz) in pts {
        if px < mnx {
            mnx = px;
        }
        if px > mxx {
            mxx = px;
        }
        if pz < mnz {
            mnz = pz;
        }
        if pz > mxz {
            mxz = pz;
        }
    }
    (mnx, mxx, mnz, mxz)
}

/// A rectangle in room coordinates.
#[derive(Clone, Copy, Debug)]
struct Bx {
    x0: f64,
    x1: f64,
    z0: f64,
    z1: f64,
}

/// Strict overlap of two boxes (positive area).
fn overlaps(a: Bx, b: Bx) -> bool {
    a.x1.min(b.x1) - a.x0.max(b.x0) > 0.0 && a.z1.min(b.z1) - a.z0.max(b.z0) > 0.0
}

/// A placed footprint and its grid cells (`Geo` in gen.ts).
#[derive(Clone, Copy, Debug)]
struct Geo {
    x: f64,
    z: f64,
    rot: f64,
    fw: f64,
    fd: f64,
    x0: f64,
    x1: f64,
    z0: f64,
    z1: f64,
    i0: i64,
    i1: i64,
    j0: i64,
    j1: i64,
    has_clear: bool,
    ci0: i64,
    ci1: i64,
    cj0: i64,
    cj1: i64,
}

impl Geo {
    fn bx(&self) -> Bx {
        Bx { x0: self.x0, x1: self.x1, z0: self.z0, z1: self.z1 }
    }
}

#[derive(Clone, Copy, Debug)]
struct Spot {
    x: f64,
    z: f64,
    face: f64,
    cell: i64,
}

/// A placed item (`Rec` in gen.ts). `n` is its index in `Ctx::recs`.
#[derive(Clone, Debug)]
struct Rec<'a> {
    n: usize,
    id: String,
    kind: Kind,
    cat: &'a CatItem,
    g: Geo,
    y: f64,
    top: f64,
    host: Option<usize>,
    gi: usize,
    spots: Vec<Spot>,
    spot_clips: Vec<Option<String>>,
    /// Cells that must be reachable (spots and access cell), for solid and wall items only; -1 = outside.
    checks: Vec<i64>,
    /// Wall-mounted items: the strip in front that tall solids must not block, and the height limit.
    view: Option<Bx>,
    limit: f64,
}

#[derive(Clone, Debug)]
struct Cand {
    geo: Geo,
    rule_fit: f64,
    work_far: f64,
}

/// One item slot of a group instance, with its relation resolved (`Slot` in gen.ts).
struct Slot<'a> {
    id: String,
    cat: &'a CatItem,
    rel: String,
    gap: f64,
    face: String,
    required: bool,
    side: Option<Door>,
}

/// The row cursor and stop flag of one attempt, for `row` slots.
struct RowState {
    right: f64,
    stopped: bool,
}

struct Ctx<'a> {
    w: f64,
    d: f64,
    cell: f64,
    cols: i64,
    rows: i64,
    door: Door,
    door_half: f64,
    diag: f64,
    items: &'a BTreeMap<String, CatItem>,
    groups: &'a BTreeMap<String, CatGroup>,
    xs: Vec<f64>,
    zs: Vec<f64>,
    /// Solid footprints covering each cell (saturating), and the non-walk ones (blocking). u8 like the web's arrays.
    occ: Vec<u8>,
    blk: Vec<u8>,
    /// Clear-zone owner: -1 none, -3 several, otherwise the record index.
    res: Vec<i64>,
    /// Door zone or spine.
    ds: Vec<bool>,
    door_cells: Vec<usize>,
    blocked: usize,
    mark: Vec<u32>,
    vis: Vec<u32>,
    stamp: u32,
    queue: Vec<usize>,
    recs: Vec<Rec<'a>>,
    /// Record indices of the group being placed: their clear zones may be used by each other.
    members: Vec<usize>,
    /// Group id per instance, in creation order.
    group_ids: Vec<String>,
    rng: Rng,
    /// The sides that have a wall (all four for a closed room; none when open).
    walls: Vec<Door>,
    /// Robot spots must lie inside [sx0, sx1] x [sz0, sz1]: inset 0.15 from each wall that exists.
    sx0: f64,
    sx1: f64,
    sz0: f64,
    sz1: f64,
}

impl Ctx<'_> {
    fn cell_at(&self, i: i64, j: i64) -> usize {
        (j * self.cols + i) as usize
    }
}

/// The clear zone of an item: (front, side), both 0 when absent.
fn clear_of(cat: &CatItem) -> (f64, f64) {
    match cat.clear {
        Some(c) => (c.front, c.side),
        None => (0.0, 0.0),
    }
}

fn make_geo(ctx: &Ctx, cat: &CatItem, x: f64, z: f64, rot: f64) -> Geo {
    let kind = kind_of(cat);
    let fw = cat.size[0];
    let fd = if kind == Kind::Wall { WALLMOUNT_DEPTH } else { cat.size[2] };
    let along = rot % 180.0 == 0.0;
    let sx = if along { fw } else { fd };
    let sz = if along { fd } else { fw };
    let x0 = x - sx / 2.0;
    let x1 = x + sx / 2.0;
    let z0 = z - sz / 2.0;
    let z1 = z + sz / 2.0;
    let (i0, i1) = idx_range(x0, x1, ctx.w / 2.0, ctx.cell, ctx.cols);
    let (j0, j1) = idx_range(z0, z1, ctx.d / 2.0, ctx.cell, ctx.rows);
    let mut g = Geo {
        x,
        z,
        rot,
        fw,
        fd,
        x0,
        x1,
        z0,
        z1,
        i0,
        i1,
        j0,
        j1,
        has_clear: false,
        ci0: 0,
        ci1: -1,
        cj0: 0,
        cj1: -1,
    };
    let (front, side) = clear_of(cat);
    if front > 0.0 && kind != Kind::Floor && kind != Kind::Surface {
        let lx = fw / 2.0 + side;
        let pts = [
            rot_pt(-lx, fd / 2.0, rot),
            rot_pt(lx, fd / 2.0, rot),
            rot_pt(lx, fd / 2.0 + front, rot),
            rot_pt(-lx, fd / 2.0 + front, rot),
        ];
        let (mnx, mxx, mnz, mxz) = extent(&pts);
        let (ci0, ci1) = idx_range(x + mnx, x + mxx, ctx.w / 2.0, ctx.cell, ctx.cols);
        let (cj0, cj1) = idx_range(z + mnz, z + mxz, ctx.d / 2.0, ctx.cell, ctx.rows);
        g.has_clear = true;
        g.ci0 = ci0;
        g.ci1 = ci1;
        g.cj0 = cj0;
        g.cj1 = cj1;
    }
    g
}

/// The cell containing a point, clamped to the grid (`cellIndexOf`).
fn cell_index_of(ctx: &Ctx, x: f64, z: f64) -> usize {
    let i = (((x + ctx.w / 2.0) / ctx.cell).floor() as i64).clamp(0, ctx.cols - 1);
    let j = (((z + ctx.d / 2.0) / ctx.cell).floor() as i64).clamp(0, ctx.rows - 1);
    ctx.cell_at(i, j)
}

/// The cell containing a point, or -1 when the point is outside the grid (`exactCell`: no clamping).
fn exact_cell(ctx: &Ctx, x: f64, z: f64) -> i64 {
    let i = ((x + ctx.w / 2.0) / ctx.cell).floor();
    let j = ((z + ctx.d / 2.0) / ctx.cell).floor();
    if i < 0.0 || i >= ctx.cols as f64 || j < 0.0 || j >= ctx.rows as f64 {
        return -1;
    }
    ctx.cell_at(i as i64, j as i64) as i64
}

/// The view strip in front of a wall-mounted item (`viewOf`).
fn view_of(g: &Geo, rot: f64) -> Bx {
    let lw = g.fw / 2.0 + MOUNT_VIEW_SIDE;
    let d0 = g.fd / 2.0;
    let d1 = g.fd / 2.0 + MOUNT_VIEW;
    let pts = [rot_pt(-lw, d0, rot), rot_pt(lw, d0, rot), rot_pt(lw, d1, rot), rot_pt(-lw, d1, rot)];
    let (mnx, mxx, mnz, mxz) = extent(&pts);
    Bx { x0: g.x + mnx, x1: g.x + mxx, z0: g.z + mnz, z1: g.z + mxz }
}

fn coplanar_clash(ctx: &Ctx, top: f64, g: &Geo) -> bool {
    ctx.recs.iter().any(|r| r.kind != Kind::Floor && (r.top - top).abs() <= COPLANAR_TOL && overlaps(g.bx(), r.g.bx()))
}

/// Build a record (`makeRec`): the spots transformed, the check cells, the view strip of a wall item.
fn make_rec<'a>(
    ctx: &Ctx,
    id: &str,
    cat: &'a CatItem,
    g: Geo,
    kind: Kind,
    host: Option<usize>,
    y: f64,
    top: f64,
    gi: usize,
) -> Rec<'a> {
    let mut spots = Vec::with_capacity(cat.spots.len());
    let mut spot_clips = Vec::with_capacity(cat.spots.len());
    for s in &cat.spots {
        let (dx, dz) = rot_pt(s.at[0], s.at[1], g.rot);
        let x = g.x + dx;
        let z = g.z + dz;
        let face = (((s.face + g.rot) % 360.0) + 360.0) % 360.0;
        spots.push(Spot { x, z, face, cell: exact_cell(ctx, x, z) });
        spot_clips.push(s.clip.clone());
    }
    let mut checks = Vec::new();
    if kind == Kind::Solid || kind == Kind::Wall {
        for s in &spots {
            checks.push(s.cell);
        }
        if cat.walk {
            checks.push(cell_index_of(ctx, g.x, g.z) as i64);
        } else {
            let (front, _) = clear_of(cat);
            let (dx, dz) = rot_pt(0.0, g.fd / 2.0 + front.max(CLEAR_MIN_ACCESS) / 2.0, g.rot);
            checks.push(cell_index_of(ctx, g.x + dx, g.z + dz) as i64);
        }
    }
    let view = if kind == Kind::Wall { Some(view_of(&g, g.rot)) } else { None };
    let limit = if kind == Kind::Wall { cat.mount.unwrap_or(0.0) + cat.size[1] / 2.0 } else { 0.0 };
    Rec {
        n: ctx.recs.len(),
        id: id.to_string(),
        kind,
        cat,
        g,
        y,
        top,
        host,
        gi,
        spots,
        spot_clips,
        checks,
        view,
        limit,
    }
}

/// Every spot inside the robot rectangle, inside the grid, and not on a BLOCKING cell (`spotsValid`). The
/// candidate's own footprint counts as blocking unless the candidate is a walk item.
fn spots_valid(ctx: &Ctx, rec: &Rec, cand_blocks: bool) -> bool {
    let g = &rec.g;
    for s in &rec.spots {
        if s.x < ctx.sx0 || s.x > ctx.sx1 || s.z < ctx.sz0 || s.z > ctx.sz1 {
            return false;
        }
        if s.cell < 0 {
            return false;
        }
        let c = s.cell as usize;
        if ctx.blk[c] > 0 {
            return false;
        }
        if cand_blocks {
            let i = (c as i64) % ctx.cols;
            let j = (c as i64 - i) / ctx.cols;
            if i >= g.i0 && i <= g.i1 && j >= g.j0 && j <= g.j1 {
                return false;
            }
        }
    }
    true
}

/// Every check cell reachable from the door zone over non-blocking cells, with the candidate's footprint as a
/// solid (`reachable`). A check outside the grid (-1) is never reached.
fn reachable(ctx: &mut Ctx, cand: &Rec, cand_blocks: bool) -> bool {
    ctx.stamp += 1;
    let s = ctx.stamp;
    let cols = ctx.cols;
    let rows = ctx.rows;
    let g = cand.g;
    if cand_blocks {
        for j in g.j0..=g.j1 {
            for i in g.i0..=g.i1 {
                let c = (j * cols + i) as usize;
                ctx.mark[c] = s;
            }
        }
    }
    let Ctx { blk, mark, vis, queue, door_cells, recs, .. } = ctx;
    let blocked_at = |c: usize| blk[c] > 0 || (cand_blocks && mark[c] == s);
    let mut qh = 0;
    queue.clear();
    for &c in door_cells.iter() {
        if blocked_at(c) || vis[c] == s {
            continue;
        }
        vis[c] = s;
        queue.push(c);
    }
    while qh < queue.len() {
        let c = queue[qh];
        qh += 1;
        let ci = c as i64 % cols;
        let cj = (c as i64 - ci) / cols;
        let visit = |n: usize, vis: &mut Vec<u32>, queue: &mut Vec<usize>| {
            if vis[n] != s && !blocked_at(n) {
                vis[n] = s;
                queue.push(n);
            }
        };
        if ci > 0 {
            visit(c - 1, vis, queue);
        }
        if ci < cols - 1 {
            visit(c + 1, vis, queue);
        }
        if cj > 0 {
            visit(c - cols as usize, vis, queue);
        }
        if cj < rows - 1 {
            visit(c + cols as usize, vis, queue);
        }
    }
    let seen = |c: i64| c >= 0 && vis[c as usize] == s;
    for r in recs.iter() {
        if r.kind != Kind::Solid && r.kind != Kind::Wall {
            continue;
        }
        if !r.checks.iter().all(|&c| seen(c)) {
            return false;
        }
    }
    cand.checks.iter().all(|&c| seen(c))
}

/// Try one placement (no random draws). Returns the record, not yet committed. `ban`: the item may not stand in
/// the middle region (non-centre groups); floor items are exempt (`tryPlace`).
fn try_place<'a>(
    ctx: &mut Ctx<'a>,
    id: &str,
    cat: &'a CatItem,
    g: &Geo,
    host: Option<usize>,
    gi: usize,
    ban: bool,
) -> Option<Rec<'a>> {
    let kind = kind_of(cat);
    if kind == Kind::Surface {
        let hi = host?;
        let (htop, hg) = {
            let h = &ctx.recs[hi];
            (h.top, h.g)
        };
        if !(g.x0 >= hg.x0 && g.x1 <= hg.x1 && g.z0 >= hg.z0 && g.z1 <= hg.z1) {
            return None;
        }
        let top = htop + cat.size[1];
        if coplanar_clash(ctx, top, g) {
            return None;
        }
        let srec = make_rec(ctx, id, cat, *g, kind, Some(hi), htop, top, gi);
        return if spots_valid(ctx, &srec, false) { Some(srec) } else { None };
    }
    if !(g.x0 >= -ctx.w / 2.0 && g.x1 <= ctx.w / 2.0 && g.z0 >= -ctx.d / 2.0 && g.z1 <= ctx.d / 2.0) {
        return None;
    }
    if ban && kind != Kind::Floor && g.x.abs() <= ctx.w / 4.0 && g.z.abs() <= ctx.d / 4.0 {
        return None;
    }
    for j in g.j0..=g.j1 {
        for i in g.i0..=g.i1 {
            if ctx.ds[ctx.cell_at(i, j)] {
                return None;
            }
        }
    }
    let y = if kind == Kind::Wall { cat.mount.unwrap_or(0.0) } else { 0.0 };
    let top = y + cat.size[1];
    if kind == Kind::Floor {
        let frec = make_rec(ctx, id, cat, *g, kind, host, y, top, gi);
        return if spots_valid(ctx, &frec, false) { Some(frec) } else { None };
    }
    if kind == Kind::Solid {
        for j in g.j0..=g.j1 {
            for i in g.i0..=g.i1 {
                let c = ctx.cell_at(i, j);
                // Cell occupancy is coarse: a group member's footprint is tested exactly (below), so it may
                // share a cell with its own anchor. The anchor itself is tested against every occupied cell.
                if ctx.members.is_empty() && ctx.occ[c] > 0 {
                    return None;
                }
                // A reserved cell may be used only by a clear zone of this group's own items.
                let r = ctx.res[c];
                if r != -1 && !(r >= 0 && ctx.members.contains(&(r as usize))) {
                    return None;
                }
            }
        }
        for r in &ctx.recs {
            if r.kind == Kind::Solid && overlaps(g.bx(), r.g.bx()) {
                return None;
            }
        }
        // A tall solid may not stand in the view strip of a wall-mounted item.
        for r in &ctx.recs {
            if let (Kind::Wall, Some(v)) = (r.kind, r.view) {
                if overlaps(v, g.bx()) && top > r.limit {
                    return None;
                }
            }
        }
    }
    if g.has_clear {
        for j in g.cj0..=g.cj1 {
            for i in g.ci0..=g.ci1 {
                if ctx.occ[ctx.cell_at(i, j)] > 0 {
                    return None;
                }
            }
        }
    }
    if coplanar_clash(ctx, top, g) {
        return None;
    }
    let rec = make_rec(ctx, id, cat, *g, kind, host, y, top, gi);
    if kind == Kind::Wall {
        // A wall-mounted item may not sit in front of a tall solid.
        if let Some(view) = rec.view {
            for r in &ctx.recs {
                if r.kind == Kind::Solid && overlaps(view, r.g.bx()) && r.top > rec.limit {
                    return None;
                }
            }
        }
    }
    if !spots_valid(ctx, &rec, kind == Kind::Solid && !cat.walk) {
        return None;
    }
    if (kind == Kind::Solid || kind == Kind::Wall) && !reachable(ctx, &rec, kind == Kind::Solid && !cat.walk) {
        return None;
    }
    Some(rec)
}

/// Record a placed item in the grid: occupancy, blocking and clear-zone reservations (`commit`).
fn commit<'a>(ctx: &mut Ctx<'a>, rec: Rec<'a>) {
    let g = rec.g;
    if rec.kind == Kind::Solid {
        let walk = rec.cat.walk;
        for j in g.j0..=g.j1 {
            for i in g.i0..=g.i1 {
                let c = ctx.cell_at(i, j);
                if ctx.occ[c] < 255 {
                    ctx.occ[c] += 1;
                }
                if !walk {
                    if ctx.blk[c] == 0 {
                        ctx.blocked += 1;
                    }
                    ctx.blk[c] = ctx.blk[c].wrapping_add(1);
                }
            }
        }
    }
    if g.has_clear {
        for j in g.cj0..=g.cj1 {
            for i in g.ci0..=g.ci1 {
                let c = ctx.cell_at(i, j);
                let r = ctx.res[c];
                ctx.res[c] = if r == -1 {
                    rec.n as i64
                } else if r == rec.n as i64 {
                    r
                } else {
                    -3
                };
            }
        }
    }
    ctx.recs.push(rec);
}

/// The best spread of the same item id near a point (`spreadAt`).
fn spread_at(ctx: &Ctx, id: &str, x: f64, z: f64) -> f64 {
    let mut best = 0.0_f64;
    for r in &ctx.recs {
        if r.id != id {
            continue;
        }
        let dx = x - r.g.x;
        let dz = z - r.g.z;
        let v = 1.0 - (dx * dx + dz * dz).sqrt() / (0.5 * ctx.diag);
        let s = if v > 0.0 { v } else { 0.0 };
        if s > best {
            best = s;
        }
    }
    best
}

/// True when the lateral interval [lo, hi] lies beyond the door opening (on a stub), not over it (`beyondOpening`).
fn beyond_opening(door_half: f64, lo: f64, hi: f64) -> bool {
    lo >= door_half - EPS || hi <= -door_half + EPS
}

fn door_fit(near: f64) -> f64 {
    if near >= -EPS && near <= DOOR_NEAR {
        1.0
    } else {
        0.0
    }
}

/// Candidates for a group's anchor item, in enumeration order (the order is part of the algorithm)
/// (`anchorCandidates`). wall: sides n, e, s, w (not the door), cells along each; corner: nw, ne, se, sw (not the
/// door's corners); centre (and any other rule): cells in the middle half; door: cells along the door's own wall,
/// rule fit 1 when beside the door; wallmount: cells along each side but the door.
fn anchor_candidates(ctx: &Ctx, cat: &CatItem, rule: Anchor) -> Vec<Cand> {
    let mut out = Vec::new();
    let has_spots = !cat.spots.is_empty();
    let door_pt = match ctx.door {
        Door::N => (0.0, -ctx.d / 2.0),
        Door::E => (ctx.w / 2.0, 0.0),
        Door::S => (0.0, ctx.d / 2.0),
        Door::W => (-ctx.w / 2.0, 0.0),
    };
    let push = |out: &mut Vec<Cand>, x: f64, z: f64, rot: f64, rule_fit: f64| {
        let dx = x - door_pt.0;
        let dz = z - door_pt.1;
        let work_far = if has_spots { (dx * dx + dz * dz).sqrt() / ctx.diag } else { 0.0 };
        out.push(Cand { geo: make_geo(ctx, cat, x, z, rot), rule_fit, work_far });
    };
    let fw = cat.size[0];
    let fd0 = cat.size[2];
    match rule {
        Anchor::Wall | Anchor::Wallmount | Anchor::Door => {
            let fd_use = if rule == Anchor::Wallmount { WALLMOUNT_DEPTH } else { fd0 };
            for side in SIDES {
                if rule == Anchor::Door && side != ctx.door {
                    continue;
                }
                // A wall-anchored item needs its wall: no candidates on a side without one.
                if rule != Anchor::Door && !ctx.walls.contains(&side) {
                    continue;
                }
                // On the door side a wall-anchored item stands on a stub: its footprint lies beyond the opening's edge.
                let on_door_side = side == ctx.door && rule != Anchor::Door;
                let rot = side_rot(side);
                let along = rot % 180.0 == 0.0;
                let hx = (if along { fw } else { fd_use }) / 2.0;
                let hz = (if along { fd_use } else { fw }) / 2.0;
                let base = if rule == Anchor::Wall { 1.0 } else { 0.0 };
                if side == Door::N || side == Door::S {
                    let z = if side == Door::N { -ctx.d / 2.0 + hz + INSET } else { ctx.d / 2.0 - hz - INSET };
                    for i in 0..ctx.cols {
                        let x = ctx.xs[i as usize];
                        if on_door_side && !beyond_opening(ctx.door_half, x - hx, x + hx) {
                            continue;
                        }
                        let rf = if rule == Anchor::Door { door_fit(x.abs() - ctx.door_half - hx) } else { base };
                        // A door candidate must hug the opening (within DOOR_NEAR of its edge); others are not candidates.
                        if rule == Anchor::Door && rf == 0.0 {
                            continue;
                        }
                        push(&mut out, x, z, rot, rf);
                    }
                } else {
                    let x = if side == Door::W { -ctx.w / 2.0 + hx + INSET } else { ctx.w / 2.0 - hx - INSET };
                    for j in 0..ctx.rows {
                        let z = ctx.zs[j as usize];
                        if on_door_side && !beyond_opening(ctx.door_half, z - hz, z + hz) {
                            continue;
                        }
                        let rf = if rule == Anchor::Door { door_fit(z.abs() - ctx.door_half - hz) } else { base };
                        if rule == Anchor::Door && rf == 0.0 {
                            continue;
                        }
                        push(&mut out, x, z, rot, rf);
                    }
                }
            }
        }
        Anchor::Corner => {
            let corners = [(-1.0, -1.0, Door::N, Door::W), (1.0, -1.0, Door::N, Door::E), (1.0, 1.0, Door::S, Door::E), (-1.0, 1.0, Door::S, Door::W)];
            for (sx, sz, a, b) in corners {
                if a == ctx.door || b == ctx.door {
                    continue;
                }
                // The rotation comes from the centre computed with the rot-0 footprint; the centre then uses that rot.
                let rot = facing(sx * (ctx.w / 2.0 - INSET - fw / 2.0), sz * (ctx.d / 2.0 - INSET - fd0 / 2.0));
                let along = rot % 180.0 == 0.0;
                let hx = (if along { fw } else { fd0 }) / 2.0;
                let hz = (if along { fd0 } else { fw }) / 2.0;
                push(&mut out, sx * (ctx.w / 2.0 - INSET - hx), sz * (ctx.d / 2.0 - INSET - hz), rot, 1.0);
            }
        }
        // centre (and any other rule): row-major, the middle half only.
        _ => {
            for j in 0..ctx.rows {
                for i in 0..ctx.cols {
                    let x = ctx.xs[i as usize];
                    let z = ctx.zs[j as usize];
                    if !(x.abs() <= ctx.w / 4.0 && z.abs() <= ctx.d / 4.0) {
                        continue;
                    }
                    push(&mut out, x, z, facing(x, z), 0.0);
                }
            }
        }
    }
    out
}

/// The slots of one group instance, in order (`buildSlots`). Draws, once per group instance and before any
/// candidate: a count per `[lo, hi]` entry, then, for an `around` entry, a Fisher-Yates shuffle of the sides
/// n, e, s, w. Copy c of an entry is required when c <= lo and the entry is not optional; the anchor is always
/// required.
fn build_slots<'a>(ctx: &mut Ctx<'a>, gid: &str, station: Option<&str>) -> Vec<Slot<'a>> {
    let items = ctx.items;
    let groups = ctx.groups;
    let def = groups.get(gid).unwrap_or_else(|| panic!("rooms: unknown group \"{gid}\""));
    let mut slots: Vec<Slot<'a>> = Vec::new();
    for (idx, e) in def.items.iter().enumerate() {
        let id = if e.id == "$station" { station.unwrap_or("") } else { e.id.as_str() };
        let cat = match items.get(id) {
            Some(c) if !id.is_empty() => c,
            _ => panic!("rooms: group \"{gid}\" item \"{}\" unresolved", e.id),
        };
        let mut lo: i64 = 1;
        let mut n: i64 = 1;
        if let Some([l, h]) = e.count {
            lo = l;
            let r = ctx.rng.next_f64();
            n = lo + (r * ((h - lo + 1) as f64)).floor() as i64;
        }
        let mut sides = SIDES;
        if e.rel.as_deref() == Some("around") {
            for i in (1..=3_usize).rev() {
                let j = (ctx.rng.next_f64() * (i + 1) as f64).floor() as usize;
                sides.swap(i, j);
            }
        }
        for c in 1..=n {
            let is_anchor = idx == 0 && c == 1;
            let mut rel = e.rel.clone().unwrap_or_else(|| if idx == 0 { "row".to_string() } else { String::new() });
            if is_anchor {
                rel.clear();
            }
            let side = if rel == "around" { sides.get((c - 1) as usize).copied() } else { None };
            slots.push(Slot {
                id: id.to_string(),
                cat,
                gap: e.gap.unwrap_or(0.0),
                face: e.face.clone().unwrap_or_default(),
                required: is_anchor || (!e.optional && c <= lo),
                rel,
                side,
            });
        }
    }
    assert!(!slots.is_empty() && !slots[0].id.is_empty(), "rooms: group \"{gid}\" has no anchor");
    slots
}

/// The geometry of one non-anchor slot, from the anchor's placed record. Offsets are in the anchor's frame
/// (rot-0 sizes), then turned by the anchor's rot. `None` when the relation does not apply (`slotGeo`).
fn slot_geo(ctx: &Ctx, s: &Slot, a: &Rec, row: &mut RowState) -> Option<Geo> {
    let aw = a.cat.size[0];
    let ad = a.cat.size[2];
    let ahx = aw / 2.0;
    let ahz = ad / 2.0;
    let halves = |rr: f64| -> (f64, f64) {
        let hw = s.cat.size[0] / 2.0;
        let hd = s.cat.size[2] / 2.0;
        if rr % 180.0 == 0.0 {
            (hw, hd)
        } else {
            (hd, hw)
        }
    };
    let mut lx = 0.0;
    let mut lz = 0.0;
    let mut rr = 0.0;
    match s.rel.as_str() {
        "front" => {
            rr = if s.face == "anchor" { 180.0 } else { 0.0 };
            lz = ahz + s.gap + halves(rr).1;
        }
        "beside" => {
            lx = -(ahx + s.gap + halves(0.0).0);
        }
        "beside-other" => {
            lx = ahx + s.gap + halves(0.0).0;
        }
        "row" => {
            if row.stopped {
                return None;
            }
            let hx = halves(0.0).0;
            lx = row.right + s.gap + hx;
            row.right = lx + hx;
        }
        "on" => {
            lz = -ahz + halves(0.0).1 + ON_SURFACE_OFFSET;
        }
        "around" => {
            let side = s.side?;
            rr = side_rot(side);
            let (hx, hz) = halves(rr);
            match side {
                Door::N => lz = -(ahz + s.gap + hz),
                Door::S => lz = ahz + s.gap + hz,
                Door::E => lx = ahx + s.gap + hx,
                Door::W => lx = -(ahx + s.gap + hx),
            }
        }
        "mirror" => {
            // Reflect the anchor's centre across the door's axis, same rot.
            let mx = if ctx.door == Door::N || ctx.door == Door::S { -a.g.x } else { a.g.x };
            let mz = if ctx.door == Door::E || ctx.door == Door::W { -a.g.z } else { a.g.z };
            return Some(make_geo(ctx, s.cat, mx, mz, a.g.rot));
        }
        _ => return None,
    }
    let (dx, dz) = rot_pt(lx, lz, a.g.rot);
    Some(make_geo(ctx, s.cat, a.g.x + dx, a.g.z + dz, ((a.g.rot + rr) % 360.0 + 360.0) % 360.0))
}

/// One whole group at one anchor placement: the anchor, then each later slot in order (`attemptGroup`). The
/// state is restored on any required failure, so an attempt leaves nothing behind when it fails.
fn attempt_group<'a>(ctx: &mut Ctx<'a>, gi: usize, anchor: &Geo, slots: &[Slot<'a>], rule: Anchor) -> bool {
    let occ = ctx.occ.clone();
    let blk = ctx.blk.clone();
    let res = ctx.res.clone();
    let blocked = ctx.blocked;
    let nrec = ctx.recs.len();
    let ok = place_slots(ctx, gi, anchor, slots, rule);
    if !ok {
        ctx.occ = occ;
        ctx.blk = blk;
        ctx.res = res;
        ctx.blocked = blocked;
        ctx.recs.truncate(nrec);
    }
    ctx.members.clear();
    ok
}

fn place_slots<'a>(ctx: &mut Ctx<'a>, gi: usize, anchor: &Geo, slots: &[Slot<'a>], rule: Anchor) -> bool {
    let ban = rule != Anchor::Center;
    ctx.members.clear();
    let first = &slots[0];
    let Some(ar) = try_place(ctx, &first.id, first.cat, anchor, None, gi, ban) else {
        return false;
    };
    let an = ar.n;
    commit(ctx, ar);
    ctx.members.push(an);
    let mut row = RowState { right: first.cat.size[0] / 2.0, stopped: false };
    for s in &slots[1..] {
        let geo = {
            let a = &ctx.recs[an];
            slot_geo(ctx, s, a, &mut row)
        };
        let host = if s.rel == "on" { Some(an) } else { None };
        let rec = geo.and_then(|g| try_place(ctx, &s.id, s.cat, &g, host, gi, ban));
        match rec {
            None => {
                if s.rel == "row" {
                    row.stopped = true;
                }
                if s.required {
                    return false;
                }
            }
            Some(r) => {
                let n = r.n;
                commit(ctx, r);
                ctx.members.push(n);
            }
        }
    }
    ctx.members.clear();
    true
}

/// The anchor rules of a group for a room, in preference order. Desk-sets of `"room"` roles prefer a corner (the
/// user faces into the room) and fall back to a wall; every other group has its catalogue rule (`anchorRulesOf`).
fn anchor_rules_of(ctx: &Ctx, gid: &str, desk_facing: &str) -> Vec<Anchor> {
    if gid == "desk-set" && desk_facing == "room" {
        return vec![Anchor::Corner, Anchor::Wall];
    }
    let def = ctx.groups.get(gid).unwrap_or_else(|| panic!("rooms: unknown group \"{gid}\""));
    vec![def.anchor]
}

/// Place one group instance (`placeGroup`): the anchor candidates of each rule (the first rule's candidates
/// get a +0.5 preference), scored, tried in score order; the first whole success is kept. Returns true when placed.
fn place_group<'a>(ctx: &mut Ctx<'a>, gid: &str, station: Option<&str>, rules: &[Anchor]) -> bool {
    let gi = ctx.group_ids.len();
    let slots = build_slots(ctx, gid, station);
    let mut cands: Vec<Cand> = Vec::new();
    for (ri, &rule) in rules.iter().enumerate() {
        for mut c in anchor_candidates(ctx, slots[0].cat, rule) {
            if ri == 0 {
                c.rule_fit += 0.5;
            }
            cands.push(c);
        }
    }
    let rule = rules[0];
    let mut score = vec![0.0_f64; cands.len()];
    for (k, c) in cands.iter().enumerate() {
        let sp = spread_at(ctx, &slots[0].id, c.geo.x, c.geo.z);
        score[k] = c.rule_fit + 0.5 * c.work_far - 0.3 * sp + 0.05 * ctx.rng.next_f64();
    }
    let mut order: Vec<usize> = (0..cands.len()).collect();
    order.sort_by(|&a, &b| score[b].partial_cmp(&score[a]).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b)));
    for k in order {
        let geo = cands[k].geo;
        if attempt_group(ctx, gi, &geo, &slots, rule) {
            ctx.group_ids.push(gid.to_string());
            return true;
        }
    }
    false
}

/// Place a template entry's group (`placeTop`): its rules come from the room's desk facing.
fn place_ref(ctx: &mut Ctx, group: &str, station: Option<&str>, desk_facing: &str) -> bool {
    let rules = anchor_rules_of(ctx, group, desk_facing);
    place_group(ctx, group, station, &rules)
}

/// The desk count of a room when the input names none: `clamp(floor(w * d / 6), 1, 4)`.
fn default_agents(w: f64, d: f64) -> usize {
    ((w * d / 6.0).floor() as i64).clamp(1, 4) as usize
}

/// Generate the room layout for one input (`generate`). Pure: the same input and data give the same output.
pub fn generate(input: &Input, data: &Data) -> Layout {
    let tpl = template_for(&input.role, &data.roles)
        .unwrap_or_else(|| panic!("rooms: no template for role \"{}\" and no \"other\" family", input.role));
    let rng = Rng::new(input.seed);
    let cell = data.catalog.cell.unwrap_or(CELL_DEFAULT);
    let w = input.w;
    let d = input.d;
    let door = input.door;
    let cols = (w / cell).floor() as i64;
    let rows = (d / cell).floor() as i64;
    let xs: Vec<f64> = (0..cols).map(|i| -w / 2.0 + (i as f64 + 0.5) * cell).collect();
    let zs: Vec<f64> = (0..rows).map(|j| -d / 2.0 + (j as f64 + 0.5) * cell).collect();
    let n_cells = (cols * rows) as usize;
    let mut ctx = Ctx {
        w,
        d,
        cell,
        cols,
        rows,
        door,
        door_half: 0.0,
        diag: (w * w + d * d).sqrt(),
        items: &data.catalog.items,
        groups: &data.catalog.groups,
        xs,
        zs,
        occ: vec![0; n_cells],
        blk: vec![0; n_cells],
        res: vec![-1; n_cells],
        ds: vec![false; n_cells],
        door_cells: Vec::new(),
        blocked: 0,
        mark: vec![0; n_cells],
        vis: vec![0; n_cells],
        stamp: 0,
        queue: Vec::with_capacity(n_cells),
        recs: Vec::new(),
        members: Vec::new(),
        group_ids: Vec::new(),
        rng,
        walls: Vec::new(),
        sx0: 0.0,
        sx1: 0.0,
        sz0: 0.0,
        sz1: 0.0,
    };

    // Shell draws, in order: floor, wall, open; then the density draw.
    let floor = pick_str(&tpl.shell.floor, &mut ctx.rng);
    let wall = pick_str(&tpl.shell.wall, &mut ctx.rng);
    let open_room = ctx.rng.next_f64() < tpl.shell.open;
    // Every closed room has a wall on all four sides (the door side has stubs around the opening, A0).
    let walls: Vec<Door> = if open_room { Vec::new() } else { SIDES.to_vec() };
    ctx.walls = walls.clone();
    // Robot spots stay 0.15 off each wall that exists (the room edge where there is none).
    ctx.sx0 = if walls.contains(&Door::W) { -w / 2.0 + 0.15 } else { -w / 2.0 };
    ctx.sx1 = if walls.contains(&Door::E) { w / 2.0 - 0.15 } else { w / 2.0 };
    ctx.sz0 = if walls.contains(&Door::N) { -d / 2.0 + 0.15 } else { -d / 2.0 };
    ctx.sz1 = if walls.contains(&Door::S) { d / 2.0 - 0.15 } else { d / 2.0 };
    let (d_lo, d_hi) = (tpl.density[0], tpl.density[1]);
    let density = d_lo + ctx.rng.next_f64() * (d_hi - d_lo);

    // Door zone (1.0 deep, centred, at most side - 0.5 wide) and the spine from the door to the centre.
    let side_len = if door == Door::N || door == Door::S { w } else { d };
    let door_width = 1.0_f64.min(side_len - 0.5);
    let half_width = door_width / 2.0;
    ctx.door_half = half_width;
    for j in 0..rows {
        for i in 0..cols {
            let x = ctx.xs[i as usize];
            let z = ctx.zs[j as usize];
            let c = ctx.cell_at(i, j);
            let (depth, lateral, toward_centre) = match door {
                Door::N => (z + d / 2.0, x, z <= 0.0),
                Door::S => (d / 2.0 - z, x, z >= 0.0),
                Door::E => (w / 2.0 - x, z, x >= 0.0),
                Door::W => (x + w / 2.0, z, x <= 0.0),
            };
            if depth <= 1.0 && lateral.abs() <= half_width {
                ctx.ds[c] = true;
                ctx.door_cells.push(c);
            } else if lateral.abs() <= 0.25 && toward_centre {
                ctx.ds[c] = true;
            }
        }
    }

    // Required groups first (the station, then the per-agent desk-sets), then preferred (skipped, never dropped),
    // then the pool. A per-agent entry repeats `agents` times (default cap(w, d)).
    let desk_facing = tpl.desk_facing.clone().unwrap_or_else(|| "wall".to_string());
    let agents = input.agents.unwrap_or_else(|| default_agents(w, d));
    let mut dropped: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut required_ordered: Vec<&RoleGroupRef> = tpl.required.iter().filter(|e| e.per.as_deref() != Some("agent")).collect();
    required_ordered.extend(tpl.required.iter().filter(|e| e.per.as_deref() == Some("agent")));
    for e in required_ordered {
        let count = if e.per.as_deref() == Some("agent") { agents } else { 1 };
        for _ in 0..count {
            if !place_ref(&mut ctx, &e.group, e.station.as_deref(), &desk_facing) {
                dropped.push(e.group.clone());
            }
        }
    }
    for e in &tpl.preferred {
        let count = if e.per.as_deref() == Some("agent") { agents } else { 1 };
        for _ in 0..count {
            if !place_ref(&mut ctx, &e.group, e.station.as_deref(), &desk_facing) {
                skipped.push(e.group.clone());
            }
        }
    }

    // Pool: weighted picks until the density is reached or 20 tries fail in a row.
    let pool = &tpl.pool;
    let mut counts = vec![0_usize; pool.len()];
    let weight_of = |k: usize| pool[k].w.unwrap_or(1.0);
    let max_of = |k: usize| pool[k].max.unwrap_or(f64::INFINITY);
    let eligible = |k: usize, counts: &[usize]| (counts[k] as f64) < max_of(k);
    let mut fails = 0;
    while fails < MAX_FAILED_TRIES {
        if ctx.blocked as f64 / n_cells as f64 >= density {
            break;
        }
        let mut total = 0.0;
        for k in 0..pool.len() {
            if eligible(k, &counts) {
                total += weight_of(k);
            }
        }
        if !(total > 0.0) {
            break;
        }
        let u = ctx.rng.next_f64() * total;
        let mut acc = 0.0;
        let mut chosen: Option<usize> = None;
        for k in 0..pool.len() {
            if !eligible(k, &counts) {
                continue;
            }
            acc += weight_of(k);
            if acc > u {
                chosen = Some(k);
                break;
            }
        }
        if chosen.is_none() {
            chosen = (0..pool.len()).rev().find(|&k| eligible(k, &counts));
        }
        let k = chosen.expect("a pool entry is eligible when the total weight is positive");
        if place_ref(&mut ctx, &pool[k].group, None, &desk_facing) {
            counts[k] += 1;
            fails = 0;
        } else {
            fails += 1;
        }
    }

    // Output: rounded, items in placement order, spots transformed.
    let mut items = Vec::with_capacity(ctx.recs.len());
    let mut spots = Vec::new();
    for r in &ctx.recs {
        items.push(LayoutItem {
            n: r.n,
            id: r.id.clone(),
            x: round_num(r.g.x),
            y: round_num(r.y),
            z: round_num(r.g.z),
            rot: r.g.rot,
            host: r.host,
            g: r.gi,
        });
        for (s, clip) in r.spots.iter().zip(&r.spot_clips) {
            spots.push(LayoutSpot { item: r.n, x: round_num(s.x), z: round_num(s.z), face: s.face, clip: clip.clone() });
        }
    }
    Layout {
        shell: LayoutShell {
            floor,
            wall,
            walls,
            door: LayoutDoor { side: door, at: 0.0, width: round_num(door_width) },
        },
        items,
        spots,
        groups: ctx.group_ids.clone(),
        dropped,
        skipped,
    }
}

// ---- the golden fixtures (tests) -----------------------------------------------------------------

/// One golden case of `assets/rooms/fixtures.json`: the input, the web's layout hash, and the counts of skipped
/// and dropped items (the fixture file keeps counts, not the lists).
#[cfg(test)]
pub struct FixtureCase {
    pub input: Input,
    pub hash: String,
    pub skipped: usize,
    pub dropped: usize,
}

/// The shared golden set, read from the repository's `assets/rooms/fixtures.json`.
#[cfg(test)]
pub fn fixture_cases() -> Vec<FixtureCase> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/rooms/fixtures.json");
    let text = std::fs::read_to_string(path).expect("assets/rooms/fixtures.json is readable");
    let doc: Value = serde_json::from_str(&text).expect("fixtures.json parses");
    let cases = doc["cases"].as_array().expect("fixtures.json has a cases array");
    cases
        .iter()
        .map(|c| {
            let count = |key: &str| c[key].as_u64().unwrap_or_else(|| panic!("fixture field {key}")) as usize;
            FixtureCase {
                input: Input {
                    role: c["role"].as_str().expect("fixture role").to_string(),
                    w: c["w"].as_f64().expect("fixture w"),
                    d: c["d"].as_f64().expect("fixture d"),
                    door: Door::parse(c["door"].as_str().expect("fixture door")).expect("a door side"),
                    seed: c["seed"].as_u64().expect("fixture seed") as u32,
                    agents: Some(count("agents")),
                },
                hash: c["hash"].as_str().expect("fixture hash").to_string(),
                skipped: count("skipped"),
                dropped: count("dropped"),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_role_order_is_the_files_and_names_every_role() {
        let order = role_order();
        let first = ROLES_JSON.find("\"roles\"").map(|at| &ROLES_JSON[at..]).and_then(|r| r.split('"').nth(3)).unwrap();
        assert_eq!(order[0], first, "the first role is the file's first");
        let mut sorted = order.to_vec();
        sorted.sort();
        assert_eq!(sorted, data().roles.roles.keys().cloned().collect::<Vec<_>>());
    }

    /// The golden set: every case's layout hash equals the one the web's generator (gen.ts, retired with three.js) wrote,
    /// and its skipped and dropped counts too. A mismatch names its case: a change to this generator that moves a hash
    /// is a change to every room's furniture, made on purpose or not at all.
    #[test]
    fn fixtures_match_the_web_layout_hashes() {
        let cases = fixture_cases();
        assert_eq!(cases.len(), 200, "the golden set has 200 cases");
        let d = data();
        let mut bad = Vec::new();
        for (k, c) in cases.iter().enumerate() {
            let layout = generate(&c.input, d);
            let got = format!("{:08x}", layout_hash(&layout));
            if got != c.hash || layout.skipped.len() != c.skipped || layout.dropped.len() != c.dropped {
                bad.push(format!(
                    "case {k}: {} w={} d={} door={} seed={} agents={:?}: rust {got} golden {} (skipped {} vs {}, dropped {} vs {})",
                    c.input.role,
                    c.input.w,
                    c.input.d,
                    c.input.door.as_str(),
                    c.input.seed,
                    c.input.agents,
                    c.hash,
                    layout.skipped.len(),
                    c.skipped,
                    layout.dropped.len(),
                    c.dropped,
                ));
            }
        }
        assert!(
            bad.is_empty(),
            "{} of {} golden cases differ from the generator: every room's furniture moved. Only an intended change to \
             the generator may update assets/rooms/fixtures.json. First cases:\n{}",
            bad.len(),
            cases.len(),
            bad[..bad.len().min(5)].join("\n")
        );
        eprintln!("fixtures: {} cases match the web", cases.len());
    }

    /// Every row of the minSize table holds six counts, and the table covers the templates' roles.
    #[test]
    fn minsize_table_covers_known_roles_and_six_counts() {
        let d = data();
        let table = min_size_table();
        let mut want: Vec<&str> = d.roles.roles.keys().map(String::as_str).collect();
        assert!(d.roles.families.contains_key("other"), "the family fallback row");
        want.push("other");
        let mut have: Vec<&str> = table.keys().map(String::as_str).collect();
        want.sort_unstable();
        have.sort_unstable();
        assert_eq!(have, want, "one row per role, plus the family fallback `other`");
        for (role, row) in table {
            assert_eq!(row.len(), 6, "{role}: six desk counts");
        }
    }

    /// `min_size` returns the table's value for every role and count the table holds.
    #[test]
    fn min_size_returns_every_table_entry() {
        let d = data();
        for (role, row) in min_size_table() {
            for (i, v) in row.iter().enumerate() {
                let got = min_size(role, i + 1, d);
                match v {
                    Some(s) => assert_eq!(got, *s, "{role} agents {}", i + 1),
                    None => assert!(got.is_infinite(), "{role} agents {}", i + 1),
                }
            }
        }
    }

    /// The minSize table against a fresh computation (`computeMinSize`, every seed 1..=500 and each door). Slow:
    /// run with `cargo test -p agentplaza-desktop rooms -- --ignored computed_min_size`. It names every row that differs.
    #[test]
    #[ignore = "slow (minutes): computes minSize from scratch for 17 roles x 6 desk counts"]
    fn computed_min_size_equals_the_table() {
        let d = data();
        let mut differ = Vec::new();
        for (role, row) in min_size_table() {
            for (i, v) in row.iter().enumerate() {
                let want = v.unwrap_or(f64::INFINITY);
                let got = compute_min_size(role, i + 1, d);
                if got != want {
                    differ.push(format!("{role} agents {}: table {want} computed {got}", i + 1));
                }
            }
        }
        assert!(differ.is_empty(), "the minsize table differs from the computed minSize:\n{}", differ.join("\n"));
    }

    #[test]
    fn fnv1a_matches_the_published_vectors() {
        assert_eq!(fnv1a32(b""), 0x811c_9dc5);
        assert_eq!(fnv1a32(b"a"), 0xe40c_292c);
        assert_eq!(fnv1a32(b"foobar"), 0xbf9c_f968);
    }

    #[test]
    fn mulberry32_matches_the_reference_vectors() {
        // Computed with the reference JS (`node`), the same expression the web runs.
        let check = |seed: u32, want: &[f64]| {
            let mut r = Rng::new(seed);
            for w in want {
                let g = r.next_f64();
                assert!((g - w).abs() < 5e-11, "seed {seed:#x}: {g} vs {w}");
            }
        };
        check(0, &[0.2664292087, 0.0003297457, 0.2232720274, 0.1462021479]);
        check(0xDEAD_BEEF, &[0.9413696141, 0.2671957498, 0.7720333575]);
        // The seed at the top of the u32 range: the first add wraps past 2^32.
        check(u32::MAX, &[0.8964226141, 0.1894782567]);
    }

    #[test]
    fn rounding_matches_js_math_round() {
        assert_eq!(round_num(-0.0), 0.0);
        assert_eq!(round_num(0.0005), 0.001); // Math.round(0.5) = 1
        assert_eq!(round_num(-0.0005), 0.0); // Math.round(-0.5) = -0, then mapped to 0
        assert_eq!(round_num(-0.0015), -0.001); // Math.round(-1.5) = -1
        assert_eq!(round_num(1.23456), 1.235);
    }

    #[test]
    fn canonical_numbers_have_no_trailing_zeros_and_no_negative_zero() {
        assert_eq!(num(-0.0), "0");
        assert_eq!(num(1.5), "1.5");
        assert_eq!(num(2.0), "2");
        assert_eq!(num(-0.125), "-0.125");
    }

    /// The canonical form is generic: keys are sorted at every depth, and an unknown key changes the hash.
    #[test]
    fn canonical_json_is_generic_over_keys() {
        let v: Value = serde_json::json!({ "b": [1.0, -0.0, 2.25], "a": { "z": null, "y": "s" }, "g": 0 });
        assert_eq!(canonical_value(&v), r#"{"a":{"y":"s","z":null},"b":[1,0,2.25],"g":0}"#);
        let mut more = v.clone();
        more["extra"] = serde_json::json!(3);
        assert_ne!(canonical_value(&v), canonical_value(&more));
    }

    #[test]
    fn smoke_every_role_generates_at_a_typical_size() {
        let d = data();
        let mut drops = Vec::new();
        for role in d.roles.roles.keys() {
            for seed in 1..=3 {
                let input = Input { role: role.clone(), w: 4.0, d: 3.5, door: Door::S, seed, agents: None };
                let l = generate(&input, d);
                if !l.dropped.is_empty() {
                    drops.push(format!("{role} seed {seed}: {:?}", l.dropped));
                }
                assert!(!l.items.is_empty(), "{role} seed {seed}");
                assert!(l.items.iter().all(|it| it.g < l.groups.len()), "{role} seed {seed}: g indexes groups");
            }
        }
        eprintln!("drops: {drops:?}");
    }

    /// Parity with the web: `APZ_ROOM_DUMP=<file>` names a JSON-lines file (one object per line) with `role, w, d,
    /// door, seed, agents?, hash, dropped, skipped, items` produced by the web's `gen.ts`. Run with `--ignored`.
    #[test]
    #[ignore = "needs APZ_ROOM_DUMP=<web dump>; the shipped check is fixtures.json"]
    fn matches_the_web_dump() {
        let path = std::env::var("APZ_ROOM_DUMP").expect("APZ_ROOM_DUMP names the web dump");
        let text = std::fs::read_to_string(path).expect("the dump is readable");
        let d = data();
        let mut bad = Vec::new();
        let mut total = 0;
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let r: Value = serde_json::from_str(line).expect("a dump line parses");
            total += 1;
            let input = Input {
                role: r["role"].as_str().unwrap().to_string(),
                w: r["w"].as_f64().unwrap(),
                d: r["d"].as_f64().unwrap(),
                door: Door::parse(r["door"].as_str().unwrap()).unwrap(),
                seed: r["seed"].as_u64().unwrap() as u32,
                agents: r["agents"].as_u64().map(|a| a as usize),
            };
            let l = generate(&input, d);
            let got = format!("{:08x}", layout_hash(&l));
            let want = r["hash"].as_str().unwrap();
            let want_dropped: Vec<String> = r["dropped"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
            let want_skipped: Vec<String> = r["skipped"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
            if got != want || l.dropped != want_dropped || l.skipped != want_skipped || l.items.len() as u64 != r["items"].as_u64().unwrap() {
                bad.push(format!(
                    "{} {}x{} {} seed {}: rust {got} web {want} (dropped {:?} vs {:?}, skipped {:?} vs {:?})",
                    input.role,
                    input.w,
                    input.d,
                    input.door.as_str(),
                    input.seed,
                    l.dropped,
                    want_dropped,
                    l.skipped,
                    want_skipped
                ));
            }
        }
        assert!(bad.is_empty(), "{} of {total} differ, first: {:?}", bad.len(), &bad[..bad.len().min(8)]);
        eprintln!("matches_the_web_dump: {total} cases match");
    }

    #[test]
    fn every_role_resolves_to_a_template() {
        let d = data();
        assert!(d.roles.roles.len() >= 17);
        for role in d.roles.roles.keys() {
            assert!(template_for(role, &d.roles).is_some(), "{role}");
        }
        assert!(template_for("no-such-role", &d.roles).is_some(), "the family fallback");
    }

    /// Every group of the catalogue names known items, and every role's group is a catalogue group.
    #[test]
    fn every_group_and_role_group_resolves() {
        let d = data();
        for (gid, g) in &d.catalog.groups {
            for it in &g.items {
                assert!(it.id == "$station" || d.catalog.items.contains_key(&it.id), "{gid}: item {}", it.id);
            }
        }
        for (role, t) in &d.roles.roles {
            for e in t.required.iter().chain(&t.preferred) {
                assert!(d.catalog.groups.contains_key(&e.group), "{role}: group {}", e.group);
            }
            for p in &t.pool {
                assert!(d.catalog.groups.contains_key(&p.group), "{role}: pool group {}", p.group);
            }
        }
    }
}
