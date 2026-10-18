//! Where packs come from: a pack folder (`crates/weft/packs/<name>/pack.json` and its query) and the grammar's
//! WebAssembly, fetched from the grammar's npm package (official tree-sitter grammars publish one) or given as
//! a local file. A pack folder may carry its grammar as `grammar.wasm` (for grammars that publish none, built
//! once with `tree-sitter build --wasm`). The result is a `pack.add` body and the files to store as blobs.

use std::io::Read;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use crate::hash::Hash;
use crate::model::PackAdd;

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub version: u32,
    pub language: String,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub filenames: Vec<String>,
    #[serde(default)]
    pub shebangs: Vec<String>,
    #[serde(default)]
    pub source: Value,
    /// `symbols.scm`, a relative path, or `upstream:<path in the npm package>`.
    pub query: String,
}

/// A pack ready to store: the body of `pack.add` (with the files' hashes) and the files.
pub struct Prepared {
    pub pack: PackAdd,
    pub wasm: Vec<u8>,
    pub query: Vec<u8>,
}

/// Prepares the pack in `dir`. The grammar comes from `wasm_file` when given, else the folder's
/// `grammar.wasm`, else npm. `cache` keeps fetched packages (`<package>@<version>/`) so each is fetched once.
pub fn prepare(
    dir: &Path,
    wasm_file: Option<&Path>,
    cache: Option<&Path>,
) -> Result<Prepared, String> {
    let text = std::fs::read_to_string(dir.join("pack.json"))
        .map_err(|e| format!("{}: {e}", dir.join("pack.json").display()))?;
    let m: Manifest = serde_json::from_str(&text).map_err(|e| format!("pack.json: {e}"))?;
    let mut npm_files: Option<Vec<(String, Vec<u8>)>> = None;
    let mut npm = || -> Result<Vec<(String, Vec<u8>)>, String> {
        if let Some(f) = &npm_files {
            return Ok(f.clone());
        }
        let package = m.source["npm"]
            .as_str()
            .ok_or("the pack names no npm package; give the grammar with --wasm")?;
        let version = m.source["version"]
            .as_str()
            .ok_or("the pack names no version")?;
        let files = cached_tarball(package, version, cache)?;
        npm_files = Some(files.clone());
        Ok(files)
    };
    let local = dir.join("grammar.wasm");
    let wasm_file = wasm_file.or(local.exists().then_some(local.as_path()));
    let wasm = match wasm_file {
        Some(p) => std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?,
        None => {
            let want = m.source["wasm"]
                .as_str()
                .ok_or("the pack names no wasm file")?;
            npm()?
                .into_iter()
                .find(|(path, _)| path.ends_with(want))
                .map(|(_, b)| b)
                .ok_or_else(|| format!("{want} is not in the package"))?
        }
    };
    if !wasm.starts_with(b"\0asm") {
        return Err("the grammar is not a WebAssembly module".into());
    }
    let query = if let Some(up) = m.query.strip_prefix("upstream:") {
        npm()?
            .into_iter()
            .find(|(path, _)| path.ends_with(up))
            .map(|(_, b)| b)
            .ok_or_else(|| format!("{up} is not in the package"))?
    } else {
        let p = dir.join(&m.query);
        std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?
    };
    let pack = PackAdd {
        name: m.name,
        version: m.version,
        language: m.language,
        extensions: m.extensions,
        filenames: m.filenames,
        shebangs: m.shebangs,
        wasm: Hash::of(&wasm),
        tags: Hash::of(&query),
    };
    Ok(Prepared { pack, wasm, query })
}

/// An npm package's files, through `cache` when given.
pub fn cached_tarball(
    package: &str,
    version: &str,
    cache: Option<&Path>,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let Some(cache) = cache else {
        return npm_tarball(package, version);
    };
    let file = cache.join(format!("{}@{version}.tar", package.replace('/', "+")));
    if let Ok(tar) = std::fs::read(&file) {
        return untar(&tar);
    }
    let files = npm_tarball(package, version)?;
    let _ = std::fs::create_dir_all(cache);
    let _ = std::fs::write(&file, tar(&files));
    Ok(files)
}

