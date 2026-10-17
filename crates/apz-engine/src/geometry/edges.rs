//! Feature edges: the lines a technical drawing would draw on a mesh (its silhouette creases and its open borders),
//! for the kit's line styles.

/// The feature edges of an indexed triangle mesh, as a line list: pairs of indices into `positions`.
///
/// Vertices that share a position (to within 1e-4 in each axis) are welded first, so a box with split vertices for
/// flat shading still knows which faces meet at an edge. An edge is kept when exactly one face uses it (an open
/// border), or when the two faces that share it meet at an angle of more than `threshold_deg` degrees between their
/// normals, or when more than two faces share it. Degenerate triangles (zero area) are ignored. Each kept edge appears
/// once, as the lower index first; the list is sorted, so the result is deterministic.
pub fn feature_edges(positions: &[[f32; 3]], indices: &[u32], threshold_deg: f32) -> Vec<u32> {
    let _ = (positions, indices, threshold_deg);
    todo!()
}
