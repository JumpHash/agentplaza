//! The adapters of the ecosystems other than Cargo (PLAN 24.2). Each reads its outdated dependencies from its own tool's
//! output (Gradle's from its dependency-updates report), applies a batch and each major with its tool or with an edit the
//! adapter makes to the project's files, and runs its suite. The tool runs in a scratch copy of the project and is never
//! installed. A major's edit is the requirement that names it: a Go major moves the module's import path to `/vN`, a pip
//! major rewrites the package's requirement, a SwiftPM major its `from:` requirement, a NuGet major its project's version,
//! a Gradle major its coordinate in the build files. Test selection is not built for these ecosystems: their batches run
//! the whole suite.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::ecosystem::Ecosystem;
use super::version::{Bump, Version};
use super::{Exec, Update};

/// What an ecosystem would update: its batch (patch and minor), and its majors (each its own change).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outdated {
    pub minor: Vec<Update>,
    pub majors: Vec<Update>,
}

/// The update of `name` from `from_v` (as `from` writes it) to `to_v` (as `to` writes it), when `to_v` is newer.
fn between(name: &str, from: &str, to: &str, from_v: Version, to_v: Version) -> Option<Update> {
    (to_v > from_v).then(|| Update {
        name: name.to_string(),
        from: from.to_string(),
        to: to.to_string(),
        bump: Bump::between(&from_v, &to_v),
        req: None,
    })
}

/// The update of `name` from `from` to `to`, both release numbers.
fn update(name: &str, from: &str, to: &str) -> Option<Update> {
    between(name, from, to, Version::parse(from)?, Version::parse(to)?)
}

/// As [`update`], for a version a build file writes with a suffix: `31.0-jre` is `31.0`.
fn update_loose(name: &str, from: &str, to: &str) -> Option<Update> {
    between(name, from, to, numbers(from)?, numbers(to)?)
}

/// The release numbers at the start of a version as a build file writes it (`31.0-jre` is `31.0`).
fn numbers(raw: &str) -> Option<Version> {
    let core: String = raw
        .trim()
        .trim_start_matches('v')
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    Version::parse(core.trim_end_matches('.'))
}

/// Splits a list of updates into the batch and the majors.
fn split(updates: Vec<Update>) -> Outdated {
    let (majors, minor): (Vec<Update>, Vec<Update>) =
        updates.into_iter().partition(|u| u.bump == Bump::Major);
    Outdated { minor, majors }
}

/// The stdout of a tool, or its last error line when it failed without output.
fn stdout_of(
    exec: &dyn Exec,
    dir: &Path,
    env: &[(String, String)],
    program: &str,
    args: &[&str],
) -> Result<String, String> {
    let out = exec.run(dir, program, args, env)?;
    // Outdated listings exit non-zero when something is outdated; their output is still the answer.
    if out.stdout.trim().is_empty() && !out.ok {
        return Err(format!(
            "{program} {}: {}",
            args.join(" "),
            out.stderr.lines().last().unwrap_or("")
        ));
    }
    Ok(out.stdout)
}

