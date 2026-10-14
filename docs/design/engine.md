# apz-engine: one 3D engine for the desktop and the browser

Owner's decision (2026-10-13): one 3D implementation, in Rust on wgpu. It runs natively in the desktop app and in
the browser (WebAssembly, using WebGPU with a WebGL2 fallback). three.js is removed from the web app.

Status (2026-10-14): phases 1 to 4 are built (the switch is `987394f`). The engine is the 3D of both apps. Phase 5
is open: the board view's budget is not measured yet (`engine-port.md`, row 22).

## Starting point

- **Desktop:** `apps/desktop/src/console/floor3d/` was already a Rust/wgpu renderer (wgpu 27, through iced 0.14; the folder is gone, its code is `crates/apz-engine` now),
  about 23k lines. It covers the toon shading, outlines, hardware models, robots, rooms, world and layout, and
  the entry flight. The engine itself does not depend on iced; the iced side is only the host adapter, `apps/desktop/src/console/floor_engine.rs`.
- **Web:** `apps/web/src/views/floor3d/`, about 24k lines of three.js (deleted in `987394f`). `floor.ts` stays as the Floor view's shell. The web was AHEAD
  on several features:
  - redesigned robots and eye seating;
  - nav, walk surfaces, gravity and lifts;
  - aisles and grounding;
  - the machine list with its morph;
  - the ENTER fixes;
  - GPU cards (in progress);
  - the portrait studio.

## Architecture

```
crates/apz-board/        pure logic, shared (docs/design/apz-board.md): hardware facts, layout, walk surfaces, nav,
                         motion, lifts, audits
crates/apz-engine/       the renderer + scenes, platform-agnostic, wgpu only. It never creates a window or a surface:
                         the host hands it a wgpu Device/Queue and a target TextureView, plus inputs and data.
  core/                  the bevy_ecs world and its schedules, and the typed asset registry (`engine-core.md`)
  render.rs  shaders/  floor.wgsl   pipelines (toon, outline, shadow, sprite/label, blit), instancing, merged static
                         meshes, frame-on-demand; the shaders are separate .wgsl files
  scene.rs  stage.rs  entry.rs  machine_stage.rs  portrait.rs  hardware_lab.rs
                         machines view (fleet + cables), board view, entry flight, machine thumbnails and their
                         orbit-morph stage, robot portrait studio, labs
  world/  robots/  rooms/  hardware.rs  robot.rs  fleet.rs  controls.rs  labels (world/labels.rs: screen-space
                         positions out; hosts draw text)
crates/apz-engine-web/   wasm-bindgen shell: canvas → wgpu Surface (WebGPU, else WebGL2), rAF loop, input,
                         resize/DPR, a typed data feed (fleet, agents, events), label and hit results back to the page
apps/desktop             the iced adapter, `console/floor_engine.rs`: iced's shader widget hands Device/Queue/target to apz-engine
apps/web                 the TS views keep the DOM UI (panels, popups, labels as HTML); the 3D canvas is the engine
                         (`views/engine-mount.ts`)
```

**Host contract** (the same engine on both hosts; the names are the Rust ones, the web's are wasm-bindgen methods of
`crates/apz-engine-web`):
- `Engine::new(device, queue, format)`: the assets are embedded in the crate, so there is no assets argument.
- `engine.set_data(Feed)` (web: `set_feed(json)`): fleet, agents, machines, events. One serde type, from the hub's JSON.
- `engine.input(Event) -> bool`: pointer, wheel, keys and resize, in logical pixels plus DPR.
- `engine.frame(now, target, size_px, scale) -> FrameOut`: `needs_redraw`, labels, hover/pick and camera state. The host
  calls `frame` only when `needs_redraw` is set or input arrives, so the engine renders on demand.
- `engine.command(Cmd)`: select a machine, ENTER, Back, reset view, open a row's orbit stage, portrait for an
  agent.

## Performance budget (enforced by tests and a perf HUD)

- Plaza: at most 400 draws and 300k triangles. Machines view: at most 300 draws.
- No work when idle: zero frames rendered when nothing moves.
- At most 1 GPU context per page.
- Select hitch at most 50 ms. Pipelines are compiled once at startup, in WGSL.
- Static hardware is merged per material and repeated parts are instanced. Outlines come from inverted-hull
  instanced draws or a screen-space edge pass, picked by measurement. Shadows come from merged proxies and are
  only re-rendered on change.

## Phases

1. **Extract.** Move the desktop floor3d into `crates/apz-engine` behind the host contract. The desktop keeps
   working through an iced adapter, with byte-identical snapshots before and after. The crate also compiles for
   wasm32.
2. **Web shell.** `apz-engine-web`, plus an `apps/web` canvas mount (`apps/web/src/views/engine-mount.ts`). It first sat behind a `?engine=wasm` flag; the flag is gone, and the engine is the default.
   The machines view renders in the browser from the hub feed, under both WebGPU and WebGL2.
3. **Parity.** Port the features the web has and the engine lacks: robots, rooms, aisles, grounding,
   `apz-board` logic, lifts, ENTER, machine list and morph, portraits, robot lab, GPU cards. A parity harness
   renders the same scene in both (three.js, while it existed, against the engine) and diffs screenshots.
4. **Switch.** Done in `987394f`.
   - The web uses the engine by default.
   - Every three.js view is replaced: floor, machine list, portraits, robot lab, and the landing crowd renderer
     script.
   - `three` is removed from package.json.
   - The TS floor3d logic is deleted.
5. **Budget pass.** Hold every scene within the budget on a mid-range iGPU profile as well as the 3090.

Each phase ends green: desktop `cargo test`, the web `npm run check`, and the shared crates' tests.
