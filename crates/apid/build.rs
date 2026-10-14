//! Embeds the robot species the hub deals (`assets/robots/index.json`): one `include_str!` per
//! listed species file, generated into `OUT_DIR/species_files.rs`. A listed species whose file
//! is missing is embedded as an empty body (the build still works while an author is mid-way;
//! the apid test `every_listed_species_has_a_file` fails until the file lands).

use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let robots = manifest.join("..").join("..").join("assets").join("robots");
    let index = robots.join("index.json");
    let species_dir = robots.join("species");
    println!("cargo:rerun-if-changed={}", index.display());
    println!("cargo:rerun-if-changed={}", species_dir.display());

    let text =
        fs::read_to_string(&index).unwrap_or_else(|e| panic!("read {}: {e}", index.display()));
    let doc: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", index.display()));
    let ids = doc["species"]
        .as_array()
        .unwrap_or_else(|| panic!("{}: no species array", index.display()));

    let mut out = String::from(
        "/// Every species listed in `assets/robots/index.json`, in index order: (id, file body).\npub const SPECIES_FILES: &[(&str, &str)] = &[\n",
    );
    for id in ids {
        let id = id
            .as_str()
            .unwrap_or_else(|| panic!("{}: species ids are strings", index.display()));
        assert!(
            !id.is_empty()
                && id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "species id {id:?} must be lowercase letters, digits and dashes"
        );
        let file = species_dir.join(format!("{id}.json"));
        if file.is_file() {
            println!("cargo:rerun-if-changed={}", file.display());
            out.push_str(&format!(
                "    ({id:?}, include_str!({:?})),\n",
                path_str(&file)
            ));
        } else {
            out.push_str(&format!("    ({id:?}, \"\"),\n"));
        }
    }
    out.push_str("];\n");

    let target = PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"))
        .join("species_files.rs");
    fs::write(&target, out).unwrap_or_else(|e| panic!("write {}: {e}", target.display()));
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("species paths are UTF-8")
}