/// The outdated dependencies of the project in `dir`, by its ecosystem's tool.
pub fn outdated(
    eco: Ecosystem,
    dir: &Path,
    exec: &dyn Exec,
    env: &[(String, String)],
) -> Result<Outdated, String> {
    match eco {
        Ecosystem::Npm => {
            let text = stdout_of(exec, dir, env, "npm", &["outdated", "--json"])?;
            let value: Value = if text.trim().is_empty() {
                Value::Object(Default::default())
            } else {
                serde_json::from_str(&text).map_err(|e| format!("npm outdated: {e}"))?
            };
            let mut updates = Vec::new();
            for (name, info) in value.as_object().into_iter().flatten() {
                let current = info["current"].as_str().unwrap_or_default();
                let wanted = info["wanted"].as_str().unwrap_or(current);
                let latest = info["latest"].as_str().unwrap_or(wanted);
                updates.extend(update(name, current, wanted));
                updates.extend(update(name, wanted, latest).filter(|u| u.bump == Bump::Major));
            }
            Ok(split(updates))
        }
        Ecosystem::Pip => {
            let text = stdout_of(
                exec,
                dir,
                env,
                "uv",
                &["pip", "list", "--outdated", "--format", "json"],
            )?;
            let value: Value =
                serde_json::from_str(&text).map_err(|e| format!("uv pip list: {e}"))?;
            let updates = value
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| {
                    update(
                        p["name"].as_str()?,
                        p["version"].as_str()?,
                        p["latest_version"].as_str()?,
                    )
                })
                .collect();
            Ok(split(updates))
        }
        Ecosystem::Go => {
            let text = stdout_of(exec, dir, env, "go", &["list", "-m", "-u", "-json", "all"])?;
            let mut updates = Vec::new();
            for module in serde_json::Deserializer::from_str(&text).into_iter::<Value>() {
                let module = module.map_err(|e| format!("go list: {e}"))?;
                let (Some(path), Some(version)) =
                    (module["Path"].as_str(), module["Version"].as_str())
                else {
                    continue;
                };
                // A newer release on the module's own major line is a minor or patch.
                if let Some(newer) = module["Update"]["Version"].as_str()
                    && let Some(u) = update(path, version, newer)
                    && u.bump != Bump::Major
                {
                    updates.push(u);
                }
                // The next major line is a module path of its own, with /vN: a major when it has a release.
                updates.extend(go_major(exec, dir, env, path, version));
            }
            Ok(split(updates))
        }
        Ecosystem::Nuget => {
            let text = stdout_of(
                exec,
                dir,
                env,
                "dotnet",
                &["list", "package", "--outdated", "--format", "json"],
            )?;
            let value: Value =
                serde_json::from_str(&text).map_err(|e| format!("dotnet list package: {e}"))?;
            let mut updates = Vec::new();
            for project in value["projects"].as_array().into_iter().flatten() {
                let path = project["path"].as_str().unwrap_or_default();
                for framework in project["frameworks"].as_array().into_iter().flatten() {
                    for package in framework["topLevelPackages"]
                        .as_array()
                        .into_iter()
                        .flatten()
                    {
                        let (Some(id), Some(now), Some(latest)) = (
                            package["id"].as_str(),
                            package["resolvedVersion"].as_str(),
                            package["latestVersion"].as_str(),
                        ) else {
                            continue;
                        };
                        if let Some(mut u) = update(id, now, latest) {
                            // The project file the package is in, so the upgrade changes it there.
                            u.req = Some(path.to_string());
                            updates.push(u);
                        }
                    }
                }
            }
            Ok(split(updates))
        }
        Ecosystem::Swiftpm => {
            let resolved = std::fs::read_to_string(dir.join("Package.resolved"))
                .map_err(|e| format!("Package.resolved: {e}"))?;
            let value: Value =
                serde_json::from_str(&resolved).map_err(|e| format!("Package.resolved: {e}"))?;
            let mut updates = Vec::new();
            for pin in value["pins"].as_array().into_iter().flatten() {
                let (Some(identity), Some(location), Some(current)) = (
                    pin["identity"].as_str(),
                    pin["location"].as_str(),
                    pin["state"]["version"].as_str(),
                ) else {
                    continue;
                };
                let tags = stdout_of(
                    exec,
                    dir,
                    env,
                    "git",
                    &["ls-remote", "--tags", "--refs", location],
                )?;
                let releases: Vec<(Version, String)> = tags
                    .lines()
                    .filter_map(|l| l.rsplit("refs/tags/").next())
                    .filter_map(|t| Version::parse(t).map(|v| (v, t.to_string())))
                    .collect();
                let Some(from) = Version::parse(current) else {
                    continue;
                };
                // The newest release on the pin's line is a minor or patch; the newest of any line a major.
                let line = releases
                    .iter()
                    .filter(|(v, _)| v.compat() == from.compat())
                    .max_by_key(|(v, _)| *v);
                if let Some((_, tag)) = line {
                    updates.extend(update(identity, current, tag.trim_start_matches('v')));
                }
                let newest = releases.iter().max_by_key(|(v, _)| *v);
                if let Some((_, tag)) = newest {
                    let major = update(identity, current, tag.trim_start_matches('v'))
                        .filter(|u| u.bump == Bump::Major);
                    updates.extend(major.map(|mut u| {
                        u.req = Some(location.to_string());
                        u
                    }));
                }
            }
            Ok(split(updates))
        }
        Ecosystem::Gradle => {
            // The dependency-updates plugin writes its report to the build; without the plugin there is no report.
            let out = exec.run(
                dir,
                "gradle",
                &["dependencyUpdates", "-Drevision=release"],
                env,
            )?;
            if !out.ok {
                return Err(format!(
                    "gradle dependencyUpdates: {} (the build applies the dependency-updates plugin)",
                    out.stderr.lines().last().unwrap_or("failed")
                ));
            }
            let report = dir.join("build/dependencyUpdates/report.json");
            let text = std::fs::read_to_string(&report)
                .map_err(|e| format!("{}: {e}", report.display()))?;
            let value: Value =
                serde_json::from_str(&text).map_err(|e| format!("the dependency report: {e}"))?;
            let mut updates = Vec::new();
            for dep in value["outdated"]["dependencies"]
                .as_array()
                .into_iter()
                .flatten()
            {
                let (Some(group), Some(name), Some(version), Some(release)) = (
                    dep["group"].as_str(),
                    dep["name"].as_str(),
                    dep["version"].as_str(),
                    dep["available"]["release"].as_str(),
                ) else {
                    continue;
                };
                updates.extend(update_loose(&format!("{group}:{name}"), version, release));
            }
            Ok(split(updates))
        }
        Ecosystem::Cargo => Err("Cargo's plan is its own (cargo.rs)".into()),
    }
}

