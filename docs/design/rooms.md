# Rooms: a seeded generator of furniture groups, a verifier, one catalogue

Status: v2, built (2026-10-14; the algorithm was written on 2026-10-13). v1 placed items one by one: they floated, sat at odd angles and chairs stood alone in the
middle of the floor. v2 places **groups**: furniture compositions with a purpose (a desk and its chair and screen,
a reading nook, a meeting, a station with its stool, a storage wall, a door flank, wall art). Every item belongs to
exactly one group. Rooms are **generated** from the catalogue and per-role templates by a deterministic algorithm,
**verified** by a checker, and **identical in both apps** (the same JSON in, the same layout out, proven by the
fixtures). The algorithm, bit for bit, is in the appendix "v2 algorithm details"; the v1 resolutions that still
apply are folded in at the end of this file.

## Files

```
assets/rooms/
  catalog.json     the items (footprint, height, placement, clearance, spots, model) and the groups
  roles.json       per role (and family fallback): shell options, required and preferred groups, decor pool, density
  fixtures.json    golden cases: (role, w, d, door, seed) -> layout hash, checked by the Rust tests
  minsize.json     per role and desk count (1..6): the smallest room side at which the required groups always place
docs/design/rooms.md
generator: crates/apz-board/src/rooms/generate.rs; verifier: crates/apz-board/src/rooms/verify.rs
engine:    crates/apz-engine/src/rooms/ (build.rs: layout -> meshes, kit.rs, lab.rs, overlay.rs); web room lab at #/room-lab
(The TypeScript files `gen.ts`, `verify.ts`, `build.ts`, `kit.ts` and `rooms/index.ts` named in the text below were
retired in `987394f`; the Rust files above replace them.)
desktop: the same crates, no copy of its own; APZ_ROOM_LAB (the room lab's switch)
```

## Inputs and output

Input: `{ role, w, d, door, seed }` (plus optional `agents`)
- `w`, `d`: the room's inner size in units (the board cell minus a 0.15 wall margin);
- `agents`: optional desk count (default `cap(w, d) = clamp(floor(w*d/6), 1, 4)`);
- `door`: the side facing the nearest walkway: `"n" | "e" | "s" | "w"` (-z, +x, +z, -x);
- `seed`: u32 = `fnv1a32(machineId + "/" + role + "/" + roomIndex)` on the floor; any u32 in the lab.

