//! The machine model kit (`assets/hardware/kit`, written by `tools/apz-kit`): the ten machines as glTF, parsed once
//! from the kit's files and turned into scene nodes in a [`Palette`]'s materials. The kit's contract (the node and
//! material names this reads) is documented in `tools/apz-kit/src/lib.rs`.
//!
//! Where the files come from: the native builds (the desktop, the tests) embed the kit (the `embedded-kit` feature,
//! on by default), so `KIT_FILES` holds it. The web build leaves it out: the page fetches the files and hands them
//! over with [`provide`] before the first frame.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use glam::{Mat4, Quat, Vec3};

use crate::geometry::edges::feature_edges;
use crate::ktx::texture_from_ktx2;
use crate::scene::{Geo, Geometry, MachineLed, Mat, NodeAnim, Obj, Side, Tex};
use crate::style::Palette;

// `KIT_FILES`: every `.glb` and `.ktx2` of the kit, by file name (build.rs). Empty without the `embedded-kit` feature.
include!(concat!(env!("OUT_DIR"), "/kit_files.rs"));

/// The kit's files as [`provide`] was given them, by file name. Set once.
static PROVIDED: OnceLock<Vec<(String, Vec<u8>)>> = OnceLock::new();

/// The parsed kit, set by the first [`models`] call.
static KIT: OnceLock<Vec<KitModel>> = OnceLock::new();

/// Gives the kit's files to the engine: each `.glb` and `.ktx2` by file name. The web build calls this before its
/// first frame, since its wasm carries no embedded kit; the native builds, which embed the kit, need no call.
/// Errors when the files were provided already, or after [`models`] has parsed the kit (a parsed kit does not change).
pub fn provide(files: Vec<(String, Vec<u8>)>) -> Result<(), String> {
    if KIT.get().is_some() {
        return Err(
            "the kit is parsed already: provide its files before the first frame".to_string(),
        );
    }
    PROVIDED
        .set(files)
        .map_err(|_| "the kit's files are provided already".to_string())
}

/// The kit's files: the ones [`provide`] was given, else the embedded `KIT_FILES`.
fn kit_files() -> Vec<(&'static str, &'static [u8])> {
    select_files(PROVIDED.get().map(Vec::as_slice))
}

/// The file list [`kit_files`] picks from: `provided` when there is one, else the embedded `KIT_FILES`.
fn select_files(provided: Option<&[(String, Vec<u8>)]>) -> Vec<(&str, &[u8])> {
    match provided {
        Some(files) => files
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
            .collect(),
        None => KIT_FILES.to_vec(),
    }
}

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

/// The parsed kit, in the files' name order. Parsed on first use from the kit's files (the embedded ones, or the ones
/// [`provide`] was given). A file that fails to parse panics with its name (the kit is built and checked by
/// `tools/apz-kit`, so that is a build error). On the web, [`provide`] must come before the first call: a kit parsed
/// with no files stays empty.
pub fn models() -> &'static [KitModel] {
    KIT.get_or_init(|| {
        kit_files()
            .into_iter()
            .filter_map(|(name, bytes)| {
                let id = name.strip_suffix(".glb")?;
                Some(parse_glb(id, bytes).unwrap_or_else(|e| panic!("kit file {name}: {e}")))
            })
            .collect()
    })
}

/// The model with this id.
pub fn model(id: &str) -> Option<&'static KitModel> {
    models().iter().find(|m| m.id == id)
}

/// Parses one `.glb` of the kit: the scene's extras give `kind` and `size`; its three roots `lod0`, `lod1` and `lod2`
/// give the levels. Each primitive becomes a [`crate::scene::Geometry`] (positions, normals, uvs, u32 indices) with its
/// feature edges ([`crate::geometry::edges::feature_edges`] at [`EDGE_ANGLE`], as a geometry with the same positions and
/// the pairs as indices), and its material's name, `doubleSided` and `extras.texture`.
///
/// A node with several primitives keeps its first as its mesh; each further one becomes a child named `<node>#<i>`
/// with the identity transform (the node's own transform applies to it).
pub fn parse_glb(id: &str, bytes: &[u8]) -> Result<KitModel, String> {
    let gltf = gltf::Gltf::from_slice(bytes).map_err(|e| format!("{id}: {e}"))?;
    let blob = gltf.blob.as_deref();
    let scene = gltf
        .default_scene()
        .or_else(|| gltf.scenes().next())
        .ok_or_else(|| format!("{id}: no scene"))?;
    let raw = scene
        .extras()
        .as_ref()
        .ok_or_else(|| format!("{id}: the scene has no extras"))?;
    let extras: serde_json::Value =
        serde_json::from_str(raw.get()).map_err(|e| format!("{id}: extras: {e}"))?;
    let kind = extras["kind"]
        .as_str()
        .ok_or_else(|| format!("{id}: no kind in the extras"))?
        .to_string();
    let dims: Vec<f32> = extras["size"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_f64())
        .map(|v| v as f32)
        .collect();
    if dims.len() != 3 {
        return Err(format!("{id}: size is not three numbers"));
    }
    let size = Vec3::new(dims[0], dims[1], dims[2]);

    let mut levels: [Vec<KitNode>; 3] = Default::default();
    for (i, level) in levels.iter_mut().enumerate() {
        let name = format!("lod{i}");
        let root = scene
            .nodes()
            .find(|n| n.name() == Some(name.as_str()))
            .ok_or_else(|| format!("{id}: no {name} root"))?;
        *level = root
            .children()
            .map(|c| convert_node(c, blob))
            .collect::<Result<_, _>>()?;
    }
    Ok(KitModel {
        id: id.to_string(),
        kind,
        size,
        levels,
    })
}