/// The next major line of a Go module, when it has a release: `example.com/c` at v1 has `example.com/c/v2`'s releases.
fn go_major(
    exec: &dyn Exec,
    dir: &Path,
    env: &[(String, String)],
    path: &str,
    version: &str,
) -> Option<Update> {
    let from = Version::parse(version)?;
    if from.major == 0 {
        return None;
    }
    let base = match path.rsplit_once("/v") {
        Some((base, n)) if n.parse::<u64>().is_ok_and(|n| n >= 2) => base,
        _ => path,
    };
    let next = from.major + 1;
    let target = format!("{base}/v{next}");
    let out = exec
        .run(dir, "go", &["list", "-m", "-versions", &target], env)
        .ok()?;
    if !out.ok {
        return None;
    }
    let newest = out
        .stdout
        .split_whitespace()
        .filter_map(Version::parse)
        .filter(|v| v.major == next)
        .max()?;
    Some(Update {
        name: target,
        from: version.to_string(),
        to: format!("v{newest}"),
        bump: Bump::Major,
        req: None,
    })
}

/// Applies a patch and minor batch with the ecosystem's tool (or Gradle's edit) in `dir`.
pub fn apply_minor(
    eco: Ecosystem,
    dir: &Path,
    exec: &dyn Exec,
    env: &[(String, String)],
    updates: &[Update],
) -> Result<(), String> {
    let commands: Vec<(&str, Vec<String>)> = match eco {
        Ecosystem::Npm => vec![("npm", vec!["update".into()])],
        Ecosystem::Pip => vec![("uv", vec!["lock".into(), "--upgrade".into()])],
        Ecosystem::Go => vec![
            ("go", vec!["get".into(), "-u".into(), "./...".into()]),
            ("go", vec!["mod".into(), "tidy".into()]),
        ],
        Ecosystem::Nuget => updates
            .iter()
            .map(|u| {
                let project = u.req.clone().unwrap_or_default();
                (
                    "dotnet",
                    vec![
                        "add".into(),
                        project,
                        "package".into(),
                        u.name.clone(),
                        "--version".into(),
                        u.to.clone(),
                    ],
                )
            })
            .collect(),
        Ecosystem::Swiftpm => vec![("swift", vec!["package".into(), "update".into()])],
        Ecosystem::Gradle => return gradle_edit(dir, updates),
        Ecosystem::Cargo => return Err("Cargo's apply is its own (cargo.rs)".into()),
    };
    run_all(exec, dir, env, commands)
}

