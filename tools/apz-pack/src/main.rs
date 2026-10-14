//! apz-pack: packages the AgentPlaza web engine for the browser.
//!
//! `apz-pack web-engine --out <dir>` builds crates/apz-engine-web for wasm32 with the
//! wasm-release profile, binds it with wasm-bindgen (web target, as a library at the version
//! Cargo.lock pins), and writes the glue and the wasm into <dir> under content-hashed names, with
//! `.br` and `.gz` siblings. `apz-pack web-ide --out <dir>` does the same for crates/apz-ide-web,
//! the IDE model's bindings. Built with `--features wasm-opt` it also runs wasm-opt -Oz and keeps
//! that output only when it is smaller over the wire (brotli): an LTO'd opt-level z build can come
//! out of -Oz smaller raw but larger compressed. Each prints one JSON line with the names and sizes.
//!
//! `apz-pack compress <dir>` writes `.br` and `.gz` siblings for every js, css, wasm, html and svg
//! file in <dir>, for the hub to serve to browsers that accept them.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use wasm_bindgen_cli_support::Bindgen;

/// A browser crate the packer builds: its package name, and its library name (the stem of its glue and wasm files).
struct WebCrate {
    package: &'static str,
    lib: &'static str,
}

const ENGINE: WebCrate = WebCrate { package: "apz-engine-web", lib: "apz_engine_web" };
const IDE: WebCrate = WebCrate { package: "apz-ide-web", lib: "apz_ide_web" };

/// Extensions that get precompressed siblings. Fonts and images are already compressed.
const COMPRESSIBLE: [&str; 5] = ["js", "css", "wasm", "html", "svg"];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(line) => println!("{line}"),
        Err(e) => {
            eprintln!("apz-pack: {e}");
            std::process::exit(1);
        }
    }
}

fn run(args: &[String]) -> Result<String, String> {
    match args {
        [cmd, flag, out] if cmd == "web-engine" && flag == "--out" => web_crate(Path::new(out), &ENGINE),
        [cmd, flag, out] if cmd == "web-ide" && flag == "--out" => web_crate(Path::new(out), &IDE),
        [cmd, dir] if cmd == "compress" => compress_dir(Path::new(dir)),
        _ => Err("usage: apz-pack web-engine|web-ide --out <dir> | apz-pack compress <dir>".to_string()),
    }
}

/// The repository root: the working directory when it is the repository (build.mjs runs apz-pack from there), else the
/// path this binary was built from. A cached binary built in another checkout (a git worktree) must not send the
/// engine build to that checkout, which may be gone.
fn repo_root() -> PathBuf {
    if let Ok(cwd) = std::env::current_dir() {
        if cwd.join("tools/apz-pack/Cargo.toml").is_file() {
            return cwd;
        }
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn web_crate(out: &Path, krate: &WebCrate) -> Result<String, String> {
    let lib = krate.lib;
    let root = repo_root();
    build_wasm(&root, krate.package)?;

    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), |t| root.join(t));
    let wasm_in = target.join("wasm32-unknown-unknown/wasm-release").join(format!("{lib}.wasm"));
    let scratch = target.join("apz-pack").join(lib);
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).map_err(|e| format!("scratch dir: {e}"))?;

    // wasm-bindgen as a library, with the options of `wasm-bindgen --target web` (which keeps the
    // default module path: the glue fetches its wasm by file name next to itself).
    let mut bindgen = Bindgen::new();
    bindgen.input_path(&wasm_in).out_name(lib).typescript(false).omit_default_module_path(false);
    bindgen.web(true).map_err(|e| format!("wasm-bindgen: {e:#}"))?;
    bindgen.generate(&scratch).map_err(|e| format!("wasm-bindgen: {e:#}"))?;

    let bindgen_wasm = scratch.join(format!("{lib}_bg.wasm"));
    let plain = fs::read(&bindgen_wasm).map_err(|e| format!("read {}: {e}", bindgen_wasm.display()))?;
    let plain_br = brotli_bytes(&plain)?;
    let (wasm, wasm_br, wasm_opt) = match optimise(&scratch, lib, &bindgen_wasm)? {
        Some(opt) => {
            let opt_br = brotli_bytes(&opt)?;
            if opt_br.len() < plain_br.len() { (opt, opt_br, "kept") } else { (plain, plain_br, "larger") }
        }
        None => (plain, plain_br, "off"),
    };

    // Content-hashed names: the glue points at the fingerprinted wasm, rewritten exactly once.
    let hash = fingerprint(&wasm);
    let (js_name, wasm_name) = names(lib, &hash);
    let glue_path = scratch.join(format!("{lib}.js"));
    let glue_raw = fs::read_to_string(&glue_path).map_err(|e| format!("read {}: {e}", glue_path.display()))?;
    let glue = rewrite_glue(&glue_raw, &format!("'{lib}_bg.wasm'"), &format!("'{wasm_name}'"))?;

    fs::create_dir_all(out).map_err(|e| format!("out dir: {e}"))?;
    let wasm_gz = gzip_bytes(&wasm)?;
    write(&out.join(&wasm_name), &wasm)?;
    write(&out.join(format!("{wasm_name}.br")), &wasm_br)?;
    write(&out.join(format!("{wasm_name}.gz")), &wasm_gz)?;
    write(&out.join(&js_name), glue.as_bytes())?;
    let _ = fs::remove_dir_all(&scratch);

    Ok(serde_json::json!({
        "js": js_name,
        "wasm": wasm_name,
        "bytes": wasm.len(),
        "gzip": wasm_gz.len(),
        "brotli": wasm_br.len(),
        "wasm_opt": wasm_opt,
    })
    .to_string())
}

