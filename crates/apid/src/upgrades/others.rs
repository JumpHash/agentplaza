//! The adapters of the ecosystems other than Cargo (PLAN 24.2). Each reads its outdated dependencies from its own tool's
//! output, applies a batch with its tool, and runs its suite. The tool runs in a scratch copy of the project and is never
//! installed. What an adapter does not build is said where it is refused: a major update of pip, Go or SwiftPM (their
//! requirements name import paths or manifests that the adapter does not rewrite), and Gradle's outdated listing (the
//! build needs a dependency-updates plugin for it). Test selection is not built for these ecosystems: their batches run
//! the whole suite.

use std::path::Path;

use serde_json::Value;

use super::ecosystem::Ecosystem;
use super::version::{Bump, Version};
use super::{Exec, Update};

/// What an ecosystem would update: its batch (patch and minor), and its majors.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outdated {
    pub minor: Vec<Update>,
    pub majors: Vec<Update>,
}

/// The update of `name` from `from` to `to`, when `to` is a newer release.
fn update(name: &str, from: &str, to: &str) -> Option<Update> {
    let from_v = Version::parse(from)?;
    let to_v = Version::parse(to)?;
    if to_v <= from_v {
        return None;
    }
    Some(Update {
        name: name.to_string(),
        from: from.to_string(),
        to: to.to_string(),
        bump: Bump::between(&from_v, &to_v),
        req: None,
    })
}

/// Splits a plan's updates into the batch and the majors.
fn split(updates: Vec<Update>) -> Outdated {
    let (majors, minor): (Vec<Update>, Vec<Update>) =
        updates.into_iter().partition(|u| u.bump == Bump::Major);
    Outdated { minor, majors }
}

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
                if let Some(u) = update(name, current, wanted) {
                    updates.push(u);
                }
                if let Some(u) = update(name, wanted, latest).filter(|u| u.bump == Bump::Major) {
                    updates.push(u);
                }
            }
            Ok(split(updates))
        }
        Ecosystem::Pip => {
            let text = stdout_of(exec, dir, env, "uv", &["pip", "list", "--outdated", "--format", "json"])?;
            let value: Value = serde_json::from_str(&text).map_err(|e| format!("uv pip list: {e}"))?;
            let updates = value
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| update(p["name"].as_str()?, p["version"].as_str()?, p["latest_version"].as_str()?))
                .filter(|u| u.bump != Bump::Major)
                .collect();
            Ok(split(updates))
        }
        Ecosystem::Go => {
            let text = stdout_of(exec, dir, env, "go", &["list", "-m", "-u", "-json", "all"])?;
            let mut updates = Vec::new();
            for module in serde_json::Deserializer::from_str(&text).into_iter::<Value>() {
                let module = module.map_err(|e| format!("go list: {e}"))?;
                if let (Some(path), Some(version), Some(newer)) = (
                    module["Path"].as_str(),
                    module["Version"].as_str(),
                    module["Update"]["Version"].as_str(),
                ) && let Some(u) = update(path, version, newer)
                    && u.bump != Bump::Major
                {
                    updates.push(u);
                }
            }
            Ok(split(updates))
        }
        Ecosystem::Nuget => {
            let text = stdout_of(exec, dir, env, "dotnet", &["list", "package", "--outdated", "--format", "json"])?;
            let value: Value = serde_json::from_str(&text).map_err(|e| format!("dotnet list package: {e}"))?;
            let mut updates = Vec::new();
            for project in value["projects"].as_array().into_iter().flatten() {
                let path = project["path"].as_str().unwrap_or_default();
                for framework in project["frameworks"].as_array().into_iter().flatten() {
                    for package in framework["topLevelPackages"].as_array().into_iter().flatten() {
                        let (Some(id), Some(now), Some(latest)) = (
                            package["id"].as_str(),
                            package["resolvedVersion"].as_str(),
                            package["latestVersion"].as_str(),
                        ) else {
                            continue;
                        };
                        if let Some(mut u) = update(id, now, latest) {
                            // The project file the package is in, so the upgrade adds it there.
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
            let value: Value = serde_json::from_str(&resolved).map_err(|e| format!("Package.resolved: {e}"))?;
            let mut updates = Vec::new();
            for pin in value["pins"].as_array().into_iter().flatten() {
                let (Some(identity), Some(location), Some(current)) = (
                    pin["identity"].as_str(),
                    pin["location"].as_str(),
                    pin["state"]["version"].as_str(),
                ) else {
                    continue;
                };
                let tags = stdout_of(exec, dir, env, "git", &["ls-remote", "--tags", "--refs", location])?;
                let newest = tags
                    .lines()
                    .filter_map(|l| l.rsplit("refs/tags/").next())
                    .filter_map(|t| Version::parse(t).map(|v| (v, t.to_string())))
                    .filter(|(v, _)| Version::parse(current).is_some_and(|c| v.compat() == c.compat()))
                    .max_by_key(|(v, _)| *v);
                if let Some((_, tag)) = newest
                    && let Some(u) = update(identity, current, tag.trim_start_matches('v'))
                {
                    updates.push(u);
                }
            }
            Ok(split(updates))
        }
        Ecosystem::Gradle => Err(
            "the build keeps no dependency report: its outdated dependencies need the dependency-updates plugin".into(),
        ),
        Ecosystem::Cargo => Err("Cargo's plan is its own (cargo.rs)".into()),
    }
}

/// Applies a patch and minor batch with the ecosystem's tool in `dir`.
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
        other => return Err(format!("{} has no apply in this build", other.name())),
    };
    run_all(exec, dir, env, commands)
}

