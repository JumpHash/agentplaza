# apz-engine

One 3D engine for AgentPlaza's machines, on wgpu, for the desktop app and the browser. It has no window and no
surface of its own: the host gives it a `wgpu::Device`, a `wgpu::Queue`, a target format and a target
`TextureView`. The desktop reaches it through iced's shader widget (`apps/desktop`); the browser reaches it through
`apz-engine-web`, a wasm-bindgen crate that puts the engine on one canvas. `apps/web/build.mjs` builds that crate
through `tools/apz-pack`.

Status: the desktop's and the web's Machines view and one-machine stage both render through this crate. Without
WebGPU and WebGL2 the web shows an empty state and points to the machine list.

## Module map

| Module | What it holds |
|---|---|
| `engine` | The host contract: `Engine`, `Feed`, `Cmd`, `FrameOut` (below). |
| `input` | The pointer state machine: `input::Event` in, `input::Action` out (orbit, pan, dolly, hover, select, resize). |
| `stage` | The screen's state: `Stage` (the Machines view, the one-machine stage, the labs), the camera's moves, the perf log lines. |
| `world` | The scene the Machines view stands in: `Floor` (sky, stars, ground, dust, the orbit, the world's light, the baked statics) and `tags` (the machines' name tags). |
| `render` | The wgpu pipelines and the frame's passes: `Pipeline::new`, `Primitive::prepare`, `Primitive::draw`, `Rect`. |
| `scene` | The scene graph the ports build: `Frame`, `Draw`, `Material`, `Geometry`, `Camera`, `World`. |
| `fleet` | The Machines view's scene and the fleet snapshot (`FleetResponse`, `FleetScene`). |
| `hardware` | The machine models (exteriors and parts), built from the embedded JSON. |
| `robot`, `robots` | Robot rigs, clips, the species and tool registry, validation, and the robot lab (`APZ_ROBOT_LAB`). |
| `roles` | A role's family and label. |
| `geometry`, `canvas`, `textures`, `toon`, `props`, `controls` | Meshes, the 2D canvas the textures paint on, the toon shading, props, and the orbit camera (`Orbit`). |
| `machine_stage`, `portrait`, `gpus`, `assets`, `core`, `debug`, `hardware_lab`, `lab_catalog` | One machine's exterior on the studio stage, an agent's robot portrait, the graphics-card specs, the asset registry (`Handle`s), the `bevy_ecs` engine core (nothing uses it yet), the debug hooks (view, camera, scene graph), the hardware lab (`APZ_HW_LAB`), and the labs' option lists. |

Several modules carry notes naming the three.js floor they were ported from (`apps/web/src/views/floor3d/robot.ts` and
the like). That code is deleted (the engine is the web's 3D now, since commit 987394f); the notes stay as provenance,
and the files are in the git history before that commit.

## Host contract

```rust
let mut engine = Engine::new(&device, &queue, format);   // assets are embedded: nothing else to pass
engine.set_data(Feed { fleet: Some(snapshot), agents: Some(list), fleet_error: None });
let captured = engine.input(input::Event::Pressed { button, at });   // pointer and wheel, logical px
engine.command(Cmd::ShowMachine { id: "n0".into() });            // or LeaveMachine, ResetView, OpenPopup
let out = engine.frame(now, &target_view, (width_px, height_px), scale);  // FrameOut
```

- `set_data(Feed)`: each source applies only when it is `Some`. `Feed` holds the fleet snapshot, the agents
  list (the hosts' machine popups name them), and the last fleet error.
- `input(Event) -> bool`: the pointer state machine (`input::Pointer`) turns the event into an `Action` and
  applies it to the stage. The return value says whether the host should capture the event.
- `command(Cmd)`: the screen's controls. `ShowMachine` puts one machine on the studio stage, `LeaveMachine` returns to
  the Machines view, `OpenPopup` opens or closes a machine's popup, `ResetView` frames the view afresh.
- `frame(now, target, size_px, scale) -> FrameOut`: advances the stage to `now` (an `Instant`), renders into `target`
  (cleared to transparent first), and returns `motion` (`Active`, `Ambient` throttled to 12 fps, or `Rest`: how soon
  the host should draw again) and `camera`.
- `pick(at) -> Option<Pick>`: the machine (`Pick::Machine`) under a logical px point. `busy()` says whether the engine
  needs another frame.

The desktop's Floor screen (`apps/desktop/src/console/screens/floor.rs`) holds a `State` of an `Engine` (built with
`Engine::detached()`) and the motion of the last frame, and the `State` derefs to `Stage`. Its iced adapter
(`apps/desktop/src/console/floor_engine.rs`) turns iced's shader traits into `Renderer` and `input::Pointer` calls.

## Assets

Every asset is compiled in: JSON with `include_str!` and the fonts with `include_bytes!` (Martian Mono, from the
repo's `assets/`), and the WGSL shaders from this crate (`src/floor.wgsl`, `src/shaders/`). `build.rs` lists the
hardware models and the robot files into `OUT_DIR`, so a new model or robot file needs no Rust edit. Nothing is read from disk at runtime, except the
desktop's system-font lookup, which is native-only.

## wasm32

- `cargo build -p apz-engine --target wasm32-unknown-unknown` builds with no warnings.
- wgpu uses `webgpu` and `webgl` on `wasm32` (WebGPU, with WebGL2 as the fallback).
- `Instant` is `web_time::Instant` (the browser has no `std::time::Instant::now`).
- APZ_* knobs read through `crate::env_var`, which reports every knob unset on `wasm32` unless the host sets them with
  `set_env_overrides` (`apz-engine-web` does, from the page).
- The system-font search is native-only; the browser uses the bundled face.

## Tests

```sh
cargo test -p apz-engine
cargo build -p apz-engine --target wasm32-unknown-unknown
cargo test -p agentplaza-desktop                      # the adapter, the screens and the UI
```

## Snapshots (the desktop parity check)

The app captures its own frame; never grab the screen. One snapshot at a time, against the one shared stand-in
hub:

```sh
APZ_SNAPSHOT=/path/machines.png APZ_CONNECT=http://127.0.0.1:7791 APZ_SCREEN=Floor target/debug/agentplaza-desktop
```

Compare the decoded pixels, not the PNG bytes (the encoder's output is not stable across runs).
