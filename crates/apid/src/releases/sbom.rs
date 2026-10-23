//! Software bills of materials (PLAN 22.2, 9.3): one CycloneDX 1.5 document per artifact, listing the components its
//! build resolved. The source is the lock file at the revision the candidate was cut from, chosen by the target's
//! ecosystem ([`source`]): `Cargo.lock`, `package-lock.json`, a Python lock or requirements file, or `go.sum`. An
//! ecosystem with no lock file gets a document that says its dependencies are unknown, so it never refuses a candidate.
//!
//! The document names the artifact it describes (its SHA-256 in the metadata component), so [`check`] can tell it is
//! the one the release recorded. Its own digest goes into the provenance, which signs it (`attest.rs`).

use base64::Engine;
use serde_json::{Value, json};

/// One resolved component of a build: a crate, a package or a module, with its checksum when its lock file records
/// one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    pub name: String,
    pub version: String,
    /// `cargo`, `npm`, `pypi` or `golang`: the purl's type.
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
            "pypi" => format!("pkg:pypi/{}@{}", self.name, self.version),
            "golang" => format!("pkg:golang/{}@{}", self.name, self.version),
            _ => format!("pkg:cargo/{}@{}", self.name, self.version),
        }
    }
}

/// The ecosystem a target builds from: the text before the first `:` of its name (`python` for
/// `python:app@linux-x86_64`), or empty when the name has no `:`.
pub fn ecosystem_of(target: &str) -> &str {
    target.split_once(':').map_or("", |(eco, _)| eco)
}

/// The lock file an ecosystem's build is read from, among the revision's root files (paths with no `/`), and its
/// format: `Cargo.lock` for cargo, `package-lock.json` for npm, `uv.lock`, `poetry.lock` or a requirements file for
/// python (in that order of preference), `go.sum` for go. `None` when the ecosystem has no reader or no file is there.
pub fn source(ecosystem: &str, root_files: &[String]) -> Option<(String, &'static str)> {
    let found = |file: &str, format: &'static str| {
        root_files
            .iter()
            .any(|f| f.as_str() == file)
            .then(|| (file.to_string(), format))
    };
    match ecosystem {
        "cargo" => found("Cargo.lock", "cargo"),
        "npm" => found("package-lock.json", "npm"),
        "python" => {
            for (file, format) in [
                ("uv.lock", "uv"),
                ("poetry.lock", "poetry"),
                ("requirements.txt", "requirements"),
            ] {
                if let Some(hit) = found(file, format) {
                    return Some(hit);
                }
            }
            let mut extra: Vec<&str> = root_files
                .iter()
                .map(String::as_str)
                .filter(|f| f.starts_with("requirements") && f.ends_with(".txt"))
                .collect();
            extra.sort();
            extra.first().map(|file| (file.to_string(), "requirements"))
        }
        "go" => found("go.sum", "go"),
        _ => None,
    }
}

