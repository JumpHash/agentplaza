//! Feature edges: the lines a technical drawing would draw on a mesh (its silhouette creases and its open borders),
//! for the kit's line styles.

use std::collections::HashMap;

/// Positions are welded when they agree to this many units in every axis.
const WELD_STEP: f32 = 1.0e-4;

/// The feature edges of an indexed triangle mesh, as a line list: pairs of indices into `positions`.
///
/// Vertices that share a position (to within 1e-4 in each axis) are welded first, so a box with split vertices for
/// flat shading still knows which faces meet at an edge. An edge is kept when exactly one face uses it (an open
/// border), or when the two faces that share it meet at an angle of more than `threshold_deg` degrees between their
/// normals, or when more than two faces share it. Degenerate triangles (zero area) are ignored. Each kept edge appears
/// once, as the lower index first; the list is sorted, so the result is deterministic.
pub fn feature_edges(positions: &[[f32; 3]], indices: &[u32], threshold_deg: f32) -> Vec<u32> {
    let cos_threshold = threshold_deg.to_radians().cos();

    // Weld: each position, quantised to the weld step, names one canonical vertex: the first index seen there.
    let mut cells: HashMap<[i64; 3], u32> = HashMap::with_capacity(positions.len());
    let quantise = |c: f32| (c / WELD_STEP).round() as i64;
    let canonical: Vec<u32> = positions
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let cell = [quantise(p[0]), quantise(p[1]), quantise(p[2])];
            *cells.entry(cell).or_insert(i as u32)
        })
        .collect();

    // Each undirected edge (by canonical pair) with the unit normals of the faces that use it.
    let mut faces_by_edge: HashMap<(u32, u32), Vec<[f32; 3]>> = HashMap::new();
    for tri in indices.chunks_exact(3) {
        let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        let (a, b, c) = (positions[i0], positions[i1], positions[i2]);
        let normal = cross(sub(b, a), sub(c, a));
        let len = dot(normal, normal).sqrt();
        if len == 0.0 {
            continue;
        }
        let n = [normal[0] / len, normal[1] / len, normal[2] / len];

        let corners = [canonical[i0], canonical[i1], canonical[i2]];
        for (x, y) in [(corners[0], corners[1]), (corners[1], corners[2]), (corners[2], corners[0])] {
            if x == y {
                continue;
            }
            let key = if x < y { (x, y) } else { (y, x) };
            faces_by_edge.entry(key).or_default().push(n);
        }
    }

    let mut pairs: Vec<(u32, u32)> = faces_by_edge
        .into_iter()
        .filter(|(_, normals)| match normals.as_slice() {
            [n0, n1] => dot(*n0, *n1) < cos_threshold,
            _ => true,
        })
        .map(|(edge, _)| edge)
        .collect();
    pairs.sort_unstable();
    pairs.into_iter().flat_map(|(a, b)| [a, b]).collect()
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[cfg(test)]
mod tests {
    use super::feature_edges;

    /// A unit cube centred on the origin, flat-shaded: 4 split vertices per face, so 24 positions and 12 triangles.
    fn split_cube() -> (Vec<[f32; 3]>, Vec<u32>) {
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        for axis in 0..3 {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for sign in [-0.5f32, 0.5] {
                let base = positions.len() as u32;
                for (a, b) in [(-0.5f32, -0.5f32), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
                    let mut p = [0.0f32; 3];
                    p[axis] = sign;
                    p[u] = a;
                    p[v] = b;
                    positions.push(p);
                }
                indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
            }
        }
        (positions, indices)
    }

    /// A flat 2x2 grid of quads in the z = 0 plane, two counter-clockwise triangles each, with shared vertices.
    /// The point (i, j) is position `i * 3 + j`.
    fn flat_grid() -> (Vec<[f32; 3]>, Vec<u32>) {
        let mut positions = Vec::new();
        for i in 0..3 {
            for j in 0..3 {
                positions.push([i as f32, j as f32, 0.0]);
            }
        }
        let mut indices = Vec::new();
        for i in 0..2u32 {
            for j in 0..2u32 {
                let a = i * 3 + j;
                let (b, c, d) = (a + 3, a + 4, a + 1);
                indices.extend([a, b, c, a, c, d]);
            }
        }
        (positions, indices)
    }

    #[test]
    fn cube_has_twelve_creases_at_30_degrees() {
        let (positions, indices) = split_cube();
        let edges = feature_edges(&positions, &indices, 30.0);
        assert_eq!(edges.len(), 24, "12 cube edges, as pairs");
        for pair in edges.chunks_exact(2) {
            assert!(pair[0] < pair[1], "lower index first");
        }
    }

    #[test]
    fn flat_grid_keeps_only_its_border() {
        let (positions, indices) = flat_grid();
        let edges = feature_edges(&positions, &indices, 30.0);
        assert_eq!(edges, vec![0, 1, 0, 3, 1, 2, 2, 5, 3, 6, 5, 8, 6, 7, 7, 8]);
    }

    #[test]
    fn same_input_gives_same_edges() {
        let (positions, indices) = split_cube();
        let first = feature_edges(&positions, &indices, 30.0);
        let second = feature_edges(&positions, &indices, 30.0);
        assert_eq!(first, second);
    }

    #[test]
    fn empty_mesh_has_no_edges() {
        assert!(feature_edges(&[], &[], 30.0).is_empty());
    }
}
