//! The runtimes the hub provisions for stdio servers, and the binaries they run (docs/design/mcp.md, "Running a
//! server").
//!
//! Node runs the npm packages and uv runs the PyPI packages. Both are pinned here: the version, and the SHA-256 of
//! the official archive for each platform, as the publisher published it (the nodejs.org `SHASUMS256.txt` and the
//! GitHub release digests of astral-sh/uv). A runtime is downloaded on first use through `ureq`, its SHA-256 is
//! checked before anything is unpacked, and it is unpacked once into `<state>/runtimes/`. A missing or mismatched
//! hash refuses with a sentence, and nothing is installed.
//!
//! A binary package (a URL and its SHA-256 in the owner's config) goes through the same checks, into
//! `<state>/mcp/<id>/bin/<sha256>/`.

use std::fs;
use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use sha2::{Digest, Sha256};

/// Node's LTS line, v24.22.1 (nodejs.org/dist, 2026-10-20).
pub const NODE_VERSION: &str = "24.22.1";
/// uv 0.13.5 (github.com/astral-sh/uv/releases).
pub const UV_VERSION: &str = "0.13.5";
/// The largest download accepted (Node's archive is about 30 MB; a binary package is capped the same way).
const MAX_DOWNLOAD: u64 = 256 << 20;
/// The marker a finished unpack leaves in its directory.
const MARKER: &str = ".apz-complete";

/// One platform's download: where, and the SHA-256 of the file as published.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pin {
    pub target: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
}

/// Node v24.22.1 for each platform: the SHA-256 is the line in nodejs.org's `SHASUMS256.txt` for that file.
pub const NODE_PINS: &[Pin] = &[
    Pin {
        target: "darwin-arm64",
        url: "https://nodejs.org/dist/v24.22.1/node-v24.22.1-darwin-arm64.tar.gz",
        sha256: "89c682593d76cd44233fa53b1563264e7de9cbb189cad169135c5b4fadfbc0a7",
    },
    Pin {
        target: "darwin-x64",
        url: "https://nodejs.org/dist/v24.22.1/node-v24.22.1-darwin-x64.tar.gz",
        sha256: "d132557e7ee2b85af08b0c8c34338e65c6430f2238d01f727728ef42dc83b035",
    },
    Pin {
        target: "linux-x64",
        url: "https://nodejs.org/dist/v24.22.1/node-v24.22.1-linux-x64.tar.gz",
        sha256: "eb6728e77c576a35162c122c45ccadf224d89ff00abc8dae46cc2897c6002d7c",
    },
    Pin {
        target: "linux-arm64",
        url: "https://nodejs.org/dist/v24.22.1/node-v24.22.1-linux-arm64.tar.gz",
        sha256: "33c37be234ba8bee041961047b19160f4d7d0f164e8e246f78bdd10feff148b3",
    },
    Pin {
        target: "win-x64",
        url: "https://nodejs.org/dist/v24.22.1/node-v24.22.1-win-x64.zip",
        sha256: "d2046d28c2c3dcd211ac5145c0bf30e2047ef9dfb306693c8c6e2eccd8b95fa5",
    },
];

/// uv 0.13.5 for each platform: the SHA-256 is the release asset's digest.
pub const UV_PINS: &[Pin] = &[
    Pin {
        target: "darwin-arm64",
        url: "https://github.com/astral-sh/uv/releases/download/0.13.5/uv-aarch64-apple-darwin.tar.gz",
        sha256: "743f85035a1996ecb350659e01cd9faea9709829be09b1b4b03c73eaf3c9cd73",
    },
    Pin {
        target: "darwin-x64",
        url: "https://github.com/astral-sh/uv/releases/download/0.13.5/uv-x86_64-apple-darwin.tar.gz",
        sha256: "b0f52b0e3945b4107d89024ea38e625683011fbfc05a0d98f9fa2d6dcee28c66",
    },
    Pin {
        target: "linux-x64",
        url: "https://github.com/astral-sh/uv/releases/download/0.13.5/uv-x86_64-unknown-linux-gnu.tar.gz",
        sha256: "f68d291c75783122dad58ab77def0e13f3233f358731db5d6d7e8715603b928f",
    },
    Pin {
        target: "linux-arm64",
        url: "https://github.com/astral-sh/uv/releases/download/0.13.5/uv-aarch64-unknown-linux-gnu.tar.gz",
        sha256: "4bb69cf95ed13f23713bb7bc09f241896ea0c75b7c557e615a20604b82f9c403",
    },
    Pin {
        target: "win-x64",
        url: "https://github.com/astral-sh/uv/releases/download/0.13.5/uv-x86_64-pc-windows-msvc.zip",
        sha256: "a38b8d1902b7cfbc76b423ba1bb456f623f42064cdc72628b6075b89afd1019a",
    },
];

