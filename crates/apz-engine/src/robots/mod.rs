//! The robots, as the web's `floor3d/robots/`: the data (`types`, `files` with the registry and the
//! build's file list, `validate`), the part geometry (`geometry`), the clips (`clips`), the baked rig
//! (`rig`), the effects (`fx`) and the robot lab (`lab`, `APZ_ROBOT_LAB`). The API is `floor3d::robot`.

pub mod audit;
pub mod clips;
pub mod files;
pub mod fx;
pub mod geometry;
pub mod lab;
pub mod props;
pub mod rig;
pub mod types;
pub mod validate;

#[cfg(test)]
mod tests;
