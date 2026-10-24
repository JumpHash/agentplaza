//! The build graph (PLAN 15.4): the workspace packages of a repository and what each depends on, read from the package
//! manager's own output: `cargo metadata` for Rust, `go list -json` for Go, and the workspace manifests for npm.
//!
//! The graph is read, not computed: the caller runs the tool (on the recording machine) and passes its output here.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::Error;
use crate::llvm::relative;
use crate::map::Package;

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<CargoPackage>,
}

#[derive(Deserialize)]
struct CargoPackage {
    name: String,
    manifest_path: String,
    #[serde(default)]
    dependencies: Vec<CargoDep>,
}

#[derive(Deserialize)]
struct CargoDep {
    name: String,
}

/// The workspace packages of `cargo metadata --no-deps --format-version 1`, with the workspace packages each depends on.
/// `root` is the repository's root; directories are named from it.
pub fn cargo_packages(metadata_json: &str, root: &str) -> Result<Vec<Package>, Error> {
    let metadata: Metadata = serde_json::from_str(metadata_json)
        .map_err(|e| Error::Graph(format!("cargo metadata: {e}")))?;
    let names: BTreeSet<&str> = metadata.packages.iter().map(|p| p.name.as_str()).collect();
    let mut out = Vec::new();
    for package in &metadata.packages {
        let dir = match package.manifest_path.rsplit_once('/') {
            Some((dir, _manifest)) => relative(dir, root),
            None => String::new(),
        };
        let deps = deps_within(
            package.dependencies.iter().map(|d| d.name.as_str()),
            &names,
            &package.name,
        );
        out.push(Package {
            name: package.name.clone(),
            dir,
            deps,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

#[derive(Deserialize)]
struct GoPackage {
    #[serde(rename = "ImportPath")]
    import_path: String,
    #[serde(rename = "Dir")]
    dir: String,
    #[serde(rename = "Imports", default)]
    imports: Vec<String>,
}

/// The packages of `go list -json ./...` (a stream of JSON objects), with the module packages each imports. A package
/// is named by its import path, which is what `go test <path>` takes.
pub fn go_packages(list_stream: &str, root: &str) -> Result<Vec<Package>, Error> {
    let mut found = Vec::new();
    for item in serde_json::Deserializer::from_str(list_stream).into_iter::<GoPackage>() {
        found.push(item.map_err(|e| Error::Graph(format!("go list: {e}")))?);
    }
    let names: BTreeSet<&str> = found.iter().map(|p| p.import_path.as_str()).collect();
    let mut out: Vec<Package> = found
        .iter()
        .map(|p| Package {
            name: p.import_path.clone(),
            dir: relative(&p.dir, root),
            deps: deps_within(p.imports.iter().map(String::as_str), &names, &p.import_path),
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// The workspace packages of an npm workspace: each member's `package.json` (its directory from the root, and its text),
/// with the members it depends on through `dependencies`, `devDependencies` or `peerDependencies`.
pub fn npm_packages(members: &[(String, String)]) -> Result<Vec<Package>, Error> {
    let mut manifests: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
    for (dir, text) in members {
        let value: serde_json::Value = serde_json::from_str(text)
            .map_err(|e| Error::Graph(format!("{dir}/package.json: {e}")))?;
        let Some(name) = value.get("name").and_then(|n| n.as_str()) else {
            return Err(Error::Graph(format!("{dir}/package.json has no name")));
        };
        let mut depended = Vec::new();
        for field in ["dependencies", "devDependencies", "peerDependencies"] {
            if let Some(map) = value.get(field).and_then(|m| m.as_object()) {
                depended.extend(map.keys().cloned());
            }
        }
        manifests.insert(
            name.to_string(),
            (dir.trim_matches('/').to_string(), depended),
        );
    }
    let names: BTreeSet<&str> = manifests.keys().map(String::as_str).collect();
    let mut out: Vec<Package> = manifests
        .iter()
        .map(|(name, (dir, depended))| Package {
            name: name.clone(),
            dir: dir.clone(),
            deps: deps_within(depended.iter().map(String::as_str), &names, name),
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// The names among `candidates` that are workspace packages and not `own`: sorted, without repeats.
fn deps_within<'a>(
    candidates: impl Iterator<Item = &'a str>,
    workspace: &BTreeSet<&str>,
    own: &str,
) -> Vec<String> {
    let set: BTreeSet<String> = candidates
        .filter(|name| *name != own && workspace.contains(name))
        .map(str::to_string)
        .collect();
    set.into_iter().collect()
}

/// The package that owns `path` (a `/`-separated path from the root): the one with the longest directory that contains
/// it. A package in the root (`dir` is "") owns what no other package does.
pub fn package_of<'a>(packages: &'a [Package], path: &str) -> Option<&'a Package> {
    packages
        .iter()
        .filter(|p| owns(&p.dir, path))
        .max_by_key(|p| p.dir.len())
}

fn owns(dir: &str, path: &str) -> bool {
    dir.is_empty() || path == dir || path.starts_with(&format!("{dir}/"))
}

/// The packages that `names` are, and every package that depends on one of them, directly or through others.
pub fn with_dependents(packages: &[Package], names: &BTreeSet<String>) -> BTreeSet<String> {
    let mut closure = names.clone();
    loop {
        let before = closure.len();
        for package in packages {
            if package.deps.iter().any(|dep| closure.contains(dep)) {
                closure.insert(package.name.clone());
            }
        }
        if closure.len() == before {
            return closure;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_list_stream_names_packages_by_import_path_and_keeps_module_imports() {
        let stream = r#"{"ImportPath":"ex.com/m/a","Dir":"/repo/a","Imports":["fmt","ex.com/m/b"]}
{"ImportPath":"ex.com/m/b","Dir":"/repo/b","Imports":["ex.com/m/a"]}
"#;
        let packages = go_packages(stream, "/repo").unwrap();
        assert_eq!(packages[0].name, "ex.com/m/a");
        assert_eq!(packages[0].dir, "a");
        assert_eq!(packages[0].deps, vec!["ex.com/m/b".to_string()]);
        assert_eq!(packages[1].deps, vec!["ex.com/m/a".to_string()]);
    }

    #[test]
    fn npm_workspace_members_depend_on_members_only() {
        let members = vec![
            ("packages/ui".to_string(), r#"{"name":"@x/ui","dependencies":{"react":"18"}}"#.to_string()),
            (
                "packages/app".to_string(),
                r#"{"name":"@x/app","devDependencies":{"@x/ui":"*"},"dependencies":{"react":"18"}}"#.to_string(),
            ),
        ];
        let packages = npm_packages(&members).unwrap();
        let app = packages.iter().find(|p| p.name == "@x/app").unwrap();
        assert_eq!(app.deps, vec!["@x/ui".to_string()]);
        assert_eq!(app.dir, "packages/app");
        assert!(
            packages
                .iter()
                .find(|p| p.name == "@x/ui")
                .unwrap()
                .deps
                .is_empty()
        );
    }

    #[test]
    fn the_owner_of_a_path_is_its_longest_package_directory() {
        let packages = vec![
            Package {
                name: "root".into(),
                dir: "".into(),
                deps: vec![],
            },
            Package {
                name: "a".into(),
                dir: "crates/a".into(),
                deps: vec![],
            },
            Package {
                name: "a-sub".into(),
                dir: "crates/a/sub".into(),
                deps: vec![],
            },
        ];
        assert_eq!(
            package_of(&packages, "crates/a/sub/x.rs").unwrap().name,
            "a-sub"
        );
        assert_eq!(package_of(&packages, "crates/a/x.rs").unwrap().name, "a");
        // Not a prefix on a path boundary: `crates/ab` is not in `crates/a`.
        assert_eq!(
            package_of(&packages, "crates/ab/x.rs").unwrap().name,
            "root"
        );
    }
}