/// The components a lock file lists, read in its format (see [`source`]), sorted and without duplicates.
pub fn components(format: &str, text: &str) -> Result<Vec<Component>, String> {
    let mut out = match format {
        "cargo" => cargo_lock(text)?,
        "npm" => package_lock(text)?,
        "uv" => pypi_lock("uv.lock", text)?,
        "poetry" => pypi_lock("poetry.lock", text)?,
        "requirements" => requirements(text),
        "go" => go_sum(text),
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

/// The packages of a `uv.lock` or `poetry.lock` (`file`, for its errors): every `[[package]]` with a name and a
/// version, as a PyPI component with a lowercase name.
fn pypi_lock(file: &str, lock: &str) -> Result<Vec<Component>, String> {
    let doc: toml::Table = lock
        .parse()
        .map_err(|e| format!("{file} does not parse: {e}"))?;
    let Some(packages) = doc.get("package").and_then(|p| p.as_array()) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for package in packages {
        let field = |k: &str| package.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let (Some(name), Some(version)) = (field("name"), field("version")) else {
            return Err(format!("a {file} package has no name or version"));
        };
        out.push(Component {
            name: name.to_lowercase(),
            version,
            kind: "pypi",
            hash: None,
        });
    }
    Ok(out)
}

/// The pinned requirements of a requirements file: each `name==version` line is a PyPI component with a lowercase
/// name. Options (`-r`, `--hash`), comments and blank lines are skipped, and so is a requirement without an exact pin,
/// because its version is not known.
fn requirements(text: &str) -> Vec<Component> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        // A continuation line (such as a `--hash` line) starts with `-`, so it is skipped here.
        if line.is_empty() || line.starts_with('#') || line.starts_with('-') {
            continue;
        }
        // Drop the environment markers, the trailing comment and the line continuation.
        let spec = line.split(';').next().unwrap_or(line);
        let spec = spec
            .split(" #")
            .next()
            .unwrap_or(spec)
            .trim_end_matches('\\')
            .trim();
        // A direct reference (`name @ url`) carries no version to pin.
        if spec.contains("://") || spec.contains('@') {
            continue;
        }
        let Some((name, version)) = spec.split_once("==") else {
            continue;
        };
        // Extras (`django[bcrypt]`) are not part of the name; a range such as `1.*` is not a pin.
        let name = name.split('[').next().unwrap_or(name).trim();
        let version = version.trim();
        if name.is_empty() || version.is_empty() || version.contains(['*', ',', ' ', '=']) {
            continue;
        }
        out.push(Component {
            name: name.to_lowercase(),
            version: version.to_string(),
            kind: "pypi",
            hash: None,
        });
    }
    out
}

/// The modules of a `go.sum`: each `<module> <version> <hash>` line is a component, and a `/go.mod` line only pins the
/// module's go.mod file, so it is not one. The `h1:` hashes are not SHA-256 digests of the module zip, so none is kept.
fn go_sum(text: &str) -> Vec<Component> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let (module, version) = (fields.next()?, fields.next()?);
            if version.ends_with("/go.mod") {
                return None;
            }
            Some(Component {
                name: module.to_string(),
                version: version.to_string(),
                kind: "golang",
                hash: None,
            })
        })
        .collect()
}

