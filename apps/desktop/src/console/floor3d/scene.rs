//! The Floor's scene graph: a small, thread-safe cousin of three.js's, so the web floor's
//! modules (`apps/web/src/views/floor3d/*`, `floor.ts`) port line by line.
//!
//! - [`Obj`] is a node (group, mesh, instanced mesh, points, point light) with a position, an
//!   Euler rotation (XYZ, radians) or a quaternion, a scale, children and three's flags
//!   (`visible`, `cast_shadow`, `receive_shadow`, `render_order`, `userData.dynamic`,
//!   `userData.outline`).
//! - [`Geo`] is shared, immutable vertex data ([`Geometry`]); `geometry.rs` makes them.
//! - [`Mat`] is a shared, mutable material (several meshes share one, as in three; a robot's
//!   highlight changes its shell materials' emissive).
//! - [`Tex`] is an RGBA texture (sRGB) with three's repeat / offset / wrap.
//! - [`Color`] is linear RGB, built from sRGB hex or HSL exactly as three's `Color` does.
//!
//! The renderer never sees `Obj`s: [`flatten`] turns a scene into [`Draw`]s each frame, and
//! [`bake`] merges the static ones once (the web floor's `bakeStatic`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use glam::{EulerRot, Mat3, Mat4, Quat, Vec3, Vec4};

/// A process-unique id for geometries, materials and textures (the GPU caches key on it).
pub fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---- Colour ------------------------------------------------------------------------------------

/// Linear RGB, like three's `Color` under its default colour management: hex, CSS and HSL
/// values are sRGB and converted on the way in.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

fn srgb_to_linear(c: f32) -> f32 {
    if c < 0.04045 { c * 0.077_399_38 } else { ((c * 0.947_867_3) + 0.052_132_7).powf(2.4) }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c < 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(0.416_666_66) - 0.055 }
}

fn hue2rgb(p: f32, q: f32, mut t: f32) -> f32 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        p + (q - p) * 6.0 * t
    } else if t < 0.5 {
        q
    } else if t < 2.0 / 3.0 {
        p + (q - p) * 6.0 * (2.0 / 3.0 - t)
    } else {
        p
    }
}

impl Color {
    pub const WHITE: Color = Color { r: 1.0, g: 1.0, b: 1.0 };
    pub const BLACK: Color = Color { r: 0.0, g: 0.0, b: 0.0 };

    /// Linear components as given.
    pub const fn linear(r: f32, g: f32, b: f32) -> Color {
        Color { r, g, b }
    }

    /// sRGB components 0–1 (three's `setRGB` with the sRGB colour space).
    pub fn srgb(r: f32, g: f32, b: f32) -> Color {
        Color { r: srgb_to_linear(r), g: srgb_to_linear(g), b: srgb_to_linear(b) }
    }

    /// `0xrrggbb`, sRGB (three's `new Color(0x…)`).
    pub fn hex(hex: u32) -> Color {
        let c = |shift: u32| ((hex >> shift) & 0xff) as f32 / 255.0;
        Color::srgb(c(16), c(8), c(0))
    }

    /// HSL in sRGB, each 0–1 (three's `setHSL`).
    pub fn hsl(h: f32, s: f32, l: f32) -> Color {
        let h = h.rem_euclid(1.0);
        let s = s.clamp(0.0, 1.0);
        let l = l.clamp(0.0, 1.0);
        if s == 0.0 {
            return Color::srgb(l, l, l);
        }
        let q = if l <= 0.5 { l * (1.0 + s) } else { l + s - l * s };
        let p = 2.0 * l - q;
        Color::srgb(hue2rgb(p, q, h + 1.0 / 3.0), hue2rgb(p, q, h), hue2rgb(p, q, h - 1.0 / 3.0))
    }

    /// HSL in sRGB (three's `getHSL`): (h, s, l), each 0–1.
    pub fn to_hsl(self) -> (f32, f32, f32) {
        let [r, g, b] = self.to_srgb();
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let l = (min + max) / 2.0;
        if (max - min).abs() < f32::EPSILON {
            return (0.0, 0.0, l);
        }
        let delta = max - min;
        let s = if l <= 0.5 { delta / (max + min) } else { delta / (2.0 - max - min) };
        let h = if max == r {
            (g - b) / delta + if g < b { 6.0 } else { 0.0 }
        } else if max == g {
            (b - r) / delta + 2.0
        } else {
            (r - g) / delta + 4.0
        };
        (h / 6.0, s, l)
    }

    /// three's `offsetHSL`.
    pub fn offset_hsl(self, dh: f32, ds: f32, dl: f32) -> Color {
        let (h, s, l) = self.to_hsl();
        Color::hsl(h + dh, s + ds, l + dl)
    }