/// The platform this hub runs on, as a pin's target. None on a platform no runtime is pinned for.
pub fn host_target() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("darwin-arm64"),
        ("macos", "x86_64") => Some("darwin-x64"),
        ("linux", "x86_64") => Some("linux-x64"),
        ("linux", "aarch64") => Some("linux-arm64"),
        ("windows", "x86_64") => Some("win-x64"),
        _ => None,
    }
}

/// How a download is stored: a gzipped tar, a zip, or the file itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Archive {
    TarGz,
    Zip,
    Raw,
}

/// The archive kind a URL names by its extension.
pub fn archive_kind(url: &str) -> Archive {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    if path.ends_with(".tar.gz") || path.ends_with(".tgz") {
        Archive::TarGz
    } else if path.ends_with(".zip") {
        Archive::Zip
    } else {
        Archive::Raw
    }
}

/// Fetches the bytes of an https URL. The hub uses `HttpFetch`; tests use a fake that never touches the network.
pub trait Fetch: Send + Sync {
    /// The body of `url`, at most `limit` bytes.
    fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>, String>;
}

/// The hub's own HTTP client (ureq), https only.
pub struct HttpFetch;

impl Fetch for HttpFetch {
    fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>, String> {
        if !url.starts_with("https://") {
            return Err(format!("{url} is not an https URL; nothing was downloaded"));
        }
        let mut response = ureq::get(url)
            .call()
            .map_err(|e| format!("cannot download {url}: {e}"))?;
        response
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(|e| format!("cannot read {url}: {e}"))
    }
}

/// The SHA-256 of `bytes` as lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Downloads `url` and checks the bytes against `sha256` before anything else. A mismatch, or no pinned hash at
/// all, refuses with a sentence; nothing is returned.
pub fn fetch_verified(fetch: &dyn Fetch, url: &str, sha256: &str) -> Result<Vec<u8>, String> {
    if sha256.is_empty() {
        return Err(format!(
            "no SHA-256 is pinned for {url}; nothing was downloaded or installed"
        ));
    }
    let bytes = fetch.get(url, MAX_DOWNLOAD)?;
    let got = sha256_hex(&bytes);
    if !got.eq_ignore_ascii_case(sha256) {
        return Err(format!(
            "the download of {url} does not match its pinned SHA-256 (expected {sha256}, got {got}); nothing was installed"
        ));
    }
    Ok(bytes)
}

/// Unpacks an archive into `dest` (which must exist and be empty). Entries that would leave `dest` are refused.
pub fn unpack(bytes: &[u8], kind: Archive, dest: &Path) -> Result<(), String> {
    match kind {
        Archive::TarGz => {
            let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
            archive.set_preserve_permissions(true);
            archive
                .unpack(dest)
                .map_err(|e| format!("cannot unpack the archive: {e}"))
        }
        Archive::Zip => {
            let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
                .map_err(|e| format!("cannot read the zip archive: {e}"))?;
            for index in 0..archive.len() {
                let mut file = archive
                    .by_index(index)
                    .map_err(|e| format!("cannot read the zip archive: {e}"))?;
                let Some(relative) = file.enclosed_name() else {
                    let name = file
                        .name()
                        .map(|n| n.to_string())
                        .unwrap_or_else(|_| "?".into());
                    return Err(format!(
                        "the archive has an entry outside its directory: {name}"
                    ));
                };
                let out = dest.join(relative);
                if file.is_dir() {
                    fs::create_dir_all(&out).map_err(|e| e.to_string())?;
                    continue;
                }
                if let Some(parent) = out.parent() {
                    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                let mut data = Vec::new();
                file.read_to_end(&mut data).map_err(|e| e.to_string())?;
                fs::write(&out, &data).map_err(|e| e.to_string())?;
                #[cfg(unix)]
                if let Some(mode) = file.unix_mode() {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&out, fs::Permissions::from_mode(mode & 0o777))
                        .map_err(|e| e.to_string())?;
                }
            }
            Ok(())
        }
        Archive::Raw => Err("a raw file is not unpacked".into()),
    }
}