/// Applies one major update with the ecosystem's tool in `dir`, where the adapter builds it.
pub fn apply_major(
    eco: Ecosystem,
    dir: &Path,
    exec: &dyn Exec,
    env: &[(String, String)],
    update: &Update,
) -> Result<(), String> {
    let commands: Vec<(&str, Vec<String>)> = match eco {
        Ecosystem::Npm => vec![(
            "npm",
            vec!["install".into(), format!("{}@{}", update.name, update.to)],
        )],
        Ecosystem::Nuget => vec![(
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
        other => return Err(format!("a major update of {} is not built", other.name())),
    };
    run_all(exec, dir, env, commands)
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
pub fn allowed(eco: Ecosystem, major: bool, path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upgrades::Output;
    use std::path::PathBuf;

    /// Answers each program with a canned output.
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
            let text = self
                .0
                .iter()
                .find(|(k, _)| key.starts_with(k))
                .map(|(_, v)| v.to_string())
                .ok_or_else(|| format!("no canned answer for {key}"))?;
            Ok(Output {
                ok: true,
                stdout: text,
                stderr: String::new(),
            })
        }
    }

    fn dir() -> PathBuf {
        PathBuf::from("/nonexistent")
    }

    #[test]
    fn npm_outdated_splits_wanted_from_latest() {
        let exec = Canned(vec![(
            "npm outdated --json",
            r#"{"left-pad":{"current":"1.0.0","wanted":"1.2.0","latest":"1.2.0"},"chalk":{"current":"4.1.0","wanted":"4.1.2","latest":"5.3.0"}}"#,
        )]);
        let plan = outdated(Ecosystem::Npm, &dir(), &exec, &[]).unwrap();
        let minors: Vec<(&str, &str)> = plan
            .minor
            .iter()
            .map(|u| (u.name.as_str(), u.to.as_str()))
            .collect();
        assert!(
            minors.contains(&("left-pad", "1.2.0")) && minors.contains(&("chalk", "4.1.2")),
            "{minors:?}"
        );
        assert_eq!(plan.majors.len(), 1);
        assert_eq!(
            (plan.majors[0].name.as_str(), plan.majors[0].to.as_str()),
            ("chalk", "5.3.0")
        );
    }

    #[test]
    fn go_modules_and_nuget_packages_are_read_from_their_tools() {
        let go = Canned(vec![(
            "go list",
            "{\"Path\":\"example.com/a\",\"Version\":\"v1.2.0\",\"Update\":{\"Version\":\"v1.3.1\"}}\n{\"Path\":\"example.com/b\",\"Version\":\"v1.0.0\"}\n{\"Path\":\"example.com/c\",\"Version\":\"v1.0.0\",\"Update\":{\"Version\":\"v2.0.0\"}}\n",
        )]);
        let plan = outdated(Ecosystem::Go, &dir(), &go, &[]).unwrap();
        assert_eq!(plan.minor.len(), 1, "{plan:?}");
        assert_eq!(plan.minor[0].to, "v1.3.1");
        assert!(
            plan.majors.is_empty(),
            "a Go major is not built: it is not planned"
        );

        let nuget = Canned(vec![(
            "dotnet list",
            r#"{"projects":[{"path":"App/App.csproj","frameworks":[{"topLevelPackages":[{"id":"Newtonsoft.Json","resolvedVersion":"13.0.1","latestVersion":"13.0.3"},{"id":"Serilog","resolvedVersion":"2.0.0","latestVersion":"4.0.0"}]}]}]}"#,
        )]);
        let plan = outdated(Ecosystem::Nuget, &dir(), &nuget, &[]).unwrap();
        assert_eq!(plan.minor[0].req.as_deref(), Some("App/App.csproj"));
        assert_eq!(plan.majors[0].name, "Serilog");
    }

    #[test]
    fn swiftpm_reads_its_pins_tags_and_gradle_says_why_it_cannot() {
        let scratch = std::env::temp_dir().join(format!("apid-swift-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).unwrap();
        std::fs::write(
            scratch.join("Package.resolved"),
            r#"{"pins":[{"identity":"swift-log","location":"https://example.com/swift-log.git","state":{"version":"1.4.0"}}],"version":2}"#,
        )
        .unwrap();
        let git = Canned(vec![(
            "git ls-remote",
            "abc\trefs/tags/1.4.0\ndef\trefs/tags/1.5.2\nghi\trefs/tags/2.0.0\n",
        )]);
        let plan = outdated(Ecosystem::Swiftpm, &scratch, &git, &[]).unwrap();
        assert_eq!(plan.minor.len(), 1);
        assert_eq!(plan.minor[0].to, "1.5.2");
        let _ = std::fs::remove_dir_all(&scratch);
        let why = outdated(Ecosystem::Gradle, &dir(), &git, &[]).unwrap_err();
        assert!(why.contains("dependency-updates plugin"));
    }

    #[test]
    fn a_batch_may_change_its_lockfile_and_a_major_may_also_change_the_manifest() {
        assert!(allowed(Ecosystem::Npm, false, "package-lock.json"));
        assert!(!allowed(Ecosystem::Npm, false, "package.json"));
        assert!(allowed(Ecosystem::Npm, true, "package.json"));
        assert!(allowed(Ecosystem::Nuget, false, "App/packages.lock.json"));
        assert!(!allowed(Ecosystem::Nuget, false, "App/App.csproj"));
        assert!(allowed(Ecosystem::Nuget, true, "App/App.csproj"));
        assert!(!allowed(Ecosystem::Gradle, true, "build.gradle"));
    }
}
