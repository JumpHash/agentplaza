//! The drawn tops' support field of a scene (`ground.ts` `supportField`): the upward faces of a root's solids, in the
//! root's own frame, bucketed by `FIELD_CELL` so `top_below` can find what stands under a point.

use apz_board::nav::drawn::{key_of, SupportField, FIELD_CELL};
use glam::{Mat4, Vec3};

use crate::scene::{Geometry, Kind, Mat, Obj};

type V = [f64; 3];

fn to_f64(v: Vec3) -> V {
    [v.x as f64, v.y as f64, v.z as f64]
}

fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: V, b: V) -> V {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// ground.ts supportField: the upward faces of `root`'s solids (meshes and each instance of an instanced mesh, not decals,
/// not additive glows), in root's frame (root's own local matrix included), bucketed by FIELD_CELL.
pub fn support_field(root: &Obj) -> SupportField {
    let mut field = SupportField { cell: FIELD_CELL, tris: Vec::new(), cells: Default::default() };
    collect(root, Mat4::IDENTITY, &mut field);
    field
}

fn collect(node: &Obj, parent: Mat4, field: &mut SupportField) {
    // `local_matrix` locks the node itself, so it is read before the guard below.
    let world = parent * node.local_matrix();
    let (kind, children) = {
        let n = node.get();
        (n.kind.clone(), n.children.clone())
    };
    match &kind {
        Kind::Mesh { geo, mat } if is_solid(mat) => add_geometry(geo, world, field),
        Kind::Instanced { geo, mat, instances, count } if is_solid(mat) => {
            for inst in instances.iter().take(*count) {
                add_geometry(geo, world * inst.matrix, field);
            }
        }
        _ => {}
    }
    for child in &children {
        collect(child, world, field);
    }
}

fn is_solid(mat: &Mat) -> bool {
    let m = mat.get();
    !m.decal && !m.additive
}

/// The upward triangles of `geo` under `m`, appended to `field`.
fn add_geometry(geo: &Geometry, m: Mat4, field: &mut SupportField) {
    let indexed = !geo.indices.is_empty();
    let count = (if indexed { geo.indices.len() } else { geo.positions.len() }) / 3;
    let corner = |t: usize, k: usize| -> usize {
        if indexed {
            geo.indices[t * 3 + k] as usize
        } else {
            t * 3 + k
        }
    };
    let cell = field.cell;
    for t in 0..count {
        let a = to_f64(m.transform_point3(Vec3::from(geo.positions[corner(t, 0)])));
        let b = to_f64(m.transform_point3(Vec3::from(geo.positions[corner(t, 1)])));
        let c = to_f64(m.transform_point3(Vec3::from(geo.positions[corner(t, 2)])));
        let n = cross(sub(b, a), sub(c, a));
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        // Only the upward faces hold things up.
        if len < 1e-12 || n[1] <= 0.5 * len {
            continue;
        }
        let index = field.tris.len() / 9;
        for p in [a, b, c] {
            field.tris.extend_from_slice(&p);
        }
        let minx = a[0].min(b[0]).min(c[0]);
        let maxx = a[0].max(b[0]).max(c[0]);
        let minz = a[2].min(b[2]).min(c[2]);
        let maxz = a[2].max(b[2]).max(c[2]);
        for i in (minx / cell).floor() as i64..=(maxx / cell).floor() as i64 {
            for j in (minz / cell).floor() as i64..=(maxz / cell).floor() as i64 {
                field.cells.entry(key_of(i, j)).or_default().push(index);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::support_field;
    use crate::scene::{Mat, Material, Obj};
    use apz_board::nav::drawn::{top_below, Support};

    /// A 2 x 1 x 2 box standing on the floor, under a group root.
    fn box_on_floor(decal: bool) -> Obj {
        let root = Obj::group();
        let mat = Mat::new(Material { decal, ..Material::default() });
        let mesh = Obj::mesh(&crate::geometry::box_geo(2.0, 1.0, 2.0), &mat);
        mesh.set_position(0.0, 0.5, 0.0);
        root.add(&mesh);
        root
    }

    #[test]
    fn the_top_of_a_box_holds_up_what_is_over_it() {
        let sup = Support { field: support_field(&box_on_floor(false)), scale: 1.0 };
        assert!((top_below(Some(&sup), 0.0, 0.0, f64::INFINITY) - 1.0).abs() < 1e-6);
        assert!(top_below(Some(&sup), 5.0, 5.0, f64::INFINITY).abs() < 1e-6);
    }

    #[test]
    fn a_decal_holds_nothing_up() {
        assert!(support_field(&box_on_floor(true)).tris.is_empty());
    }
}
