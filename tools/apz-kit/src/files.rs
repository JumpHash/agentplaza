//! The kit's files as `apz-kit build` writes them: one `.glb` per model, the textures as KTX2 and `manifest.json`.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::glb::{Node, write_glb};
use crate::ktx2w::{mip_chain, write_ktx2};
use crate::models::{common::GRILLE_TEXTURE, kit};
use crate::tex::grille;

/// The grille texture's size in texels (a multiple of 32, so it tiles).
const GRILLE_SIZE: u32 = 1024;

fn triangles(node: &Node) -> usize {
    node.mesh.as_ref().map_or(0, |(m, _)| m.triangles())
        + node.children.iter().map(triangles).sum::<usize>()
}

/// Every file of the kit: its name and bytes, in a fixed order.
pub fn files() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut models = Vec::new();
    for (stem, scene) in kit() {
        let file = format!("{stem}.glb");
        let lods: Vec<usize> = scene.roots.iter().map(triangles).collect();
        models.push(json!({ "id": stem, "file": file, "kind": scene.extras["kind"], "size": scene.extras["size"], "triangles": lods }));
        out.push((file, write_glb(&scene)));
    }
    out.push((
        GRILLE_TEXTURE.to_string(),
        write_ktx2(&mip_chain(&grille(GRILLE_SIZE), true), true),
    ));
    let manifest: Value = json!({ "schema": 1, "models": models, "textures": [GRILLE_TEXTURE] });
    let mut text = serde_json::to_string_pretty(&manifest).expect("the manifest serializes");
    text.push('\n');
    out.push(("manifest.json".to_string(), text.into_bytes()));
    out
}

/// Where the kit is committed: `assets/hardware/kit`.
pub fn default_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/hardware/kit")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed kit is what this code builds: a change to a model or a texture comes with `apz-kit build`'s output.
    #[test]
    fn the_committed_kit_is_up_to_date() {
        let dir = default_dir();
        let mut stale = Vec::new();
        for (name, built) in files() {
            let committed = std::fs::read(dir.join(&name)).unwrap_or_default();
            if committed == built {
                continue;
            }
            // Where the two first differ, so a failure on another machine says what changed.
            let at = committed.iter().zip(&built).position(|(a, b)| a != b);
            let around =
                |v: &[u8], i: usize| v[i.saturating_sub(8)..(i + 24).min(v.len())].to_vec();
            let detail = match at {
                Some(i) => format!(
                    "first differs at byte {i}: committed {:?}, built {:?}",
                    around(&committed, i),
                    around(&built, i)
                ),
                None => String::from("one is a prefix of the other"),
            };
            stale.push(format!(
                "{name} ({} bytes committed, {} built; {detail})",
                committed.len(),
                built.len()
            ));
            if name.ends_with(".json") {
                stale.push(format!("built {name}: {}", String::from_utf8_lossy(&built)));
            }
        }
        assert!(
            stale.is_empty(),
            "out of date in assets/hardware/kit (run `cargo run -p apz-kit -- build`):\n{}",
            stale.join("\n")
        );
    }
}
