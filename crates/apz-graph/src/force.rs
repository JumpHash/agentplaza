//! A force-directed layout for graphs whose nodes have no natural order: Fruchterman and Reingold's
//! spring model, run for a fixed number of steps on a cooling schedule. The web runs it as wasm
//! (crates/apz-graph-web) and the desktop natively, so one graph lays out the same way on both consoles.
//!
//! The arithmetic is plain IEEE f32 (addition, multiplication, division and square root) in a fixed
//! order, and the nodes start on a circle in index order, so no random seed is needed. The circle's
//! angles come from [`cos_sin`], which uses no platform trigonometry, so native and wasm agree bit for bit.
//!
//! Connected components are laid out apart and packed left to right, the largest first, each in a
//! slice of the box as wide as its share of the nodes. A single node sits at the centre of the box.

use std::f32::consts::TAU;

/// The steps a layout runs when the caller names none.
pub const DEFAULT_ITERATIONS: u32 = 300;

/// The node size a layout keeps clear of the box's edges when the caller names none.
pub const DEFAULT_NODE_SIZE: f32 = 28.0;

/// The box a layout fills, and the size of its nodes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForceOptions {
    /// The box's width and height.
    pub width: f32,
    pub height: f32,
    /// The steps of the simulation. More settle a larger graph better, at a cost in proportion.
    pub iterations: u32,
    /// The size of a node: each centre stays this far inside the box's edges.
    pub node_size: f32,
}

/// Lays out `node_count` nodes joined by `edges` (pairs of node indices) in the box `opts` names. Returns
/// one centre per node, in index order, each inside `[node_size, width - node_size] x [node_size,
/// height - node_size]`.
///
/// An edge naming a node out of range, a node joined to itself and a repeated edge are left out, so a
/// graph goes in as it comes. A box too small for its node size holds every node at its middle. Sizes
/// that are not finite, or are negative, count as 0.
pub fn layout(node_count: usize, edges: &[(usize, usize)], opts: &ForceOptions) -> Vec<(f32, f32)> {
    if node_count == 0 {
        return Vec::new();
    }
    let width = sane(opts.width);
    let height = sane(opts.height);
    let node_size = sane(opts.node_size);
    if node_count == 1 {
        return vec![(width / 2.0, height / 2.0)];
    }
    let (min_x, max_x) = span(0.0, width, node_size);
    let (min_y, max_y) = span(0.0, height, node_size);

    let mut pairs: Vec<(usize, usize)> = edges
        .iter()
        .filter(|&&(a, b)| a != b && a < node_count && b < node_count)
        .map(|&(a, b)| (a.min(b), a.max(b)))
        .collect();
    pairs.sort_unstable();
    pairs.dedup();

    // The connected components, each with its nodes in index order. A component's first node is its
    // smallest index, so the components come out in index order too.
    let mut adjacent: Vec<Vec<usize>> = vec![Vec::new(); node_count];
    for &(a, b) in &pairs {
        adjacent[a].push(b);
        adjacent[b].push(a);
    }
    let mut component_of = vec![usize::MAX; node_count];
    let mut components: Vec<Vec<usize>> = Vec::new();
    for start in 0..node_count {
        if component_of[start] != usize::MAX {
            continue;
        }
        let id = components.len();
        component_of[start] = id;
        let mut members = vec![start];
        let mut next = 0;
        while next < members.len() {
            let node = members[next];
            next += 1;
            for &other in &adjacent[node] {
                if component_of[other] == usize::MAX {
                    component_of[other] = id;
                    members.push(other);
                }
            }
        }
        members.sort_unstable();
        components.push(members);
    }
    // Each node's index within its component, and each component's edges in those indices.
    let mut local = vec![0; node_count];
    for members in &components {
        for (at, &node) in members.iter().enumerate() {
            local[node] = at;
        }
    }
    let mut component_edges: Vec<Vec<(usize, usize)>> = vec![Vec::new(); components.len()];
    for &(a, b) in &pairs {
        component_edges[component_of[a]].push((local[a], local[b]));
    }

    // The largest component goes first, on the left. The sort is stable, so equal sizes keep index order.
    let mut order: Vec<usize> = (0..components.len()).collect();
    order.sort_by_key(|&id| std::cmp::Reverse(components[id].len()));

    let half_gap = if components.len() > 1 {
        node_size / 2.0
    } else {
        0.0
    };
    let mut positions = vec![(0.0_f32, 0.0_f32); node_count];
    let mut start = min_x;
    for &id in &order {
        let members = &components[id];
        let share = (max_x - min_x) * members.len() as f32 / node_count as f32;
        let end = start + share;
        let (low, high) = span(start + half_gap, end - half_gap, 0.0);
        let placed = relax(
            members.len(),
            &component_edges[id],
            (low, high),
            (min_y, max_y),
            opts.iterations,
        );
        for (at, &node) in members.iter().enumerate() {
            positions[node] = placed[at];
        }
        start = end;
    }
    for point in &mut positions {
        point.0 = point.0.clamp(min_x, max_x);
        point.1 = point.1.clamp(min_y, max_y);
    }
    positions
}

