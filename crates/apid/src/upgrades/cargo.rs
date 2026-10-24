//! The Cargo adapter (PLAN 24.2). A project's registry is read from its `.cargo/config.toml`: a `[source]` that replaces
//! crates.io with a local registry (a directory laid out as the index, with the `.crate` files beside it) or a sparse index
//! URL; crates.io's sparse index otherwise. The candidates of each locked crate are read from that index; a minor or
//! patch update is `cargo update` (a lock change only, within the manifest's requirements); a major update edits the
//! requirement in `Cargo.toml` and updates that one crate. The selected tests of a batch are the workspace packages that
//! reach an updated crate in the resolved graph (`cargo metadata`); a major runs the whole workspace.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use super::version::{Bump, Version};
use super::{Exec, Fetch, Tree, Update};

/// The public index of crates.io, read through the sparse protocol.
pub const CRATES_IO: &str = "https://index.crates.io";

/// Where a project's crate index is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexSource {
    /// A local registry: `<dir>/index/<path>` and `<dir>/<name>-<version>.crate`.
    Dir(PathBuf),
    /// A sparse index at this URL (no trailing slash).
    Sparse(String),
}

/// The index a project's Cargo reads, from its `.cargo/config.toml` (or `.cargo/config`).
pub fn index_source(tree: &Tree) -> IndexSource {
    let text = tree
        .get(".cargo/config.toml")
        .or_else(|| tree.get(".cargo/config"))
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default();
    let config: toml::Table = toml::from_str(&text).unwrap_or_default();
    let sources = config.get("source").and_then(|v| v.as_table());
    let replaced = sources
        .and_then(|s| s.get("crates-io"))
        .and_then(|v| v.as_table())
        .and_then(|t| t.get("replace-with"))
        .and_then(|v| v.as_str());
    let source = replaced
        .and_then(|name| sources.and_then(|s| s.get(name)))
        .and_then(|v| v.as_table());
    if let Some(src) = source {
        if let Some(dir) = src.get("local-registry").and_then(|v| v.as_str()) {
            return IndexSource::Dir(PathBuf::from(dir));
        }
        if let Some(url) = src.get("registry").and_then(|v| v.as_str()) {
            return IndexSource::Sparse(
                url.trim_start_matches("sparse+")
                    .trim_end_matches('/')
                    .to_string(),
            );
        }
    }
    IndexSource::Sparse(CRATES_IO.to_string())
}

/// The path of a crate's index file, in the sparse layout (`ti/ny/tiny`, `3/a/abc`).
pub fn index_path(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    match lower.len() {
        1 => format!("1/{lower}"),
        2 => format!("2/{lower}"),
        3 => format!("3/{}/{lower}", &lower[..1]),
        _ => format!("{}/{}/{lower}", &lower[..2], &lower[2..4]),
    }
}

#[derive(Deserialize)]
struct IndexLine {
    vers: String,
    #[serde(default)]
    yanked: bool,
}

/// The releases of crate `name` the index offers, not yanked and parseable (`Vec` in index order).
pub fn candidates(
    src: &IndexSource,
    name: &str,
    fetch: &dyn Fetch,
) -> Result<Vec<Version>, String> {
    let path = index_path(name);
    let text = match src {
        IndexSource::Dir(dir) => match std::fs::read(dir.join("index").join(&path)) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("the local registry's index for {name}: {e}")),
        },
        IndexSource::Sparse(base) => fetch.bytes(&format!("{base}/{path}"))?,
    };
    let Some(bytes) = text else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for line in String::from_utf8_lossy(&bytes)
        .lines()
        .filter(|l| !l.trim().is_empty())
    {
        let entry: IndexLine =
            serde_json::from_str(line).map_err(|e| format!("the index of {name}: {e}"))?;
        if entry.yanked {
            continue;
        }
        if let Some(v) = Version::parse(&entry.vers) {
            out.push(v);
        }
    }
    Ok(out)
}

/// The packages of a `Cargo.lock`: name, version, and whether they come from a registry (not a path or a git repository).
pub fn locked(lock: &str) -> Result<Vec<(String, String, bool)>, String> {
    let table: toml::Table = toml::from_str(lock).map_err(|e| format!("Cargo.lock: {e}"))?;
    let packages = table
        .get("package")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for package in packages {
        let t = package
            .as_table()
            .ok_or("Cargo.lock: a package is not a table")?;
        let name = t
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let version = t
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let registry = t
            .get("source")
            .and_then(|v| v.as_str())
            .is_some_and(|s| s.starts_with("registry+"));
        out.push((name, version, registry));
    }
    Ok(out)
}