/// Applies one major update in `dir`: the edit that names it, then the tool that takes it.
pub fn apply_major(
    eco: Ecosystem,
    dir: &Path,
    exec: &dyn Exec,
    env: &[(String, String)],
    update: &Update,
) -> Result<(), String> {
    match eco {
        Ecosystem::Npm => run_all(
            exec,
            dir,
            env,
            vec![(
                "npm",
                vec!["install".into(), format!("{}@{}", update.name, update.to)],
            )],
        ),
        Ecosystem::Nuget => run_all(
            exec,
            dir,
            env,
            vec![(
                "dotnet",
                vec![
                    "add".into(),
                    update.req.clone().unwrap_or_default(),
                    "package".into(),
                    update.name.clone(),
                    "--version".into(),
                    update.to.clone(),
                ],
            )],
        ),
        Ecosystem::Pip => {
            rewrite_file(dir, "pyproject.toml", |text| {
                rewrite_pip_requirement(text, &update.name, &update.to)
            })?;
            run_all(
                exec,
                dir,
                env,
                vec![(
                    "uv",
                    vec![
                        "lock".into(),
                        "--upgrade-package".into(),
                        update.name.clone(),
                    ],
                )],
            )
        }
        Ecosystem::Go => {
            go_import_major(dir, update)?;
            run_all(
                exec,
                dir,
                env,
                vec![("go", vec!["mod".into(), "tidy".into()])],
            )
        }
        Ecosystem::Swiftpm => {
            let location = update
                .req
                .clone()
                .ok_or("a SwiftPM major names its package's location")?;
            rewrite_file(dir, "Package.swift", |text| {
                rewrite_swift_requirement(text, &location, &update.to)
            })?;
            run_all(
                exec,
                dir,
                env,
                vec![(
                    "swift",
                    vec!["package".into(), "update".into(), update.name.clone()],
                )],
            )
        }
        Ecosystem::Gradle => gradle_edit(dir, std::slice::from_ref(update)),
        Ecosystem::Cargo => Err("Cargo's major is its own (cargo.rs)".into()),
    }
}

fn run_all(
    exec: &dyn Exec,
    dir: &Path,
    env: &[(String, String)],
    commands: Vec<(&str, Vec<String>)>,
) -> Result<(), String> {
    for (program, args) in commands {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = exec.run(dir, program, &refs, env)?;
        if !out.ok {
            return Err(format!(
                "{program} {}: {}",
                args.join(" "),
                out.stderr.lines().last().unwrap_or("failed")
            ));
        }
    }
    Ok(())
}

/// The suite of an ecosystem's project: the whole of it, as a batch's tests (test selection is not built here).
pub fn suite(eco: Ecosystem) -> (&'static str, Vec<&'static str>) {
    match eco {
        Ecosystem::Npm => ("npm", vec!["test"]),
        Ecosystem::Pip => ("uv", vec!["run", "pytest"]),
        Ecosystem::Go => ("go", vec!["test", "./..."]),
        Ecosystem::Nuget => ("dotnet", vec!["test"]),
        Ecosystem::Swiftpm => ("swift", vec!["test"]),
        Ecosystem::Gradle => ("gradle", vec!["test"]),
        Ecosystem::Cargo => ("cargo", vec!["test", "--workspace"]),
    }
}