    /// A CSS colour: `#rgb`, `#rrggbb`, `rgb(r, g, b)`, `hsl(h, s%, l%)` (h in degrees).
    pub fn css(text: &str) -> Option<Color> {
        let t = text.trim();
        if let Some(hex) = t.strip_prefix('#') {
            let full: String = match hex.len() {
                3 => hex.chars().flat_map(|c| [c, c]).collect(),
                6 => hex.to_string(),
                _ => return None,
            };
            return u32::from_str_radix(&full, 16).ok().map(Color::hex);
        }
        let args = |prefix: &str| -> Option<Vec<f32>> {
            let inner = t.strip_prefix(prefix)?.trim_start_matches('a').strip_prefix('(')?.strip_suffix(')')?;
            inner.split([',', ' ', '/']).filter(|p| !p.is_empty()).map(|p| p.trim_end_matches(['%', 'g', 'd', 'e']).parse().ok()).collect()
        };
        if let Some(v) = args("rgb") {
            return (v.len() >= 3).then(|| Color::srgb(v[0] / 255.0, v[1] / 255.0, v[2] / 255.0));
        }
        if let Some(v) = args("hsl") {
            return (v.len() >= 3).then(|| Color::hsl(v[0] / 360.0, v[1] / 100.0, v[2] / 100.0));
        }
        None
    }

    /// sRGB components 0–1.
    pub fn to_srgb(self) -> [f32; 3] {
        [linear_to_srgb(self.r.clamp(0.0, 1.0)), linear_to_srgb(self.g.clamp(0.0, 1.0)), linear_to_srgb(self.b.clamp(0.0, 1.0))]
    }

    /// sRGB bytes, for painting textures.
    pub fn to_srgb_u8(self) -> [u8; 3] {
        let [r, g, b] = self.to_srgb();
        [(r * 255.0).round() as u8, (g * 255.0).round() as u8, (b * 255.0).round() as u8]
    }

    /// three's `lerp` (in linear space, as three does).
    pub fn lerp(self, to: Color, t: f32) -> Color {
        Color { r: self.r + (to.r - self.r) * t, g: self.g + (to.g - self.g) * t, b: self.b + (to.b - self.b) * t }
    }

    /// three's `multiplyScalar`.
    pub fn scale(self, k: f32) -> Color {
        Color { r: self.r * k, g: self.g * k, b: self.b * k }
    }

    /// Component-wise product (three's `multiply`).
    pub fn mul(self, other: Color) -> Color {
        Color { r: self.r * other.r, g: self.g * other.g, b: self.b * other.b }
    }

    pub fn to_array(self) -> [f32; 3] {
        [self.r, self.g, self.b]
    }
}

// ---- Geometry ----------------------------------------------------------------------------------

/// Indexed triangles with normals and uvs (one entry per vertex), and their bounding sphere.
#[derive(Debug, Default)]
pub struct Geometry {
    pub id: u64,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub center: [f32; 3],
    pub radius: f32,
}

pub type Geo = Arc<Geometry>;

impl Geometry {
    /// A geometry from its parts: assigns an id and computes the bounding sphere (three's
    /// `computeBoundingSphere`: the box centre, the farthest vertex). Missing normals or uvs
    /// are filled with zeros.
    pub fn new(positions: Vec<[f32; 3]>, mut normals: Vec<[f32; 3]>, mut uvs: Vec<[f32; 2]>, indices: Vec<u32>) -> Geo {
        normals.resize(positions.len(), [0.0; 3]);
        uvs.resize(positions.len(), [0.0; 2]);
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for p in &positions {
            lo = lo.min(Vec3::from(*p));
            hi = hi.max(Vec3::from(*p));
        }
        let center = if positions.is_empty() { Vec3::ZERO } else { (lo + hi) * 0.5 };
        let radius = positions.iter().map(|p| Vec3::from(*p).distance(center)).fold(0.0, f32::max);
        Arc::new(Geometry { id: next_id(), positions, normals, uvs, indices, center: center.to_array(), radius })
    }
}

// ---- Textures ----------------------------------------------------------------------------------

/// An RGBA8 image (sRGB unless `srgb` is false) and how it is sampled.
#[derive(Debug, Clone)]
pub struct Texture {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
    /// three's `repeat` and `offset` (uv' = uv * repeat + offset).
    pub repeat: [f32; 2],
    pub offset: [f32; 2],
    /// Repeat-wrapped (three's `RepeatWrapping`) or clamped.
    pub wrap: bool,
    /// Nearest filtering (pixel art, ramps) or linear with mipmaps.
    pub nearest: bool,
    pub srgb: bool,
}

pub type Tex = Arc<Texture>;

impl Texture {
    /// A clamped, linearly filtered sRGB texture.
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Texture {
        Texture { id: next_id(), width, height, rgba: Arc::new(rgba), repeat: [1.0, 1.0], offset: [0.0, 0.0], wrap: false, nearest: false, srgb: true }
    }

