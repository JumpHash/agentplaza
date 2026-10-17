//! Mesh primitives for the kit: indexed triangles with normals and uvs, y up, right-handed (as glTF).

use glam::{Mat4, Quat, Vec3};

/// Indexed triangles: one normal and one uv per position, counter-clockwise front faces.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

impl Mesh {
    /// Adds `other`'s triangles to this mesh.
    pub fn append(&mut self, other: &Mesh) {
        todo!("append {} triangles", other.triangles())
    }

    /// This mesh with every position carried by `m` and every normal by its inverse transpose (renormalised).
    pub fn transformed(&self, m: Mat4) -> Mesh {
        let _ = m;
        todo!()
    }

    pub fn translated(&self, t: Vec3) -> Mesh {
        self.transformed(Mat4::from_translation(t))
    }

    pub fn rotated(&self, q: Quat) -> Mesh {
        self.transformed(Mat4::from_quat(q))
    }

    /// The axis-aligned bounds (min, max); (0, 0) for an empty mesh.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        todo!()
    }

    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }
}

/// A box `w` (x) by `h` (y) by `d` (z), centred on the origin, flat-shaded (24 vertices, 12 triangles).
pub fn cuboid(w: f32, h: f32, d: f32) -> Mesh {
    let _ = (w, h, d);
    todo!()
}

/// A box `w` by `h` by `d` centred on the origin whose footprint (x, z) has corners rounded with radius `r`, and whose
/// top and bottom edges are bevelled by `bevel` (a quarter round in 3 segments). `r` is clamped to just under half the
/// smaller of `w` and `d`, `bevel` to under a quarter of `h`; `bevel` 0 gives sharp top and bottom edges. Each rounded
/// corner has 8 segments. Normals are smooth around the corners and the bevels and flat on the caps.
pub fn rounded_box(w: f32, h: f32, d: f32, r: f32, bevel: f32) -> Mesh {
    let _ = (w, h, d, r, bevel);
    todo!()
}

/// A cylinder along y, centred on the origin, radius `r_top` at +h/2 and `r_bottom` at -h/2, with caps.
pub fn cylinder(r_top: f32, r_bottom: f32, h: f32, segments: u32) -> Mesh {
    let _ = (r_top, r_bottom, h, segments);
    todo!()
}

/// A torus in the xy plane (its axis along z), ring radius `r`, tube radius `tube`.
pub fn torus(r: f32, tube: f32, radial: u32, tubular: u32) -> Mesh {
    let _ = (r, tube, radial, tubular);
    todo!()
}

/// A flat ring (annulus) in the xy plane facing +z.
pub fn ring(r_in: f32, r_out: f32, segments: u32) -> Mesh {
    let _ = (r_in, r_out, segments);
    todo!()
}

/// A `w` by `h` rectangle in the xy plane facing +z, uv (0, 0) at the bottom left and (1, 1) at the top right.
pub fn plane(w: f32, h: f32) -> Mesh {
    let _ = (w, h);
    todo!()
}

/// A uv sphere of radius `r`.
pub fn sphere(r: f32, width_segments: u32, height_segments: u32) -> Mesh {
    let _ = (r, width_segments, height_segments);
    todo!()
}