fn convert_node(node: gltf::Node<'_>, blob: Option<&[u8]>) -> Result<KitNode, String> {
    let name = node.name().unwrap_or("").to_string();
    let (t, r, s) = node.transform().decomposed();
    let mut mesh = None;
    let mut children = Vec::new();
    if let Some(m) = node.mesh() {
        for (i, prim) in m.primitives().enumerate() {
            let km = convert_primitive(&prim, blob).map_err(|e| format!("{name}: {e}"))?;
            if i == 0 {
                mesh = Some(km);
            } else {
                children.push(KitNode {
                    name: format!("{name}#{i}"),
                    translation: Vec3::ZERO,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                    mesh: Some(km),
                    children: Vec::new(),
                });
            }
        }
    }
    for child in node.children() {
        children.push(convert_node(child, blob)?);
    }
    Ok(KitNode {
        name,
        translation: Vec3::from_array(t),
        rotation: Quat::from_array(r),
        scale: Vec3::from_array(s),
        mesh,
        children,
    })
}

fn convert_primitive(prim: &gltf::Primitive<'_>, blob: Option<&[u8]>) -> Result<KitMesh, String> {
    let reader = prim.reader(|b| if b.index() == 0 { blob } else { None });
    let positions: Vec<[f32; 3]> = reader.read_positions().ok_or("no positions")?.collect();
    let normals: Vec<[f32; 3]> = reader
        .read_normals()
        .map(|n| n.collect())
        .unwrap_or_default();
    let uvs: Vec<[f32; 2]> = reader
        .read_tex_coords(0)
        .map(|t| t.into_f32().collect())
        .unwrap_or_default();
    let indices: Vec<u32> = match reader.read_indices() {
        Some(i) => i.into_u32().collect(),
        None => (0..positions.len() as u32).collect(),
    };

    let edge_pairs = feature_edges(&positions, &indices, EDGE_ANGLE);
    let edges = (!edge_pairs.is_empty())
        .then(|| Geometry::new(positions.clone(), vec![], vec![], edge_pairs));
    let geo = Geometry::new(positions, normals, uvs, indices);

    let material = prim.material();
    let texture = material
        .extras()
        .as_ref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw.get()).ok())
        .and_then(|v| v["texture"].as_str().map(str::to_string));
    Ok(KitMesh {
        geo,
        edges,
        material: material.name().unwrap_or("").to_string(),
        double_sided: material.double_sided(),
        texture,
    })
}

/// A kit texture by file name (`grille.ktx2`), decoded once and repeat-wrapped.
pub fn texture(name: &str) -> Option<Tex> {
    static CACHE: OnceLock<Mutex<HashMap<String, Tex>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(tex) = cache.get(name) {
        return Some(Arc::clone(tex));
    }
    let entry = kit_files().into_iter().find(|e| e.0 == name)?;
    let mut tex = texture_from_ktx2(entry.1).ok()?;
    tex.wrap = true;
    let tex: Tex = Arc::new(tex);
    cache.insert(name.to_string(), Arc::clone(&tex));
    Some(tex)
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
        let level = &self.levels[lod];
        let mut build = Build {
            palette,
            mats: HashMap::new(),
            edge: palette.edge_material().map(Mat::new),
            plates: Vec::new(),
            screens: Vec::new(),
            slots: Vec::new(),
        };
        let root = Obj::group();
        root.set_name(&self.id);
        for node in level {
            let child = build.node(node, false);
            root.add(&child);
        }

        let (mut label, mut port) = (None, None);
        for node in level {
            anchors(node, Mat4::IDENTITY, &mut label, &mut port);
        }
        Instance {
            root,
            label: label.unwrap_or(Vec3::new(0.0, self.size.y + 0.3, 0.0)),
            port: port.unwrap_or(Vec3::new(0.0, self.size.y / 2.0, -self.size.z / 2.0)),
            size: self.size,
            plates: build.plates,
            screens: build.screens,
            slots: build.slots,
        }
    }
}

