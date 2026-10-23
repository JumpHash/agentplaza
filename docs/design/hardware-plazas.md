# Hardware plazas: the Machines view and a plaza per machine

Status (2026-10-14): built, in both apps, through the engine (`crates/apz-engine`, with the board logic in
`crates/apz-board`). The three.js sources named below are retired (`987394f`); the Rust code they map to is named
where each rule is stated.

The Floor becomes two levels, in the web console and the desktop app alike (1:1):

1. **Machines** — every machine of the plaza (the hub's own and every joined node) as a 3D model
   of its real hardware, on the night plaza ground, connected by animated cables to the hub. Each
   shows its status. Clicking one opens a popup (its hardware, its agents, its state) with an
   **ENTER** button.
2. **The machine's plaza** — entering flies the camera into the machine: the case opens and the
   view lands on its board. The plaza *is* the hardware: a PCB with its chips (CPU, memory, GPU,
   storage, network, power…), traces carrying animated current, and the agents' rooms built as
   mini rooms on top of the components. Robots walk along the traces between rooms.

Machine kinds: `workstation`, `laptop`, `edge` (single-board computers, a Raspberry Pi), `phone`
(Android, later), `server`. A machine with GPUs shows them (outside: in the case window / on the
card; inside: as GPU boards with their own rooms).

**Apple is its own family.** A Mac reports `vendor: "apple"` and its form factor as `kind`
(`laptop` for a MacBook, `workstation` for a Mac mini, Mac Studio, Mac Pro or iMac) plus its
`model` ("MacBook Pro (16-inch, M3 Max)", "Mac Studio (M2 Ultra)"). Apple machines never use the
generic models: they have their own — `exterior/apple-laptop.json` (the aluminium unibody
MacBook), `exterior/apple-desktop.json` (the Mac mini / Mac Studio block; `params.studio` picks
the taller Studio) — and their inside is `board/apple-soc.json`: an Apple Silicon logic board
built around the SoC package, which carries the CPU clusters (performance and efficiency cores
as separate blocks), the GPU core array, the Neural Engine, the media engines and the unified
memory packages on the same substrate; around it the SSD controller and NAND, the Thunderbolt
controllers and the power stages. Rooms go on those blocks (slot types `cpu` for the P and E
clusters, `gpu` for the GPU array, `memory` for the unified memory, `npu` for the Neural Engine,
`storage`, `io` for Thunderbolt, `power`). The machine's `apple` details (chip name, P/E core
counts, GPU core count, unified memory) feed the board's params (`p_cores`, `e_cores`,
`gpu_cores`, `memory_gib`) so it shows the right number of cores. Model selection:
`vendor == "apple"` → the apple-* models; otherwise `kind` → the generic ones.

## One source of truth: hardware models as data

Every model is a JSON file under `assets/hardware/`, read by the one renderer, the Rust engine
(`crates/apz-engine/src/hardware.rs`, geometry in `crates/apz-board/src/hardware.rs`), on both the web and the desktop. (The three.js
renderer, `apps/web/src/views/floor3d/hardware.ts`, was retired in `987394f`.) Neither app hand-codes a machine: parity comes
from the shared data.

```
assets/hardware/
  exterior/{workstation,laptop,edge,phone,server}.json   the machine as seen in the Machines view
  exterior/{apple-laptop,apple-desktop}.json             Macs (vendor "apple")
  board/apple-soc.json                                    a Mac's inside: the Apple Silicon logic board
  board/{workstation,laptop,edge,phone,server}.json      its inside: the plaza
  parts/*.json                                           reusable pieces (gpu-pcie, dimm, fan, m2-ssd, …)
  palette.json                                           named colours shared by all models
```

### Units and axes

One unit ≈ one robot's width. Y is up. A model's origin is the centre of its footprint on the
ground (y = 0). Exteriors are 6–14 units tall (a phone ~4, a server rack ~14); boards are laid on
y = 0 and span 40–80 units so rooms (3–6 units wide) fit on the components. Angles are degrees.

### Schema (version 1)