/// The dependencies `Cargo.toml` names with a registry requirement: (name, requirement).
pub fn direct_requirements(manifest: &str) -> Result<Vec<(String, String)>, String> {
    let table: toml::Table = toml::from_str(manifest).map_err(|e| format!("Cargo.toml: {e}"))?;
    let mut out = Vec::new();
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        let Some(deps) = table.get(section).and_then(|v| v.as_table()) else {
            continue;
        };
        for (name, spec) in deps {
            let req = match spec {
                toml::Value::String(s) => Some(s.clone()),
                toml::Value::Table(t) => t
                    .get("version")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                _ => None,
            };
            if let Some(req) = req {
                out.push((name.clone(), req));
            }
        }
    }
    Ok(out)
}

/// What a project's Cargo would update: the in-requirement updates (patch and minor, one batch) and the majors (each its
/// own change, with the requirement that moves to it).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub minor: Vec<Update>,
    pub majors: Vec<Update>,
}

/// Plans the updates of `tree`'s Cargo.lock against its index.
pub fn plan(tree: &Tree, fetch: &dyn Fetch) -> Result<Plan, String> {
    let src = index_source(tree);
    let lock = text_of(tree, "Cargo.lock")?;
    let manifest = text_of(tree, "Cargo.toml")?;
    let mut plan = Plan::default();
    let mut seen = BTreeSet::new();
    for (name, text, registry) in locked(&lock)? {
        if !registry || !seen.insert((name.clone(), text.clone())) {
            continue;
        }
        let Some(from) = Version::parse(&text) else {
            continue;
        };
        let newest = candidates(&src, &name, fetch)?
            .into_iter()
            .filter(|v| v.compat() == from.compat() && *v > from)
            .max();
        if let Some(to) = newest {
            plan.minor.push(Update {
                name: name.clone(),
                from: text.clone(),
                to: to.to_string(),
                bump: Bump::between(&from, &to),
                req: None,
            });
        }
    }
    // A direct dependency with a newer release of another class: its requirement moves with it (a major).
    let locked_versions: BTreeMap<String, String> = locked(&lock)?
        .into_iter()
        .filter(|(_, _, registry)| *registry)
        .map(|(name, version, _)| (name, version))
        .collect();
    for (name, _req) in direct_requirements(&manifest)? {
        let Some(from) = locked_versions.get(&name).and_then(|v| Version::parse(v)) else {
            continue;
        };
        let newest = candidates(&src, &name, fetch)?
            .into_iter()
            .filter(|v| v.compat() != from.compat() && *v > from)
            .max();
        if let Some(to) = newest {
            plan.majors.push(Update {
                name: name.clone(),
                from: from.to_string(),
                to: to.to_string(),
                bump: Bump::Major,
                req: Some(requirement_of(&to)),
            });
        }
    }
    plan.majors.sort_by(|a, b| a.name.cmp(&b.name));
    plan.majors.dedup_by(|a, b| a.name == b.name);
    Ok(plan)
}

/// The requirement a release of class `v` is written as in a manifest (`2`, or `0.3` for a `0.3.x` release).
pub fn requirement_of(v: &Version) -> String {
    if v.major > 0 {
        v.major.to_string()
    } else {
        format!("0.{}", v.minor)
    }
}

fn text_of(tree: &Tree, path: &str) -> Result<String, String> {
    tree.get(path)
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .ok_or_else(|| format!("{path} is not in the project"))
}

/// Rewrites the requirement of dependency `name` in `manifest` to `req`. Handles the two forms a manifest writes it in:
/// `name = "1"` and `name = { version = "1", ... }`. Any other form is refused, so the edit is never guessed.
pub fn rewrite_requirement(manifest: &str, name: &str, req: &str) -> Result<String, String> {
    let mut done = false;
    let mut out = Vec::new();
    for line in manifest.lines() {
        let trimmed = line.trim_start();
        let is_dep = trimmed
            .strip_prefix(name)
            .is_some_and(|rest| rest.starts_with([' ', '=']));
        if is_dep
            && !done
            && let Some((_, value)) = trimmed.split_once('=')
        {
            let value = value.trim_start();
            let rewritten = if value.starts_with('"') {
                Some(replace_first_quoted(value, req))
            } else if value.starts_with('{') {
                value.find("version").and_then(|at| {
                    let tail = &value[at..];
                    tail.find('"').map(|q| {
                        let (head, rest) = value.split_at(at + q);
                        format!("{head}{}", replace_first_quoted(rest, req))
                    })
                })
            } else {
                None
            };
            if let Some(new_value) = rewritten {
                let indent = &line[..line.len() - trimmed.len()];
                let key = &trimmed[..trimmed.len() - value.len()];
                out.push(format!("{indent}{key}{new_value}"));
                done = true;
                continue;
            }
        }
        out.push(line.to_string());
    }
    if !done {
        return Err(format!(
            "the requirement of {name} is not a plain version in Cargo.toml: it is edited by hand"
        ));
    }
    let mut text = out.join("\n");
    if manifest.ends_with('\n') {
        text.push('\n');
    }
    Ok(text)
}