// wasm-opt -Oz on the wasm-bindgen output (the bytes the glue loads); `None` without the feature.
#[cfg(feature = "wasm-opt")]
fn optimise(scratch: &Path, lib: &str, bindgen_wasm: &Path) -> Result<Option<Vec<u8>>, String> {
    let opt_wasm = scratch.join(format!("{lib}_opt.wasm"));
    let mut opts = wasm_opt::OptimizationOptions::new_optimize_for_size_aggressively();
    opts.all_features().debug_info(false);
    opts.run(bindgen_wasm, &opt_wasm).map_err(|e| format!("wasm-opt: {e}"))?;
    fs::read(&opt_wasm).map(Some).map_err(|e| format!("read {}: {e}", opt_wasm.display()))
}

#[cfg(not(feature = "wasm-opt"))]
fn optimise(_scratch: &Path, _lib: &str, _bindgen_wasm: &Path) -> Result<Option<Vec<u8>>, String> {
    Ok(None)
}

// Runs cargo for the crate. Its stdout goes to our stderr so the JSON line is the only stdout.
fn build_wasm(root: &Path, package: &str) -> Result<(), String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .args(["build", "-p", package, "--target", "wasm32-unknown-unknown", "--profile", "wasm-release"])
        .current_dir(root)
        .stdout(Stdio::from(std::io::stderr()))
        .status()
        .map_err(|e| format!("cargo: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("cargo build of {package} failed: {status}")) }
}

// Writes `.br` and `.gz` next to each compressible file in `dir` (not recursive), dropping stale
// siblings whose source is gone. A sibling that would not be smaller than its source is not written.
fn compress_dir(dir: &Path) -> Result<String, String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
    let mut names: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_file()).collect();
    names.sort();
    let (mut files, mut raw, mut br_total) = (0usize, 0usize, 0usize);
    for path in &names {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext == "br" || ext == "gz" {
            if !path.with_extension("").exists() {
                fs::remove_file(path).map_err(|e| format!("remove {}: {e}", path.display()))?;
            }
            continue;
        }
        if !COMPRESSIBLE.contains(&ext) {
            continue;
        }
        let bytes = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let br = brotli_bytes(&bytes)?;
        let gz = gzip_bytes(&bytes)?;
        for (suffix, packed) in [("br", &br), ("gz", &gz)] {
            let sibling = sibling(path, suffix);
            if packed.len() < bytes.len() {
                write(&sibling, packed)?;
            } else {
                let _ = fs::remove_file(&sibling);
            }
        }
        files += 1;
        raw += bytes.len();
        br_total += br.len().min(bytes.len());
    }
    Ok(serde_json::json!({ "files": files, "bytes": raw, "brotli": br_total }).to_string())
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".");
    name.push(suffix);
    PathBuf::from(name)
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::write(path, bytes).map_err(|e| format!("write {}: {e}", path.display()))
}

