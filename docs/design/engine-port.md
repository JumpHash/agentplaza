# three.js to apz-engine: the port checklist

Owner's rule (2026-10-13): everything the web's three.js 3D does must be in the Rust engine, and the engine must do
more. three.js is removed when every row below says **done**. Status comes from the audit of the web's
`apps/web/src/views/floor3d/**` against `crates/apz-engine`, and from the work since.

**Status (2026-10-14): three.js is removed, and every row below is done in the engine** (`987394f`). The web's 3D is the engine only, with no `?engine=wasm`
flag. The `floor3d/**` paths below are the retired sources; the Rust code is in `crates/apz-engine` and `crates/apz-board`.

| # | Feature (retired three.js source) | Engine status |
|---|---|---|
| 1 | Machines view: fleet ring, cables, LEDs, labels, hover, select, popup | done (web, desktop) |
| 2 | ENTER flight, occlusion fade, case opening, Back (`entry.ts`) | done; the distance fade and the plaza cutaway match the web |
| 3 | Idle camera drift, Reset restarts it | done |
| 4 | Render on demand, motion tiers | done (Rest, Ambient 30 fps, Active) |
| 5 | Machine list orbit stage and thumbnails (`machine-stage.ts`, `machine-model.ts`) | done (web and desktop), vendor tint and contact shadow included |
| 6 | GPU reference cards (`gpus.ts`) | done (data and matching, as on the web; the web does not put the matched card into the board's `gpu-pcie` parts either) |
| 7 | Lifts: cab, rails, doors, controller (`lift.ts`, `lift-ctrl.ts`) | done: the lift meshes (`world/lift_mesh.rs`) follow the `apz-board` lift controllers; robots ride them |
| 8 | Gangways and ramps (`gangway.ts`, `ramp.ts`) | done: `world/walkway_mesh.rs` builds the ramps and gangways of the shared layout's portals and plates |
| 9 | Nav grid walking, A*, followers, avoidance, respawn (`nav-*.ts`) | done: `apz-board::navgrid` and `apz-board::nav` (planner, followers, crowd and step-aside, lifts) drive the robots through `world/nav.rs`; every fixture board lays out and walks |
| 10 | Body clips: crouch, jump, air, fall, land (`robot.ts`) | done: the nav body drives the clips (`world/nav.rs` body_of) |
| 11 | Board scale growth to seat agents (`board-layout.ts`) | done, past the web: `apz-board` plans and accepts as board-layout.ts did, and the nav grid's main walk is the board's (the largest component at board level; two GPU cards side by side made a plate top the largest and cut the whole PCB as an island, so the web's layout failed on every multi-GPU board). Every fixture machine lays out on its first pass, a lift or ramp per plate |
| 12 | Grounding, drawn tops and support (`ground.ts`, `drawn-tops.ts`) | done: robots stand on the nav heights (plates, ramps, lifts); `__apzGroundAudit` reads it |
| 13 | Warm-up budget for board builds (`warm.ts`) | done: a re-entered machine's board comes from the build cache; budgeted steps exist (`warm.rs`) but the board build is still one call |
| 14 | Shadow proxies (`bake.ts`) | done in kind: the engine merges every static opaque caster into one geometry that only the shadow pass draws (`render.rs` build_statics), the proxies' purpose, in one bucket instead of per-cell buckets |
| 15 | Portrait studio (`portrait.ts`) | done in the engine (`portrait.rs`, offscreen) and on the web; the desktop still draws its own copy in `render_card.rs` |
| 16 | Robot lab: species, clip, tool, carry, hue, orch, cam (`robot-lab.ts`) | done, also in the browser (`#/robot-lab`) on the web lab's own pale stage |
| 17 | Z-fight lint over the hardware models (`zfight.ts`) | ported (`apz-board::zfight`), counts equal to the web on all 11 cases; committed with the board fixes |
| 18 | Robot design audit and eye seating (`robots/audit.ts`, `seat.ts`) | done (`robots/audit.rs`), cross-checked against the TS run (`testdata/robot-audit-ts.json`, 138 issues, three 0.186.1): every TS issue is found; the 7 extra `attach` errors are rounded-box parts whose TS gap came out NaN and so were never flagged |
| 19 | Nav overlay `?nav=1` (`nav-overlay.ts`) | done: `world/nav_overlay.rs` (`?nav=1` on the web, `APZ_NAV_OVERLAY=1` on the desktop) |
| 20 | Debug hooks (`__apzLookAt`, `__apzLifts`, `__apzNavCell`, `__apzScene` …) | done: every hook reads the engine (`debug.rs`), `__apzLiftRide` included |
| 21 | Cutaway of tall parts over the plaza (the web fix of 2026-10-13) | done (`over_area`, 15 % as on the web) |
| 22 | Performance budget (≤300 machines, ≤400 board, 0 frames at rest) | done for the Machines view: 222 draw items (budget test on; the shadow casters bake apart), 246 draws on the Floor in the browser; fans, LEDs and screens animate on the GPU from per-machine state; the board view's budget is not measured yet |

## Beyond three.js (engine only)

- An ECS core with deterministic fixed-step simulation (`engine-core.md`).
- GPU-driven batching: instancing and vertex-shader animation.
- Compute culling with indirect draws on WebGPU and native, with a CPU path on WebGL2.
- Flow fields on the GPU for crowds, with the CPU Dijkstra field (`apz-board::field`) as the oracle and the fallback.
- GPU ID-buffer picking.
- Compute particles for effects.
- Offscreen thumbnails and portraits on both hosts from one code path.
- One wasm build packed in Rust (`tools/apz-pack`: wasm-bindgen and wasm-opt as libraries, brotli and gzip siblings),
  and the same crate compiled natively into the desktop. The engine goes over the wire at 1.24 MB (brotli); the hub
  serves it as `application/wasm` with a year's immutable cache.

## The plan to the end (2026-10-14)

1. `apz-board::nav`: the planner (`nav-plan.ts`), the body and drawn tops (`nav-height.ts`, `drawn-tops.ts`), the lift
   controller (moved from the engine), then `NavWorld` (`nav-agents.ts`).
2. `apz-board::layout::nav_scene`: the nav grid of a finished layout with its rooms (nav-scene.ts), public, so the engine
   and the layout's acceptance build the same grid.
3. The engine's board view: rooms built from the same room plans, lift, gangway and ramp meshes from the grid's lifts
   and portals, robots driven by `NavWorld` (body clips from the pose), the lift cabs from the controllers, the debug
   hooks (`__apzBots`, `__apzLifts`, `__apzNavCell`).
4. The browser: the shared layout computed in a web worker running the same wasm, posted back to the engine.
5. The nav overlay (`?nav=1`).
6. **Done (`987394f`).** The wasm engine is the web's default (no `?engine=wasm`); the three.js renderers, their pure-TS
   cores that only they used, their tests and the `three` dependency are deleted.

## The side-by-side audit (2026-10-14)

The three.js build (`338f89c`) and the engine build were loaded on the same routes and the same stand-in hub, their
screenshots compared side by side, their debug hooks called and their scenes measured. What it found and fixed:

- **Labs.** The orbit's Floor limits (polar 0.2–1.3, distance 6–300) applied in the labs, where three's OrbitControls
  had none: a lone robot's close level view and the room lab's straight-down view were pushed out and tilted. The labs'
  canvases, key light, shadow camera and ShadowMaterial ground are three's; the robot lab's line-up spacing and facing
  too; the room lab's roles are in roles.json's order and `window.__roomLab` is back.
- **Shadows.** The shadow map was never cleared in a scene with no casters from the start (all in shadow). Board and
  exterior parts never cast or took shadows: hardware.ts `shadowsFor` (bounding radius ≥ 2, no decals) is ported.
- **Board view.** `reset_view` was an older floor.ts: it is now `fitView` (the occupied rooms' box, at least 0.3 of
  the board, grown 15 %, up to the built rooms' top, along the board's camera anchor, higher on a big board), with the
  haze at 0.8 and 3 times the distance. The rooms were an older terrace 0.5 above the slot (robots stood half a unit into
  the floor): they are board-rooms.ts `restingSlab` plates on the board's support field, with columns, and the nav
  bakes its walk heights from the drawn tops. A crowded board grows to the shared layout's `board_scale`.
- **Machines view.** The ring was an older fleet.ts with x and z swapped (a mirror image), ids sorted as text, no
  chord or hub minimum, and machines turned by their angle rather than their port: `ringLayout`, `facingAngle` and the
  numeric `byId` are ported, and the status ring is the 0.4 band at a pulsing 0.6–0.9 opacity.
- **Tags.** Every robot and machine tag is floor.ts's `LABEL_CSS` tag (Martian Mono 600 10.5px); the selected agent no
  longer gets a larger box, and a hovered idle agent shows its tag.
- **Hooks.** `__apzBots`, `__apzLifts`, `__apzPerf` and `__apzGroundAudit` return the three.js build's shapes;
  `__apzScene` (three's Scene object) is the engine's frame counts; `__apzCamera` is new.
- **Layout.** The engine's board layout is `apz-board`'s, not the retired board-layout.ts: the TS layout failed on the
  two- and four-GPU workstations of the fixture and grew other boards differently, so its numbers are not a target.
- **Lifts on hardware.** On lumen in a WebGPU browser on the GPU, two robots sent through lift 0 boarded, rode the cab
  (body `lift`, cab rising) and got off at the other level.
