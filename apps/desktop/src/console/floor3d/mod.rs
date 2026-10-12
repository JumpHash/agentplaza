//! The Floor in 3D, native: a 1:1 port of the web console's floor (`apps/web/src/views/floor.ts`
//! and `floor3d/*`) onto wgpu. `scene` is the three.js-like scene graph the ports build,
//! `render` draws it inside iced's shader widget, and `world` is the floor itself (districts,
//! robots, walking, packets, labels), driven by the console's agent list.
//!
//! Module ↔ web source: `geometry` ↔ three's geometries; `canvas` ↔ the 2D canvas the textures
//! paint on; `textures` ↔ `floor3d/textures.ts`; `toon` ↔ `floor3d/toon.ts`; `robot` ↔
//! `floor3d/robot.ts`; `props` ↔ `floor3d/props.ts`; `rooms` ↔ `floor3d/rooms/*`; `controls` ↔
//! three's OrbitControls; `world` ↔ `floor.ts`.

pub mod canvas;
pub mod controls;
pub mod geometry;
pub mod props;
pub mod render;
pub mod robot;
pub mod rooms;
pub mod scene;
pub mod textures;
pub mod toon;
pub mod world;