/// The Fruchterman and Reingold steps for `count` nodes joined by `edges` (indices within the
/// component), kept inside the box `x` by `y`. The nodes start on a circle in the box's middle.
///
/// Each step, every pair of nodes repels and every edge pulls its ends together, the shift of each node
/// is capped by a temperature that falls to 0 over the steps, and each node moves by its shift.
fn relax(
    count: usize,
    edges: &[(usize, usize)],
    x: (f32, f32),
    y: (f32, f32),
    iterations: u32,
) -> Vec<(f32, f32)> {
    let (mid_x, mid_y) = ((x.0 + x.1) / 2.0, (y.0 + y.1) / 2.0);
    if count == 1 {
        return vec![(mid_x, mid_y)];
    }
    let (width, height) = (x.1 - x.0, y.1 - y.0);
    let radius = 0.45 * width.min(height);
    let mut pos: Vec<(f32, f32)> = (0..count)
        .map(|at| {
            let (cos, sin) = cos_sin(TAU * at as f32 / count as f32);
            (mid_x + radius * cos, mid_y + radius * sin)
        })
        .collect();

    // The ideal distance between two nodes: the box's area shared out evenly.
    let k = (width * height / count as f32).sqrt().max(1e-3);
    let start_temperature = width.max(height) / 10.0;
    for step in 0..iterations {
        let temperature = start_temperature * (1.0 - step as f32 / iterations as f32);
        let mut shift = vec![(0.0_f32, 0.0_f32); count];
        for a in 0..count {
            for b in a + 1..count {
                let dx = pos[a].0 - pos[b].0;
                let dy = pos[a].1 - pos[b].1;
                let push = k * k / (dx * dx + dy * dy).max(1e-6);
                shift[a].0 += dx * push;
                shift[a].1 += dy * push;
                shift[b].0 -= dx * push;
                shift[b].1 -= dy * push;
            }
        }
        for &(a, b) in edges {
            let dx = pos[a].0 - pos[b].0;
            let dy = pos[a].1 - pos[b].1;
            let pull = (dx * dx + dy * dy).sqrt() / k;
            shift[a].0 -= dx * pull;
            shift[a].1 -= dy * pull;
            shift[b].0 += dx * pull;
            shift[b].1 += dy * pull;
        }
        for (point, &(sx, sy)) in pos.iter_mut().zip(&shift) {
            let length = (sx * sx + sy * sy).sqrt();
            let scale = if length > temperature && length > 0.0 {
                temperature / length
            } else {
                1.0
            };
            point.0 = (point.0 + sx * scale).clamp(x.0, x.1);
            point.1 = (point.1 + sy * scale).clamp(y.0, y.1);
        }
    }
    pos
}

