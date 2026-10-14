# apz-board: one Rust core for the board, on the desktop and in the browser

Status (2026-10-14): built. The crate has the scope below and its own tests (`cargo test -p apz-board`). The
desktop links it natively; the web reaches it through the engine's WebAssembly build. The names in **API** are the
ones in the code.

This crate implements the parts the web and desktop apps must agree on exactly. It does it once, in Rust. The
desktop links it natively. The web loads it as WebAssembly, inside `crates/apz-engine-web` (built by `tools/apz-pack`;
apz-board has no wasm crate of its own). This was the owner's decision on 2026-10-13. It replaced the TypeScript
`board-layout.ts`, `nav-*.ts`, `ground.ts` and `lift-ctrl.ts` (all deleted in `987394f`), and the partial Rust copies
that lived in the desktop's `floor3d/world/` (also gone).

## Scope

**In the crate** (pure logic, no rendering, no I/O, deterministic):
1. **Hardware boards.** Parse `assets/hardware/board/*.json` and its `parts/`, and compute each resting part's
   world box and top surface. Rendering still builds meshes from the same JSON in each app, so this is about
   geometry facts, not drawing.
2. **Board layout.** Place slots, terraces (plates), columns, rooms (their footprints and floor heights),
   gangways, lifts and homes for N agents. Apply `boardScale`. The output is a `Layout`.
3. **Walk surfaces** (`docs/design/walk-surfaces.md`). The layout emits a list of walk surfaces and links. Plates,
   columns, landings, bridges and the lift cab are generated from it, so they rest on drawn tops by construction.
4. **Nav graph.** Rasterise the walk surfaces into the grid (cells, heights, components, drop edges, portals)
   and plan paths with A* over it. Also a fine height field for feet (0.05).
5. **Motion.** The `NavWorld` step: robots, commit-point invariants, separation, yield and hold, gravity, jump
   and drop, respawn, lift riding.
6. **Lift controller.** Queue, capacity, doors and travel.
7. **Audits.** Grounding (nothing floats or sinks), z-fight (no coplanar overlaps among generated surfaces), and
   nav invariants (`onWall`, `hover`, reachability). These run as the crate's own tests and are callable from the
   apps' debug hooks.

**Not in the crate:** rendering, robot rigs and clips, labels and cameras (the engine draws them), and the behaviour
engine (what a robot wants to do next: `crates/apz-engine/src/world/behaviour.rs`, which sets each robot's goal through
`NavWorld::set_goal`). Room *footprints and floors* are in the crate (`rooms/`, `navgrid/rooms.rs`); their furniture is
drawn by the engine inside the footprint and must stay inside it, which the crate's audit checks.

## Shape

```
crates/apz-board/                  the logic; `std`, no threads, no I/O; f64 math; serde for inputs/outputs
  build.rs                         embeds assets/hardware (board, parts, palette.json) with include_str!
  src/lib.rs                       Board, Aabb, the support field and the part count (hardware.rs, scene.rs, support.rs)
  src/layout.rs  layout/           the board layout: aisle, board_grid, columns, mouth, rooms, scene, surfaces
  src/walk.rs  grid.rs  navgrid.rs  navgrid/rooms.rs   walk surfaces, the nav grid and the room cells
  src/plan.rs  nav.rs  nav/        A* planner, body and heights (height.rs), drawn tops (drawn.rs), lift controller
                                   (lift.rs), NavWorld (nav/world/: api, crowd, follow, stuck, tests)
  src/rooms.rs  rooms/             room plans and their generation (generate.rs) and checks (verify.rs)
  src/field.rs  mesh.rs  geom.rs  zfight.rs  audit.rs   support field, meshes, geometry, z-fight lint, audits
  tests/                           fixtures.rs (boards.json), zfight.rs
apps/desktop                       depends on apz-board directly (Cargo.toml)
crates/apz-engine/                 the 3D engine: its world/ modules drive apz-board and draw the result
crates/apz-engine-web/             the wasm shell; the web's layout worker runs the same wasm
```

**API** (the Rust names; the web calls the same functions through `apz-engine-web`):
- `Board::from_kind(kind, params) -> Board`: the assets are embedded in the crate at build time, so both apps get
  the same data with no I/O. `board.parts()`, `board.part_boxes()`, `board.bounds()`, `board.support_top(x, z)`,
  `board.part_count()`.
- `layout(board_kind, &LayoutInput) -> Layout`. `LayoutInput` takes the agents (`AgentSpec`) and the GPUs. `Layout`
  holds the slots, plates, columns, rooms, lifts, homes, landings, walk surfaces and scale, and is serde-serialisable.
- `nav_scene(layout, gpus, machine)` (in `layout/scene.rs`): the nav grid of a finished layout with its rooms, so the
  engine and the layout's acceptance build the same grid.
- `NavWorld::new(grid)`, then `add(NavRobotSpec)` and `remove(id)`.
- `set_goal(id, NavGoal)`, `set_home(id, x, z)`, `set_priority(id, p)`.
- `step(dt)`: fixed step, deterministic.
- `pose(id) -> NavPose` and `path(id)`: the robot's pose and its planned path; `lift_state(lift)`, `lift_mut(id)`.
- `audit()`, `debug_cell(x, z)`, `take_events()`.

**Web boundary:** the engine keeps the `NavWorld` in Rust memory and draws it; the page gets labels and hit results
back through `apz-engine-web` (`label_positions`, `screen_of`, `on_pick`), not robot poses. The layout is computed in
a web worker that runs the same wasm (`startLayoutWorker` in `apps/web/src/views/engine-mount.ts`) and posted back to the engine.

Superseded: an earlier plan had the web hold the `World` and read robot poses through a `Float32Array` over WASM
memory each frame. That is not how it is built.

**Determinism:** the fixed step, a seeded RNG and no hash-map iteration order in outputs make the same input
give the same output on both targets. Golden fixtures (`assets/nav/fixtures.json` and `crates/apz-board/tests/fixtures/boards.json`)
are generated by the crate and checked on both sides.

## Migration

1. Build the crate with its own test suite. It ports the logic from the TS sources and folds in the walk-surface
   rules, and it is validated by the audits rather than by matching the old TS output byte for byte. The old
   output had the bugs the owner reported. (Done.)
2. The web swaps `board-layout.ts`, `nav-*.ts`, `ground.ts` and `lift-ctrl.ts` for the WASM module behind the
   same call sites (done: the web loads this crate through `apz-engine-web`; the TS files and `floor3d/board-core.ts` are
   deleted). The renderers consume `Layout` and robot poses.
3. The desktop swaps `world/board_layout.rs`, `layout.rs` and `motion.rs` for the crate. (Done: the desktop links
   apz-board directly, and it draws through `apz-engine`, whose `world/` modules are the consumers.)
4. The old TS and Rust copies are deleted. The web test suite keeps its rendering-side tests and calls the crate
   for logic; the logic tests live in the crate (`cargo test -p apz-board`). (Done.)
5. Web build: `tools/apz-pack` builds `crates/apz-engine-web` for `wasm32-unknown-unknown`, binds it with
   wasm-bindgen and writes the fingerprinted glue and `.wasm` to `dist` (`apps/web/build.mjs` calls it). The first
   size target, under 400 KB gzipped for the board alone, is superseded: the whole engine is 1.24 MB brotli over
   the wire, and its budget is in `engine-core.md`.