/// Whether a changed file may belong to a batch: the lockfiles for a patch or minor, and the manifests too for a major.
/// Gradle keeps its versions in its build files, so those are its batches' files, minor or major.
pub fn allowed(eco: Ecosystem, major: bool, path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    if eco == Ecosystem::Gradle {
        return matches!(
            name,
            "build.gradle" | "build.gradle.kts" | "gradle.properties" | "libs.versions.toml"
        );
    }
    let lock = match eco {
        Ecosystem::Cargo => name == "Cargo.lock",
        Ecosystem::Npm => name == "package-lock.json",
        Ecosystem::Pip => name == "uv.lock",
        Ecosystem::Go => name == "go.sum",
        Ecosystem::Nuget => name == "packages.lock.json",
        Ecosystem::Swiftpm => name == "Package.resolved",
        Ecosystem::Gradle => false,
    };
    let manifest = match eco {
        Ecosystem::Cargo => name == "Cargo.toml",
        Ecosystem::Npm => name == "package.json",
        Ecosystem::Pip => name == "pyproject.toml",
        Ecosystem::Go => name == "go.mod",
        Ecosystem::Nuget => name.ends_with(".csproj"),
        Ecosystem::Swiftpm => name == "Package.swift",
        Ecosystem::Gradle => false,
    };
    lock || (major && manifest)
}

// ── the edits of a batch's files ──────────────────────────────────────────────────────────────────────────────────

/// Reads `dir/file`, applies `edit` and writes it back. The edit's own error says why it refused.
fn rewrite_file(
    dir: &Path,
    file: &str,
    edit: impl FnOnce(&str) -> Result<String, String>,
) -> Result<(), String> {
    let path = dir.join(file);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{file}: {e}"))?;
    let edited = edit(&text)?;
    std::fs::write(&path, edited).map_err(|e| format!("{file}: {e}"))
}

/// The requirement of pip package `name` in a `pyproject.toml`, replaced by `>= to`: the quoted spec that names it.
fn rewrite_pip_requirement(text: &str, name: &str, to: &str) -> Result<String, String> {
    let key = format!("\"{name}");
    let at = text.find(&key).ok_or_else(|| {
        format!("pyproject.toml names no requirement for {name}: it is edited by hand")
    })?;
    let close = text[at + 1..]
        .find('"')
        .ok_or("pyproject.toml: an unclosed requirement")?;
    let end = at + 1 + close + 1;
    Ok(format!("{}\"{name}>={to}\"{}", &text[..at], &text[end..]))
}

/// The `from:` requirement of the SwiftPM package at `location`, replaced by `to`.
fn rewrite_swift_requirement(text: &str, location: &str, to: &str) -> Result<String, String> {
    let key = format!("\"{location}\"");
    let at = text
        .find(&key)
        .ok_or_else(|| format!("Package.swift names no package at {location}"))?;
    let marker = "from: \"";
    let from_at = text[at..]
        .find(marker)
        .ok_or_else(|| format!("Package.swift's requirement for {location} is not a from: version: it is edited by hand"))?;
    let start = at + from_at + marker.len();
    let end = start
        + text[start..]
            .find('"')
            .ok_or("Package.swift: an unclosed version")?;
    Ok(format!("{}{to}{}", &text[..start], &text[end..]))
}

/// The coordinates of Gradle dependencies in the build files: `group:name:from` becomes `group:name:to`. Refused when no
/// build file names one.
fn gradle_edit(dir: &Path, updates: &[Update]) -> Result<(), String> {
    let files = [
        "build.gradle",
        "build.gradle.kts",
        "gradle.properties",
        "gradle/libs.versions.toml",
    ];
    for update in updates {
        let from = format!("{}:{}", update.name, update.from);
        let to = format!("{}:{}", update.name, update.to);
        let mut found = false;
        for file in files {
            let path = dir.join(file);
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if text.contains(&from) {
                found = true;
                std::fs::write(&path, text.replace(&from, &to))
                    .map_err(|e| format!("{file}: {e}"))?;
            }
        }
        if !found {
            return Err(format!("no build file names {from}: it is edited by hand"));
        }
    }
    Ok(())
}

