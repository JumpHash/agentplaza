//! The ten machines of the kit. Each builder returns the scene for one `.glb` (see the crate docs for the names the
//! engine looks for, and [`common`] for the parts they are made of).

pub mod common;
mod devices;
mod racks;
mod towers;

use crate::glb::Scene;

/// Every model of the kit: its file stem and its scene, in a fixed order.
pub fn kit() -> Vec<(&'static str, Scene)> {
    vec![
        ("hub", racks::hub()),
        ("server", racks::server()),
        ("rack", racks::rack()),
        ("gpu", towers::gpu()),
        ("tower", towers::tower()),
        ("edge", towers::edge()),
        ("mini", devices::mini()),
        ("studio", devices::studio()),
        ("laptop", devices::laptop()),
        ("phone", devices::phone()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glb::Node;

    fn names(node: &Node, out: &mut Vec<String>) {
        out.push(node.name.clone());
        for c in &node.children {
            names(c, out);
        }
    }

    fn triangles(node: &Node) -> usize {
        node.mesh.as_ref().map_or(0, |(m, _)| m.triangles())
            + node.children.iter().map(triangles).sum::<usize>()
    }

    #[test]
    fn every_model_has_three_levels_a_status_strip_and_anchors_within_budget() {
        let kit = kit();
        assert_eq!(kit.len(), 10);
        for (stem, scene) in &kit {
            assert_eq!(scene.roots.len(), 3, "{stem}: lod0, lod1, lod2");
            let roots: Vec<&str> = scene.roots.iter().map(|r| r.name.as_str()).collect();
            assert_eq!(roots, ["lod0", "lod1", "lod2"], "{stem}");
            for root in &scene.roots {
                let mut all = Vec::new();
                names(root, &mut all);
                assert!(
                    all.iter().any(|n| n == "status"),
                    "{stem}/{}: a status strip",
                    root.name
                );
                assert!(
                    all.iter().any(|n| n == "anchor:label"),
                    "{stem}/{}: a label anchor",
                    root.name
                );
            }
            let (full, simple, blocky) = (
                triangles(&scene.roots[0]),
                triangles(&scene.roots[1]),
                triangles(&scene.roots[2]),
            );
            assert!(full <= 40_000, "{stem}: {full} triangles in full detail");
            assert!(
                simple < full,
                "{stem}: simple ({simple}) is lighter than full ({full})"
            );
            assert!(blocky <= 200, "{stem}: {blocky} triangles as a block");
            assert!(
                scene.extras["size"]
                    .as_array()
                    .is_some_and(|s| s.len() == 3),
                "{stem}: a size"
            );
            for node in &scene.roots {
                let mut stack = vec![node];
                while let Some(n) = stack.pop() {
                    if let Some((mesh, m)) = &n.mesh {
                        assert!(
                            *m < scene.materials.len(),
                            "{stem}/{}: material {m}",
                            n.name
                        );
                        assert!(!mesh.indices.is_empty(), "{stem}/{}: an empty mesh", n.name);
                    }
                    stack.extend(n.children.iter());
                }
            }
        }
    }

    #[test]
    fn every_model_writes_a_glb_that_parses() {
        for (stem, scene) in kit() {
            let bytes = crate::glb::write_glb(&scene);
            let doc = gltf::Gltf::from_slice(&bytes).unwrap_or_else(|e| panic!("{stem}: {e}"));
            assert!(doc.nodes().any(|n| n.name() == Some("status")), "{stem}");
        }
    }
}
