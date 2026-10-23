//! Software bills of materials (PLAN 22.2, 9.3): one CycloneDX 1.5 document per artifact, listing the components its
//! build resolved. The source is the lock file at the revision the candidate was cut from: `Cargo.lock` for a `cargo:`
//! target, `package-lock.json` for an `npm:` one. Another ecosystem has no source yet and its candidate is refused.
//!
//! The document names the artifact it describes (its SHA-256 in the metadata component), so [`check`] can tell it is
//! the one the release recorded. Its own digest goes into the provenance, which signs it (`attest.rs`).

use base64::Engine;
use serde_json::{Value, json};

/// One resolved component of a build: a crate or a package, with its checksum when its lock file records one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    pub name: String,
    pub version: String,
    /// `cargo` or `npm`: the purl's type.
    pub kind: &'static str,
    /// The algorithm and the digest in hex (`SHA-256` for a crate checksum, `SHA-512` for an npm integrity).
    pub hash: Option<(&'static str, String)>,
}

impl Component {
    fn purl(&self) -> String {
        match self.kind {
            // An npm scope is written with its `@` percent-encoded in a purl.
            "npm" => match self.name.strip_prefix('@') {
                Some(scoped) => format!("pkg:npm/%40{scoped}@{}", self.version),
                None => format!("pkg:npm/{}@{}", self.name, self.version),
            },
            _ => format!("pkg:cargo/{}@{}", self.name, self.version),
        }
    }
}

/// The ecosystem a target builds from, by its name's prefix (`cargo:app@linux-x86_64`), and the lock file it reads.
pub fn lock_file_of(target: &str) -> Result<(&'static str, &'static str), String> {
    match target.split_once(':').map(|(eco, _)| eco) {
        Some("cargo") => Ok(("cargo", "Cargo.lock")),
        Some("npm") => Ok(("npm", "package-lock.json")),
        _ => Err(format!(
            "{target}: no SBOM source yet (9.3 reads Cargo.lock and package-lock.json)"
        )),
    }
}

/// The components a lock file lists, read for its ecosystem.
pub fn components(kind: &str, lock: &str) -> Result<Vec<Component>, String> {
    let mut out = match kind {
        "cargo" => cargo_lock(lock)?,
        "npm" => package_lock(lock)?,
        other => return Err(format!("no lock file reader for {other}")),
    };
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.version.cmp(&b.version)));
    out.dedup();
    Ok(out)
}

fn cargo_lock(lock: &str) -> Result<Vec<Component>, String> {
    let doc: toml::Table = lock
        .parse()
        .map_err(|e| format!("Cargo.lock does not parse: {e}"))?;
    let packages = doc
        .get("package")
        .and_then(|p| p.as_array())
        .ok_or("Cargo.lock lists no packages")?;
    let mut out = Vec::new();
    for package in packages {
        let field = |k: &str| package.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let (Some(name), Some(version)) = (field("name"), field("version")) else {
            return Err("a Cargo.lock package has no name or version".into());
        };
        // A checksum is the registry's SHA-256 of the crate, in hex.
        let hash = field("checksum").map(|c| ("SHA-256", c));
        out.push(Component {
            name,
            version,
            kind: "cargo",
            hash,
        });
    }
    Ok(out)
}

fn package_lock(lock: &str) -> Result<Vec<Component>, String> {
    let doc: Value =
        serde_json::from_str(lock).map_err(|e| format!("package-lock.json does not parse: {e}"))?;
    let packages = doc["packages"]
        .as_object()
        .ok_or("package-lock.json lists no packages")?;
    let mut out = Vec::new();
    for (path, entry) in packages {
        // The root project is the artifact itself; each installed package sits under its `node_modules/` path.
        let Some(at) = path.rfind("node_modules/") else {
            continue;
        };
        let name = path[at + "node_modules/".len()..].to_string();
        let version = entry["version"]
            .as_str()
            .ok_or_else(|| format!("{name} in package-lock.json has no version"))?
            .to_string();
        // An npm integrity is `sha512-<base64>`; a document carries hex.
        let hash = match entry["integrity"]
            .as_str()
            .and_then(|i| i.strip_prefix("sha512-"))
        {
            Some(b64) => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .map_err(|e| format!("the integrity of {name} is not base64: {e}"))?;
                Some(("SHA-512", weft::hash::hex(&bytes)))
            }
            None => None,
        };
        out.push(Component {
            name,
            version,
            kind: "npm",
            hash,
        });
    }
    Ok(out)
}