/// A Go major: every import of the old module path becomes the `/vN` path, and the requirement in `go.mod` moves to it.
fn go_import_major(dir: &Path, update: &Update) -> Result<(), String> {
    let (base, next) = update
        .name
        .rsplit_once("/v")
        .and_then(|(b, n)| n.parse::<u64>().ok().map(|n| (b.to_string(), n)))
        .ok_or_else(|| format!("{} is not a /vN module path", update.name))?;
    let old = if next == 2 {
        base.clone()
    } else {
        format!("{base}/v{}", next - 1)
    };
    let new = &update.name;
    for file in go_files(dir)? {
        let text = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
        // The subpackages first: the exact path's rewrite would otherwise match the new path again.
        let edited = text
            .replace(&format!("\"{old}/"), &format!("\"{new}/"))
            .replace(&format!("\"{old}\""), &format!("\"{new}\""));
        if edited != text {
            std::fs::write(&file, edited).map_err(|e| e.to_string())?;
        }
    }
    let gomod = dir.join("go.mod");
    let text = std::fs::read_to_string(&gomod).map_err(|e| format!("go.mod: {e}"))?;
    let edited = text.replace(
        &format!("{old} {}", update.from),
        &format!("{new} {}", update.to),
    );
    if edited == text {
        return Err(format!(
            "go.mod requires {old} {}, not the release the batch names",
            update.from
        ));
    }
    std::fs::write(&gomod, edited).map_err(|e| format!("go.mod: {e}"))
}

