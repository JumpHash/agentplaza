# apz-engine

One 3D engine for AgentPlaza's Floor, on wgpu, for the desktop app and the browser. It has no window and no
surface of its own: the host gives it a `wgpu::Device`, a `wgpu::Queue`, a target format and a target
`TextureView`. The desktop reaches it through iced's shader widget (`apps/desktop`); the browser will reach it
through a canvas (Phase 2, `apz-engine-web`).

Status: Phase 1 (extract). The desktop's Floor renders through this crate and its snapshots match the pre-move
build pixel for pixel. The browser shell is not built yet.

## Module map

| Module | What it holds |
|---|---|
| `engine` | The host contract: `Engine`, `Feed`, `Cmd`, `FrameOut` (below). |
| `input` | The pointer state machine: `input::Event` in, `input::Action` out (orbit, pan, dolly, hover, select, resize). |
| `stage` | The Floor's state and its transitions: `View` (Machines, a machine's board, Plaza), `Stage`, ENTER and Back, the capture hooks, the perf log lines. |
| `world` | The floor itself: `Floor` (districts, robots, walking, lifts), the board layout and motion, the behaviour engine, the view's overlays, labels and pick. |
| `render` | The wgpu pipelines and the frame's passes: `Pipeline::new`, `Primitive::prepare`, `Primitive::draw`, `Rect`. |
| `scene` | The scene graph the ports build: `Frame`, `Draw`, `Material`, `Geometry`, `Camera`, `World`. |
| `fleet` | The Machines view's scene and the fleet snapshot (`FleetResponse`, `FleetScene`). |
| `hardware` | The machine models (exterior, board, parts), built from the embedded JSON. |
| `robot`, `robots` | Robot rigs, clips, the species and tool registry, validation, and the robot lab (`APZ_ROBOT_LAB`). |
| `rooms` | Room generation, the kit, the audits, and the room lab (`APZ_ROOM_LAB`). |
| `entry`, `entry_scene` | The ENTER flight: its timing and its scene. |
| `roles` | A role's family and label (the Floor's grouping and plaques). |
| `geometry`, `canvas`, `textures`, `toon`, `props`, `outskirts`, `controls` | Meshes, the 2D canvas the textures paint on, the toon shading, props, the outskirts, and the orbit camera (`Orbit`). |

Several modules carry notes naming the web's three.js source they were ported from (`floor3d/robot.ts` and the
like). That code is retired (the engine is the web's renderer now); the notes stay as provenance, and the files are in
the git history before the three.js removal.

## Host contract

```rust
let mut engine = Engine::new(&device, &queue, format);   // assets are embedded: nothing else to pass
engine.set_data(Feed { fleet: Some(snapshot), agents: Some(list), fleet_error: None });
let captured = engine.input(input::Event::Pressed { button, at });   // pointer and wheel, logical px
engine.command(Cmd::Enter("n0".into()));                          // or ShowMachines, SelectMachine, Back, ResetView, OpenPopup
let out = engine.frame(dt, &target_view, (width_px, height_px), scale);  // FrameOut
```

- `set_data(Feed)`: each source applies only when it is `Some`. `Feed` holds the fleet snapshot, the agents
  list, and the last fleet error.
- `input(Event) -> bool`: the pointer state machine (`input::Pointer`) turns the event into an `Action` and
  applies it to the stage. The return value says whether the host should capture the event.
- `command(Cmd)`: the Floor screen's controls. `Enter` starts the flight, `Back` reverses it.
- `frame(dt, target, size_px, scale) -> FrameOut`: advances the stage by `dt`, renders into `target` (cleared to
  transparent first), and returns `labels` (in logical px), `camera`, `needs_redraw` and `pick`.
- Known gaps in this phase: `needs_redraw` is always `true` (no idle skipping yet), and `pick` is always `None`.
  Both are Phase 2 work, with the performance budget (no frames while idle).

The desktop does not call `Engine` yet. Its screen (`apps/desktop/src/console/screens/floor.rs`) holds a
`State` that derefs to `Stage`, and its iced adapter (`apps/desktop/src/console/floor_engine.rs`) turns iced's
shader traits into `render::Pipeline` and `input::Pointer` calls. Switching the desktop to `Engine` is a
Phase 2 step.

## Assets

Every asset is compiled in: JSON and WGSL with `include_str!`, fonts with `include_bytes!`, all from the repo's
`assets/` (and `floor.wgsl` from this crate). `build.rs` lists the hardware models and the robot files into
`OUT_DIR`, so a new model or robot file needs no Rust edit. Nothing is read from disk at runtime, except the
desktop's system-font lookup, which is native-only.

## wasm32

- `cargo build -p apz-engine --target wasm32-unknown-unknown` builds with no warnings.
- wgpu uses `webgpu` and `webgl` on `wasm32` (WebGPU, with WebGL2 as the fallback).
- `Instant` is `web_time::Instant` (the browser has no `std::time::Instant::now`).
- APZ_* knobs read through `crate::env_var`, which reports every knob unset on `wasm32`.
- The system-font search is native-only; the browser uses the bundled face.

## Tests

```sh
cargo test -p apz-engine                              # 150 pass, 4 ignored
cargo build -p apz-engine --target wasm32-unknown-unknown
cargo test -p agentplaza-desktop                      # the adapter, the screens and the UI
```

The four ignored tests are opt-in: the board parity test (superseded by `apz-board`, owner decision
2026-10-13), two web-dump checks that need `APZ_ROOM_DUMP=<web dump>`, and the slow computed-minimum-size check
in `rooms::gen`.

## Snapshots (the desktop parity check)

The app captures its own frame; never grab the screen. One snapshot at a time, against the one shared stand-in
hub:

```sh
APZ_SNAPSHOT=/path/machines.png APZ_CONNECT=http://127.0.0.1:7791 APZ_SCREEN=Floor target/debug/agentplaza-desktop
APZ_SNAPSHOT=/path/board.png APZ_CONNECT=http://127.0.0.1:7791 APZ_SCREEN=Floor APZ_FLOOR_MACHINE=n0 target/debug/agentplaza-desktop
```

Compare the decoded pixels, not the PNG bytes (the encoder's output is not stable across runs). Both views
were pixel-identical before and after the Phase 1 extract.