/// The cosine and sine of `angle` (radians), by Taylor series after folding the angle into one turn.
/// Only additions, multiplications and divisions take part, which IEEE f32 rounds the same way on every
/// platform, where the libm trigonometry may differ in its last bit between native and wasm.
fn cos_sin(angle: f32) -> (f32, f32) {
    let turns = (angle / TAU + 0.5).floor();
    let x = angle - TAU * turns;
    let x2 = x * x;
    let (mut sin, mut cos) = (0.0_f32, 0.0_f32);
    let (mut sin_term, mut cos_term) = (x, 1.0_f32);
    for n in 0..12_u32 {
        let n = n as f32;
        sin += sin_term;
        cos += cos_term;
        sin_term *= -x2 / ((2.0 * n + 2.0) * (2.0 * n + 3.0));
        cos_term *= -x2 / ((2.0 * n + 1.0) * (2.0 * n + 2.0));
    }
    (cos, sin)
}

/// The range `[start + inset, end - inset]`, or its middle when the inset leaves no room.
fn span(start: f32, end: f32, inset: f32) -> (f32, f32) {
    let (low, high) = (start + inset, end - inset);
    if low <= high {
        (low, high)
    } else {
        let mid = (start + end) / 2.0;
        (mid, mid)
    }
}

/// The size as given when it is a finite, positive number, else 0.
fn sane(value: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(width: f32, height: f32) -> ForceOptions {
        ForceOptions {
            width,
            height,
            iterations: DEFAULT_ITERATIONS,
            node_size: DEFAULT_NODE_SIZE,
        }
    }

    fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
        ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
    }

    /// Thirty nodes in a ring, then a path of three, then three nodes with no links.
    fn mixed() -> (usize, Vec<(usize, usize)>) {
        let mut edges: Vec<(usize, usize)> = (0..30).map(|at| (at, (at + 1) % 30)).collect();
        edges.push((30, 31));
        edges.push((31, 32));
        (36, edges)
    }

    #[test]
    fn the_same_graph_lays_out_the_same_way_twice() {
        let (count, edges) = mixed();
        let first = layout(count, &edges, &opts(900.0, 600.0));
        let second = layout(count, &edges, &opts(900.0, 600.0));
        assert_eq!(first, second);
    }

    #[test]
    fn every_position_is_inside_the_box() {
        let (count, edges) = mixed();
        let positions = layout(count, &edges, &opts(900.0, 600.0));
        assert_eq!(positions.len(), count);
        for (x, y) in positions {
            assert!((28.0..=872.0).contains(&x), "x {x} is outside the box");
            assert!((28.0..=572.0).contains(&y), "y {y} is outside the box");
        }
    }

    #[test]
    fn connected_nodes_end_closer_than_unconnected_ones() {
        let positions = layout(4, &[(0, 1), (1, 2), (2, 3)], &opts(800.0, 600.0));
        assert!(distance(positions[0], positions[1]) < distance(positions[0], positions[3]));
    }

    #[test]
    fn a_ring_of_thirty_keeps_its_nodes_apart() {
        let edges: Vec<(usize, usize)> = (0..30).map(|at| (at, (at + 1) % 30)).collect();
        let positions = layout(30, &edges, &opts(800.0, 600.0));
        for a in 0..30 {
            for b in a + 1..30 {
                let apart = distance(positions[a], positions[b]);
                assert!(apart >= 14.0, "nodes {a} and {b} are {apart} apart");
            }
        }
    }

    #[test]
    fn components_are_packed_left_to_right_largest_first() {
        // Node 0 alone, nodes 1 to 3 in a path: the path goes on the left.
        let positions = layout(4, &[(1, 2), (2, 3)], &opts(800.0, 600.0));
        let rightmost_path = positions[1].0.max(positions[2].0).max(positions[3].0);
        assert!(positions[0].0 > rightmost_path);
    }

    #[test]
    fn an_empty_graph_has_no_positions_and_one_node_sits_at_the_centre() {
        assert!(layout(0, &[], &opts(800.0, 600.0)).is_empty());
        assert_eq!(layout(1, &[], &opts(800.0, 600.0)), vec![(400.0, 300.0)]);
    }

    #[test]
    fn edges_that_name_no_node_or_loop_back_are_left_out() {
        let with_junk = layout(2, &[(0, 9), (1, 1), (0, 1), (1, 0)], &opts(800.0, 600.0));
        let clean = layout(2, &[(0, 1)], &opts(800.0, 600.0));
        assert_eq!(with_junk, clean);
    }
}
