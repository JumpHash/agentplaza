# Hardware plazas: the Machines view and a plaza per machine

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
card; inside: as GPU boards with their own rooms). Apple Silicon machines are `laptop` or
`workstation` by chassis, with an SoC board (CPU, GPU and unified memory on one package).

## One source of truth: hardware models as data

Every model is a JSON file under `assets/hardware/`, read by both renderers — three.js on the web
(`apps/web/src/views/floor3d/hardware.ts`) and the desktop's renderer
(`apps/desktop/src/console/floor3d/hardware.rs`). Neither app hand-codes a machine: parity comes
from the shared data.

```
assets/hardware/
  exterior/{workstation,laptop,edge,phone,server}.json   the machine as seen in the Machines view
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
  "repeat": { "count": 4, "step": [dx, dy, dz] },        // DIMM rows, fins, cells, LEDs
  "when": "gpus >= 1",                 // shown only if the expression on params holds (gpus, gpus >= n, !gpus)
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

`Trace`: `{ "path": [[x, z], ...], "y": 0.06, "width": 0.25, "from": "cpu0", "to": "dimm_a" }` —
polylines on the PCB, drawn as copper; current pulses travel along them; robots walk them.

## The atom at the centre

The plaza's atom — the glowing nucleus with its electron shells that hovers over today's central
dais (`xenonAtom` in `apps/web/src/views/floor3d/props.ts`, its port in
`apps/desktop/src/console/floor3d/props.rs`) — is kept exactly as it is and sits **at the centre
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
| command (orchestrator, mayor, mayor-office) | cpu (the orchestrator on cpu0's centre) | soc |
| build (architect, builder, qa, reviewer, release) | gpu | cpu, soc |
| knowledge (librarian, researcher, planner) | memory | storage |
| operations (edge-operator, incident, security-reviewer) | network | io, bmc, modem |
| money (finance, auditor) | power | io |
| other | storage | any |

A room is today's district room (`rooms/` in both apps), scaled to its share of the slot's
`rect`, its floor plate replaced by a thin heat-spreader plate (metal, the role's hue on its
edge). Robots live in their rooms and walk to each other along the traces (shortest path on the
trace graph, joining the nearest trace point of each slot).

## The scenes

**Machines.** The night ground and sky of today's Floor (outskirts). The hub's machine at the
centre, nodes on a ring around it (sorted by id), spaced by their sizes. A cable from each node's
`port` to the hub's (a sagging curve of short cylinder segments) with light packets travelling
along it while the node is online (both ways, rate by agent activity). Under each machine a status
ring: online `--ok`, draining amber, offline grey, revoked `--crit`; LEDs tagged `status` take the
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
    "status": "online",                                            // online | draining | offline | revoked
    "arch": "x86_64", "os": "linux", "backend": "kvm",
    "cpu": { "model": "AMD Ryzen 9 5900X", "cores": 12, "threads": 24 },
    "memory_mib": 128320,
    "gpus": [{ "name": "NVIDIA GeForce RTX 3090", "vendor": "NVIDIA", "memory_mib": 24576 }],
    "storage_gib": 1863,
    "agents": [200, 201],                                          // cids running there
    "working": 0,                                                  // of those, how many are working
    "stats": { "cpu_pct": 7.5, "mem_used_mib": 18000 },            // null when unknown
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