```jsonc
{
  "schema": 1,
  "kind": "workstation",              // exterior/board: the machine kind; parts: the part name
  "view": "exterior",                 // "exterior" | "board" | "part"
  "size": [w, h, d],                  // bounding box, for camera framing
  "params": { "gpus": 1 },            // optional inputs the model reacts to (see "when")
  "parts": [ Part, ... ],
  "anchors": {                        // named points (model space)
    "label": [x, y, z],               // where the name/status tag sits (exterior)
    "port": [x, y, z],                // where cables attach (exterior)
    "camera": { "at": [x,y,z], "look": [x,y,z] },  // the ENTER fly-in end pose (exterior) / the default view (board)
    "open": "side_panel",             // exterior: the part id that animates away on ENTER
    "atom": [x, y, z]                 // where the plaza's atom sits (board: the centre, above cpu0/soc;
                                      // exterior: behind the window / on the screen, small)
  },
  "slots": [ Slot, ... ],             // board only: where rooms go
  "traces": [ Trace, ... ]            // board only: copper paths, also the robots' walkways
}
```

`Part`:

```jsonc
{
  "id": "gpu0",                        // optional, unique in the file; referenced by anchors/slots/anim
  "geo": Geo,                          // or "use": "parts/gpu-pcie" (a part file, instanced)
  "at": [x, y, z], "rot": [rx, ry, rz], "scale": [sx, sy, sz] | s,
  "mat": "pcb" | { "color": "#1f6b3a", "emissive": "#000", "glow": 0..4, "opacity": 0..1,
                   "additive": false, "flat": false },   // a palette name or an inline material
  "outline": true,                     // the toon ink outline (default true for solids, false for decals)
  "detail": 1 | 2,                     // shown by distance: 1 within 1.6 model radii of the camera, 2 within 0.8 (with hysteresis)
  "cutaway": true,                     // fades to 25 % while it stands between the camera and the focus point
  "repeat": { "count": 4, "step": [dx, dy, dz],         // DIMM rows, fins, cells, LEDs; count may be a
              "wrap": 4, "row": [dx, dy, dz] },         // param name ("p_cores") as well as a number;
                                       // with "wrap", every `wrap` copies start a new row offset by `row`
                                       // (a grid: 10 P-cores as rows of 4)
  "when": "gpus >= 1",                 // shown only if the expression on params holds: <param>, !<param>,
                                       // <param> >= n (params: gpus, p_cores, e_cores, gpu_cores, memory_gib, studio)
  "anim": Anim,
  "tag": "status",                     // semantic tag: status LEDs take the machine's status colour
                                       // ("status"), "activity" blinks with agents working,
                                       // "fan" spins when online, "screen" shows the plaza's glow
  "label": { "text": "CPU0", "size": 0.6, "color": "#e8eadf" },  // silkscreen text on the part's top
  "children": [ Part, ... ]            // transformed by this part
}
```

`Geo` (both renderers implement exactly these):

| type | fields |
|---|---|
| `box` | `w h d` |
| `rbox` | `w h d r` (rounded box, radius r) |
| `cyl` | `rt rb h` (+ `seg`, default 24) |
| `cone` | `r h` |
| `sphere` | `r` |
| `torus` | `r tube` (+ `arc` degrees) |
| `plane` | `w d` (horizontal, facing up) |
| `ring` | `ri ro` (flat, horizontal) |

`Anim`: `{ "spin": { "axis": "y", "rps": 2 } }` · `{ "pulse": { "period": 1.6 } }` (emissive breathes) ·
`{ "blink": { "period": 0.8, "phase": 0.3 } }` · `{ "slide": { "to": [x,y,z], "on": "enter" } }`
(the ENTER opening: e.g. a side panel slides off, a laptop lid swings — `{ "swing": { "axis": "x",
"to": -110, "on": "enter" } }`).

`Slot` (board): a surface rooms are built on.

```jsonc
{ "id": "cpu0", "on": "cpu0",          // the part it sits on
  "type": "cpu",                       // cpu | gpu | memory | storage | network | power | io | soc | modem | bmc
  "top": y,                            // the surface height
  "rect": [x, z, w, d],                // the buildable area on that surface (model space)
  "capacity": 4 }                      // how many rooms fit
```

`Trace`: `{ "path": [[x, z], ...], "y": 0.42, "width": 0.25, "from": "cpu0", "to": "dimm_a" }` (`y` is
the absolute height: just above the PCB top) —
polylines on the PCB, drawn as copper; current pulses travel along them; robots walk them.

## Grounding