    /// The same pixels with another repeat (a new id: three would clone the texture).
    pub fn with_repeat(&self, x: f32, y: f32) -> Tex {
        Arc::new(Texture { id: next_id(), repeat: [x, y], ..self.clone() })
    }
}

// ---- Materials ---------------------------------------------------------------------------------

/// How a surface is shaded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shading {
    /// three's `MeshToonMaterial` with the plaza's three-band ramp: lit by the sun (banded,
    /// shadowed), the hemisphere light and point lights.
    Toon,
    /// three's `MeshBasicMaterial`: colour × map, unlit.
    Basic,
    /// three's `ShadowMaterial`: transparent except where shadowed (`color` × `opacity`).
    Shadow,
    /// The painted sky dome (`toon.ts` `sky`): bands from `top` down to `horizon`, `ground` below.
    Sky { top: Color, horizon: Color, ground: Color },
    /// Grass tufts (`toon.ts` `grassTufts`): toon, swayed by the wind, paler at the tips.
    Grass,
    /// three's `PointsMaterial`: round-less square points `size` world units wide, attenuated.
    Points { size: f32 },
}

/// three's `side`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Front,
    Back,
    Double,
}

/// A material's values.
#[derive(Debug, Clone)]
pub struct Material {
    pub shading: Shading,
    pub color: Color,
    pub emissive: Color,
    pub emissive_intensity: f32,
    pub map: Option<Tex>,
    pub opacity: f32,
    pub transparent: bool,
    /// three's `AdditiveBlending`.
    pub additive: bool,
    pub depth_write: bool,
    pub depth_test: bool,
    pub side: Side,
    /// Fogged (three's `fog: true`, the default for its built-in materials).
    pub fog: bool,
    pub visible: bool,
}

impl Default for Material {
    fn default() -> Self {
        Material {
            shading: Shading::Toon,
            color: Color::WHITE,
            emissive: Color::BLACK,
            emissive_intensity: 1.0,
            map: None,
            opacity: 1.0,
            transparent: false,
            additive: false,
            depth_write: true,
            depth_test: true,
            side: Side::Front,
            fog: true,
            visible: true,
        }
    }
}

/// A shared, mutable material (three's materials are shared by reference).
#[derive(Debug, Clone)]
pub struct Mat(Arc<(u64, Mutex<Material>)>);

impl Mat {
    pub fn new(material: Material) -> Mat {
        Mat(Arc::new((next_id(), Mutex::new(material))))
    }

    /// `toonMat({ color })`.
    pub fn toon(color: Color) -> Mat {
        Mat::new(Material { color, ..Material::default() })
    }

    /// `new MeshBasicMaterial({ color })`.
    pub fn basic(color: Color) -> Mat {
        Mat::new(Material { shading: Shading::Basic, color, ..Material::default() })
    }

    pub fn id(&self) -> u64 {
        self.0.0
    }

    pub fn get(&self) -> MutexGuard<'_, Material> {
        lock(&self.0.1)
    }

    /// A copy of the values now.
    pub fn snapshot(&self) -> Material {
        self.get().clone()
    }
}

// ---- Nodes -------------------------------------------------------------------------------------

/// One instance of an instanced mesh: its matrix and colour (three's `setMatrixAt` /
/// `setColorAt`; the colour multiplies the material's).
#[derive(Debug, Clone, Copy)]
pub struct Instance {
    pub matrix: Mat4,
    pub color: Color,
}

/// What a node is.
#[derive(Debug, Clone)]
pub enum Kind {
    Group,
    Mesh { geo: Geo, mat: Mat },
    /// three's `InstancedMesh` (`count` ≤ `instances.len()` are drawn).
    Instanced { geo: Geo, mat: Mat, instances: Arc<Vec<Instance>>, count: usize },
    /// three's `Points`: one point per position, drawn with a `Shading::Points` material.
    Points { positions: Arc<Vec<[f32; 3]>>, mat: Mat },
    /// three's `PointLight` at the node's position.
    PointLight { color: Color, intensity: f32, distance: f32, decay: f32 },
}

/// A node's state. Reach it through [`Obj::get`].
#[derive(Debug, Clone)]
pub struct Node {
    pub name: String,
    pub kind: Kind,
    pub position: Vec3,
    /// Euler angles, XYZ order (three's default), in radians — used unless `quaternion` is set.
    pub rotation: Vec3,
    pub quaternion: Option<Quat>,
    pub scale: Vec3,
    pub visible: bool,
    pub cast_shadow: bool,
    pub receive_shadow: bool,
    pub render_order: i32,
    /// `userData.dynamic`: moves or changes after the world is built, so it is never baked.
    pub dynamic: bool,
    /// The ink hull `outline()` gave this mesh (`toon.ts`), drawn behind it.
    pub hull: Option<Geo>,
    /// `userData.outline === false`: `outline()` leaves it alone.
    pub no_outline: bool,
    /// `userData.walk`: robots walk over it (rugs, floor markings): it has no footprint.
    pub walk: bool,
    /// An opaque tag for picking (a robot's cid) and lookups.
    pub tag: Option<u64>,
    pub children: Vec<Obj>,
}

