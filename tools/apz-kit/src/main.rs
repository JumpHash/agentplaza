//! `apz-kit build [out]`: writes the machine model kit into `out` (default `assets/hardware/kit`): one `.glb` per model,
//! the textures as KTX2 and `manifest.json` (each model's file, kind, size and triangles per level of detail).
//! `apz-kit check [dir]`: rebuilds the kit in memory and fails when a file in `dir` differs from it (CI keeps the
//! committed kit in step with this code).

use std::path::PathBuf;
use std::process::ExitCode;

use apz_kit::files::{default_dir, files};

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
                eprintln!(
                    "apz-kit: out of date in {}: {} (run `cargo run -p apz-kit -- build`)",
                    dir.display(),
                    stale.join(", ")
                );
                ExitCode::FAILURE
            }
        }
        other => {
            eprintln!("apz-kit: unknown command {other:?} (build or check)");
            ExitCode::FAILURE
        }
    }
}