/// `text` with its first quoted string replaced by `req` (the text starts at or before the quote).
fn replace_first_quoted(text: &str, req: &str) -> String {
    let Some(open) = text.find('"') else {
        return text.to_string();
    };
    let after = &text[open + 1..];
    match after.find('"') {
        Some(close) => format!("\"{req}\"{}", &after[close + 1..]),
        None => text.to_string(),
    }
}

/// The updates of a plan's minor batch, applied in `dir` (the project's files): `cargo update`.
pub fn apply_minor(dir: &Path, exec: &dyn Exec, env: &[(String, String)]) -> Result<(), String> {
    cargo(exec, dir, &["update"], env).map(|_| ())
}

/// Moves crate `update.name` to its new requirement in `dir`'s manifest and updates it.
pub fn apply_major(
    dir: &Path,
    exec: &dyn Exec,
    env: &[(String, String)],
    update: &Update,
) -> Result<(), String> {
    let req = update
        .req
        .as_deref()
        .ok_or_else(|| format!("{} has no requirement to move to", update.name))?;
    edit_manifest(dir, &update.name, req)?;
    cargo(exec, dir, &["update", "-p", &update.name], env).map(|_| ())
}

/// Moves crate `name` to `fixed` (a CVE's fix): within its requirement, or with the requirement edited when `fixed` is in
/// another class.
pub fn apply_fix(
    dir: &Path,
    exec: &dyn Exec,
    env: &[(String, String)],
    name: &str,
    fixed: &str,
) -> Result<(), String> {
    let version = Version::parse(fixed).ok_or_else(|| format!("{fixed} is not a release"))?;
    let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).map_err(|e| e.to_string())?;
    let req = direct_requirements(&manifest)?
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, r)| r);
    if let Some(req) = req
        && Version::parse(&req).is_some_and(|r| r.compat() != version.compat())
    {
        edit_manifest(dir, name, &requirement_of(&version))?;
    }
    cargo(exec, dir, &["update", "-p", name, "--precise", fixed], env).map(|_| ())
}

fn edit_manifest(dir: &Path, name: &str, req: &str) -> Result<(), String> {
    let path = dir.join("Cargo.toml");
    let manifest = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let edited = rewrite_requirement(&manifest, name, req)?;
    std::fs::write(&path, edited).map_err(|e| e.to_string())
}

/// The workspace packages that reach one of `updated` in the resolved graph, by name (`cargo metadata`).
pub fn selected(
    dir: &Path,
    exec: &dyn Exec,
    env: &[(String, String)],
    updated: &BTreeSet<String>,
) -> Result<Vec<String>, String> {
    let text = cargo(exec, dir, &["metadata", "--format-version", "1"], env)?;
    let meta: Value = serde_json::from_str(&text).map_err(|e| format!("cargo metadata: {e}"))?;
    let names: BTreeMap<String, String> = meta["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            Some((
                p["id"].as_str()?.to_string(),
                p["name"].as_str()?.to_string(),
            ))
        })
        .collect();
    let edges: BTreeMap<String, Vec<String>> = meta["resolve"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| {
            let id = n["id"].as_str()?.to_string();
            let deps = n["deps"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|d| d["pkg"].as_str().map(str::to_string))
                .collect();
            Some((id, deps))
        })
        .collect();
    let mut chosen = Vec::new();
    for member in meta["workspace_members"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let mut seen = BTreeSet::new();
        let mut stack = vec![member.to_string()];
        let mut reaches = false;
        while let Some(id) = stack.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            if names.get(&id).is_some_and(|n| updated.contains(n)) && id != member {
                reaches = true;
                break;
            }
            stack.extend(edges.get(&id).cloned().unwrap_or_default());
        }
        if reaches && let Some(name) = names.get(member) {
            chosen.push(name.clone());
        }
    }
    chosen.sort();
    chosen.dedup();
    Ok(chosen)
}