Every solid of a model rests on what is directly under it (the full rule and the attached exemptions: `assets/hardware/README.md`, "Grounding"). Its bottom is at the top of the highest solid beneath it, within 0.02, and it does not sink into a solid beneath it or below the ground. The audit (`groundAudit`, retired with `apps/web/src/views/floor3d/ground.ts`; its support field is now `crates/apz-board/src/support.rs`, and the grounding audit is `crates/apz-board/src/audit.rs`) checks this for every board at 1, 4, 8 and 20 agents, and `cargo test -p apz-board` fails on any floating or sinking part.

- **Parts:** a solid that is not `attached` rests on the parts under it. An `attached` part (a fan on its shroud, a cable, a glow, a light) is not tested. Decals (flat `plane` and `ring`), the copper traces and additive glows neither rest nor hold anything up.
- **Slots and plates:** a slot's `top` is the top of its `on` part plus the plate thickness (0.2). The terrace plate spans from the highest solid under it up to its top; where it stands over lower ground, its columns go down to what is under them. The plate never hangs in air and never sinks into a part.
- **The Rust port (`crates/apz-board/src/support.rs`, `crates/apz-board/src/layout/`) mirrors this, and the desktop uses the same code:** it reads the same `attached` flag and the same slot tops, and builds each terrace plate the same way. The plate's underside is the highest solid under its footprint, never less than 0.2 below its top. Each 0.5-unit cell that stands over lower ground gets a column from the support under it up to the underside, the support being the highest solid box at the cell's centre and four inset corners (the web reads the solids' boxes, taken before the bake, since the baked meshes are merged). The grounding check must hold the same zero-finding bar on the board data.

## The atom at the centre

The plaza's atom — the glowing nucleus with its electron shells that hovers over today's central
dais (`xenonAtom`, formerly in the retired `apps/web/src/views/floor3d/props.ts`; now `crates/apz-engine/src/props.rs`) — is kept exactly as it is and sits **at the centre
of every machine's plaza**: over the main CPU's die (cpu0) or, on an SoC board, over the SoC,
always at the board's centre point. Every board model has an anchor `"atom": [x, y, z]` at its
centre above the main processor; the renderer places the existing atom builder there (scaled to
the board, same animation). The orchestrator's room is built around it on that chip, as today's
dais is. In the Machines view, each machine shows a small copy of the atom glowing through its
window/screen (an `anchors.atom` on exteriors too) while it is online.

## Rooms on components

Each agent family (the Society families: command, build, operations, knowledge, money, other)
prefers a component type; each role is one room (as today's districts), placed in the first slot
of the preferred type with room, then the fallbacks, then any slot:

| family | prefers | then |
|---|---|---|
| command (orchestrator) | cpu (the orchestrator on cpu0's centre) | soc |
| build (architect, builder, qa, reviewer, release) | gpu | npu, cpu, soc |
| knowledge (librarian, researcher, planner) | memory | storage |
| operations (edge-operator, incident, security-reviewer) | network | io, bmc, modem |
| money (finance, auditor) | power | io |
| other | storage | any |

A room is today's district room (drawn by `crates/apz-engine/src/rooms/`, generated by `crates/apz-board/src/rooms/`), scaled to its share of the slot's
`rect`, its floor plate replaced by a thin heat-spreader plate (metal, the role's hue on its
edge). Robots live in their rooms and walk to each other along the traces (shortest path on the
trace graph, joining the nearest trace point of each slot). Each room has a minimum size, the square side
its role needs for its agent count, read at layout time from `assets/rooms/minsize.json`. A slot holds
only as many rooms as leave every cell at least its rooms' minimum side, so a slot that is too small
holds fewer, larger cells. A room that no slot of its family can hold moves on to the family's next slot
type, then to any slot with room, and the orchestrator still takes the cpu cell nearest the atom. A role
with no size that fits is placed by the old rules and logged as a warning.

**Joining (every agent has a desk; no room is under its minimum).** A role whose minimum no free cell of its
family's slots holds does not get a room of its own and does not shrink a cell. Its agents **join a built room**:
1. The candidate hosts are the built rooms of the same family, nearest first (the distance from the room's slot to
   the centre of the role's first family slot type, else to the atom; ties by role name). The first host whose cell
   still reaches the minimum for its new desk count takes the agents.
2. If no same-family host fits, the largest built room on the board (the largest of its cell's shorter side) takes
   the agents, and a warning says so.
A host's desk count rises by the joiners, so its minimum is recomputed for the new count (the minimum table, read at
layout time). Joining adds desks, not cells, so no other room's cell changes. Each agent's home is a desk in its
room: the square grid of desks, three apart, closer when the cell is small. With the 17-role roster, every model
seats all 17 agents and no room is under its minimum for its desk count (tests: `board join`).
A slot the board's default camera cannot see is not used. The camera is `anchors.camera.at` of the board model. For each slot, nine samples sit in a 3 by 3 grid over the slot's top, one unit above it; a sample is blocked when the segment from the camera to it crosses a part box other than the slot's own part (`Board::part_boxes`, `crates/apz-board/src/lib.rs`). A slot is occluded when MORE than a third of its samples are blocked (4 or more of 9). The Rust port is in `crates/apz-board/src/layout.rs` (`OCCLUSION_GRID`, `OCCLUSION_HEIGHT`, `OCCLUSION_SHARE`); the slot's `occluded` field, when set, overrides it (the workstation leaves it unset). The third is a measured choice: on the workstation the RAM slot is a third blocked (its near row sits behind the GPU card) and stays usable, m2_2 is four ninths blocked and is hidden. A 25% share would hide the RAM. Per-cell occlusion (hiding only the cells that are blocked) is the principled refinement and is not done yet. On the workstation with one GPU the occluded slots are the chipset, m2_1 and m2_2; with two GPUs, m2_3 is occluded too. The other five models have no occluded slot.
An agent with no role (an empty role from the fleet) gets a room of its own, built as the `other` family (its own minimum row in minsize.json), rather than one room shared by every unnamed agent.

## Aisles, landings and the gangway

Each slot is a terrace plate at the component's top. Its rooms are placed around an aisle, so every door faces open floor
and no door faces a part or another room's wall.

- **Aisle.** An aisle is a 1.0 unit strip. Where the plate's depth allows (at least two rooms of the median role minimum
  plus the aisle), the aisle runs down the plate's middle, with rooms on both sides and their doors facing it (a
  double-loaded aisle). Otherwise it runs along one long side (a single row). The side with the most open board beside
  it is chosen; ties go to the camera side.
- **Landings.** A plate reaches the board through one landing. Its aisle ends sit on the plate's perimeter. The end with
  the shorter multi-source search to the board's main component (parts only, at most 40 cells) gives the landing. A
  landing is a ramp when the plate's top is at most 1.5 units above the PCB, otherwise a lift. If neither aisle end
  reaches the main component, a spur (a 1.0 unit gap between two rooms, perpendicular to the aisle) reaches any
  perimeter point. A slot with no landing is unusable and its rooms go to other slots.
- **Gangway.** Where no plain landing exists, a gangway carries the plate over the solid cells to a lift shaft on the
  first walkable board cell outside the part's footprint. The bridge runs at plate height, walkable, and the lift
  drops to the board from its end. The reach is `GANGWAY_REACH` (formerly board-layout.ts, now `crates/apz-board/src/layout/aisle.rs`), 3.25 units. The task's first draft
  said 2.5; that would leave the cooler unusable (the nearest walkable cell is 3.1 units out). The orchestrator's room
  is on the cooler next to the atom, so the cooler stays usable: **3.25 is a deliberate deviation from 2.5.**
- **The lift and the gangway (`crates/apz-engine/src/world/lift_mesh.rs`, `world/walkway_mesh.rs`, `crates/apz-board/src/nav/lift.rs`).** A lift is a cab in a shaft beside a plate, 1.6 wide along the wall,
  between the board (PCB top) and the plate top, open at both landings with a gate and a call panel at each stop, and a
  counterweight on a cable. The gangway is only its bridge, which ends flush at the cab's plate side. The cab's rules (up to
  three riders, a queue in arrival order, eased travel, doors that open only at a stop) are in navigation.md, "Lifts". The
  lift's and the gangway's materials are shared per palette and compiled at warm-up (`prewarmLift` in the retired TS; the
  engine's warm-up is `crates/apz-engine/src/warm.rs`), so ENTER adds no shader links.

- **Balance.** Each room goes to the open slot with the lowest fill (load over the rooms its aisle holds at the room's
  minimum). Family preference only breaks ties. The orchestrator takes the open slot nearest the atom.
- **Result (17 agents, joins allowed).** Workstation with one GPU: 17 of 17 (GPU 10, cooler 2, RAM 4, M.2 1).
  Workstation with two GPUs: 17 of 17 (GPU 7, GPU2 5, cooler 2, RAM 3). Laptop 12, server 17, edge 15, phone 9,
  apple-soc 3 (its memory plates are too shallow for a second row; a best-fit placement may give a fourth).

## The scenes

**Machines.** The night ground and sky of today's Floor (outskirts). The hub's machine at the
centre, nodes on a ring around it (sorted by id), spaced by their sizes. A cable from each node's
`port` to the hub's (a sagging curve of short cylinder segments) with light packets travelling
along it while the node is online (both ways, rate by agent activity). Under each machine a status
ring: online `--ok`, draining amber (stopped by the owner, n0 too), offline grey, revoked `--crit`; LEDs tagged `status` take the
same colour, `activity` LEDs blink while its agents work, fans spin while online. A small tag above
each machine: its name and a status square (like the Floor's compact labels). Hover lifts the
machine slightly; click opens the popup.

**Popup** (an overlay card, same style as the Floor's agent card): the machine's name and kind; a
facts list (CPU model, cores/threads, memory, GPUs with memory, architecture, hypervisor backend,
OS, role hub/node, last seen); its agents (count by status, and the first few names); and the
**ENTER** button (the console's primary button, signal orange). Esc or a click outside closes it.

**Entering** (≈1.6 s, eased): the popup folds; the camera flies to the machine's
`anchors.camera`; the part named by `anchors.open` animates away (`slide`/`swing` on "enter"); the
screen dips through a short glow (the board's emissive colour), and the board scene builds in:
the PCB fades up, components rise into place in a wave from the CPU outward (staggered ~40 ms),
traces light up from the CPU along their paths, then the rooms assemble (scale up from the slot)
and the robots drop in. A breadcrumb "Machines › zarathos-pc" and a "Back to machines" button
reverse it (the board folds away, the camera pulls back out of the machine).

**The board** is lit like the Floor (toon, ink outlines) but dark solder mask and copper, with a
soft emissive glow on active parts: a component whose rooms have working agents glows brighter,
its traces carry more pulses; a GPU running work shows its fans spinning faster.

## Data: `GET /v1/fleet`

```jsonc
{
  "machines": [{
    "id": "n0", "name": "zarathos-pc", "role": "hub",            // hub | node
    "kind": "workstation",                                         // workstation | laptop | edge | phone | server
    "vendor": "apple",                                             // lower-case maker: apple, dell, lenovo, raspberrypi, samsung, … (null when unknown)
    "model": "Mac Studio (M2 Ultra)",                              // the maker's model name (null when unknown)
    "apple": { "chip": "Apple M2 Ultra", "p_cores": 16, "e_cores": 8, "gpu_cores": 76,
               "neural_cores": 32, "unified_memory_gib": 192 },    // only on Macs
    "status": "online",                                            // online | draining (stopped by the owner, n0 too) | offline | revoked
    "arch": "x86_64", "os": "linux", "backend": "kvm",
    "cpu": { "model": "AMD Ryzen 9 5900X", "cores": 12, "threads": 24 },
    "memory_mib": 128320,
    "gpus": [{ "name": "NVIDIA GeForce RTX 3090", "vendor": "NVIDIA", "memory_mib": 24576 }],
    "storage_gib": 1863,
    "agents": [200, 201],                                          // cids running there
    "working": 0,                                                  // of those, how many are working
    "stats": { "cpu_pct": 7.5, "mem_used_mib": 18000 },            // live load; remote nodes send it with every heartbeat; null when unknown
    "last_seen_ms": 1791852847610
  }],
  "links": [{ "a": "n0", "b": "n3", "online": true }]
}
```

Kind detection (noded, reported by the node's heartbeat through apz-link):
Android → `phone`; a Raspberry Pi / other SBC device-tree model → `edge`; DMI chassis type
(`/sys/class/dmi/id/chassis_type`) 8, 9, 10, 14, 31, 32 or a battery present → `laptop`; 17, 23,
25, 28 → `server`; else `workstation`. macOS: `hw.model` containing "Book" → `laptop`, else
`workstation`. Windows: `Win32_SystemEnclosure.ChassisTypes` with the same table. CPU model:
`/proc/cpuinfo` "model name" (or the device-tree model on arm SBCs), `machdep.cpu.brand_string`
on macOS, the registry `ProcessorNameString` on Windows.
