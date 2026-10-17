//! A scene of named nodes and materials, written as one glTF binary (`.glb`). The output is deterministic: the same
//! scene always gives the same bytes.

use crate::mesh::Mesh;

/// A material the engine finds by `name`. `base` is linear RGBA, `emissive` linear RGB. `texture` names a KTX2 file
/// beside the `.glb` (it goes in the material's extras as `{"texture": name}`: glTF's own image slots take PNG or
/// JPEG, and KHR_texture_basisu only Basis payloads).
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    pub name: String,
    pub base: [f32; 4],
    pub emissive: [f32; 3],
    pub metallic: f32,
    pub roughness: f32,
    pub texture: Option<String>,
    pub double_sided: bool,
    /// Blend mode BLEND when `base[3]` < 1.
    pub transparent: bool,
}

/// A node: a name, an optional mesh drawn with `materials[material]`, a transform and children.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Node {
    pub name: String,
    pub mesh: Option<(Mesh, usize)>,
    pub translation: [f32; 3],
    /// A unit quaternion, x y z w.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    pub children: Vec<Node>,
    pub extras: Option<serde_json::Value>,
}

impl Node {
    /// A named node with an identity transform and nothing in it.
    pub fn group(name: &str) -> Node {
        Node { name: name.to_string(), rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3], ..Node::default() }
    }

    /// A named node drawing `mesh` with material index `material`.
    pub fn mesh(name: &str, mesh: Mesh, material: usize) -> Node {
        Node { mesh: Some((mesh, material)), ..Node::group(name) }
    }

    pub fn at(mut self, x: f32, y: f32, z: f32) -> Node {
        self.translation = [x, y, z];
        self
    }

    pub fn with(mut self, child: Node) -> Node {
        self.children.push(child);
        self
    }
}

/// One `.glb`: its materials, its root nodes (one glTF scene) and the scene's extras.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub name: String,
    pub materials: Vec<Material>,
    pub roots: Vec<Node>,
    pub extras: serde_json::Value,
}

/// Writes `scene` as a glTF 2.0 binary: a JSON chunk (asset version "2.0", generator "apz-kit") and one BIN chunk.
/// Each node's mesh is one primitive with POSITION (with min and max), NORMAL, TEXCOORD_0 and u32 indices; buffer
/// views are 4-byte aligned; chunks are padded (JSON with spaces, BIN with zeros). Node and material names and extras
/// are kept.
pub fn write_glb(scene: &Scene) -> Vec<u8> {
    let _ = scene;
    todo!()
}
