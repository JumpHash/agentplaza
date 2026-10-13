# three.js to apz-engine: the port checklist

Owner's rule (2026-10-13): everything the web's three.js 3D does must be in the Rust engine, and the engine must do
more. three.js is removed when every row below says **done**. Status comes from the audit of the web's
`apps/web/src/views/floor3d/**` against `crates/apz-engine`, and from the work since.

| # | Feature (three.js source) | Engine status |
|---|---|---|
| 1 | Machines view: fleet ring, cables, LEDs, labels, hover, select, popup | done (web `?engine=wasm`, desktop) |
| 2 | ENTER flight, occlusion fade, case opening, Back (`entry.ts`) | done; check the distance fade and the cutaway over the plaza against the web fixes |
| 3 | Idle camera drift, Reset restarts it | done |
| 4 | Render on demand, motion tiers | done (Rest, Ambient 12 fps, Active) |
| 5 | Machine list orbit stage and thumbnails (`machine-stage.ts`, `machine-model.ts`) | done (web and desktop), vendor tint and contact shadow included |
| 6 | GPU reference cards (`gpus.ts`) | done (data and matching); wire the card choice into the board's `gpu-pcie` parts |
| 7 | Lifts: cab, rails, doors, controller (`lift.ts`, `lift-ctrl.ts`) | missing; needs `apz-board` lifts and landings first |
| 8 | Gangways and ramps (`gangway.ts`, `ramp.ts`) | missing; needs `apz-board` links |
| 9 | Nav grid walking, A*, followers, avoidance, respawn (`nav-*.ts`) | in `apz-board` (grid, Lazy Theta*, flow fields); motion and the engine hookup are missing |
| 10 | Body clips: crouch, jump, air, fall, land (`robot.ts`) | done in the engine (`BodyMode`, tested pose for pose against the web formulas); `apz-board` motion does not drive it yet |
| 11 | Board scale growth to seat agents (`board-layout.ts`) | in `apz-board`; the engine board must use the `apz-board` Layout |
| 12 | Grounding, drawn tops and support (`ground.ts`, `drawn-tops.ts`) | in `apz-board` (support field); engine hookup missing |
| 13 | Warm-up budget for board builds (`warm.ts`) | `warm.rs`: budgeted steps and an LRU build cache; the ENTER board does not go through the cache yet (`stage.rs` `refresh_board`) |
| 14 | Shadow proxies (`bake.ts`) | missing |
| 15 | Portrait studio (`portrait.ts`) | done in the engine (`portrait.rs`, offscreen) and on the web under `?engine=wasm`; the desktop still draws its own copy in `render_card.rs` |
| 16 | Robot lab: species, clip, tool, carry, hue, orch, cam (`robot-lab.ts`) | hue, orchestrator, camera presets, frozen clock and background are in the engine lab; the web hookup needs a wasm lab entry, and the background is not drawn yet |
| 17 | Z-fight lint over the hardware models (`zfight.ts`) | ported (`apz-board::zfight`), counts equal to the web on all 11 cases; committed with the board fixes |
| 18 | Robot design audit and eye seating (`robots/audit.ts`, `seat.ts`) | done (`robots/audit.rs`); issue counts not yet cross-checked against the TS run |
| 19 | Nav overlay `?nav=1` (`nav-overlay.ts`) | missing |
| 20 | Debug hooks (`__apzLookAt`, `__apzLifts`, `__apzNavCell`, `__apzScene` …) | the web's names are installed in engine mode; LookAt, EntryTime and Bots, and the nav and lift hooks, are stubs until the engine has them |
| 21 | Cutaway of tall parts over the plaza (the web fix of 2026-10-13) | the ENTER distance fade matches the web; the hardware cutaway (`overArea`, `CUTAWAY_OPACITY` in `hardware.ts`) is being checked |
| 22 | Performance budget (≤300 machines, ≤400 board, 0 frames at rest) | in progress: 689 draws (identical rigs instanced); next, the 267 static batches and the loose per-frame draws |

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
