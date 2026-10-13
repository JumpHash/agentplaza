# Rooms: a seeded generator, a verifier, one catalogue

Status: design (2026-10-13). Replaces the hand-placed room styles (`rooms/styles-*.ts`, `styles_*.rs`): items
floated, sat in odd places, and every room of a role looked the same. Rooms are now **generated** from a
catalogue and per-role templates by a deterministic algorithm, **verified** by a checker, and **identical in
both apps** (the same JSON in, the same layout out, proven by shared fixtures).

## Files

```
assets/rooms/
  catalog.json     the items: footprint, height, placement rule, facing, clearance, model
  roles.json       per role (and family fallback): shell options, required items, optional pool, density
  fixtures.json    golden cases: (role, w, d, door, seed) → layout hash, checked by both apps
docs/design/rooms.md
web:     apps/web/src/views/floor3d/rooms/gen.ts (generator), verify.ts, build.ts (layout → meshes); #/room-lab
desktop: apps/desktop/src/console/floor3d/rooms/gen.rs, verify.rs, build.rs; APZ_ROOM_LAB
```

## Inputs and output

Input: `{ role, w, d, door, seed }`
- `w`, `d`: the room's inner size in units (the cell the board layout gave it, minus a 0.15 wall margin);
- `agents`: optional desk count (default `cap = clamp(floor(w*d/6), 1, 4)`); see "Role templates";
- `door`: the side facing the nearest walkway/trace: `"n" | "e" | "s" | "w"` (−z, +x, +z, −x);
- `seed`: u32 = `fnv1a32(machineId + "/" + role + "/" + roomIndex)` on the floor; any u32 in the lab.

Output (plain JSON, rounded to 3 decimals; the room's local frame: origin at the floor centre, y up, floor top
at y = 0):

```jsonc
{
  "shell": { "floor": "planks", "wall": "panel", "walls": ["n", "e", "w"], "door": { "side": "s", "at": 0.0, "width": 1.0 } },
  "items": [
    { "n": 0, "id": "desk", "x": -1.1, "y": 0, "z": -1.35, "rot": 0, "host": null },
    { "n": 1, "id": "chair", "x": -1.1, "y": 0, "z": -0.75, "rot": 180, "host": 0 },
    { "n": 2, "id": "monitor", "x": -1.1, "y": 0.72, "z": -1.45, "rot": 0, "host": 0 }
  ],
  "spots": [ { "item": 0, "x": -1.1, "z": -0.75, "face": 0, "clip": "type" } ],
  "skipped": [ 4 ],                       // preferred items that did not fit (indices of the catalogue entries, not violations)
  "dropped": []                           // required items that could not be placed (the verifier fails on any)
}
```