/// The command that tests a Cargo project: the selected packages, or the whole workspace.
pub fn test_args(selection: Option<&[String]>) -> Vec<String> {
    match selection {
        None => vec!["test".into(), "--workspace".into()],
        Some(packages) => {
            let mut args = vec!["test".to_string()];
            for p in packages {
                args.push("-p".into());
                args.push(p.clone());
            }
            args
        }
    }
}

/// Runs `cargo` with `args` in `dir`; its stdout on success, its stderr's tail on failure.
pub fn cargo(
    exec: &dyn Exec,
    dir: &Path,
    args: &[&str],
    env: &[(String, String)],
) -> Result<String, String> {
    let out = exec.run(dir, "cargo", args, env)?;
    if out.ok {
        Ok(out.stdout)
    } else {
        Err(format!("cargo {}: {}", args.join(" "), tail(&out.stderr)))
    }
}

/// The last lines of a command's output, for a record.
pub fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let from = lines.len().saturating_sub(12);
    lines[from..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_paths_follow_the_sparse_layout() {
        assert_eq!(index_path("a"), "1/a");
        assert_eq!(index_path("ab"), "2/ab");
        assert_eq!(index_path("abc"), "3/a/abc");
        assert_eq!(index_path("tiny"), "ti/ny/tiny");
        assert_eq!(index_path("serde_json"), "se/rd/serde_json");
    }

    #[test]
    fn the_registry_is_read_from_the_projects_source_replacement() {
        let mut tree = Tree::new();
        assert_eq!(index_source(&tree), IndexSource::Sparse(CRATES_IO.into()));
        tree.insert(
            ".cargo/config.toml".into(),
            b"[source.crates-io]\nreplace-with = \"local\"\n\n[source.local]\nlocal-registry = \"/r\"\n".to_vec(),
        );
        assert_eq!(index_source(&tree), IndexSource::Dir(PathBuf::from("/r")));
        tree.insert(
            ".cargo/config.toml".into(),
            b"[source.crates-io]\nreplace-with = \"mirror\"\n\n[source.mirror]\nregistry = \"sparse+https://m.example/index/\"\n".to_vec(),
        );
        assert_eq!(
            index_source(&tree),
            IndexSource::Sparse("https://m.example/index".into())
        );
    }

    #[test]
    fn the_lock_and_the_manifest_are_read() {
        let lock = "version = 4\n\n[[package]]\nname = \"sample\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"tiny\"\nversion = \"1.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"x\"\n";
        let packages = locked(lock).unwrap();
        assert_eq!(packages[0], ("sample".into(), "0.1.0".into(), false));
        assert_eq!(packages[1], ("tiny".into(), "1.0.0".into(), true));
        let reqs = direct_requirements(
            "[package]\nname = \"sample\"\n\n[dependencies]\ntiny = \"1\"\nserde = { version = \"1.0\", features = [\"derive\"] }\nlocal = { path = \"../x\" }\n",
        )
        .unwrap();
        assert_eq!(
            reqs,
            vec![("serde".into(), "1.0".into()), ("tiny".into(), "1".into())]
        );
    }

    #[test]
    fn a_requirement_is_rewritten_in_its_two_forms_and_never_guessed() {
        let manifest = "[dependencies]\ntiny = \"1\"\nserde = { version = \"1.0\", features = [] }\ntiny-extra = \"9\"\n";
        let edited = rewrite_requirement(manifest, "tiny", "2").unwrap();
        assert!(
            edited.contains("tiny = \"2\"\n") && edited.contains("tiny-extra = \"9\""),
            "{edited}"
        );
        let edited = rewrite_requirement(manifest, "serde", "2.0").unwrap();
        assert!(
            edited.contains("serde = { version = \"2.0\", features = [] }"),
            "{edited}"
        );
        assert!(rewrite_requirement("[dependencies]\nx = { path = \"..\" }\n", "x", "1").is_err());
    }

    #[test]
    fn a_fix_in_another_class_is_a_major_requirement() {
        assert_eq!(requirement_of(&Version::parse("2.4.0").unwrap()), "2");
        assert_eq!(requirement_of(&Version::parse("0.3.9").unwrap()), "0.3");
        assert_eq!(test_args(None), ["test", "--workspace"]);
        assert_eq!(
            test_args(Some(&["a".into(), "b".into()])),
            ["test", "-p", "a", "-p", "b"]
        );
    }
}