Output (plain JSON, rounded to 3 decimals; the room's local frame: origin at the floor centre, y up, floor top at y = 0):

```jsonc
{
  "shell": { "floor": "planks", "wall": "panel", "walls": ["n", "e", "s", "w"], "door": { "side": "s", "at": 0, "width": 1 } },
    // the door side has two stubs with the opening (door.width, centred at door.at) between them
  "items": [
    { "n": 0, "id": "desk",    "x": 1.4,  "y": 0,    "z": -1.7, "rot": 0,   "host": null, "g": 0 },
    { "n": 1, "id": "chair",   "x": 1.4,  "y": 0,    "z": -1.14, "rot": 180, "host": null, "g": 0 },
    { "n": 2, "id": "monitor", "x": 1.4,  "y": 0.56, "z": -1.81, "rot": 0,   "host": 0,    "g": 0 }
  ],
  "spots": [ { "item": 0, "x": 1.4, "z": -1.1, "face": 180, "clip": "type" } ],
  "groups": ["desk-set"],        // groups[g] = the group id of instance g
  "dropped": [],                 // required group ids that could not be placed (the verifier fails on any)
  "skipped": []                  // preferred group ids that did not fit (not a violation)
}
```

`rot` is degrees about y, one of 0/90/180/270 (an item's front is +z at rot 0). `y` is 0 for floor items and the host's
top for surface items, never anything else. `spots` come from the catalogue item's spots (where a robot stands to
use it), transformed. `g` is the group instance; the first item with a given `g` is that group's anchor.

## The catalogue (`catalog.json`)

```jsonc
{ "schema": 2, "cell": 0.25, "walls": { ... },
  "items": {
    "desk":    { "model": "kit:deskTop", "size": [1, 0.56, 0.55], "place": "wall", "face": "in",
                 "clear": { "front": 0.7, "side": 0.1 }, "spots": [{ "at": [0, 0.6], "face": 180, "clip": "type" }] },
    "chair":   { "model": "kit:chair", "size": [0.42, 0.84, 0.42], "place": "free", "walk": true,
                 "spots": [{ "at": [0, 0], "face": 0, "clip": "sit" }] },
    "monitor": { "model": "kit:monitor", "size": [0.62, 0.56, 0.18], "place": "surface" } },
  "groups": {
    "desk-set": { "anchor": "wall", "items": [ { "id": "desk" },
                 { "id": "chair", "rel": "front", "gap": 0.05, "face": "anchor" },
                 { "id": "monitor", "rel": "on" }, { "id": "bin", "rel": "beside", "gap": 0.05, "optional": true } ] } } }
```

- Items: `model` (`kit:<name>` from the room kit, `prop:<id>` from assets/robots/props, `part:<id>`), `size` = [w, h, d]
  (footprint w along x by d along z at rot 0, and height h above the floor or the host top), `place` (`wall`, `corner`,
  `center`, `free`, `surface`, `wallmount`; `free` items are only placed inside groups), `face`, `clear` (`front`
  and `side`: space kept free in front and at the sides, reserved for the item's own group), `spots` (in the item's
  rot-0 frame, `face` in that frame), `walk` (cells are occupied for overlap but do not block the BFS: seats),
  `layer: "floor"` (rugs: never occupy cells, y = 0), `mount` (wall items: height of the bottom above the floor).
  v2 removed `attach`: the chairs and screens of a desk are group members.
- Groups: `anchor` (`wall`, `corner`, `center`, `door`, `wallmount`: where the group's first item goes, see
  "Groups") and `items`, in order. An item entry: `id` (a catalogue item, or `"$station"`: the template's station),
  `rel` (how it relates to the anchor, see "Groups"), `gap` (metres between faces, default 0), `face`
  (`"anchor"`: the item faces the anchor), `count` (`[lo, hi]`: copies, see the algorithm), `optional`.

## Groups

The v2 unit of placement. A group instance is an anchor (the first item) with the other items placed relative to it.
A group is accepted whole, or not at all; optional items may drop out of an accepted group.

| group | anchor | items (in order) |
|---|---|---|
| `desk-set` | wall (corner for `desk_facing: "room"`, see Templates) | desk; chair (front, face anchor); monitor (on the desk); bin (beside, optional) |
| `reading-nook` | corner | armchair; floor-lamp (beside, optional); small-shelf (beside-other, optional) |
| `meeting` | center | meeting-table; 2-4 chairs (around, face the table) |
| `lounge` | wall | sofa; rug (front); coffee-table (front, gap 0.3, optional); side-table (beside, optional) |
| `storage-wall` | wall | shelf (1-3 copies in a row along the wall) |
| `station` | wall | `$station` (a seat station: drafting table, counter, lab bench, microscope desk); stool (front, face anchor, optional) |
| `bench` | wall | `$station` (a standing station: workbench, crate stack, server rack, guard post, plant bed) |
| `corner-station` | corner | `$station` (alarm post) |
| `podium-stage` | wall | `$station` (podium) |
| `safe-corner`, `coat-corner`, `bin-corner`, `corner-plant` | corner | safe / coat-stand / bin / plant |
| `easel-corner` | corner | easel |
| `board-wall` | wall | whiteboard |
| `crate-pile` | wall | small-crate (1-3 copies in a row) |
| `cart-bay` / `dock-bay` / `garden-bench` | wall | book-cart / charging-dock / bench |
| `door-flank` | door | plant; plant (mirror across the door's axis, optional) |
| `wall-art` / `wall-clock` | wallmount | poster / clock |

Relations (in the anchor's frame; the anchor's rot-0 sizes; the item's own size turned by its relative rot `rr`):
- `front`: on the anchor's +z side at `gap`; `rr` = 180 with `face: "anchor"` (the item faces the anchor), else 0.
- `beside` / `beside-other`: on the anchor's −x / +x side, flush at `gap`; `rr` = 0.
- `row`: along +x of the anchor; each copy starts `gap` after the previous one's right face; `rr` = 0. A failed copy
  stops the row.
- `on`: stands on the anchor's top, against its back edge (+z-negative side) offset 0.05 inward; host = the anchor; `rr` = 0.
- `around`: on the side `s` of the anchor given by the entry's shuffled side order; `rr` = the side's rot
  (n 0, e 270, s 180, w 90), so the chair faces the table.
- `mirror`: the anchor's centre reflected across the door's axis, same rot as the anchor.

## Role templates (`roles.json`)

```jsonc
{ "schema": 2, "roles": {
  "orchestrator": { "family": "command", "desk_facing": "wall",
    "shell": { "floor": ["checker", "marble"], "wall": ["stone", "panel"], "open": 0 },
    "required": [ { "group": "podium-stage", "station": "podium" }, { "group": "meeting" }, { "group": "desk-set", "per": "agent" } ],
    "preferred": [],
    "pool": [ { "group": "corner-plant", "w": 2, "max": 3 }, { "group": "lounge", "w": 1, "max": 1 } ],
    "density": [0.45, 0.6] } },
  "families": { "other": { "...": "fallback for roles without an entry" } } }
```

- `required` = the role's signature groups, in order (its stations, and the groups that give the room its purpose:
  the meeting, the easel, the reading nook and cart bay, the board), then `desk-set` per agent (`per: "agent"`,
  repeated `agents` times). These always place; a failure is a violation. A room without its signature group would
  lose its point, so every signature group is required; `minsize.json` grows to fit them.
- `preferred`: extra stations, in order: placed if they fit, skipped silently otherwise (listed in `skipped`). Only
  breeder (a lab bench beside its plant bed) uses it.
- `pool`: decor groups with weights `w` and maxima `max`, placed after required and preferred until the density is reached.
- `desk_facing`: `"wall"` (the default): desk-sets anchor at a wall, the user faces the wall. `"room"`: desk-sets
  prefer a corner (the user faces into the room) and fall back to a wall.
- Signature groups (all required): orchestrator: podium stage and meeting; mayor, mayor-office: podium stage and
  counter; architect: drafting table and easel; reviewer: meeting; librarian: storage wall, reading nook and cart bay;
  researcher: lab bench and microscope desk (both with stools); planner: whiteboard and meeting; breeder: plant bed;
  finance, auditor: counter (with its stool) and a safe corner in the pool; builder, QA: workbench; release: crate stack;
  edge operator: server rack; incident: alarm post; security reviewer: guard post.

`minSize(role, agents)` is the smallest square side (on a 0.5 step from 2.5) at which the required groups always
place, checked by generating over seeds 1..500 and the four doors (`minsize.json` holds 1..6 desks per role). The
the board (`layoutOnBoard` in `board-layout.ts`) sizes each room's CELL to at least `minSize(role, agents)` where the slot allows, lowering the slot's capacity as needed, and moves a room that no slot of its family can hold to the next slot, then `any` (see `hardware-plazas.md`). `buildRoom` (`rooms/index.ts`) builds the room at its cell's size and never enlarges it; a cell still under its minimum logs a warning.

## The verifier

`verify(layout, input) -> violations[]` (empty = valid). v1 checks:
- every item inside the room; floor items `y = 0`; surface items `y` = host top;
- no two solid footprints overlap; no solid inside the door zone; a clear zone is free of solids, except those of
  the same group (a chair may stand in its desk's clear zone);
- every spot and item front reachable from the door; every spot inside the room and not inside a solid;
- `dropped` is empty; required groups present with the counts the template asks for;
- (z-fighting) no two items' top faces coplanar and overlapping unless one is a `floor` layer item.

v2 purpose rules (the spec's "Purpose rules"):
- every item has a group `g` that is a catalogue group; every seat (chair, armchair, stool) is in a desk-set,
  meeting, station or reading-nook group and faces its anchor (or the table) within 10 degrees;
- wall anchors: the back is within 0.08 of an existing wall (a side without a wall does not count), rot = that side's rot;
  corner anchors touch two room edges; door anchors touch the door edge; wall-mounted items touch their wall;
- every rot is 0, 90, 180 or 270;
- nothing stands in the middle region (|x| <= W/4 and |z| <= D/4) unless its group's anchor is `center` (floor items
  exempt);
- a wall-mounted item's view strip (MOUNT_VIEW = 0.6 deep, its width plus 0.1 each side) holds no solid whose top is
  above the mount + half the item's height;
- a surface item's host is in its own group;
- every spot faces its item within 30 degrees;
- required groups: each required entry's group occurs as often as the template asks (per agent: the desk count).

Not yet checked: the seat's offset from its intended relative position within 0.15 (the generator places it exactly;
the check is a follow-up).

## Testing and reproducing

- **Property test** (the `npm run rooms:prop` script is removed; the checks are Rust tests, `cargo test -p apz-board`): every role x sizes 2.5-8 (step 0.5, and the w != d pairs) x 4 doors x
  `--seeds` seeds (default 500) -> zero violations and zero dropped. The desk count is `cap(w, d)`; the sweep starts at
  the role's minSize. A failure prints its input as a lab URL. The run reports, per role and size, how often a
  preferred group was skipped.
- **Fixtures** (`cargo test -p apz-board`, `fixtures.json`, 200 cases): the generator computes the layout hash and must
  match. Regenerate only when the algorithm or the catalogue changes on purpose.
- **Room lab**: `#/room-lab?role=<role>&w=4&d=3.5&door=s&seed=7` (one room; `debug=1` overlays the grid, solids,
  reservations, spots and violations), `&grid=1` (12 seeds side by side), `&role=all` (every role).
  Desktop: `APZ_ROOM_LAB=<role>` with `APZ_ROOM_SEED`, `APZ_ROOM_SIZE=4x3.5`, `APZ_ROOM_DOOR`.

## On the board

The board layout (`crates/apz-board/src/layout/rooms.rs`) gives each room its cell; the engine computes `door` (the side
nearest the room's trace walkway) and the seed, calls the generator and builds the layout with the kit and the props
(`crates/apz-engine/src/rooms/`). The behaviour engine (robots.md)
reads `spots` as the room's stations; `g` is available to group-aware behaviour (a seat is a group member).

## Appendix: v2 algorithm details (binding; both apps match bit for bit)

### A0. The shell (changed in v2: the door side has stubs)

- Every closed room has a wall on all four sides. The room's `open` chance is 0 in every role (a wall-less room would
  leave wall-anchored groups standing in the open).
- The door side has two **stubs**, one either side of the **opening**: opening = `door.width` = `min(1.0, sideLength - 0.5)`,
  centred at `door.at` = 0; a stub runs from the opening's edge (|lateral| = width/2) to the outer end: the north and south
  walls run `w/2 + t` (they cover the corners), east and west run `d/2` (as the other walls). A thin lintel (0.2 high,
  the opening's width, at the wall's top) spans the opening, not on hedges or glass (render only, not a solid).
- The stubs are walls: they are not item solids and occupy no grid cells (they sit outside the inner size, like every wall).
  They bound the room exactly as the other walls do. Navigation must read `shell.walls` and `shell.door` to treat the
  stub segments as walls (the opening is the door zone's cells).
- Wall-anchored groups on the door side (`wall`, `wallmount`, and the `corner` rule as v2) use the stubs: a candidate's
  footprint lateral interval must lie beyond the opening, `lo >= width/2 - EPS` or `hi <= -width/2 + EPS`.
  The `door` rule (door-flank) is unchanged in shape: its footprint lies within `DOOR_NEAR` = 0.6 of the opening's edge, beyond it
  (raised from 0.3 so a robot can pass the door). The desktop rooms port must mirror `DOOR_NEAR` = 0.6 (its `gen.rs` constant is still 0.3 until it does).
- The verifier's wall test (`edges`) is the walls that exist, all four sides; an item on the door side must not cover
  the opening (the door zone rule).
- Desktop port: the shell is `walls` = all four sides (or none), plus `door`; the stubs are the door side split at the
  opening, as above. The lintel is render-only.

Everything here is what the generator (`rooms/generate.rs`, formerly `gen.ts`) does. Where the v1 text above or the v1 resolutions below differ, this appendix decides.

### A1. Constants and helpers

- `cell` = catalog `cell` (0.25). `INSET` = 0.05. `WALLMOUNT_DEPTH` = 0.1. `MOUNT_VIEW` = 0.6. `MOUNT_VIEW_SIDE` = 0.1.
  `CLEAR_MIN_ACCESS` = 0.25. `ON_SURFACE_OFFSET` = 0.05. `DOOR_NEAR` = 0.6. `EPS` = 1e-9. `COPLANAR_TOL` = 1e-3.
  `MAX_FAILED_TRIES` = 20. Default `gap` = 0.
- `SIDES` = n, e, s, w. `SIDE_ROT`: n 0, e 270, s 180, w 90 (the rot that turns a wall item's front to the room).
- `rotPt(x, z, rot)`: 0 -> (x, z); 90 -> (z, -x); 180 -> (-x, -z); 270 -> (-z, x).
- `facing(cx, cz)`: rot in 0, 90, 180, 270 maximising `dot(front, (-cx, -cz))` where `front = (sin rot, cos rot)`;
  dots in the order `[-cz, -cx, cz, cx]`; ties take the first. Used for corner and centre candidates (the item faces the
  room centre).
- `mulberry32`, `fnv1a32`, `roundNum`, `canonicalJson`, `layoutHash` as in the resolutions below.

### A2. Draw order (one mulberry32 per room, seeded with `seed`)

1. `floor = pick(shell.floor)`, `wall = pick(shell.wall)` (`pick(list) = list[floor(r() * len)]`), `openRoom = r() < open` (all roles have `open: 0` in v2: every closed room keeps its walls, because wall-anchored groups need them; the draw stays so the draw order is unchanged).
   `walls` = all four sides, or none when open (every role has `open: 0`, see A0).
2. `density = lo + r() * (hi - lo)`.
3. Door zone and spine (no draws): `doorWidth = min(1.0, sideLength - 0.5)`, `halfWidth = doorWidth / 2`. Door zone:
   cells with depth (inside the door side) <= 1.0 and |lateral| <= halfWidth. Spine: cells with |lateral| <= 0.25 between
   the door side and the room centre. Robot spots must lie inside [sx0, sx1] x [sz0, sz1]: 0.15 off each wall that exists.
4. Required groups: entries not `per: "agent"` first, then `per: "agent"` entries; each `per: "agent"` entry is placed
   `agents` times (default cap). A failed group is dropped (its group id listed in `dropped`).
5. Preferred groups, in order, `agents` times for `per: "agent"`; a failure is listed in `skipped`.
6. The pool loop (A7).

### A3. Placing one group instance

`placeGroup(gid, station, rules)`:
1. **Slots** (`buildSlots`), with draws done once, before any candidate: for each item entry in order:
   - if `count = [lo, hi]`: `n = lo + floor(r() * (hi - lo + 1))` (one draw); otherwise `lo = n = 1`;
   - if `rel = "around"`: sides `[n, e, s, w]` are shuffled by Fisher-Yates from the end: for `i = 3..1`,
     `j = floor(r() * (i + 1))`, swap `sides[i]`, `sides[j]` (three draws);
   - copies `c = 1..n`: the first entry's copy 1 is the ANCHOR (always required). Otherwise a copy is required when
     `c <= lo` and the entry is not optional. A copy's `rel` is the entry's `rel` (for entry 0's later copies, `row`
     by default); `side` = `sides[c-1]` for `around`.
   `$station` is replaced by the template's `station` param (an unresolved item is an error).
2. **Anchor candidates** (A4) for each rule, in order. With two rules (`desk-set` in a `desk_facing: "room"` role,
   rules `corner`, `wall`), the first rule's candidates get `ruleFit + 0.5`.
3. **Scores**: `score = ruleFit + 0.5 * workFar - 0.3 * spread + 0.05 * r()`, one draw per candidate in enumeration
   order; `workFar` and `spread` as in v1 (for the anchor item id). Order: descending score, ties by enumeration index.
4. For each candidate in that order: attempt (A5). The first success is kept; the group instance index `g` is the
   number of groups accepted before it. If none succeeds, the group is dropped or skipped.

### A4. Anchor candidates (enumeration order is part of the algorithm)

- `wall` (also `wallmount` and `door`, below): sides n, e, s, w (any side with a wall; on the door side only where the footprint lies beyond the opening, A0). For each
  side, `hx`, `hz` = the item's half sizes at that rot (along = rot % 180 == 0: `hx = w/2` (`d/2` for wallmount, depth
  0.1), `hz = d/2` (`w/2` else)). n: `z = -D/2 + hz + INSET`, rot 0; s: `z = D/2 - hz - INSET`, rot 180; w: `x = -W/2 + hx + INSET`,
  rot 90; e: `x = W/2 - hx - INSET`, rot 270. Positions along the side at each cell centre in increasing index.
  `ruleFit` = 1 for `wall`, 0 for `wallmount`.
- `door`: the door side only, rot `SIDE_ROT[door]`, positions along it as above. `lat` = the centre's lateral
  coordinate (x for n/s, z for e/w); `near = |lat| - halfWidth - h` (`h` the item's half size along the wall);
  `ruleFit = 1` when `-EPS <= near <= 0.6`, else 0.
- `corner`: nw (-x,-z), ne (+x,-z), se (+x,+z), sw (-x,+z); skip corners touching the door side. `rot = facing(sx*(W/2 - INSET - w/2), sz*(D/2 - INSET - d/2))`
  (the rot-0 footprint), then the centre at `sx*(W/2 - INSET - hx)`, `sz*(D/2 - INSET - hz)` with the rot's half sizes.
  `ruleFit` = 1.
- `center`: cells with |x| <= W/4 and |z| <= D/4, row-major (j outer, i inner), rot `facing(x, z)`, `ruleFit` = 0.
- `workFar` = dist(centre, door point) / diag for items with spots (door point as v1), else 0.

### A5. Attempt (`attemptGroup`)

Snapshot occ, blk, res, blocked and the record count. `members = []`.
1. The anchor: `tryPlace(anchor geo, host null, ban = rule != center)`. Failure: restore, fail.
   Commit; `members = [anchor]`. Row state: `right = anchor w / 2` (rot-0 width), `stopped = false`.
2. Each later slot: geometry by its relation (A6); `tryPlace(geo, host = anchor for "on", else null, ban)`.
   Success: commit, `members.push(n)`. Failure: a `row` slot sets `stopped`; a required slot restores and fails;
   an optional one is skipped.
3. Success: `members = []`, the group is accepted.

### A6. Slot geometry (`slotGeo`)

Anchor sizes `aw`, `ad` (rot-0 size[0], size[2]). An item's half sizes at relative rot `rr`: `(hw, hd) = (w/2, d/2)` when
`rr % 180 == 0`, else `(d/2, w/2)`; `hx`, `hz` = those. Offsets `(lx, lz)` in the anchor's frame:
- `front`: `rr = 180` if `face = anchor` else 0; `lx = 0`; `lz = ad/2 + gap + hz(rr)`;
- `beside`: `rr = 0`; `lx = -(aw/2 + gap + hx(0))`; `lz = 0`; `beside-other`: the same with `+`;
- `row`: `rr = 0`; `lx = row.right + gap + hx(0)`; then `row.right = lx + hx(0)`; `lz = 0`;
- `on`: `rr = 0`; `lx = 0`; `lz = -ad/2 + hz(0) + 0.05`;
- `around` with side s: `rr = SIDE_ROT[s]`; n: `lz = -(ad/2 + gap + hz(rr))`; s: `lz = +(...)`; e: `lx = +(aw/2 + gap + hx(rr))`;
  w: `lx = -(...)`; `lz = 0` where it applies;
- `mirror`: the anchor's centre reflected: x -> -x when the door is n or s; z -> -z when the door is e or w; same rot as
  the anchor; returned directly (not turned).
Otherwise the world centre is `anchor + rotPt(lx, lz, anchorRot)`, world rot `(anchorRot + rr) mod 360`.

### A7. Acceptance (`tryPlace`), in this order

1. Surface (`on`): the host exists; the footprint lies inside the host footprint (inclusive); no coplanar clash at the
   top `host.top + h`; spots valid (no solid blocking). Accept.
2. Bounds: the footprint lies inside the room (inclusive).
3. Middle ban: when `ban` (the group's anchor is not `center`), a non-floor item whose centre has |x| <= W/4 and |z| <= D/4 is rejected.
4. Door zone and spine: no footprint cell is a door or spine cell.
5. Floor (`layer: floor`): spots valid. Accept (occupies nothing).
6. Solid (kind `solid`):
   a. (anchor only, `members` empty) no footprint cell has occ > 0; (members) the exact overlap test below replaces the
      cell test, because a chair and its desk share a grid cell;
   b. every footprint cell's reservation is -1 or one of `members`;
   c. no solid record's footprint overlaps this footprint (exact, positive area);
   d. no wall-mounted record's view strip overlaps this footprint with this item's top above that record's limit.
7. Clear zone (when `clear.front > 0`): no clear-zone cell has occ > 0.
8. Coplanar: no non-floor record with top within 1e-3 whose footprint overlaps.
9. Wall-mounted (kind `wall`): no solid whose footprint overlaps the view strip has top above this item's limit.
10. Spots (`spotsValid`): each spot inside [sx0, sx1] x [sz0, sz1]; its cell exists and is not blocking (blk = 0); for a
    blocking candidate (solid, not walk), not inside its own footprint cells.
11. Reachability (solid and wall): BFS (A8).

Kinds: `surface` (place `surface`), `floor` (`layer: floor`), `wall` (place `wallmount`), `solid` otherwise.
View strip (wall-mounted items): the rectangle in front of the item (front = `(sin rot, cos rot)`), from its front face
out `MOUNT_VIEW`, across its width plus `MOUNT_VIEW_SIDE` on each side, AABB of the rotated corners, centred on the item.
Limit = `mount + h/2`.

### A8. Reachability

Passable cells: blk = 0, and not the candidate's own footprint cells when the candidate is a blocking solid (walk items
do not block). Sources: the door-zone cells that are passable. 4-connected BFS. Every check cell of every solid and wall
record and of the candidate must be visited. Check cells: each spot's cell (outside the grid = fail), and the centre
cell for walk items, else the access cell `centre + rotPt(0, d/2 + max(clear.front, 0.25)/2, rot)`.

### A9. Commit

Solid: occ += 1 on each footprint cell (saturating at 255); if not walk: blk += 1 (blocked += 1 when a cell first
becomes blocking). Clear zone (when `clear.front > 0`, cells of the zone rectangle as in the resolutions): res = the
record index when free, same when already that record, else -3. Surface, floor and wall records: nothing on the grid.

### A10. Pool

Each try (the loop stops when `blocked / N >= density`, checked before the try, or after 20 failed tries in a row):
- total = sum of `w` over entries with count < max; `u = r() * total`; the first entry whose cumulative `w > u`;
  if none (rounding), the last eligible entry;
- `placeGroup(entry.group, null, rules)` with the group's rules (A3); success: count += 1 and fails = 0; failure: fails += 1.
Required and preferred groups are not subject to density.

### A11. Output

Items in record order: `n` = index; `g` = group instance; rounded; `host` = the host record index or null.
Spots: for each record in item order, its catalogue spots transformed (A6's rotation), `face = (face + rot) mod 360`.
`groups` = the group ids by instance; `dropped` and `skipped` = group ids in order of failure.

### A12. Hash

FNV-1a 32 over the canonical JSON of `{ shell, items, spots, groups, dropped, skipped }`, as v1 (sorted keys, no spaces,
numbers via `String(roundNum(v))`).

### A13. minSize

`minSize(role, agents)`: the table entry for `agents` (null = no side up to 8 qualifies), else computed: the first side
from 2.5 in 0.5 steps (to 8) for which no required group is dropped over seeds 1..500 and each door.

## Appendix: v1 resolutions (folded in from the implementation resolutions file)

These were binding for v1 and still hold for v2 except where the v2 appendix above says otherwise. In v2: `attach` is gone (the desk-set places its chair and monitor), `walk` and `layer` keep their meaning, and the candidate rules of single items are used only for group anchors. The text is kept verbatim below.

Source of truth: /mnt/fast/AgentPlaza/docs/design/rooms.md. Where it is silent or ambiguous, THIS file decides.

#### Catalogue fields (assets/rooms/catalog.json)
- `size` = [w, h, d]: footprint w (x) by d (z) at rot 0, and height h above the floor (or above the host top for surface items). Rugs: h = 0.01.
- `place`: wall | corner | center | free | surface | attached | wallmount.
- `face`: in | door | wall | none (default in). `clear`: {front, side} (default 0, 0).
- `spots`: [{at:[x,z], face:deg, clip}] in item-local rot-0 frame. `face` is in the item frame; world face = (face + rot) mod 360.
- `attach`: [{id, at:[x,z], rot, on?:bool}]. `on:true` = surface item at host top (y = host top).
- `walk` (bool, default false): cells are occupied for overlap but do NOT block the BFS (chairs, sitting places).
- `layer`: "floor" for rugs: never occupies cells, never blocks, may sit under furniture, y=0.
- `mount` (wallmount only): height y of the item's bottom above the floor. Wallmount footprint depth is 0.1.
- `props:<id>` items use the prop JSON footprint/spots when assets/robots/props/<id>.json exists; the catalogue
  size must EQUAL that footprint (gen reads catalog only, so they must agree). Until then use a placeholder box.

#### Geometry
- Room: inner size w (x) by d (z), centred on the origin. Door side: door=n (-z), e (+x), s (+z), w (-x).
- Walls: a wall on a side sits OUTSIDE the inner size: slab inner face at the room edge, centre at edge + t/2 (t = wall thickness from kit wallLook). Door side never has a wall. Walls = the non-door sides, or none when r() < open.
- Footprint of an item at (x,z) with rot: at rot 0/180 it is w by d; at rot 90/270 it is d by w; centred on (x,z).
- Grid: cell = 0.25; cols = floor(w/cell), rows = floor(d/cell); cell (i,j) centre x = -w/2 + (i+0.5)cell, z = -d/2 + (j+0.5)cell.
  Cell coverage: an item covers cell (i,j) iff the open cell square overlaps the open footprint rectangle (strict).
  Index range: i from floor((x0+w/2)/cell + eps) to ceil((x1+w/2)/cell - eps) - 1, eps = 1e-9, clamped to [0, cols-1].
  Leftover strip beyond cols*cell is not in the grid (footprints may extend there; they just cover fewer cells).
- Point to cell: i = clamp(floor((x+w/2)/cell), 0, cols-1), j likewise.
- Door zone: cells whose centre lies within |lateral| <= width/2 (width = 1.0 clamped to side length - 0.5) and depth <= 1.0 inside the door side.
- Spine: cells whose centre lies within |lateral| <= 0.25 between the door side and the room centre (from the door edge to x=0 or z=0).
  Door zone and spine are reserved with owner 'door' / 'spine'.
- Reservations are owner-tagged: 'clear:<itemIndex>' for clear zones, 'door', 'spine'.
- clear zones: front = depth beyond the item's front face (+z in item frame, rotated), width = footprint width + 2*clear.side, depth = clear.front. Cells covered by the clear zone rectangle are reserved (owner = that item).
  Items with clear.front 0 reserve nothing.

#### Candidates (enumeration order is part of the algorithm; keep it)
- Score = ruleFit + 0.5*workFar - 0.3*spread + 0.05*r(); sort descending, stable by enumeration order.
  Every candidate draws exactly one r() in enumeration order, before sorting.
- ruleFit: wall and corner candidates = 1; others = 0.
- workFar: items with spots: dist(candidate centre, door point) / diag, where door point = centre of the door side (x=0,z=+d/2 for s, etc.) and diag = sqrt(w^2+d^2). Items without spots: 0.
- spread: max over placed items with the same id of max(0, 1 - dist/(0.5*diag)); 0 if none.
- Enumeration:
  - wall: sides in order n, e, s, w (the door side only beyond the opening, see A0). n: z = -d/2 + hz + 0.05, rot 0; s: z = d/2 - hz - 0.05, rot 180; w: x = -w/2 + hx + 0.05, rot 90; e: x = w/2 - hx - 0.05, rot 270. Position along the wall: centre coordinate at each cell centre along the side, in increasing index. hx/hz = half sizes at that rot.
  - corner: order nw(-x,-z), ne(+x,-z), se(+x,+z), sw(-x,+z); skip corners on the door side; centre inset 0.05 from both edges; rot = facing(centre).
  - center: cells with |x| <= w/4 and |z| <= d/4, row-major (j outer, i inner); rot = facing(centre).
  - free: every cell row-major (j outer, i inner); rot = facing(centre).
  - wallmount: sides in order n,e,s,w (skip door); cells along each side in increasing index; rot fixed by side as for wall; y = mount.
  - floor layer (rugs): center candidates as above, rot 0.
- facing(centre): rot in order 0,90,180,270 maximising dot(front, toCentre) where front = (sin rot, cos rot). Ties take the first (smallest rot). If toCentre is zero, rot 0.

#### Acceptance (for a candidate of a non-floor item)
1. Footprint inside the room (continuous): |x| edges within w/2, z edges within d/2 (no tolerance).
2. Footprint covers no occupied cell and no reserved cell (any owner).
3. Every clear-zone cell of the candidate is not occupied.
   OCCUPIED = covered by the footprint of any placed item that is not floor-layer, surface or wallmount (walk items included).
   BLOCKING = occupied and not walk. Reserved = clear/door/spine tags.
4. After marking: all spots and every item's access cell are reachable (BFS below).
   - BLOCKING cells as defined above (walk items are passable).
   - access cell of an item = centre + rot*(0, d/2 + max(clear.front, 0.25)/2) (the middle of its front clear zone, or 0.25 out).
   - spot cell = point-to-cell of spot world position (clamped). A spot may sit on a walk item, never on a BLOCKING cell.
   - BFS: 4-connected over the grid, passable = not BLOCKING. Sources = all non-blocking door-zone cells.
   - Every spot cell and access cell must be visited.
5. Floor layer items: inside the room, touching no door-zone or spine cell; they occupy nothing; no BFS.
6. Wallmount: inside room, touching the side's edge, touching no door-zone/spine cell; occupies nothing.
7. Surface items: only via an attachment with on:true; footprint inside the host footprint; occupies nothing on the floor.
8. Attachments (after the host, before the next item): same tests as 1, 2 and 4, except the host's own clear zone is allowed (it may use cells reserved as clear:<host>).
   They never use door or spine cells. A failing attachment is skipped (not dropped).

#### Pool and density
- Per room: density lo..hi drawn once: lo + r()*(hi - lo), after the shell draws and before any required item.
- Shell draws, in order: floor = pick(floors); wall = pick(walls); open = r() < open ? none : walls as above.
- Capacity (for `per: "agent"` desks): cap = clamp(floor(w*d / 6), 1, 4). Required desks = cap entries.
- Required items: in template order; each `per: agent` entry is repeated cap times.
- Pool loop: each try draws r() for the weighted pick: total = sum of w over entries with count < max; u = r()*total; first entry whose cumulative w > u.
  If no entry is eligible, stop. A try succeeds when the candidate placement is accepted. Consecutive failures >= 20 stop the loop.
  Stop also when share >= density (share = BLOCKING cells / (cols*rows)). Check it before each try.
  Required items are NOT subject to density.
- An item with no accepted candidate: required -> dropped; pool -> skipped (counts as a failure).

#### Output
- items in placement order; each attachment placed immediately after its host; n = index in items.
- spots: for each placed item (in item order), its catalogue spots, transformed: world = pos + rotate(at, rot); face = (face + rot) mod 360.
  rotate(x,z,rot): x' = x*cos + z*sin; z' = -x*sin + z*cos (rot in radians).
- Round every number: Math.round(v*1000)/1000 and +0 for -0.
- rot is an integer 0/90/180/270; host is the index or null.

#### Hash
- Canonical JSON of the layout object (shell, items, spots, dropped): object keys sorted, arrays in order, no spaces,
  strings JSON-escaped, numbers via String(n) of the rounded value (-0 -> 0), null/true/false as usual.
- FNV-1a 32 over the UTF-8 bytes: h = 0x811c9dc5; for each byte: h ^= b; h = Math.imul(h, 0x01000193) >>> 0.
- Output as 8 lowercase hex digits.

#### Floor integration (web)
- seed = fnv1a32(machineId + "/" + role + "/" + roomIndex) over UTF-8 bytes (same FNV-1a as above, no multiplier change).
- door = the side of the room facing the nearest trace point: take the room's walkway points from board-layout; pick the side (n/e/s/w, in the room's local frame after the room turns) of the minimum distance.
- Room `w = 2*s - 0.15`, `d = 2*s - 0.15` by default (the cell minus the 0.15 margin).
- Shape: rooms are rectangles; the board's `shape` for a centre seat stays round for the terrace only.

#### Added by coordinator (binding)
- Templates: `required` = one desk per agent (per:"agent") + the role's FIRST station only; `preferred` = the other stations, placed in order when they fit, skipped silently (listed in layout `skipped`, never a violation, never dropped). Placement order: required, preferred, pool. Preferred items use the same candidate/acceptance rules.
- Input gets optional `agents`; default cap(w,d) = clamp(floor(w*d/6),1,4).
- `minSize(role, agents)`: smallest side on a 0.5 step from 2.5 where the required items always place (checked over seeds 1..500 and 4 doors).
- Property test: sizes from minSize(role, cap(s)) up to 8; required never drop; report skipped preferred counts per role and size.
- Board: `layoutOnBoard` sizes the cell to at least minSize(role, agents) where the slot allows; `buildRoom` builds at the cell's size and does not enlarge it; a warning when a cell stays under its minimum.
- Layout gets `skipped` (preferred indices) and it is part of the hashed object.