`rot` is degrees about y, one of 0/90/180/270 (an item's "front" is +z at rot 0). `y` is 0 for floor items
and the host's top for surface items — never anything else (nothing floats). `spots` come from the catalogue
item's spots (where a robot stands to use it), transformed.

## The catalogue (`catalog.json`)

```jsonc
{ "schema": 1, "cell": 0.25, "items": {
  "desk":    { "model": "kit:desk",    "size": [1.2, 0.72, 0.6], "place": "wall",   "face": "in",
               "clear": { "front": 0.7, "side": 0.1 }, "spots": [{ "at": [0, 0.6], "face": 180, "clip": "type" }],
               "attach": [{ "id": "chair", "at": [0, 0.55], "rot": 180 }, { "id": "monitor", "on": true, "at": [0, -0.1] }] },
  "chair":   { "model": "kit:chair",   "size": [0.5, 0.8, 0.5], "place": "attached" },
  "monitor": { "model": "kit:monitor", "size": [0.5, 0.4, 0.15], "place": "surface" },
  "shelf":   { "model": "kit:shelf",   "size": [1.2, 1.6, 0.35], "place": "wall", "face": "in", "clear": { "front": 0.5 } },
  "plant":   { "model": "kit:plant",   "size": [0.4, 0.7, 0.4], "place": "corner" },
  "rug":     { "model": "kit:rug",     "size": [1.6, 0.01, 1.2], "place": "center", "layer": "floor" },
  "easel":   { "model": "prop:easel",  "size": [0.8, 1.5, 0.6], "place": "free", "face": "in", "clear": { "front": 0.6 } }
}}
```

- `model`: `kit:<name>` (the room kit's builders), `prop:<id>` (assets/robots/props), `part:<id>` (hardware parts).
- `size`: footprint w × d at rot 0 and height h (the top surface hosts `surface` items).
- `place`: `wall` (back to a wall, facing in), `corner` (in a corner), `center` (the middle region), `free`
  (anywhere inside, facing `face`), `surface` (on a host item's top), `attached` (beside its host, from the
  host's `attach`), `wallmount` (on a wall face at a height: posters, clocks, screens).
- `face`: `in` (toward the room centre), `door`, `wall`; `clear.front` / `clear.side`: space kept free in front /
  at the sides (merged into the occupancy as soft reservations that other items may not use, robots may).
- `layer`: `floor` items (rugs, mats) don't occupy cells and render with a polygon offset above the floor.

## Role templates (`roles.json`)

```jsonc
{ "schema": 1, "roles": {
  "architect": { "family": "build",
    "shell": { "floor": ["planks", "concrete"], "wall": ["panel", "glass"], "open": 0.2 },
    "required": [ { "id": "drafting-table" }, { "id": "easel" }, { "id": "desk", "per": "agent" } ],
    "pool": [ { "id": "shelf", "w": 3, "max": 2 }, { "id": "plant", "w": 2, "max": 3 }, { "id": "rug", "w": 1, "max": 1 },
              { "id": "poster", "w": 2, "max": 2 }, { "id": "lamp", "w": 1, "max": 1 } ],
    "density": [0.35, 0.55] } },
  "families": { "build": { "...": "fallback for roles without an entry" } } }
```

Each role has `required` and `preferred` stations. `required` = the role's FIRST station, then one desk per agent
(`per: "agent"`, the room's capacity): these must always place. The station goes first, so a small room keeps it. `preferred` = the other stations, in order:
placed if they fit, skipped silently otherwise (listed in `skipped`, never a violation, never dropped). Order of
placement: required, then preferred, then the pool fills the floor until the occupied share reaches a density
drawn from the range. `open` is the chance of a wall-less room.

`minSize(role, agents)` is the smallest square side (on a 0.5 step from 2.5) at which the required items always
place, checked by generating over seeds 1..200 and the four doors. The board builds each room at
`max(its cell, minSize(role, agents))` where the slot allows, and logs a warning when the slot is smaller.

## The algorithm (deterministic; both apps implement exactly this)

PRNG: **mulberry32** — `a += 0x6D2B79F5; t = imul(a ^ a>>>15, 1|a); t ^= t + imul(t ^ t>>>7, 61|t);
return ((t ^ t>>>14) >>> 0) / 4294967296`. One generator per room, seeded with `seed`; every random choice
draws from it in the order below. `pick(list)` = `list[floor(r() * len)]`; `shuffle` = Fisher–Yates from the end.

1. **Grid**: `cols = floor(w / cell)`, `rows = floor(d / cell)`; cell (i, j) centre
   `x = -w/2 + (i + 0.5)·cell`, `z = -d/2 + (j + 0.5)·cell`. States: free, solid, reserved.
2. **Shell**: floor = pick(shell.floor); wall = pick(shell.wall); walls = all sides but `door`, or none if
   `r() < open`. Door: centred (`at = 0`), width 1.0 (clamped to the side's length − 0.5).
3. **Reserve**: the door zone (door width × 1.0 deep inside the door) and a spine (0.5 wide) from the door to the
   room centre are reserved.
4. **Place required, then pool** (pool: repeat {weighted pick by `w` among entries under `max`} until density or
   20 failed tries). For each item:
   - **Candidates** by rule: `wall`: every cell along each wall side (not the door side), item rotated to face in,
     back flush with the wall (inset 0.05); `corner`: the 4 corners (not those touching the door zone);
     `center`: cells within the middle 50 %; `free`: every cell; `wallmount`: positions along wall faces.
   - **Score** each candidate: rule fit (wall/corner exact = 1), + 0.5 · distance from the door (work items:
     those with spots) normalised, − 0.3 · nearness to the same item id (spread), + `r()` · 0.05 (tie-break).
     Sort descending (stable by candidate order).
   - **Accept** the first candidate whose footprint (rotated) lies inside the room, touches no solid or reserved
     cell, whose `clear` zones touch no solid cell, and after which every spot and the front-access cell of every
     placed item is still reachable from the door by 4-connected BFS over non-solid cells. Mark its footprint
     solid and its clear zones reserved. Place its `attach` items (chairs: floor, beside; `on: true`: surface,
     y = host top) — an attachment that doesn't fit is skipped (not dropped).
   - If no candidate passes, a required item goes to `dropped`; a preferred item goes to `skipped`; a pool item is skipped.
   - Spot rule: a candidate is rejected if any of its transformed spots lies outside the room (inset 0.15 from each
     wall that exists; the room edge otherwise), or on a BLOCKING cell (its own footprint included). Walk items are not
     blocking. Reachability never clamps a spot: each spot is checked in its own cell.
5. **Spots**: each placed item's catalogue spots, transformed by its position and rot.
6. **Round** every number to 3 decimals; items in placement order (`n` = index).

**Layout hash** (fixtures): FNV-1a 32 over the canonical JSON of the whole layout object, `skipped` included
(keys sorted, no spaces, numbers as rounded decimals with no trailing zeros, `-0` as `0`).

## The verifier

`verify(layout, input) → violations[]` (empty = valid):
- every item inside the room (footprint, rotated); floor items `y = 0`; surface items `y` = host top ± 0.001;
- no two solid footprints overlap; no solid inside the door zone; `clear` fronts free of solids;
- wall items touch a wall that exists (or the room edge for wall-less rooms); corner items in a corner;
- every spot and item front reachable from the door; every spot inside the room and not inside a solid;
- `dropped` is empty; required items present with the counts the template asks for;
- (z-fighting) no two items' top faces coplanar and overlapping unless one is a `floor` layer item.

## Testing and reproducing

- **Property test** (both apps): every role × sizes 2.5–8 (step 0.5, w ≠ d too) × 4 doors × 500 seeds →
  zero violations and zero dropped. The agents count is `cap(w, d)`; the required items must place at every
  size at or above `minSize(role, cap)`, and the sweep starts there. A failure prints its input as a lab URL.
  The run also reports, per role and size, how often a preferred station was skipped (to tune the catalogue).
- **Fixtures** (`fixtures.json`, 200 cases): both apps compute the layout hash and must match — parity proven.
  Regenerate with `npm run rooms:fixtures` only when the algorithm or the catalogue changes on purpose.
- **Room lab**: `#/room-lab?role=<role>&w=4&d=3.5&door=s&seed=7` (one room, `debug=1` overlays the grid,
  solids, reservations, spots and violations), `&grid=1` (12 seeds side by side), `&role=all` (every role).
  Desktop: `APZ_ROOM_LAB=<role>` with `APZ_ROOM_SEED`, `APZ_ROOM_SIZE=4x3.5`, `APZ_ROOM_DOOR`.

## On the board

`board-layout` gives each room its cell; the floor computes `door` (the side nearest the room's trace walkway)
and the seed, calls the generator and builds the layout with the kit and the props. Room size grows with its
agent count as today. The behaviour engine (robots.md) reads `spots` as the room's stations.

## v2: groups with a purpose (2026-10-13, after the owner's review)

The owner found items "just out there, floating without reasoning or purpose": chairs alone in the middle of a room
facing nothing, a desk without its chair, a cabinet standing free beside a wall, a station at an odd angle. The cause:
the generator placed items one by one, with no relationships. v2 places **groups** — furniture compositions with a
purpose — and every item belongs to one.

### Groups (`catalog.json` `groups`)

```jsonc
"groups": {
  "desk-set":     { "anchor": "wall",   "items": [ { "id": "desk" }, { "id": "chair", "rel": "front", "gap": 0.05, "face": "anchor" },
                                                   { "id": "monitor", "rel": "on" } ], "spots": "desk" },
  "reading-nook": { "anchor": "corner", "items": [ { "id": "armchair", "face": "room" }, { "id": "floor-lamp", "rel": "beside" },
                                                   { "id": "small-shelf", "rel": "beside-other" } ] },
  "meeting":      { "anchor": "center", "items": [ { "id": "meeting-table" }, { "id": "chair", "rel": "around", "count": [2, 4], "face": "anchor" } ] },
  "lounge":       { "anchor": "wall",   "items": [ { "id": "sofa" }, { "id": "rug", "rel": "front" }, { "id": "coffee-table", "rel": "front", "gap": 0.3 } ] },
  "storage-wall": { "anchor": "wall",   "items": [ { "id": "shelf", "count": [1, 3], "rel": "row" } ] },
  "station":      { "anchor": "wall",   "items": [ { "id": "$station" }, { "id": "stool", "rel": "front", "optional": true } ] },
  "door-flank":   { "anchor": "door",   "items": [ { "id": "plant" }, { "id": "plant", "rel": "mirror" } ] },
  "wall-art":     { "anchor": "wallmount", "items": [ { "id": "poster" } ] }
}
```

- `anchor`: where the group's first item goes — `wall` (back to a wall that exists, centred on a wall segment, facing in),
  `corner`, `center`, `door` (either side of the door, inside), `wallmount` (on a wall face, above furniture height or on
  a bare stretch).
- `rel` (relative to the group's first item, in its frame): `front` (in front, at `gap`), `on` (its top), `beside` / 
  `beside-other` (left / right of it, flush), `row` (next along the wall), `around` (evenly around a table, facing it),
  `mirror` (mirrored across the door's axis).
- `face`: `anchor` (toward the group's first item), `room` (toward the room centre), `wall`, `door`; default: as the rule implies
  (a chair in front of a desk faces the desk; a desk faces into the room with its user's back to the room centre, or its
  back to the wall — chosen per role template `desk_facing: "wall" | "room"`).
- A group is placed whole or not at all (optional items may drop). Items keep their group id (`g` in the layout).
- **Every item belongs to a group.** There are no free-standing singles: a lone chair, a lone shelf in the middle, or a
  plant in the middle of the floor cannot be expressed. Plants go in corners (`corner` groups) or flank doors and desks.
- Role templates list groups instead of items: `required` (e.g. `{ "group": "station", "station": "lab-bench" }`,
  `{ "group": "desk-set", "per": "agent" }`), `preferred`, and the decor `pool` (groups with weights and maxes).

### Purpose rules (the verifier)

In addition to v1's checks:
- every chair is in a `desk-set`, `meeting`, `station` or `reading-nook` and faces its anchor item (±10°), within 0.15 of
  its intended relative position;
- every wall-anchored item's back is within 0.08 of an existing wall, and its facing is perpendicular to that wall;
- every item's rotation is a multiple of 90° (except `around` chairs, which face their table);
- nothing stands in the middle region unless its group's anchor is `center`;
- nothing blocks a wall-mounted item's view (a shelf in front of a poster) or a station's spot;
- each group's items are mutually consistent (the monitor on its own desk, the stool at its own station);
- spots face their item.
The property test and the fixtures cover v2; both apps match by fixture hash as before.