/// The CycloneDX 1.5 document of an artifact: the artifact as the metadata component (with its SHA-256) and the
/// components, in order. Deterministic: the same inputs give the same bytes.
pub fn document(
    name: &str,
    version: &str,
    artifact_sha256: &str,
    components: &[Component],
) -> Value {
    let comps: Vec<Value> = components
        .iter()
        .map(|c| {
            let mut comp = json!({
                "type": "library", "bom-ref": c.purl(), "name": c.name,
                "version": c.version, "purl": c.purl(),
            });
            if let Some((alg, hex)) = &c.hash {
                comp["hashes"] = json!([{ "alg": alg, "content": hex }]);
            }
            comp
        })
        .collect();
    json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "component": {
                "type": "application", "bom-ref": format!("{name}@{version}"),
                "name": name, "version": version,
                "hashes": [{ "alg": "SHA-256", "content": artifact_sha256 }],
            }
        },
        "components": comps,
    })
}

/// Checks a stored document: it is CycloneDX 1.5 and describes the artifact with SHA-256 `artifact_sha256`.
pub fn check(text: &str, artifact_sha256: &str) -> Result<(), String> {
    let doc: Value =
        serde_json::from_str(text).map_err(|e| format!("the SBOM is not JSON: {e}"))?;
    if doc["bomFormat"] != "CycloneDX" || doc["specVersion"] != "1.5" {
        return Err("the SBOM is not a CycloneDX 1.5 document".into());
    }
    let described = doc["metadata"]["component"]["hashes"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|h| h["alg"] == "SHA-256" && h["content"] == artifact_sha256);
    if !described {
        return Err("the SBOM describes another artifact".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCK: &str = "version = 4\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.219\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"5f0e2c6ed6606019b4e29e69dbaae37b4f5b6fc1c0a2e2ef3b6f7f4b4d2f7a10\"\n";

    #[test]
    fn a_cargo_lock_lists_its_packages_with_purls_and_checksums() {
        let comps = components("cargo", LOCK).unwrap();
        assert_eq!(comps.len(), 2);
        assert_eq!(comps[1].purl(), "pkg:cargo/serde@1.0.219");
        assert_eq!(comps[1].hash.as_ref().unwrap().0, "SHA-256");
        assert!(comps[0].hash.is_none());
    }

    #[test]
    fn a_package_lock_lists_installed_packages_and_scopes_are_encoded() {
        let lock = r#"{"packages":{"":{"name":"web","version":"1.0.0"},
            "node_modules/@agentplaza/ui":{"version":"2.0.0","integrity":"sha512-AAEC"},
            "node_modules/a/node_modules/left-pad":{"version":"1.3.0"}}}"#;
        let comps = components("npm", lock).unwrap();
        assert_eq!(comps.len(), 2);
        let scoped = comps.iter().find(|c| c.name == "@agentplaza/ui").unwrap();
        assert_eq!(scoped.purl(), "pkg:npm/%40agentplaza/ui@2.0.0");
        assert_eq!(scoped.hash, Some(("SHA-512", "000102".to_string())));
        assert!(comps.iter().any(|c| c.name == "left-pad"));
    }

    #[test]
    fn a_target_without_a_lock_file_source_is_refused_with_its_reason() {
        assert_eq!(
            lock_file_of("cargo:app@linux-x86_64").unwrap(),
            ("cargo", "Cargo.lock")
        );
        assert!(
            lock_file_of("gradle:app@android-aarch64")
                .unwrap_err()
                .contains("no SBOM source")
        );
    }

    #[test]
    fn a_document_describes_its_artifact_and_is_deterministic() {
        let comps = components("cargo", LOCK).unwrap();
        let doc = document("app", "0.1.0", "abc", &comps);
        let text = serde_json::to_string_pretty(&doc).unwrap();
        assert_eq!(
            text,
            serde_json::to_string_pretty(&document("app", "0.1.0", "abc", &comps)).unwrap()
        );
        check(&text, "abc").unwrap();
        assert!(
            check(&text, "def")
                .unwrap_err()
                .contains("another artifact")
        );
        assert!(check("{\"bomFormat\":\"SPDX\"}", "abc").is_err());
    }
}
