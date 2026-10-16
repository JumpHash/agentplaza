//! The asset registry: CPU-side mesh and material data, built once and cached by key. An entity holds a typed
//! [`Handle`] (a `u32` index), never the data itself. Nothing here touches the GPU; the renderer slice uploads
//! these. See `docs/design/engine-core.md`.

use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

use bevy_ecs::prelude::Resource;
use bytemuck::{Pod, Zeroable};

/// A typed index into an [`Assets<T>`]. Copyable and comparable whatever `T` is.
pub struct Handle<T> {
    index: u32,
    marker: PhantomData<fn() -> T>,
}

impl<T> Handle<T> {
    /// The position in the owning store.
    pub fn index(self) -> u32 {
        self.index
    }
}

impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Handle<T> {}

impl<T> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
    }
}

impl<T> Eq for Handle<T> {}

impl<T> Hash for Handle<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.index.hash(state);
    }
}

impl<T> fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Handle({})", self.index)
    }
}

/// A store of `T`, addressed by [`Handle`] and deduplicated by a key string. Items are never removed, so a
/// handle stays valid for the life of the store.
pub struct Assets<T> {
    items: Vec<T>,
    by_key: HashMap<String, u32>,
}

impl<T> Default for Assets<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            by_key: HashMap::new(),
        }
    }
}

impl<T> Assets<T> {
    /// The handle for `key`. `make` runs only the first time the key is seen; later calls with the same key
    /// return the first item, and `make` is not called.
    pub fn get_or_insert_with(&mut self, key: &str, make: impl FnOnce() -> T) -> Handle<T> {
        if let Some(&index) = self.by_key.get(key) {
            return Handle {
                index,
                marker: PhantomData,
            };
        }
        let index = u32::try_from(self.items.len()).expect("fewer than 2^32 assets");
        self.items.push(make());
        self.by_key.insert(key.to_owned(), index);
        Handle {
            index,
            marker: PhantomData,
        }
    }

    /// The handle already registered under `key`, if any.
    pub fn handle_for(&self, key: &str) -> Option<Handle<T>> {
        self.by_key.get(key).map(|&index| Handle {
            index,
            marker: PhantomData,
        })
    }

    pub fn get(&self, handle: Handle<T>) -> Option<&T> {
        self.items.get(handle.index as usize)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// One vertex in the layout `render.rs` uploads: position, normal, uv, colour, and the robot slot (0 unchanged,
/// 1 shell, 2 shell2, 3 panel, 4 glass). 48 bytes, no padding. `render.rs` keeps its own private copy until the
/// renderer slice moves it here.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct MeshVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 3],
    pub slot: u32,
}

/// CPU mesh data: indexed triangles.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshAsset {
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
}

impl MeshAsset {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// True when the indices are whole triangles and each one names a vertex.
    pub fn is_valid(&self) -> bool {
        self.indices.len().is_multiple_of(3)
            && self
                .indices
                .iter()
                .all(|&i| (i as usize) < self.vertices.len())
    }
}

/// Material flags, one bit each.
pub mod material_flags {
    pub const TRANSPARENT: u32 = 1;
    pub const UNLIT: u32 = 2;
    pub const DOUBLE_SIDED: u32 = 4;
}

/// CPU material data: a key into the palette, and flags from [`material_flags`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MaterialAsset {
    pub palette: String,
    pub flags: u32,
}

/// Every asset store the engine keeps, as one world resource.
#[derive(Resource, Default)]
pub struct AssetRegistry {
    pub meshes: Assets<MeshAsset>,
    pub materials: Assets<MaterialAsset>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_layout_matches_the_renderer() {
        assert_eq!(std::mem::size_of::<MeshVertex>(), 48);
    }
}