/// A scene-graph node, shared (three's `Object3D`).
#[derive(Debug, Clone)]
pub struct Obj(Arc<Mutex<Node>>);

impl PartialEq for Obj {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Obj {
    fn of(kind: Kind) -> Obj {
        Obj(Arc::new(Mutex::new(Node {
            name: String::new(),
            kind,
            position: Vec3::ZERO,
            rotation: Vec3::ZERO,
            quaternion: None,
            scale: Vec3::ONE,
            visible: true,
            cast_shadow: false,
            receive_shadow: false,
            render_order: 0,
            dynamic: false,
            hull: None,
            no_outline: false,
            walk: false,
            tag: None,
            children: Vec::new(),
        })))
    }

    /// `new THREE.Group()`.
    pub fn group() -> Obj {
        Obj::of(Kind::Group)
    }

    /// `new THREE.Mesh(geo, mat)`.
    pub fn mesh(geo: &Geo, mat: &Mat) -> Obj {
        Obj::of(Kind::Mesh { geo: Arc::clone(geo), mat: mat.clone() })
    }

    /// `new THREE.InstancedMesh(geo, mat, n)` with its instances.
    pub fn instanced(geo: &Geo, mat: &Mat, instances: Vec<Instance>) -> Obj {
        let count = instances.len();
        Obj::of(Kind::Instanced { geo: Arc::clone(geo), mat: mat.clone(), instances: Arc::new(instances), count })
    }

    /// `new THREE.Points(geo, mat)`.
    pub fn points(positions: Vec<[f32; 3]>, mat: &Mat) -> Obj {
        Obj::of(Kind::Points { positions: Arc::new(positions), mat: mat.clone() })
    }

    /// `new THREE.PointLight(color, intensity, distance, decay)`.
    pub fn point_light(color: Color, intensity: f32, distance: f32, decay: f32) -> Obj {
        Obj::of(Kind::PointLight { color, intensity, distance, decay })
    }

    /// The node's state, locked. Keep the guard short; never hold two at once.
    pub fn get(&self) -> MutexGuard<'_, Node> {
        lock(&self.0)
    }

    /// `parent.add(child)`. Adding a node that is already a child of another is the caller's
    /// mistake (three would move it); here it is simply shared.
    pub fn add(&self, child: &Obj) -> &Self {
        self.get().children.push(child.clone());
        self
    }

    /// `parent.remove(child)`.
    pub fn remove(&self, child: &Obj) {
        self.get().children.retain(|c| c != child);
    }

    pub fn set_name(&self, name: &str) -> &Self {
        self.get().name = name.to_string();
        self
    }
    pub fn set_position(&self, x: f32, y: f32, z: f32) -> &Self {
        self.get().position = Vec3::new(x, y, z);
        self
    }
    pub fn set_rotation(&self, x: f32, y: f32, z: f32) -> &Self {
        let mut node = self.get();
        node.rotation = Vec3::new(x, y, z);
        node.quaternion = None;
        self
    }
    pub fn set_quaternion(&self, q: Quat) -> &Self {
        self.get().quaternion = Some(q);
        self
    }
    pub fn set_scale(&self, x: f32, y: f32, z: f32) -> &Self {
        self.get().scale = Vec3::new(x, y, z);
        self
    }
    /// `scale.setScalar(s)`.
    pub fn set_scalar(&self, s: f32) -> &Self {
        self.get().scale = Vec3::splat(s);
        self
    }
    pub fn set_visible(&self, on: bool) -> &Self {
        self.get().visible = on;
        self
    }
    /// `castShadow` and `receiveShadow`.
    pub fn set_shadows(&self, cast: bool, receive: bool) -> &Self {
        let mut node = self.get();
        node.cast_shadow = cast;
        node.receive_shadow = receive;
        self
    }
    pub fn set_dynamic(&self, on: bool) -> &Self {
        self.get().dynamic = on;
        self
    }
    pub fn set_tag(&self, tag: u64) -> &Self {
        self.get().tag = Some(tag);
        self
    }
    pub fn set_render_order(&self, order: i32) -> &Self {
        self.get().render_order = order;
        self
    }
    /// `userData.walk = true`: no collision footprint.
    pub fn set_walk(&self) -> &Self {
        self.get().walk = true;
        self
    }
    /// `userData.outline = false`.
    pub fn no_outline(&self) -> &Self {
        self.get().no_outline = true;
        self
    }

    pub fn position(&self) -> Vec3 {
        self.get().position
    }

