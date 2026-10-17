//! A scene of named nodes and materials, written as one glTF binary (`.glb`). The output is deterministic: the same
//! scene always gives the same bytes.

use crate::mesh::Mesh;
use serde_json::{json, Value};

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

const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;
const UNSIGNED_INT: u32 = 5125;
const FLOAT: u32 = 5126;
const GLB_MAGIC: u32 = 0x4654_6C67;
const JSON_CHUNK: u32 = 0x4E4F_534A;
const BIN_CHUNK: u32 = 0x004E_4942;

/// Writes `scene` as a glTF 2.0 binary: a JSON chunk (asset version "2.0", generator "apz-kit") and one BIN chunk.
/// Each node's mesh is one primitive with POSITION (with min and max), NORMAL, TEXCOORD_0 and u32 indices; buffer
/// views are 4-byte aligned; chunks are padded (JSON with spaces, BIN with zeros). Node and material names and extras
/// are kept.
pub fn write_glb(scene: &Scene) -> Vec<u8> {
    let mut builder = Builder::default();
    let roots: Vec<usize> = scene.roots.iter().map(|root| builder.node(root)).collect();
    let materials: Vec<Value> = scene.materials.iter().map(material_json).collect();

    let mut json = json!({
        "asset": { "version": "2.0", "generator": "apz-kit" },
        "scene": 0,
        "scenes": [{ "name": scene.name, "nodes": roots, "extras": scene.extras }],
        "buffers": [{ "byteLength": builder.bin.len() }],
    });
    set_list(&mut json, "nodes", builder.nodes);
    set_list(&mut json, "meshes", builder.meshes);
    set_list(&mut json, "materials", materials);
    set_list(&mut json, "accessors", builder.accessors);
    set_list(&mut json, "bufferViews", builder.views);

    let mut json_bytes = serde_json::to_vec(&json).expect("a JSON value serializes");
    pad(&mut json_bytes, b' ');
    let mut bin = builder.bin;
    pad(&mut bin, 0);

    let bin_len = if bin.is_empty() { 0 } else { 8 + bin.len() };
    let total = 12 + 8 + json_bytes.len() + bin_len;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&GLB_MAGIC.to_le_bytes());
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&chunk_len(total).to_le_bytes());
    out.extend_from_slice(&chunk_len(json_bytes.len()).to_le_bytes());
    out.extend_from_slice(&JSON_CHUNK.to_le_bytes());
    out.extend_from_slice(&json_bytes);
    if !bin.is_empty() {
        out.extend_from_slice(&chunk_len(bin.len()).to_le_bytes());
        out.extend_from_slice(&BIN_CHUNK.to_le_bytes());
        out.extend_from_slice(&bin);
    }
    out
}

/// The growing parts of the glTF document. Nodes are numbered depth-first, roots in order, children in order.
#[derive(Default)]
struct Builder {
    nodes: Vec<Value>,
    meshes: Vec<Value>,
    accessors: Vec<Value>,
    views: Vec<Value>,
    bin: Vec<u8>,
}

impl Builder {
    fn node(&mut self, node: &Node) -> usize {
        let index = self.nodes.len();
        self.nodes.push(Value::Null);
        let mut json = json!({
            "name": node.name,
            "translation": node.translation,
            "rotation": node.rotation,
            "scale": node.scale,
        });
        if let Some((mesh, material)) = &node.mesh {
            json["mesh"] = json!(self.mesh(mesh, *material));
        }
        if !node.children.is_empty() {
            let children: Vec<usize> = node.children.iter().map(|child| self.node(child)).collect();
            json["children"] = json!(children);
        }
        if let Some(extras) = &node.extras {
            json["extras"] = extras.clone();
        }
        self.nodes[index] = json;
        index
    }

    fn mesh(&mut self, mesh: &Mesh, material: usize) -> usize {
        let position = self.attribute(&f32_bytes(&mesh.positions), ARRAY_BUFFER, FLOAT, "VEC3", mesh.positions.len());
        let (min, max) = bounds(&mesh.positions);
        self.accessors[position]["min"] = json!(min);
        self.accessors[position]["max"] = json!(max);
        let normal = self.attribute(&f32_bytes(&mesh.normals), ARRAY_BUFFER, FLOAT, "VEC3", mesh.normals.len());
        let uv = self.attribute(&f32_bytes(&mesh.uvs), ARRAY_BUFFER, FLOAT, "VEC2", mesh.uvs.len());
        let indices = self.attribute(
            &mesh.indices.iter().flat_map(|i| i.to_le_bytes()).collect::<Vec<u8>>(),
            ELEMENT_ARRAY_BUFFER,
            UNSIGNED_INT,
            "SCALAR",
            mesh.indices.len(),
        );
        self.meshes.push(json!({
            "primitives": [{
                "attributes": { "POSITION": position, "NORMAL": normal, "TEXCOORD_0": uv },
                "indices": indices,
                "material": material,
            }]
        }));
        self.meshes.len() - 1
    }