/// The scene nodes of one level, with the palette's materials and the animations, while the instance's hooks are
/// collected.
struct Build<'a> {
    palette: &'a Palette,
    /// One material per kit material name (per call).
    mats: HashMap<String, Mat>,
    edge: Option<Mat>,
    plates: Vec<Obj>,
    screens: Vec<Obj>,
    slots: Vec<(u32, Obj)>,
}

impl Build<'_> {
    fn node(&mut self, n: &KitNode, in_fan: bool) -> Obj {
        let obj = match &n.mesh {
            Some(m) => {
                let mat = self.mat(&m.material, m.double_sided, m.texture.as_deref());
                Obj::mesh(&m.geo, &mat)
            }
            None => Obj::group(),
        };
        obj.set_name(&n.name)
            .set_position(n.translation.x, n.translation.y, n.translation.z)
            .set_quaternion(n.rotation)
            .set_scale(n.scale.x, n.scale.y, n.scale.z);

        match n.name.as_str() {
            "status" => {
                obj.set_anim(NodeAnim::Machine {
                    led: MachineLed::Status,
                    glow: 1.0,
                    emissive: self.palette.lamps[0],
                    machine: None,
                });
            }
            "screen" => {
                obj.set_anim(NodeAnim::Machine {
                    led: MachineLed::Screen,
                    glow: 1.0,
                    emissive: self.palette.material("screen").emissive,
                    machine: None,
                });
            }
            "fan" => {
                obj.set_anim(NodeAnim::Spin {
                    pivot: Vec3::ZERO,
                    axis: Vec3::Z,
                    rps: 2.0,
                    phase: 0.0,
                    machine: None,
                });
            }
            _ => {}
        }
        if n.name == "plate" {
            self.plates.push(obj.clone());
        }
        if n.name == "screen" {
            self.screens.push(obj.clone());
        }
        if let Some(slot) = n
            .name
            .strip_prefix("slot:")
            .and_then(|s| s.parse::<u32>().ok())
        {
            self.slots.push((slot, obj.clone()));
        }

        if let (Some(m), false, Some(edge)) = (&n.mesh, in_fan, &self.edge) {
            let plain = !matches!(
                m.material.as_str(),
                "glass" | "status" | "led" | "signal" | "screen" | "plate"
            );
            if let (true, Some(edges)) = (plain, &m.edges) {
                obj.add(&Obj::mesh(edges, edge));
            }
        }

        let child_in_fan = in_fan || n.name == "fan";
        for child in &n.children {
            let c = self.node(child, child_in_fan);
            obj.add(&c);
        }
        obj
    }

    fn mat(&mut self, name: &str, double_sided: bool, texture_name: Option<&str>) -> Mat {
        if let Some(mat) = self.mats.get(name) {
            return mat.clone();
        }
        let mut material = self.palette.material(name);
        if double_sided {
            material.side = Side::Double;
        }
        if self.palette.textured {
            material.map = texture_name.and_then(texture);
        }
        let mat = Mat::new(material);
        self.mats.insert(name.to_string(), mat.clone());
        mat
    }
}

/// The `anchor:label` and `anchor:port` nodes' origins in the model's frame (their parents' transforms composed).
fn anchors(node: &KitNode, parent: Mat4, label: &mut Option<Vec3>, port: &mut Option<Vec3>) {
    let local = Mat4::from_scale_rotation_translation(node.scale, node.rotation, node.translation);
    let world = parent * local;
    match node.name.as_str() {
        "anchor:label" => *label = Some(world.transform_point3(Vec3::ZERO)),
        "anchor:port" => *port = Some(world.transform_point3(Vec3::ZERO)),
        _ => {}
    }
    for child in &node.children {
        anchors(child, world, label, port);
    }
}