/// A relative path from a package: no root, no `..`, no drive.
fn safe_relative(entry: &str) -> Option<PathBuf> {
    let path = Path::new(entry);
    let clean = path.components().all(|c| matches!(c, Component::Normal(_)));
    (clean && !entry.is_empty()).then(|| path.to_path_buf())
}

/// The runtime paths a stdio server needs, as provisioned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// The `node` program.
    pub node: PathBuf,
    /// npm's CLI script, run by `node`.
    pub npm_cli: PathBuf,
    /// The directory holding `node` (put first on a child's PATH).
    pub bin_dir: PathBuf,
}

/// The runtimes under `<state>/runtimes`, provisioned on first use and reused after. One install at a time: two
/// servers starting together do not unpack into the same partial directory.
pub struct Runtimes {
    root: PathBuf,
    fetch: Arc<dyn Fetch>,
    gate: Mutex<()>,
}

impl Runtimes {
    pub fn new(state_dir: &Path, fetch: Arc<dyn Fetch>) -> Runtimes {
        Runtimes {
            root: state_dir.join("runtimes"),
            fetch,
            gate: Mutex::new(()),
        }
    }

    /// Node for this host, provisioned if needed.
    pub fn node(&self) -> Result<Node, String> {
        let pin = host_pin(NODE_PINS, "node")?;
        let dir = self.provision("node", NODE_VERSION, &pin, true)?;
        Ok(if cfg!(windows) {
            Node {
                node: dir.join("node.exe"),
                npm_cli: dir.join("node_modules/npm/bin/npm-cli.js"),
                bin_dir: dir,
            }
        } else {
            Node {
                node: dir.join("bin/node"),
                npm_cli: dir.join("lib/node_modules/npm/bin/npm-cli.js"),
                bin_dir: dir.join("bin"),
            }
        })
    }

    /// The uv program for this host, provisioned if needed.
    pub fn uv(&self) -> Result<PathBuf, String> {
        let pin = host_pin(UV_PINS, "uv")?;
        let dir = self.provision("uv", UV_VERSION, &pin, true)?;
        Ok(dir.join(if cfg!(windows) { "uv.exe" } else { "uv" }))
    }

    /// Unpacks a pinned runtime into `<state>/runtimes/<name>-<version>-<target>-<hash>/` once. The archive's single
    /// top directory is taken out (Node's is `node-v24.22.1-…`); a directory with its marker is reused as it is.
    fn provision(
        &self,
        name: &str,
        version: &str,
        pin: &Pin,
        strip: bool,
    ) -> Result<PathBuf, String> {
        let _gate = lock(&self.gate);
        let short: String = pin.sha256.chars().take(12).collect();
        let dir = self
            .root
            .join(format!("{name}-{version}-{}-{short}", pin.target));
        if dir.join(MARKER).exists() {
            return Ok(dir);
        }
        let bytes = fetch_verified(self.fetch.as_ref(), pin.url, pin.sha256)?;
        let partial = self.root.join(format!(".partial-{name}-{version}-{short}"));
        let _ = fs::remove_dir_all(&partial);
        fs::create_dir_all(&partial)
            .map_err(|e| format!("cannot make {}: {e}", partial.display()))?;
        unpack(&bytes, archive_kind(pin.url), &partial)?;
        let source = if strip {
            single_directory(&partial).unwrap_or(partial.clone())
        } else {
            partial.clone()
        };
        let _ = fs::remove_dir_all(&dir);
        fs::rename(&source, &dir).map_err(|e| format!("cannot install {name}: {e}"))?;
        let _ = fs::remove_dir_all(&partial);
        fs::write(dir.join(MARKER), pin.sha256)
            .map_err(|e| format!("cannot finish {name}: {e}"))?;
        Ok(dir)
    }