// The first 4 bytes of the sha256, as 8 hex digits.
fn fingerprint(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().take(4).map(|b| format!("{b:02x}")).collect()
}

/// The glue and wasm file names for a library and its content hash.
fn names(lib: &str, hash: &str) -> (String, String) {
    (format!("{lib}-{hash}.js"), format!("{lib}-{hash}_bg.wasm"))
}

// Replaces the one reference to the wasm file in the glue. Zero or several references is an error.
fn rewrite_glue(glue: &str, from: &str, to: &str) -> Result<String, String> {
    let count = glue.matches(from).count();
    if count != 1 {
        return Err(format!("expected {from} exactly once in the wasm-bindgen glue, found {count}"));
    }
    Ok(glue.replacen(from, to, 1))
}

// Gzip at level 9.
fn gzip_bytes(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut enc = GzEncoder::new(Vec::new(), Compression::new(9));
    enc.write_all(bytes).map_err(|e| format!("gzip: {e}"))?;
    enc.finish().map_err(|e| format!("gzip: {e}"))
}

// Brotli at quality 11 with a 4 MiB window (lgwin 22), which every browser decodes.
fn brotli_bytes(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = brotli::CompressorWriter::new(&mut out, 1 << 16, 11, 22);
        enc.write_all(bytes).map_err(|e| format!("brotli: {e}"))?;
        enc.flush().map_err(|e| format!("brotli: {e}"))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    #[test]
    fn glue_rewrite_replaces_the_one_reference() {
        let glue = "new URL('apz_engine_web_bg.wasm', import.meta.url)";
        let out = rewrite_glue(glue, "'apz_engine_web_bg.wasm'", "'apz_engine_web-1234abcd_bg.wasm'").unwrap();
        assert_eq!(out, "new URL('apz_engine_web-1234abcd_bg.wasm', import.meta.url)");
    }

    #[test]
    fn glue_rewrite_refuses_zero_or_two_references() {
        let from = "'apz_engine_web_bg.wasm'";
        assert!(rewrite_glue("no reference here", from, "'x'").is_err());
        assert!(rewrite_glue("'apz_engine_web_bg.wasm' and 'apz_engine_web_bg.wasm'", from, "'x'").is_err());
    }

    #[test]
    fn fingerprint_names_carry_eight_hex_digits_of_sha256() {
        // sha256 of the empty input starts with e3b0c442.
        assert_eq!(fingerprint(b""), "e3b0c442");
        let (js, wasm) = names("apz_engine_web", "e3b0c442");
        assert_eq!(js, "apz_engine_web-e3b0c442.js");
        assert_eq!(wasm, "apz_engine_web-e3b0c442_bg.wasm");
    }

    #[test]
    fn brotli_and_gzip_round_trip() {
        let text = "the plaza seats every agent ".repeat(500);
        let br = brotli_bytes(text.as_bytes()).unwrap();
        let mut back = String::new();
        brotli::Decompressor::new(&br[..], 4096).read_to_string(&mut back).unwrap();
        assert_eq!(back, text);
        let gz = gzip_bytes(text.as_bytes()).unwrap();
        let mut back = String::new();
        flate2::read::GzDecoder::new(&gz[..]).read_to_string(&mut back).unwrap();
        assert_eq!(back, text);
        assert!(br.len() < text.len() / 10 && gz.len() < text.len() / 10);
    }

    #[test]
    fn compress_writes_siblings_and_drops_stale_ones() {
        let dir = std::env::temp_dir().join(format!("apz-pack-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("main.js"), "console.log('plaza');\n".repeat(200)).unwrap();
        fs::write(dir.join("font.ttf"), [0u8; 64]).unwrap();
        fs::write(dir.join("old.js.br"), b"stale").unwrap();
        compress_dir(&dir).unwrap();
        assert!(dir.join("main.js.br").exists() && dir.join("main.js.gz").exists());
        assert!(!dir.join("font.ttf.br").exists());
        assert!(!dir.join("old.js.br").exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