/// Binds every GPU animation under `root` (status lights, screens, fans) to fleet machine slot `machine`, so their
/// colour and fan angle come from that machine's row of the fleet's tables.
pub fn set_machine(root: &Obj, machine: usize) {
    root.traverse(&mut |obj: &Obj| {
        let mut node = obj.get();
        match &mut node.anim {
            Some(NodeAnim::Machine { machine: m, .. })
            | Some(NodeAnim::Spin { machine: m, .. }) => {
                *m = Some(machine);
            }
            _ => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "embedded-kit")]
    const MANIFEST: &str = include_str!("../../../assets/hardware/kit/manifest.json");

    #[cfg(feature = "embedded-kit")]
    fn has_node(nodes: &[KitNode], name: &str) -> bool {
        nodes
            .iter()
            .any(|n| n.name == name || has_node(&n.children, name))
    }

    /// The ids of a file list's `.glb` models, in the list's order.
    #[cfg(feature = "embedded-kit")]
    fn model_ids(files: &[(&str, &[u8])]) -> Vec<String> {
        files
            .iter()
            .filter_map(|&(name, _)| name.strip_suffix(".glb"))
            .map(str::to_string)
            .collect()
    }

    /// Files handed over at runtime (the web's `provide`) are the embedded kit's: the same names and bytes, so the
    /// same model ids.
    #[test]
    #[cfg(feature = "embedded-kit")]
    fn provided_files_are_the_embedded_kit() {
        let owned: Vec<(String, Vec<u8>)> = KIT_FILES
            .iter()
            .map(|&(name, bytes)| (name.to_string(), bytes.to_vec()))
            .collect();
        let provided = select_files(Some(owned.as_slice()));
        let embedded = select_files(None);
        assert_eq!(provided, embedded);
        let ids = model_ids(&provided);
        assert!(!ids.is_empty(), "the embedded kit has models");
        assert_eq!(ids, model_ids(&embedded));
    }

    #[test]
    #[cfg(feature = "embedded-kit")]
    fn every_model_parses_to_its_manifest_size() {
        let manifest: serde_json::Value = serde_json::from_str(MANIFEST).expect("the manifest");
        let entries = manifest["models"]
            .as_array()
            .expect("the manifest's models");
        let mut parsed = 0;
        for (name, bytes) in kit_files().into_iter().filter(|e| e.0.ends_with(".glb")) {
            let id = name.trim_end_matches(".glb");
            let model = parse_glb(id, bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(model.id, id);
            for (lod, level) in model.levels.iter().enumerate() {
                assert!(!level.is_empty(), "{name}: lod{lod} is empty");
            }
            assert!(
                has_node(&model.levels[0], "status"),
                "{name}: no status node"
            );
            assert!(
                has_node(&model.levels[0], "anchor:label"),
                "{name}: no anchor:label"
            );
            let entry = entries
                .iter()
                .find(|e| e["id"].as_str() == Some(id))
                .unwrap_or_else(|| panic!("{name}: not in the manifest"));
            let want: Vec<f64> = entry["size"]
                .as_array()
                .expect("the manifest's size")
                .iter()
                .filter_map(|v| v.as_f64())
                .collect();
            assert_eq!(want.len(), 3, "{name}: manifest size");
            for (got, want) in model.size.to_array().into_iter().zip(want.iter().copied()) {
                assert!(
                    (f64::from(got) - want).abs() < 1e-3,
                    "{name}: size {:?} vs {want:?}",
                    model.size
                );
            }
            parsed += 1;
        }
        assert_eq!(parsed, entries.len());
    }

    #[test]
    #[ignore = "needs style.rs"]
    fn gpu_instantiates_with_its_animations() {
        let palette = Palette::of(crate::style::Style::Ink, true);
        let gpu = model("gpu").expect("the gpu model");
        let inst = gpu.instantiate(0, &palette);
        let (mut fan, mut status) = (false, false);
        inst.root.traverse(&mut |o: &Obj| {
            let n = o.get();
            fan |= n.name == "fan" && matches!(n.anim, Some(NodeAnim::Spin { .. }));
            status |= n.name == "status" && matches!(n.anim, Some(NodeAnim::Machine { .. }));
        });
        assert!(fan, "no fan with a Spin animation");
        assert!(status, "no status node with a Machine animation");
        assert!(
            inst.label.y > gpu.size.y,
            "the label anchor is not above the model"
        );

        set_machine(&inst.root, 3);
        let mut bound = 0;
        inst.root.traverse(&mut |o: &Obj| {
            let n = o.get();
            if matches!(
                n.anim,
                Some(
                    NodeAnim::Machine {
                        machine: Some(3),
                        ..
                    } | NodeAnim::Spin {
                        machine: Some(3),
                        ..
                    }
                )
            ) {
                bound += 1;
            }
        });
        assert!(bound >= 2, "set_machine bound {bound} animations");
    }
}