    /// The material of a mesh, instanced mesh or points node.
    pub fn material(&self) -> Option<Mat> {
        match &self.get().kind {
            Kind::Mesh { mat, .. } | Kind::Instanced { mat, .. } | Kind::Points { mat, .. } => Some(mat.clone()),
            _ => None,
        }
    }

    /// Replaces a mesh's material (three's `mesh.material = …`).
    pub fn set_material(&self, new: &Mat) {
        if let Kind::Mesh { mat, .. } | Kind::Instanced { mat, .. } | Kind::Points { mat, .. } = &mut self.get().kind {
            *mat = new.clone();
        }
    }

    /// Replaces a points node's positions (dust drifting).
    pub fn set_points(&self, new: Vec<[f32; 3]>) {
        if let Kind::Points { positions, .. } = &mut self.get().kind {
            *positions = Arc::new(new);
        }
    }

    /// The local matrix (T · R · S).
    pub fn local_matrix(&self) -> Mat4 {
        local_of(&self.get())
    }

    /// `object.traverse(f)`: this node, then its descendants, depth first.
    pub fn traverse(&self, f: &mut dyn FnMut(&Obj)) {
        f(self);
        let children = self.get().children.clone();
        for child in &children {
            child.traverse(f);
        }
    }

    /// The world matrix of `target` within this tree (None when it is not in it).
    pub fn world_matrix_of(&self, target: &Obj) -> Option<Mat4> {
        fn walk(node: &Obj, target: &Obj, parent: Mat4) -> Option<Mat4> {
            let (world, children) = {
                let n = node.get();
                (parent * local_of(&n), n.children.clone())
            };
            if node == target {
                return Some(world);
            }
            children.iter().find_map(|child| walk(child, target, world))
        }
        walk(self, target, Mat4::IDENTITY)
    }
}

fn local_of(node: &Node) -> Mat4 {
    let rotation = node.quaternion.unwrap_or_else(|| Quat::from_euler(EulerRot::XYZ, node.rotation.x, node.rotation.y, node.rotation.z));
    Mat4::from_scale_rotation_translation(node.scale, rotation, node.position)
}

// ---- Camera, lights, frame ---------------------------------------------------------------------

/// A perspective camera looking at `target` (three's `PerspectiveCamera` + `lookAt`).
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
    /// Vertical field of view, degrees.
    pub fov: f32,
    pub near: f32,
    pub far: f32,
    /// Width / height.
    pub aspect: f32,
}

impl Camera {
    pub fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye, self.target, Vec3::Y)
    }

    /// Projection with wgpu's 0..1 depth.
    pub fn projection(&self) -> Mat4 {
        Mat4::perspective_rh(self.fov.to_radians(), self.aspect.max(1e-3), self.near, self.far)
    }

    /// A world point in pixels from the top-left of a `width` × `height` view; None behind the
    /// camera or outside the depth range (three's `vector.project(camera)` plus the CSS mapping).
    pub fn to_screen(&self, world: Vec3, width: f32, height: f32) -> Option<(f32, f32)> {
        let clip = self.projection() * self.view() * world.extend(1.0);
        if clip.w <= 0.0 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        if !(0.0..=1.0).contains(&ndc.z) {
            return None;
        }
        Some(((ndc.x * 0.5 + 0.5) * width, (-ndc.y * 0.5 + 0.5) * height))
    }

    /// The ray through a pixel (from the top-left of a `width` × `height` view): origin, unit
    /// direction (three's `Raycaster.setFromCamera`).
    pub fn ray(&self, x: f32, y: f32, width: f32, height: f32) -> (Vec3, Vec3) {
        let ndc_x = x / width.max(1.0) * 2.0 - 1.0;
        let ndc_y = 1.0 - y / height.max(1.0) * 2.0;
        let inverse = (self.projection() * self.view()).inverse();
        let near = inverse * Vec4::new(ndc_x, ndc_y, 0.0, 1.0);
        let far = inverse * Vec4::new(ndc_x, ndc_y, 1.0, 1.0);
        let (near, far) = (near.truncate() / near.w, far.truncate() / far.w);
        (near, (far - near).normalize_or_zero())
    }
}

/// The scene's lights and atmosphere (the floor sets them once, as `floor.ts` does).
#[derive(Debug, Clone, Copy)]
pub struct World {
    /// three's `HemisphereLight(sky, ground, intensity)`.
    pub hemi_sky: Color,
    pub hemi_ground: Color,
    pub hemi_intensity: f32,
    /// three's `DirectionalLight`: colour, intensity, position (it shines towards the origin).
    pub sun_color: Color,
    pub sun_intensity: f32,
    pub sun_position: Vec3,
    /// The sun's shadow camera: half-extent of its square, near, far; the bias and normal bias.
    pub shadow_extent: f32,
    pub shadow_near: f32,
    pub shadow_far: f32,
    pub shadow_bias: f32,
    pub shadow_normal_bias: f32,
    /// three's `Fog(color, near, far)`.
    pub fog_color: Color,
    pub fog_near: f32,
    pub fog_far: f32,
    /// The clear colour behind everything (the sky dome covers it).
    pub background: Color,
    /// The outline hull's thickness and ink (`toon.ts`: 0.024, 0x2b2420).
    pub outline_thickness: f32,
    pub ink: Color,
}