    /// A binary package: downloaded from `url`, checked against `sha256`, unpacked if it is an archive, and its
    /// `entry` returned. Kept in `base/<sha256>/`, so a changed hash is a new download.
    pub fn binary(
        &self,
        base: &Path,
        url: &str,
        sha256: &str,
        entry: &str,
    ) -> Result<PathBuf, String> {
        let _gate = lock(&self.gate);
        let relative = safe_relative(entry)
            .ok_or_else(|| format!("`{entry}` is not a file inside the download"))?;
        let dir = base.join(sha256);
        if dir.join(MARKER).exists() {
            return Ok(dir.join(relative));
        }
        let bytes = fetch_verified(self.fetch.as_ref(), url, sha256)?;
        let partial = base.join(format!(".partial-{sha256}"));
        let _ = fs::remove_dir_all(&partial);
        fs::create_dir_all(&partial)
            .map_err(|e| format!("cannot make {}: {e}", partial.display()))?;
        match archive_kind(url) {
            Archive::Raw => {
                let file = partial.join(&relative);
                if let Some(parent) = file.parent() {
                    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                fs::write(&file, &bytes).map_err(|e| format!("cannot write the binary: {e}"))?;
            }
            kind => unpack(&bytes, kind, &partial)?,
        }
        if !partial.join(&relative).is_file() {
            return Err(format!("the download of {url} has no file `{entry}`"));
        }
        make_executable(&partial.join(&relative));
        let _ = fs::remove_dir_all(&dir);
        fs::rename(&partial, &dir).map_err(|e| format!("cannot install the binary: {e}"))?;
        fs::write(dir.join(MARKER), sha256)
            .map_err(|e| format!("cannot finish the binary: {e}"))?;
        Ok(dir.join(relative))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn host_pin(pins: &[Pin], name: &str) -> Result<Pin, String> {
    let target = host_target().ok_or_else(|| {
        format!(
            "no pinned {name} for this platform ({} {}); remote (http) servers still work",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    pins.iter()
        .find(|pin| pin.target == target)
        .copied()
        .ok_or_else(|| format!("no pinned {name} for {target}; remote (http) servers still work"))
}

/// The one directory an unpacked archive holds, when it holds nothing else.
fn single_directory(dir: &Path) -> Option<PathBuf> {
    let mut entries = fs::read_dir(dir).ok()?.filter_map(Result::ok);
    let only = entries.next()?;
    if entries.next().is_some() || !only.file_type().ok()?.is_dir() {
        return None;
    }
    Some(only.path())
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o755));
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use super::*;

    /// Serves fixed bodies by URL, and counts the downloads it makes (no network).
    struct FakeFetch {
        bodies: BTreeMap<String, Vec<u8>>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeFetch {
        fn with(url: &str, body: Vec<u8>) -> Arc<FakeFetch> {
            Arc::new(FakeFetch {
                bodies: BTreeMap::from([(url.to_string(), body)]),
                calls: Mutex::new(Vec::new()),
            })
        }
        fn calls(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    impl Fetch for FakeFetch {
        fn get(&self, url: &str, _limit: u64) -> Result<Vec<u8>, String> {
            self.calls.lock().unwrap().push(url.to_string());
            self.bodies
                .get(url)
                .cloned()
                .ok_or_else(|| format!("no fake body for {url}"))
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apz-runtimes-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A gzipped tar holding `top/bin/node` (a small file), as Node's archive is laid out.
    fn tgz(top: &str, file: &str, body: &[u8]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, format!("{top}/{file}"), body)
            .unwrap();
        let tar_bytes = builder.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, &tar_bytes).unwrap();
        gz.finish().unwrap()
    }

    fn pin_for(url: &str, bytes: &[u8]) -> Pin {
        let sha: &'static str = Box::leak(sha256_hex(bytes).into_boxed_str());
        let url: &'static str = Box::leak(url.to_string().into_boxed_str());
        Pin {
            target: "test",
            url,
            sha256: sha,
        }
    }

    #[test]
    fn the_pins_name_every_platform_once_with_a_hash_and_the_pinned_version() {
        for (pins, version) in [(NODE_PINS, NODE_VERSION), (UV_PINS, UV_VERSION)] {
            assert_eq!(pins.len(), 5);
            for target in [
                "darwin-arm64",
                "darwin-x64",
                "linux-x64",
                "linux-arm64",
                "win-x64",
            ] {
                let pin = pins.iter().find(|p| p.target == target).unwrap();
                assert!(
                    pin.url.starts_with("https://") && pin.url.contains(version),
                    "{target}"
                );
                assert_eq!(pin.sha256.len(), 64, "{target}");
                assert!(
                    pin.sha256
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                );
            }
        }
    }

    #[test]
    fn a_download_that_does_not_match_its_pin_is_refused_and_named() {
        let fetch = FakeFetch::with("https://example.test/x.bin", b"tampered".to_vec());
        let err = fetch_verified(
            fetch.as_ref(),
            "https://example.test/x.bin",
            &"0".repeat(64),
        )
        .unwrap_err();
        assert!(err.contains("does not match its pinned SHA-256"), "{err}");
        assert!(err.contains("nothing was installed"), "{err}");
    }

    #[test]
    fn a_missing_pin_refuses_before_anything_is_downloaded() {
        let fetch = FakeFetch::with("https://example.test/x.bin", b"x".to_vec());
        let err = fetch_verified(fetch.as_ref(), "https://example.test/x.bin", "").unwrap_err();
        assert!(err.contains("no SHA-256 is pinned"), "{err}");
        assert_eq!(fetch.calls(), 0);
    }

    #[test]
    fn a_runtime_is_unpacked_once_with_its_top_directory_taken_out() {
        let root = scratch("once");
        let body = b"#!/bin/sh\necho fake node\n";
        let url = "https://example.test/node-v1-test.tar.gz";
        let archive = tgz("node-v1-test", "bin/node", body);
        let pin = pin_for(url, &archive);
        let fetch = FakeFetch::with(url, archive);
        let runtimes = Runtimes::new(&root, fetch.clone());
        let dir = runtimes.provision("node", "1", &pin, true).unwrap();
        assert_eq!(fs::read(dir.join("bin/node")).unwrap(), body);
        let again = runtimes.provision("node", "1", &pin, true).unwrap();
        assert_eq!(again, dir);
        assert_eq!(
            fetch.calls(),
            1,
            "the second use reuses the unpacked runtime"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_zip_runtime_is_unpacked_too() {
        let root = scratch("zip");
        let url = "https://example.test/uv-test.zip";
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("uv.exe", zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut writer, b"exe").unwrap();
        let archive = writer.finish().unwrap().into_inner();
        let pin = pin_for(url, &archive);
        let runtimes = Runtimes::new(&root, FakeFetch::with(url, archive));
        let dir = runtimes.provision("uv", "1", &pin, false).unwrap();
        assert_eq!(fs::read(dir.join("uv.exe")).unwrap(), b"exe");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_binary_package_is_checked_and_its_entry_is_returned() {
        let root = scratch("binary");
        let body = b"raw binary bytes";
        let url = "https://example.test/mcp_server";
        let sha = sha256_hex(body);
        let runtimes = Runtimes::new(&root, FakeFetch::with(url, body.to_vec()));
        let base = root.join("mcp/github/bin");
        let path = runtimes.binary(&base, url, &sha, "mcp_server").unwrap();
        assert_eq!(fs::read(&path).unwrap(), body);
        let wrong = runtimes.binary(&root.join("other"), url, &"f".repeat(64), "mcp_server");
        assert!(wrong.unwrap_err().contains("does not match"));
        let escape = runtimes.binary(&base, url, &sha, "../outside");
        assert!(
            escape
                .unwrap_err()
                .contains("not a file inside the download")
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_host_without_a_pinned_runtime_says_so_in_a_sentence() {
        let err = host_pin(&[], "node");
        match host_target() {
            Some(_) => assert!(err.unwrap_err().contains("no pinned node")),
            None => assert!(
                err.unwrap_err()
                    .contains("no pinned node for this platform")
            ),
        }
    }

    /// Downloads uv for this host from GitHub and runs `uv --version`. Needs the network, so it is ignored by default
    /// (`cargo test -p apid -- --ignored mcp::runtimes`).
    #[test]
    #[ignore = "downloads uv from github.com"]
    fn uv_really_downloads_verifies_and_runs() {
        let root = scratch("real");
        let runtimes = Runtimes::new(&root, Arc::new(HttpFetch));
        let uv = runtimes.uv().unwrap();
        let out = std::process::Command::new(&uv)
            .arg("--version")
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).starts_with("uv 0.13.5"));
        fs::remove_dir_all(&root).unwrap();
    }
}