/// The CycloneDX 1.5 document of an artifact: the artifact as the metadata component (with its SHA-256) and the
/// components, in order. When `unknown` gives the reason its dependencies are not known, the components are empty
/// and the metadata component says so, with the reason. Deterministic: the same inputs give the same bytes.
pub fn document(
    name: &str,
    version: &str,
    artifact_sha256: &str,
    components: &[Component],
    unknown: Option<&str>,
) -> Value {
    let comps: Vec<Value> = if unknown.is_some() {
        Vec::new()
    } else {
        components
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
            .collect()
    };
    let mut metadata = json!({
        "type": "application", "bom-ref": format!("{name}@{version}"),
        "name": name, "version": version,
        "hashes": [{ "alg": "SHA-256", "content": artifact_sha256 }],
    });
    if let Some(reason) = unknown {
        metadata["properties"] = json!([
            { "name": "apz:dependencies", "value": "unknown" },
            { "name": "apz:dependencies-note", "value": reason },
        ]);
    }
    json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "component": metadata
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

    fn files(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

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
    fn a_target_names_its_ecosystem_and_its_lock_file_is_chosen_in_order() {
        assert_eq!(ecosystem_of("cargo:app@linux-x86_64"), "cargo");
        assert_eq!(ecosystem_of("python:app@linux-x86_64"), "python");
        assert_eq!(ecosystem_of("app"), "");
        assert_eq!(
            source("cargo", &files(&["Cargo.lock"])),
            Some(("Cargo.lock".to_string(), "cargo"))
        );
        assert_eq!(source("npm", &files(&["Cargo.lock"])), None);
        assert_eq!(
            source("gradle", &files(&["build.gradle", "gradle.lockfile"])),
            None
        );
    }

    #[test]
    fn a_python_project_lists_its_pinned_requirements_and_its_uv_lock() {
        let reqs = "# pinned for the server\n-r other.txt\nrequests==2.32.3\nflask>=2\n\
            Django[bcrypt]==5.0.1 \\\n    --hash=sha256:abc\n";
        let (path, format) = source("python", &files(&["requirements.txt", "README.md"])).unwrap();
        assert_eq!(
            (path.as_str(), format),
            ("requirements.txt", "requirements")
        );
        let comps = components(format, reqs).unwrap();
        let pins: Vec<(&str, &str)> = comps
            .iter()
            .map(|c| (c.name.as_str(), c.version.as_str()))
            .collect();
        assert_eq!(pins, [("django", "5.0.1"), ("requests", "2.32.3")]);
        assert_eq!(comps[0].purl(), "pkg:pypi/django@5.0.1");
        assert_eq!(comps[0].kind, "pypi");
        assert!(comps.iter().all(|c| c.name != "flask"));

        // A uv.lock is read ahead of a requirements file.
        assert_eq!(
            source("python", &files(&["requirements.txt", "uv.lock"])),
            Some(("uv.lock".to_string(), "uv"))
        );
        let uv = "[[package]]\nname = \"Idna\"\nversion = \"3.7\"\n";
        let uv_comps = components("uv", uv).unwrap();
        assert_eq!(uv_comps.len(), 1);
        assert_eq!(uv_comps[0].purl(), "pkg:pypi/idna@3.7");
        assert!(components("uv", "[[package").is_err());
    }

    #[test]
    fn a_go_project_lists_its_modules_from_go_sum() {
        let sum = "github.com/x/y v1.2.3 h1:AAAA=\ngithub.com/x/y v1.2.3/go.mod h1:BBBB=\n\
            github.com/a/b v0.4.0 h1:CCCC=\ngithub.com/a/b v0.4.0/go.mod h1:DDDD=\n";
        let comps = components("go", sum).unwrap();
        let mods: Vec<(&str, &str)> = comps
            .iter()
            .map(|c| (c.name.as_str(), c.version.as_str()))
            .collect();
        assert_eq!(
            mods,
            [("github.com/a/b", "v0.4.0"), ("github.com/x/y", "v1.2.3")]
        );
        assert_eq!(comps[1].purl(), "pkg:golang/github.com/x/y@v1.2.3");
        assert!(comps.iter().all(|c| c.hash.is_none()));
        assert_eq!(
            source("go", &files(&["go.sum"])),
            Some(("go.sum".to_string(), "go"))
        );
    }

    #[test]
    fn an_ecosystem_without_a_lock_file_gets_a_document_that_says_its_dependencies_are_unknown() {
        assert!(source("gradle", &files(&["build.gradle"])).is_none());
        assert!(components("gradle", "").is_err());
        let doc = document("app", "1.0.0", "abc", &[], Some("no lock file for gradle"));
        let text = serde_json::to_string_pretty(&doc).unwrap();
        check(&text, "abc").unwrap();
        assert_eq!(doc["components"], json!([]));
        let props = &doc["metadata"]["component"]["properties"];
        assert_eq!(props[0]["name"], "apz:dependencies");
        assert_eq!(props[0]["value"], "unknown");
        assert_eq!(props[1]["name"], "apz:dependencies-note");
        assert_eq!(props[1]["value"], "no lock file for gradle");
    }

    #[test]
    fn a_document_describes_its_artifact_and_is_deterministic() {
        let comps = components("cargo", LOCK).unwrap();
        let doc = document("app", "0.1.0", "abc", &comps, None);
        let text = serde_json::to_string_pretty(&doc).unwrap();
        assert_eq!(
            text,
            serde_json::to_string_pretty(&document("app", "0.1.0", "abc", &comps, None)).unwrap()
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
