//! The rooms (floor3d/rooms on the web): the generator lays out a role's furniture in its room (`generate`, gen.ts) and the
//! verifier checks a layout (`verify`, verify.ts). They are pure data, so the layout can check its rooms with their
//! furniture in place, as the web's does; the engine builds the meshes from the same layouts.

pub mod generate;
pub mod verify;