impl Default for World {
    /// The web floor's values (`floor.ts` mount, `toon.ts`).
    fn default() -> Self {
        World {
            hemi_sky: Color::hex(0xdff0ff),
            hemi_ground: Color::hex(0x8a7a5a),
            hemi_intensity: 1.1,
            sun_color: Color::hex(0xfff1d6),
            sun_intensity: 2.6,
            sun_position: Vec3::new(22.0, 40.0, 16.0),
            shadow_extent: 40.0,
            shadow_near: 1.0,
            shadow_far: 140.0,
            shadow_bias: -0.0008,
            shadow_normal_bias: 0.03,
            fog_color: Color::hex(0xfde9c4),
            fog_near: 70.0,
            fog_far: 240.0,
            background: Color::hex(0xfde9c4),
            outline_thickness: 0.024,
            ink: Color::hex(0x2b2420),
        }
    }
}

/// What a draw is.
#[derive(Debug, Clone)]
pub enum DrawKind {
    Mesh,
    /// The ink hull of a mesh (`geo` is the hull).
    Outline,
    /// Instances of `geo`, each with its own matrix (relative to `model`) and colour.
    Instanced(Arc<Vec<Instance>>, usize),
    /// Points at these positions (relative to `model`).
    Points(Arc<Vec<[f32; 3]>>),
}

/// One thing the renderer draws.
#[derive(Debug, Clone)]
pub struct Draw {
    pub kind: DrawKind,
    pub geo: Geo,
    pub mat_id: u64,
    pub material: Material,
    pub model: Mat4,
    pub cast_shadow: bool,
    pub receive_shadow: bool,
    pub render_order: i32,
}

/// A point light in world space.
#[derive(Debug, Clone, Copy)]
pub struct PointLightDraw {
    pub position: Vec3,
    pub color: Color,
    pub intensity: f32,
    pub distance: f32,
    pub decay: f32,
}

/// Everything one frame needs: built on the UI thread, handed to the renderer.
#[derive(Debug, Clone)]
pub struct Frame {
    pub camera: Camera,
    pub world: World,
    /// The baked static draws (shared across frames; `statics_version` says when they change).
    pub statics: Arc<Vec<Draw>>,
    pub statics_version: u64,
    /// The dynamic draws of this frame.
    pub draws: Vec<Draw>,
    pub lights: Vec<PointLightDraw>,
    /// Seconds since the floor opened (the grass's wind).
    pub time: f32,
}

/// The draws and point lights under `root`: each visible mesh with its world matrix, its hull
/// after it, instanced meshes and points. With `dynamic_only`, only nodes under a `dynamic`
/// ancestor (or dynamic themselves); with `static_only`, only the others.
pub fn flatten(root: &Obj, filter: Filter) -> (Vec<Draw>, Vec<PointLightDraw>) {
    let mut draws = Vec::new();
    let mut lights = Vec::new();
    walk(root, Mat4::IDENTITY, false, filter, &mut draws, &mut lights);
    (draws, lights)
}

/// Which part of the scene [`flatten`] returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    Static,
    Dynamic,
}

fn walk(node: &Obj, parent: Mat4, dynamic_above: bool, filter: Filter, draws: &mut Vec<Draw>, lights: &mut Vec<PointLightDraw>) {
    let (world, dynamic, children, item) = {
        let n = node.get();
        if !n.visible {
            return;
        }
        let world = parent * local_of(&n);
        let dynamic = dynamic_above || n.dynamic;
        (world, dynamic, n.children.clone(), (n.kind.clone(), n.hull.clone(), n.cast_shadow, n.receive_shadow, n.render_order))
    };
    let wanted = match filter {
        Filter::All => true,
        Filter::Static => !dynamic,
        Filter::Dynamic => dynamic,
    };
    if wanted {
        let (kind, hull, cast_shadow, receive_shadow, render_order) = item;
        let draw = |kind: DrawKind, geo: &Geo, mat: &Mat| {
            let material = mat.snapshot();
            material.visible.then(|| Draw { kind, geo: Arc::clone(geo), mat_id: mat.id(), material, model: world, cast_shadow, receive_shadow, render_order })
        };
        match &kind {
            Kind::Group => {}
            Kind::Mesh { geo, mat } => {
                if let Some(d) = draw(DrawKind::Mesh, geo, mat) {
                    draws.push(d);
                    if let Some(hull) = &hull {
                        draws.push(Draw { kind: DrawKind::Outline, geo: Arc::clone(hull), mat_id: 0, cast_shadow: false, receive_shadow: false, ..draws.last().cloned().expect("just pushed") });
                    }
                }
            }
            Kind::Instanced { geo, mat, instances, count } => draws.extend(draw(DrawKind::Instanced(Arc::clone(instances), *count), geo, mat)),
            Kind::Points { positions, mat } => draws.extend(draw(DrawKind::Points(Arc::clone(positions)), &empty_geo(), mat)),
            Kind::PointLight { color, intensity, distance, decay } => lights.push(PointLightDraw {
                position: world.w_axis.truncate(),
                color: *color,
                intensity: *intensity,
                distance: *distance,
                decay: *decay,
            }),
        }
    }
    for child in &children {
        walk(child, world, dynamic, filter, draws, lights);
    }
}