/// The `.go` files under `dir`, skipping `vendor` and `.git`.
fn go_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        for entry in std::fs::read_dir(&at).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            if path.is_dir() {
                if name != "vendor" && name != ".git" {
                    stack.push(path);
                }
            } else if name.ends_with(".go") {
                out.push(path);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upgrades::Output;

    /// Answers a command by the start of its line; a command it does not know fails, as a tool does for a missing module.
    struct Canned(Vec<(&'static str, &'static str)>);

    impl Exec for Canned {
        fn run(
            &self,
            _dir: &Path,
            program: &str,
            args: &[&str],
            _env: &[(String, String)],
        ) -> Result<Output, String> {
            let key = format!("{program} {}", args.join(" "));
            Ok(match self.0.iter().find(|(k, _)| key.starts_with(k)) {
                Some((_, v)) => Output {
                    ok: true,
                    stdout: v.to_string(),
                    stderr: String::new(),
                },
                None => Output {
                    ok: false,
                    stdout: String::new(),
                    stderr: "not found".into(),
                },
            })
        }
    }

    /// Records the commands it is asked to run, and answers each with success.
    #[derive(Default)]
    struct Recorder(std::sync::Mutex<Vec<String>>);

    impl Exec for Recorder {
        fn run(
            &self,
            _dir: &Path,
            program: &str,
            args: &[&str],
            _env: &[(String, String)],
        ) -> Result<Output, String> {
            self.0
                .lock()
                .unwrap()
                .push(format!("{program} {}", args.join(" ")));
            Ok(Output {
                ok: true,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    /// A scratch project directory, removed when it drops.
    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Dir {
            let dir =
                std::env::temp_dir().join(format!("apid-others-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Dir(dir)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn none() -> &'static Path {
        Path::new("/nonexistent")
    }

    #[test]
    fn npm_outdated_splits_wanted_from_latest() {
        let exec = Canned(vec![(
            "npm outdated --json",
            r#"{"left-pad":{"current":"1.0.0","wanted":"1.2.0","latest":"1.2.0"},"chalk":{"current":"4.1.0","wanted":"4.1.2","latest":"5.3.0"}}"#,
        )]);
        let plan = outdated(Ecosystem::Npm, none(), &exec, &[]).unwrap();
        let minors: Vec<(&str, &str)> = plan
            .minor
            .iter()
            .map(|u| (u.name.as_str(), u.to.as_str()))
            .collect();
        assert!(
            minors.contains(&("left-pad", "1.2.0")) && minors.contains(&("chalk", "4.1.2")),
            "{minors:?}"
        );
        assert_eq!(
            (plan.majors[0].name.as_str(), plan.majors[0].to.as_str()),
            ("chalk", "5.3.0")
        );
    }

    #[test]
    fn a_go_module_has_its_minor_and_its_next_major_line() {
        let exec = Canned(vec![
            (
                "go list -m -u -json all",
                "{\"Path\":\"example.com/a\",\"Version\":\"v1.2.0\",\"Update\":{\"Version\":\"v1.3.1\"}}\n{\"Path\":\"example.com/c\",\"Version\":\"v1.2.0\"}\n",
            ),
            (
                "go list -m -versions example.com/c/v2",
                "example.com/c/v2 v2.0.0 v2.1.0\n",
            ),
            (
                "go list -m -versions example.com/a/v2",
                "example.com/a/v2\n",
            ),
        ]);
        let plan = outdated(Ecosystem::Go, none(), &exec, &[]).unwrap();
        assert_eq!(plan.minor.len(), 1, "{plan:?}");
        assert_eq!(plan.minor[0].to, "v1.3.1");
        assert_eq!(plan.majors.len(), 1, "{plan:?}");
        assert_eq!(
            (plan.majors[0].name.as_str(), plan.majors[0].to.as_str()),
            ("example.com/c/v2", "v2.1.0")
        );
    }

    #[test]
    fn a_go_major_moves_its_imports_and_its_requirement_to_the_vn_path() {
        let dir = Dir::new("go-major");
        std::fs::write(
            dir.0.join("go.mod"),
            "module app\n\nrequire example.com/c v1.2.0\n",
        )
        .unwrap();
        std::fs::write(
            dir.0.join("main.go"),
            "package main\n\nimport \"example.com/c\"\nimport \"example.com/c/sub\"\n",
        )
        .unwrap();
        let update = Update {
            name: "example.com/c/v2".into(),
            from: "v1.2.0".into(),
            to: "v2.1.0".into(),
            bump: Bump::Major,
            req: None,
        };
        let rec = Recorder::default();
        apply_major(Ecosystem::Go, &dir.0, &rec, &[], &update).unwrap();
        let main = std::fs::read_to_string(dir.0.join("main.go")).unwrap();
        assert!(
            main.contains("\"example.com/c/v2\"") && main.contains("\"example.com/c/v2/sub\""),
            "{main}"
        );
        let gomod = std::fs::read_to_string(dir.0.join("go.mod")).unwrap();
        assert!(gomod.contains("require example.com/c/v2 v2.1.0"), "{gomod}");
        assert_eq!(rec.0.lock().unwrap().as_slice(), ["go mod tidy"]);
    }

    #[test]
    fn a_pip_major_rewrites_the_requirement_and_upgrades_its_package() {
        let plan_exec = Canned(vec![(
            "uv pip list --outdated",
            r#"[{"name":"requests","version":"2.31.0","latest_version":"3.0.0"},{"name":"idna","version":"3.4","latest_version":"3.6"}]"#,
        )]);
        let plan = outdated(Ecosystem::Pip, none(), &plan_exec, &[]).unwrap();
        assert_eq!(plan.minor.len(), 1);
        assert_eq!(plan.majors[0].name, "requests");

        let dir = Dir::new("pip-major");
        std::fs::write(
            dir.0.join("pyproject.toml"),
            "[project]\ndependencies = [\"requests>=2.0,<3\", \"idna\"]\n",
        )
        .unwrap();
        let rec = Recorder::default();
        apply_major(Ecosystem::Pip, &dir.0, &rec, &[], &plan.majors[0]).unwrap();
        let text = std::fs::read_to_string(dir.0.join("pyproject.toml")).unwrap();
        assert!(text.contains("\"requests>=3.0.0\", \"idna\""), "{text}");
        assert_eq!(
            rec.0.lock().unwrap().as_slice(),
            ["uv lock --upgrade-package requests"]
        );
    }

    #[test]
    fn a_swiftpm_major_is_a_tag_on_the_other_line_and_moves_its_from_requirement() {
        let dir = Dir::new("swift-major");
        std::fs::write(
            dir.0.join("Package.resolved"),
            r#"{"pins":[{"identity":"swift-log","location":"https://example.com/swift-log.git","state":{"version":"1.4.0"}}],"version":2}"#,
        )
        .unwrap();
        let exec = Canned(vec![(
            "git ls-remote",
            "a\trefs/tags/1.4.0\nb\trefs/tags/1.5.2\nc\trefs/tags/2.0.0\n",
        )]);
        let plan = outdated(Ecosystem::Swiftpm, &dir.0, &exec, &[]).unwrap();
        assert_eq!(plan.minor[0].to, "1.5.2");
        let major = plan.majors[0].clone();
        assert_eq!(
            (major.to.as_str(), major.req.as_deref()),
            ("2.0.0", Some("https://example.com/swift-log.git"))
        );

        std::fs::write(
            dir.0.join("Package.swift"),
            "dependencies: [\n    .package(url: \"https://example.com/swift-log.git\", from: \"1.4.0\"),\n]\n",
        )
        .unwrap();
        let rec = Recorder::default();
        apply_major(Ecosystem::Swiftpm, &dir.0, &rec, &[], &major).unwrap();
        let text = std::fs::read_to_string(dir.0.join("Package.swift")).unwrap();
        assert!(text.contains("from: \"2.0.0\""), "{text}");
        assert_eq!(
            rec.0.lock().unwrap().as_slice(),
            ["swift package update swift-log"]
        );
    }

    #[test]
    fn gradle_reads_its_dependency_report_and_edits_its_coordinates() {
        let dir = Dir::new("gradle");
        std::fs::create_dir_all(dir.0.join("build/dependencyUpdates")).unwrap();
        std::fs::write(
            dir.0.join("build/dependencyUpdates/report.json"),
            r#"{"outdated":{"dependencies":[{"group":"com.google.guava","name":"guava","version":"31.0-jre","available":{"release":"31.1-jre"}},{"group":"org.example","name":"lib","version":"1.2.0","available":{"release":"2.0.0"}}],"count":2},"current":{}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.0.join("build.gradle"),
            "dependencies {\n    implementation 'com.google.guava:guava:31.0-jre'\n    implementation 'org.example:lib:1.2.0'\n}\n",
        )
        .unwrap();
        let exec = Canned(vec![("gradle dependencyUpdates", "")]);
        let plan = outdated(Ecosystem::Gradle, &dir.0, &exec, &[]).unwrap();
        assert_eq!(plan.minor.len(), 1, "{plan:?}");
        assert_eq!(
            (plan.minor[0].name.as_str(), plan.minor[0].to.as_str()),
            ("com.google.guava:guava", "31.1-jre")
        );
        assert_eq!(plan.majors[0].name, "org.example:lib");

        apply_minor(Ecosystem::Gradle, &dir.0, &exec, &[], &plan.minor).unwrap();
        let text = std::fs::read_to_string(dir.0.join("build.gradle")).unwrap();
        assert!(
            text.contains("guava:31.1-jre") && text.contains("org.example:lib:1.2.0"),
            "{text}"
        );
        apply_major(Ecosystem::Gradle, &dir.0, &exec, &[], &plan.majors[0]).unwrap();
        let text = std::fs::read_to_string(dir.0.join("build.gradle")).unwrap();
        assert!(text.contains("org.example:lib:2.0.0"), "{text}");

        // Without the plugin the outdated listing fails, and the reason says so.
        let missing = Canned(vec![]);
        let why = outdated(Ecosystem::Gradle, &dir.0, &missing, &[]).unwrap_err();
        assert!(why.contains("dependency-updates plugin"), "{why}");
    }

    #[test]
    fn a_batch_may_change_its_lockfile_and_a_major_may_also_change_the_manifest() {
        assert!(allowed(Ecosystem::Npm, false, "package-lock.json"));
        assert!(!allowed(Ecosystem::Npm, false, "package.json"));
        assert!(allowed(Ecosystem::Npm, true, "package.json"));
        assert!(allowed(Ecosystem::Nuget, false, "App/packages.lock.json"));
        assert!(!allowed(Ecosystem::Nuget, false, "App/App.csproj"));
        assert!(allowed(Ecosystem::Nuget, true, "App/App.csproj"));
        assert!(allowed(Ecosystem::Gradle, false, "build.gradle.kts"));
        assert!(!allowed(Ecosystem::Gradle, true, "settings.gradle"));
    }
}
