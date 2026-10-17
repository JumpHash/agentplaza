//! The machine model kit (`assets/hardware/kit`, written by `tools/apz-kit`): the ten machines as glTF, parsed once
//! from the embedded files and turned into scene nodes in a [`Palette`]'s materials. The kit's contract (the node and
//! material names this reads) is documented in `tools/apz-kit/src/lib.rs`.

use std::sync::OnceLock;

use glam::{Quat, Vec3};

use crate::scene::{Geo, Obj, Tex};
use crate::style::Palette;

// `KIT_FILES`: every `.glb` and `.ktx2` of the kit, by file name (build.rs).
include!(concat!(env!("OUT_DIR"), "/kit_files.rs"));

/// The feature edges' crease angle, degrees.
pub const EDGE_ANGLE: f32 = 30.0;

/// One model of the kit, parsed: its id (the file stem: `hub`, `server`, `rack`, `gpu`, `tower`, `edge`, `mini`,
/// `studio`, `laptop`, `phone`), its kind and size (from the scene's extras) and its three levels of detail.
pub struct KitModel {
    pub id: String,
    pub kind: String,
    /// Width, height and depth in kit units (the base stands on y = 0, the front faces +z).
    pub size: Vec3,
    /// `lod0`, `lod1` and `lod2`: each the level's root nodes.
    pub levels: [Vec<KitNode>; 3],
}

/// A node of a parsed model: its name, its local transform, its mesh (if any) and its children.
pub struct KitNode {
    pub name: String,
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
    pub mesh: Option<KitMesh>,
    pub children: Vec<KitNode>,
}

/// A node's mesh: its triangles, its feature edges (a line list over the same positions; None when it has none) and its
/// material by kit name, two-sided or not, with the KTX2 texture the material names.
pub struct KitMesh {
    pub geo: Geo,
    pub edges: Option<Geo>,
    pub material: String,
    pub double_sided: bool,
    pub texture: Option<String>,
}

/// The parsed kit, in the files' name order. Parsed on first use; a file that fails to parse panics with its name
/// (the kit is built and checked by `tools/apz-kit`, so that is a build error).
pub fn models() -> &'static [KitModel] {
    static KIT: OnceLock<Vec<KitModel>> = OnceLock::new();
    KIT.get_or_init(|| todo!("parse every .glb of KIT_FILES with parse_glb"))
}

/// The model with this id.
pub fn model(id: &str) -> Option<&'static KitModel> {
    models().iter().find(|m| m.id == id)
}

/// Parses one `.glb` of the kit: the scene's extras give `kind` and `size`; its three roots `lod0`, `lod1` and `lod2`
/// give the levels. Each primitive becomes a [`crate::scene::Geometry`] (positions, normals, uvs, u32 indices) with its
/// feature edges ([`crate::geometry::edges::feature_edges`] at [`EDGE_ANGLE`], as a geometry with the same positions and
/// the pairs as indices), and its material's name, `doubleSided` and `extras.texture`.
pub fn parse_glb(id: &str, bytes: &[u8]) -> Result<KitModel, String> {
    let _ = (id, bytes, Quat::IDENTITY);
    todo!()
}

/// A kit texture by file name (`grille.ktx2`), decoded once and repeat-wrapped.
pub fn texture(name: &str) -> Option<Tex> {
    let _ = name;
    todo!()
}

/// A model instantiated in a palette: its scene root and the hooks the fleet drives.
pub struct Instance {
    pub root: Obj,
    /// The label anchor (`anchor:label`) and the port anchor (`anchor:port`), in the model's frame.
    pub label: Vec3,
    pub port: Vec3,
    pub size: Vec3,
    /// The name plates (`plate`), the screens (`screen`) and the VM slot lights (`slot:<n>`, with n), to paint or light.
    pub plates: Vec<Obj>,
    pub screens: Vec<Obj>,
    pub slots: Vec<(u32, Obj)>,
}

impl KitModel {
    /// Level `lod` (0 full, 1 simple, 2 block) as scene nodes in `palette`'s materials. Every mesh node's material is
    /// `palette.material(name)`, with its side two-sided when the glTF says so and the kit texture as its map when the
    /// palette is textured. When `palette.edge_material()` is Some, every mesh node except glass, `status`, `led`,
    /// `signal`, `screen` and `plate` materials and the nodes under a `fan` gets a child: its edges with that material.
    /// Animated parts run on the GPU: a `status` node gets `NodeAnim::Machine { led: Status, glow: 1, emissive: the
    /// working colour, machine: None }`, a `screen` node `NodeAnim::Machine { led: Screen, .. }`, and a `fan` node
    /// `NodeAnim::Spin { pivot: 0, axis: +z, rps: 2, phase: 0, machine: None }` (its rotor turns about its own z axis).
    pub fn instantiate(&self, lod: usize, palette: &Palette) -> Instance {
        let _ = (lod, palette);
        todo!()
    }
}

/// Binds every GPU animation under `root` (status lights, screens, fans) to fleet machine slot `machine`, so their
/// colour and fan angle come from that machine's row of the fleet's tables.
pub fn set_machine(root: &Obj, machine: usize) {
    let _ = (root, machine);
    todo!()
}