/// The topmost dynamic nodes in `node`'s subtree (`node` included), each with its parent's world
/// matrix: the only parts a frame has to walk once the rest is baked.
pub fn dynamic_roots(node: &Obj, parent: Mat4) -> Vec<(Obj, Mat4)> {
    let mut found = Vec::new();
    fn visit(node: &Obj, parent: Mat4, found: &mut Vec<(Obj, Mat4)>) {
        let (dynamic, world, children) = {
            let n = node.get();
            (n.dynamic, parent * local_of(&n), n.children.clone())
        };
        if dynamic {
            found.push((node.clone(), parent));
            return;
        }
        for child in &children {
            visit(child, world, found);
        }
    }
    visit(node, parent, &mut found);
    found
}

/// The draws and point lights of these (dynamic) subtrees.
pub fn flatten_roots(roots: &[(Obj, Mat4)]) -> (Vec<Draw>, Vec<PointLightDraw>) {
    let mut draws = Vec::new();
    let mut lights = Vec::new();
    for (node, parent) in roots {
        walk(node, *parent, true, Filter::Dynamic, &mut draws, &mut lights);
    }
    (draws, lights)
}

fn empty_geo() -> Geo {
    static EMPTY: std::sync::OnceLock<Geo> = std::sync::OnceLock::new();
    Arc::clone(EMPTY.get_or_init(|| Geometry::new(Vec::new(), Vec::new(), Vec::new(), Vec::new())))
}

/// The static part of `root`, merged: one draw per (material, shadow flags, render order, kind)
/// with the vertices moved to world space (the web floor's `bakeStatic`). Hulls merge too.
pub fn bake(root: &Obj) -> Vec<Draw> {
    let (draws, _) = flatten(root, Filter::Static);
    let mut groups: Vec<(Draw, Vec<(Geo, Mat4)>)> = Vec::new();
    let mut index: HashMap<(u64, bool, bool, i32, u8, u64), usize> = HashMap::new();
    let mut rest = Vec::new();
    for d in draws {
        let tag = match d.kind {
            DrawKind::Mesh => 0u8,
            DrawKind::Outline => 1,
            _ => {
                rest.push(d);
                continue;
            }
        };
        // Outlines share one ink: key them by nothing but their kind. Textured materials with
        // different maps stay apart (the map is part of the material).
        let key = (d.mat_id, d.cast_shadow, d.receive_shadow, d.render_order, tag, if tag == 1 { 0 } else { d.mat_id });
        match index.get(&key) {
            Some(&at) => groups[at].1.push((Arc::clone(&d.geo), d.model)),
            None => {
                index.insert(key, groups.len());
                let parts = vec![(Arc::clone(&d.geo), d.model)];
                groups.push((d, parts));
            }
        }
    }
    let mut out: Vec<Draw> = groups
        .into_iter()
        .map(|(first, parts)| Draw { geo: merge(&parts), model: Mat4::IDENTITY, ..first })
        .collect();
    out.extend(rest);
    out
}