/// A ustar archive of `files` (the form the cache keeps).
pub fn tar(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (path, data) in files {
        let mut head = [0u8; 512];
        let name = path.as_bytes();
        if name.len() > 100 {
            let mut long = [0u8; 512];
            long[..13].copy_from_slice(b"././@LongLink");
            long[124..135].copy_from_slice(format!("{:011o}", name.len()).as_bytes());
            long[156] = b'L';
            checksum(&mut long);
            out.extend_from_slice(&long);
            out.extend_from_slice(name);
            out.resize(out.len().div_ceil(512) * 512, 0);
        }
        head[..name.len().min(100)].copy_from_slice(&name[..name.len().min(100)]);
        head[100..107].copy_from_slice(b"0000644");
        head[124..135].copy_from_slice(format!("{:011o}", data.len()).as_bytes());
        head[156] = b'0';
        checksum(&mut head);
        out.extend_from_slice(&head);
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(512) * 512, 0);
    }
    out.extend_from_slice(&[0u8; 1024]);
    out
}

fn checksum(head: &mut [u8; 512]) {
    head[148..156].copy_from_slice(b"        ");
    let sum: u32 = head.iter().map(|b| *b as u32).sum();
    head[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
}

/// The files of an npm package's tarball: `(path, bytes)`, paths without the leading `package/`.
pub fn npm_tarball(package: &str, version: &str) -> Result<Vec<(String, Vec<u8>)>, String> {
    let base = package.rsplit('/').next().unwrap_or(package);
    let url = format!("https://registry.npmjs.org/{package}/-/{base}-{version}.tgz");
    let resp = ureq::get(&url).call().map_err(|e| format!("{url}: {e}"))?;
    let mut gz = Vec::new();
    resp.into_body()
        .into_reader()
        .take(64 << 20)
        .read_to_end(&mut gz)
        .map_err(|e| format!("{url}: {e}"))?;
    let mut tar = Vec::new();
    flate2::read::GzDecoder::new(&gz[..])
        .read_to_end(&mut tar)
        .map_err(|e| format!("{url}: not a gzip tarball: {e}"))?;
    untar(&tar)
}

/// A minimal ustar reader: regular files only.
pub fn untar(tar: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut out = Vec::new();
    let mut at = 0;
    let mut long_name: Option<String> = None;
    while at + 512 <= tar.len() {
        let head = &tar[at..at + 512];
        if head.iter().all(|b| *b == 0) {
            break;
        }
        let field = |from: usize, len: usize| -> String {
            String::from_utf8_lossy(&head[from..from + len])
                .trim_end_matches('\0')
                .to_string()
        };
        let size = usize::from_str_radix(field(124, 12).trim(), 8).map_err(|_| "bad tar size")?;
        let kind = head[156];
        let mut name = field(0, 100);
        let prefix = field(345, 155);
        if !prefix.is_empty() {
            name = format!("{prefix}/{name}");
        }
        if let Some(l) = long_name.take() {
            name = l;
        }
        let data = tar
            .get(at + 512..at + 512 + size)
            .ok_or("tar is cut short")?;
        match kind {
            b'L' => {
                long_name = Some(
                    String::from_utf8_lossy(data)
                        .trim_end_matches('\0')
                        .to_string(),
                )
            }
            b'0' | 0 => {
                let path = name.strip_prefix("package/").unwrap_or(&name).to_string();
                out.push((path, data.to_vec()));
            }
            _ => {}
        }
        at += 512 + size.div_ceil(512) * 512;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untar_reads_regular_files() {
        // One file "package/a.txt" with "hi".
        let mut tar = vec![0u8; 512];
        tar[..13].copy_from_slice(b"package/a.txt");
        tar[124..135].copy_from_slice(b"00000000002");
        tar[156] = b'0';
        let mut body = vec![0u8; 512];
        body[..2].copy_from_slice(b"hi");
        tar.extend(body);
        tar.extend(vec![0u8; 1024]);
        assert_eq!(
            untar(&tar).unwrap(),
            vec![("a.txt".to_string(), b"hi".to_vec())]
        );
    }

    #[test]
    fn tar_and_untar_round_trip_long_names() {
        let long = format!("queries/{}/tags.scm", "x".repeat(120));
        let files = vec![
            ("a.txt".to_string(), b"hi".to_vec()),
            (long, vec![7u8; 700]),
        ];
        assert_eq!(untar(&tar(&files)).unwrap(), files);
    }
}
