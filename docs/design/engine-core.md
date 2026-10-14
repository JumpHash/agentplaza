# apz-engine as a game engine

Owner's question (2026-10-13): "Can't you build a rendering/game engine and make it work like that?" Yes. This
document turns `crates/apz-engine` from a renderer with a few hand-wired scenes into a small game engine. It
keeps the host contract from `engine.md`, the same as before: `Engine::new`, `set_data`, `input`, `frame`, `command`.

Status (2026-10-14): step 1 is started, not finished. `920a8d3` added the `bevy_ecs` World with startup, fixed (30 Hz),
update and render-prep schedules, Motion from marker components and the typed asset registry (`crates/apz-engine/src/core/`,
`assets/`). The scenes are not entities yet, and there is no compute: no `@compute` shader, no culling, flow fields or
particles. Steps 2 to 4 are open. Step 5's three.js removal is done (`987394f`); its portrait and lab states are not.

## Why

Today:
- each scene (Machines, the board, the machine stage, the ENTER flight) builds its own meshes, runs its own `tick`,
  and has its own draw path;
- motion and robots are updated by hand in `Stage::tick`;
- draw calls are issued per mesh, which gives about 1000 per frame against a budget of 300;
- switching views rebuilds the GPU state.

A game-engine core fixes all four with one model.

## The shape

```
apz-engine
  core/      World (ECS) + Schedules: startup, fixed (simulation, 30 Hz, deterministic), update (per frame), render-prep
  assets/    Mesh, Material, Texture and Shader handles; built once from the hardware/robot/room JSON, cached by key
  sim/       systems: nav and motion (apz-board World), lifts, behaviour, robot animation clips, fx particles, camera rigs
  render/    a render graph: shadow → opaque → outline → transparent → sprites/labels → post; GPU-driven batching
  scenes/    Machines, Board, MachineStage, Portrait, RobotLab, as *states*: entering a state spawns its entities,
             leaving it despawns them; the renderer never changes
  hosts      apz-engine-web (wasm) and the desktop iced adapter, unchanged in shape
```

**ECS:** `bevy_ecs`, used as a standalone crate.
- It is data-oriented, has a scheduler and change detection, and works on wasm32.
- We do not take the whole of Bevy. It wants to own the window and the event loop, which conflicts with embedding
  in iced and in a DOM page, and it would grow the wasm a lot.
- Entities are machines, parts, robots, rooms, lights, the camera and labels.
- Components are plain data: `Transform`, `MeshRef`, `MaterialRef`, `Machine{id}`, `Robot{cid}`, `Pulse{phase,
  rate}`, `CablePath`, `Fade`, `Pick`, and so on.

**Simulation:**
- Fixed 30 Hz, deterministic. It runs the shared `apz-board` World (nav, motion, lifts) and the behaviour engine.
- Animation is a pure function of time: clips, pulses, packets and the drift camera. So the Ambient tier, which
  renders at 12 fps, still looks right.
- `Motion` (Rest, Ambient or Active) is computed from the World: any Active component means Active. Hosts schedule
  frames from it, as they do now.

**Rendering (GPU-driven):**
- Static meshes are merged per material at spawn time.
- Repeated meshes are drawn instanced: one draw per (mesh, material), with an instance buffer built from the ECS each
  frame. Only changed instances are re-uploaded, using change detection.
- Animated items are instanced and animated in the vertex shader: packets, rings, LEDs, fans, robot idle.
- Culling:
  - **WebGPU and native:** a compute pass does frustum culling, writes a compacted instance list and fills
    `draw_indexed_indirect` arguments.
  - **WebGL2:** it has no compute or indirect drawing, so the CPU culls by bounding sphere and draws directly. The
    path is chosen from the adapter's capabilities.
- Shadows are re-rendered only when a shadow caster's transform changes.
- Every shader is its own `.wgsl` file embedded with `include_str!` (owner rule), and the WebGL2 naga test covers each
  one.

**GPU compute** (owner: "graph theory, discrete maths, linear algebra, vectors… we have wgpu"). Each use has a CPU
path for WebGL2:
- **Flow fields:** many robots heading to one place (a lift, a door, the plaza) share one field. It is computed on the
  GPU by iterative relaxation over the nav grid, with the CPU Dijkstra field as the fallback and as the test oracle.
- **Culling and LOD selection,** as above.
- **Particle effects:** sparks, steam, trails and hearts, simulated in compute.
- **Label projection:** world anchors are projected into the label buffer in one pass.

`apz-board` stays CPU-only and deterministic. Its layout and audits are golden-tested on both hosts.

**Picking:** a GPU ID buffer. The pick pass writes entity ids, and one pixel is read back under the pointer. The CPU
ray test remains the fallback.

## Migration (each step ends green, desktop snapshots equal)

1. Add `bevy_ecs`, a World, schedules, and the asset registry next to the current code. Port the Machines scene to
   entities and the instanced renderer. Done when the draw count is at most 300 and the desktop snapshot is equal.
2. Port the machine stage and thumbnails as a state.
3. Port the board: hardware, rooms, robots and lifts, with `apz-board` driving motion in the fixed schedule, and robot
   clips (walk, jump, fall, land) as components.
4. Add compute culling and indirect draws on WebGPU/native, flow fields, and particles.
5. Port the portrait studio and the robot lab as states. Then the web drops three.js (`engine.md` phase 4; done in `987394f`).

## Budgets (tests)

| What | Limit |
|---|---|
| Machines view draws | 300 or fewer |
| Board draws | 400 or fewer |
| Triangles | 300k or fewer |
| Frames rendered at rest | 0 |
| Ambient frame rate | 12 fps or less |
| Select hitch | 50 ms or less |
| wasm size over the wire | 1.5 MB or less (1.24 MB brotli today; `tools/apz-pack`) |
