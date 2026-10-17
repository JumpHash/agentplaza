//! `apz-kit build [out]`: writes the machine model kit into `out` (default `assets/hardware/kit`): one `.glb` per model,
//! the textures as KTX2 and `manifest.json` (each model's file, kind, size and triangles per level of detail).
//! `apz-kit check [dir]`: rebuilds the kit in memory and fails when a file in `dir` differs from it (CI keeps the
//! committed kit in step with this code).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use apz_kit::glb::{Node, write_glb};
use apz_kit::ktx2w::{mip_chain, write_ktx2};
use apz_kit::models::{common::GRILLE_TEXTURE, kit};
use apz_kit::tex::grille;
use serde_json::{Value, json};

/// The grille texture's size in texels (a multiple of 32, so it tiles).
const GRILLE_SIZE: u32 = 1024;

fn triangles(node: &Node) -> usize {
    node.mesh.as_ref().map_or(0, |(m, _)| m.triangles()) + node.children.iter().map(triangles).sum::<usize>()
}

/// Every file of the kit: its name and bytes, in a fixed order.
fn files() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut models = Vec::new();
    for (stem, scene) in kit() {
        let file = format!("{stem}.glb");
        let lods: Vec<usize> = scene.roots.iter().map(triangles).collect();
        models.push(json!({ "id": stem, "file": file, "kind": scene.extras["kind"], "size": scene.extras["size"], "triangles": lods }));
        out.push((file, write_glb(&scene)));
    }
    out.push((GRILLE_TEXTURE.to_string(), write_ktx2(&mip_chain(&grille(GRILLE_SIZE), true), true)));
    let manifest: Value = json!({ "schema": 1, "models": models, "textures": [GRILLE_TEXTURE] });
    let mut text = serde_json::to_string_pretty(&manifest).expect("the manifest serializes");
    text.push('\n');
    out.push(("manifest.json".to_string(), text.into_bytes()));
    out
}

fn default_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/hardware/kit")
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("build");
    let dir = args.get(1).map_or_else(default_dir, PathBuf::from);
    match command {
        "build" => {
            if let Err(e) = std::fs::create_dir_all(&dir) {
                eprintln!("apz-kit: cannot create {}: {e}", dir.display());
                return ExitCode::FAILURE;
            }
            for (name, bytes) in files() {
                let path = dir.join(&name);
                if let Err(e) = std::fs::write(&path, &bytes) {
                    eprintln!("apz-kit: cannot write {}: {e}", path.display());
                    return ExitCode::FAILURE;
                }
                println!("{name}: {} bytes", bytes.len());
            }
            ExitCode::SUCCESS
        }
        "check" => {
            let mut stale = Vec::new();
            for (name, bytes) in files() {
                if std::fs::read(dir.join(&name)).ok().as_deref() != Some(bytes.as_slice()) {
                    stale.push(name);
                }
            }
            if stale.is_empty() {
                println!("apz-kit: the kit in {} is up to date", dir.display());
                ExitCode::SUCCESS
            } else {
                eprintln!("apz-kit: out of date in {}: {} (run `cargo run -p apz-kit -- build`)", dir.display(), stale.join(", "));
                ExitCode::FAILURE
            }
        }
        other => {
            eprintln!("apz-kit: unknown command {other:?} (build or check)");
            ExitCode::FAILURE
        }
    }
}
