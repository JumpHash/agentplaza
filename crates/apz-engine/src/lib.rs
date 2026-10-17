//! The machines in 3D: one platform-agnostic wgpu engine, for the desktop (through iced's shader widget, in
//! `apps/desktop`) and the browser (WebAssembly). It never creates a window or a surface: the host hands it a
//! `wgpu::Device`, a `wgpu::Queue` and a target. `scene` is the three.js-like scene graph the ports build,
//! `render` draws it, `world` is the scene the Machines view stands in (sky, ground, camera, machine labels),
//! `fleet` is the Machines view itself, `machine_stage` the one-machine stage, and `stage` is what the screen shows
//! and how it moves. See `README.md` for the module map and the host contract.
//!
//! Module ↔ web source: `geometry` ↔ three's geometries; `canvas` ↔ the 2D canvas the textures
//! paint on; `textures` ↔ `floor3d/textures.ts`; `toon` ↔ `floor3d/toon.ts`; `robot` ↔
//! `floor3d/robot.ts` (agent portraits and the robot lab); `props` ↔ `floor3d/props.ts`; `controls` ↔ three's
//! OrbitControls.

pub mod assets;
pub mod canvas;
pub mod controls;
pub mod core;
pub mod debug;
pub mod engine;
pub mod fleet;
pub mod geometry;
pub mod gpus;
pub mod hardware_lab;
pub mod input;
pub mod kit;
pub mod ktx;
pub mod lab_catalog;
pub mod machine_stage;
pub mod portrait;
pub mod props;
pub mod render;
pub mod robot;
pub mod robots;
pub mod roles;
pub mod scene;
pub mod stage;
pub mod style;
pub mod textures;
pub mod toon;
pub mod world;

pub use engine::{
    Cmd, Engine, Feed, FrameOut, Pick, Renderer, Thumbnail, agent_list, feed_from_json,
};
pub use stage::Motion;

/// The env overrides a host sets (the web's robot lab passes its options as `APZ_ROBOT_*`). They read before the real env.
#[cfg(not(test))]
static ENV_OVERRIDES: std::sync::Mutex<std::collections::BTreeMap<String, String>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

// In tests the overrides are per thread: a test that sets them (a lab's) runs beside tests that build a Stage.
#[cfg(test)]
thread_local! {
    static TEST_OVERRIDES: std::cell::RefCell<std::collections::BTreeMap<String, String>> = Default::default();
}

/// Replaces the env overrides; an empty map clears them. A browser has no environment, so this is how the web sets knobs.
pub fn set_env_overrides(map: std::collections::BTreeMap<String, String>) {
    #[cfg(not(test))]
    if let Ok(mut m) = ENV_OVERRIDES.lock() {
        *m = map;
    }
    #[cfg(test)]
    TEST_OVERRIDES.with(|m| *m.borrow_mut() = map);
}

/// The override for `name`, if the host set one.
fn env_override(name: &str) -> Option<String> {
    #[cfg(not(test))]
    return ENV_OVERRIDES.lock().ok().and_then(|m| m.get(name).cloned());
    #[cfg(test)]
    return TEST_OVERRIDES.with(|m| m.borrow().get(name).cloned());
}

/// An `APZ_*` capture or debug knob from the environment. A browser has no environment, so every knob reads as
/// unset there (the defaults apply), unless the host set it with `set_env_overrides`.
pub(crate) fn env_var(name: &str) -> Option<String> {
    if let Some(v) = env_override(name) {
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