    /// Appends `bytes` as a buffer view (4-byte aligned: every element is 4 bytes) and an accessor over it.
    fn attribute(&mut self, bytes: &[u8], target: u32, component: u32, kind: &str, count: usize) -> usize {
        let offset = self.bin.len();
        self.bin.extend_from_slice(bytes);
        self.views.push(json!({
            "buffer": 0,
            "byteOffset": offset,
            "byteLength": bytes.len(),
            "target": target,
        }));
        self.accessors.push(json!({
            "bufferView": self.views.len() - 1,
            "componentType": component,
            "count": count,
            "type": kind,
        }));
        self.accessors.len() - 1
    }
}

fn material_json(material: &Material) -> Value {
    let mut json = json!({
        "name": material.name,
        "pbrMetallicRoughness": {
            "baseColorFactor": material.base,
            "metallicFactor": material.metallic,
            "roughnessFactor": material.roughness,
        },
        "emissiveFactor": material.emissive,
        "doubleSided": material.double_sided,
        "alphaMode": if material.transparent { "BLEND" } else { "OPAQUE" },
    });
    if let Some(texture) = &material.texture {
        json["extras"] = json!({ "texture": texture });
    }
    json
}

/// glTF forbids empty arrays, so a list with nothing in it is left out of the document.
fn set_list(json: &mut Value, key: &str, items: Vec<Value>) {
    if !items.is_empty() {
        json[key] = Value::Array(items);
    }
}

fn f32_bytes<const N: usize>(items: &[[f32; N]]) -> Vec<u8> {
    items.iter().flatten().flat_map(|f| f.to_le_bytes()).collect()
}

/// The (min, max) corners of `points`; zeros when there are none (glTF needs finite bounds).
fn bounds(points: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    if points.is_empty() {
        return ([0.0; 3], [0.0; 3]);
    }
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for p in points {
        for ((lo, hi), v) in min.iter_mut().zip(max.iter_mut()).zip(p) {
            *lo = lo.min(*v);
            *hi = hi.max(*v);
        }
    }
    (min, max)
}

fn pad(bytes: &mut Vec<u8>, fill: u8) {
    while !bytes.len().is_multiple_of(4) {
        bytes.push(fill);
    }
}

fn chunk_len(n: usize) -> u32 {
    u32::try_from(n).expect("a glb under 4 GiB")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triangle() -> Mesh {
        Mesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            indices: vec![0, 1, 2],
        }
    }

    fn scene() -> Scene {
        let ground = Material {
            name: "ground".into(),
            base: [0.2, 0.2, 0.2, 1.0],
            emissive: [0.0; 3],
            metallic: 0.0,
            roughness: 0.8,
            texture: None,
            double_sided: false,
            transparent: false,
        };
        let glass = Material {
            name: "glass".into(),
            base: [0.5, 0.8, 1.0, 0.3],
            emissive: [0.0, 0.0, 0.1],
            metallic: 0.0,
            roughness: 0.1,
            texture: Some("glass.ktx2".into()),
            double_sided: true,
            transparent: true,
        };
        let mut panel = Node::mesh("panel", triangle(), 1).at(0.0, 1.0, 0.0);
        panel.extras = Some(json!({ "id": 7 }));
        Scene {
            name: "machine".into(),
            materials: vec![ground, glass],
            roots: vec![Node::mesh("base", triangle(), 0).with(panel)],
            extras: json!({ "kit": "test" }),
        }
    }

    #[test]
    fn glb_keeps_names_hierarchy_materials_and_accessors() {
        let bytes = write_glb(&scene());
        assert_eq!(&bytes[0..4], b"glTF");

        let gltf = gltf::Gltf::from_slice(&bytes).unwrap();
        let doc = &gltf.document;
        let roots: Vec<_> = doc.default_scene().unwrap().nodes().collect();
        assert_eq!(roots.len(), 1);
        let base = &roots[0];
        assert_eq!(base.name(), Some("base"));

        let children: Vec<_> = base.children().collect();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].name(), Some("panel"));
        let node_extras: serde_json::Value =
            serde_json::from_str(children[0].extras().as_ref().unwrap().get()).unwrap();
        assert_eq!(node_extras["id"], 7);

        let materials: Vec<_> = doc.materials().collect();
        assert_eq!(materials[0].name(), Some("ground"));
        assert_eq!(materials[1].name(), Some("glass"));
        assert!(materials[0].extras().is_none());
        let glass_extras: serde_json::Value =
            serde_json::from_str(materials[1].extras().as_ref().unwrap().get()).unwrap();
        assert_eq!(glass_extras["texture"], "glass.ktx2");

        let prim = base.mesh().unwrap().primitives().next().unwrap();
        assert_eq!(prim.get(&gltf::Semantic::Positions).unwrap().count(), 3);
        assert_eq!(prim.material().index(), Some(0));
        let child_prim = children[0].mesh().unwrap().primitives().next().unwrap();
        assert_eq!(child_prim.material().index(), Some(1));
    }

    #[test]
    fn writing_twice_gives_identical_bytes() {
        assert_eq!(write_glb(&scene()), write_glb(&scene()));
    }
}
