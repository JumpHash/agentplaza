//! The Floor in 3D: one platform-agnostic wgpu engine, for the desktop (through iced's shader widget, in
//! `apps/desktop`) and the browser (WebAssembly). It never creates a window or a surface: the host hands it a
//! `wgpu::Device`, a `wgpu::Queue` and a target. `scene` is the three.js-like scene graph the ports build,
//! `render` draws it, `world` is the floor itself (districts, robots, walking, packets, labels), and `stage`
//! is what the Floor screen shows and how it moves (the machines view, a board, ENTER).
//! See `README.md` for the module map and the host contract.
//!
//! Module ↔ web source: `geometry` ↔ three's geometries; `canvas` ↔ the 2D canvas the textures
//! paint on; `textures` ↔ `floor3d/textures.ts`; `toon` ↔ `floor3d/toon.ts`; `robot` ↔
//! `floor3d/robot.ts`; `props` ↔ `floor3d/props.ts`; `outskirts` ↔ `floor3d/outskirts.ts`; `rooms` ↔
//! `floor3d/rooms/*`; `controls` ↔ three's OrbitControls; `world` ↔ `floor.ts`.

pub mod assets;
pub mod canvas;
pub mod controls;
pub mod core;
pub mod debug;
pub mod engine;
pub mod entry;
pub mod entry_scene;
pub mod fleet;
pub mod gpus;
pub mod geometry;
pub mod hardware;
pub mod input;
pub use apz_board::nav::lift as lift_ctrl;
pub mod machine_stage;
pub mod outskirts;
pub mod portrait;
pub mod props;
pub mod render;
pub mod robot;
pub mod robots;
pub mod roles;
pub mod rooms;
pub mod scene;
pub mod stage;
pub mod textures;
pub mod toon;
pub mod warm;
pub mod world;

pub use engine::{agent_list, feed_from_json, Cmd, Engine, Feed, FrameOut, Pick, Renderer, Thumbnail};
pub use stage::Motion;

/// The env overrides a host sets (the web's robot lab passes its options as `APZ_ROBOT_*`). They read before the real env.
static ENV_OVERRIDES: std::sync::Mutex<std::collections::BTreeMap<String, String>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Replaces the env overrides; an empty map clears them. A browser has no environment, so this is how the web sets knobs.
pub fn set_env_overrides(map: std::collections::BTreeMap<String, String>) {
    if let Ok(mut m) = ENV_OVERRIDES.lock() {
        *m = map;
    }
}

/// An `APZ_*` capture or debug knob from the environment. A browser has no environment, so every knob reads as
/// unset there (the defaults apply), unless the host set it with `set_env_overrides`.
pub(crate) fn env_var(name: &str) -> Option<String> {
    if let Some(v) = ENV_OVERRIDES.lock().ok().and_then(|m| m.get(name).cloned()) {
        return Some(v);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var(name).ok()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = name;
        None
    }
}
