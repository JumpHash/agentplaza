//! Plain components: data only. The systems that read and write them live in `core/mod.rs`.

use bevy_ecs::prelude::{Component, Entity};
use glam::{Affine3A, Quat, Vec3};

use crate::assets::{Handle, MaterialAsset, MeshAsset};

/// A local transform: translation, rotation and scale, relative to the parent (or the world, with no parent).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            translation: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }
}

impl Transform {
    pub fn from_translation(translation: Vec3) -> Self {
        Self {
            translation,
            ..Self::default()
        }
    }

    /// The local matrix: scale, then rotate, then translate.
    pub fn matrix(&self) -> Affine3A {
        Affine3A::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

/// The world matrix. The RenderPrep propagation writes it; `EngineWorld::spawn_node` and `spawn_child` add it.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct GlobalTransform(pub Affine3A);

impl Default for GlobalTransform {
    fn default() -> Self {
        Self(Affine3A::IDENTITY)
    }
}

/// The parent in the transform hierarchy. An entity without it is a root.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parent(pub Entity);

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshRef(pub Handle<MeshAsset>);

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaterialRef(pub Handle<MaterialAsset>);

/// Drawn. An entity without it is skipped by the renderer slice.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Visible;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickKind {
    Machine,
    Part,
    Robot,
    Room,
    Light,
    Label,
}

/// What a pointer over this entity selects.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pickable {
    pub kind: PickKind,
    pub id: u32,
}

/// A clock-driven oscillation: a pure function of time, so it can be drawn at any rate.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Pulse {
    pub phase: f32,
    pub rate: f32,
}

/// Marks something in motion (a tween, a walking robot, a flight). Any `Active` entity makes [`Motion::Active`].
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Active;

/// Marks clock-driven decoration (stars, pulses, fans, packets). With no `Active` entity it makes
/// [`Motion::Ambient`], unless reduced motion is on.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ambient;