/// Geometries moved by their matrices into one (normals by the normal matrix).
pub fn merge(parts: &[(Geo, Mat4)]) -> Geo {
    let total: usize = parts.iter().map(|(g, _)| g.positions.len()).sum();
    let (mut positions, mut normals, mut uvs, mut indices) =
        (Vec::with_capacity(total), Vec::with_capacity(total), Vec::with_capacity(total), Vec::new());
    for (geo, m) in parts {
        let base = positions.len() as u32;
        let normal_matrix = Mat3::from_mat4(*m).inverse().transpose();
        positions.extend(geo.positions.iter().map(|p| m.transform_point3(Vec3::from(*p)).to_array()));
        normals.extend(geo.normals.iter().map(|n| (normal_matrix * Vec3::from(*n)).normalize_or_zero().to_array()));
        uvs.extend_from_slice(&geo.uvs);
        if geo.indices.is_empty() {
            indices.extend((0..geo.positions.len() as u32).map(|i| base + i));
        } else {
            indices.extend(geo.indices.iter().map(|i| base + i));
        }
    }
    Geometry::new(positions, normals, uvs, indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_match_three() {
        // three: new Color(0xff8000) → linear (1, 0.2158605, 0).
        let c = Color::hex(0xff8000);
        assert!((c.r - 1.0).abs() < 1e-5 && (c.g - 0.215_860_5).abs() < 1e-4 && c.b.abs() < 1e-6);
        let (h, s, l) = Color::hsl(0.3, 0.6, 0.4).to_hsl();
        assert!((h - 0.3).abs() < 1e-3 && (s - 0.6).abs() < 1e-3 && (l - 0.4).abs() < 1e-3);
        assert_eq!(Color::css("#ff8000"), Some(c));
        assert!(Color::css("rgb(255, 128, 0)").is_some());
    }

    #[test]
    fn flatten_composes_transforms_and_skips_hidden_and_splits_static_from_dynamic() {
        let geo = Geometry::new(vec![[0.0; 3]], vec![], vec![], vec![0]);
        let mat = Mat::toon(Color::WHITE);
        let root = Obj::group();
        let parent = Obj::group();
        parent.set_position(1.0, 0.0, 0.0).set_scalar(2.0);
        let child = Obj::mesh(&geo, &mat);
        child.set_position(1.0, 0.0, 0.0);
        parent.add(&child);
        root.add(&parent);
        let hidden = Obj::mesh(&geo, &mat);
        hidden.set_visible(false);
        root.add(&hidden);
        let mover = Obj::group();
        mover.set_dynamic(true).add(&Obj::mesh(&geo, &mat));
        root.add(&mover);
        let (all, _) = flatten(&root, Filter::All);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].model.transform_point3(Vec3::ZERO), Vec3::new(3.0, 0.0, 0.0));
        assert_eq!(flatten(&root, Filter::Static).0.len(), 1);
        assert_eq!(flatten(&root, Filter::Dynamic).0.len(), 1);
        assert_eq!(root.world_matrix_of(&child).map(|m| m.transform_point3(Vec3::ZERO)), Some(Vec3::new(3.0, 0.0, 0.0)));
    }

    #[test]
    fn dynamic_roots_are_the_topmost_dynamic_nodes_with_their_parent_matrix() {
        let geo = Geometry::new(vec![[0.0; 3]], vec![], vec![], vec![0]);
        let mat = Mat::toon(Color::WHITE);
        let root = Obj::group();
        let room = Obj::group();
        room.set_position(5.0, 0.0, 0.0);
        let fan = Obj::group();
        fan.set_dynamic(true).add(&Obj::mesh(&geo, &mat));
        room.add(&fan).add(&Obj::mesh(&geo, &mat));
        root.add(&room);
        let roots = dynamic_roots(&root, Mat4::IDENTITY);
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].1.transform_point3(Vec3::ZERO), Vec3::new(5.0, 0.0, 0.0));
        let (draws, _) = flatten_roots(&roots);
        assert_eq!(draws.len(), 1);
        assert_eq!(draws[0].model.transform_point3(Vec3::ZERO), Vec3::new(5.0, 0.0, 0.0));
    }

    #[test]
    fn bake_merges_meshes_sharing_a_material() {
        let geo = Geometry::new(vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], vec![], vec![], vec![0, 1, 2]);
        let (a, b) = (Mat::toon(Color::WHITE), Mat::basic(Color::BLACK));
        let root = Obj::group();
        for x in 0..3 {
            let m = Obj::mesh(&geo, &a);
            m.set_position(x as f32, 0.0, 0.0);
            root.add(&m);
        }
        root.add(&Obj::mesh(&geo, &b));
        let baked = bake(&root);
        assert_eq!(baked.len(), 2);
        assert_eq!(baked[0].geo.positions.len(), 9);
        assert_eq!(baked[0].geo.indices, vec![0, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(baked[0].geo.positions[3], [1.0, 0.0, 0.0]);
    }

    #[test]
    fn the_camera_projects_its_target_to_the_centre_and_its_ray_points_at_it() {
        let cam = Camera { eye: Vec3::new(0.0, 10.0, 10.0), target: Vec3::ZERO, fov: 40.0, near: 0.5, far: 1000.0, aspect: 2.0 };
        let (x, y) = cam.to_screen(Vec3::ZERO, 200.0, 100.0).unwrap();
        assert!((x - 100.0).abs() < 1e-3 && (y - 50.0).abs() < 1e-3);
        let (origin, dir) = cam.ray(100.0, 50.0, 200.0, 100.0);
        let to_target = (Vec3::ZERO - origin).normalize();
        assert!(dir.dot(to_target) > 0.9999);
    }
}
