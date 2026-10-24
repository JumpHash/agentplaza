//! The ecosystems an upgrade reads (PLAN 24.2): each one is found from its manifests at the root of a project's `main`,
//! and needs its tool on the hub's PATH. A tool is never installed: an ecosystem whose tool is missing is skipped, and the
//! run says why.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ecosystem {
    Cargo,
    Npm,
    Pip,
    Go,
    Gradle,
    Nuget,
    Swiftpm,
}

impl Ecosystem {
    pub const ALL: [Ecosystem; 7] = [
        Ecosystem::Cargo,
        Ecosystem::Npm,
        Ecosystem::Pip,
        Ecosystem::Go,
        Ecosystem::Gradle,
        Ecosystem::Nuget,
        Ecosystem::Swiftpm,
    ];

    /// The name the ecosystem goes by in records and in a subject (`cargo`, `pip`, `swiftpm`).
    pub fn name(self) -> &'static str {
        match self {
            Ecosystem::Cargo => "cargo",
            Ecosystem::Npm => "npm",
            Ecosystem::Pip => "pip",
            Ecosystem::Go => "go",
            Ecosystem::Gradle => "gradle",
            Ecosystem::Nuget => "nuget",
            Ecosystem::Swiftpm => "swiftpm",
        }
    }

    pub fn parse(text: &str) -> Option<Ecosystem> {
        Ecosystem::ALL.into_iter().find(|e| e.name() == text)
    }

    /// The programs the ecosystem's adapter runs; the first is the one that must be on PATH.
    pub fn tools(self) -> &'static [&'static str] {
        match self {
            Ecosystem::Cargo => &["cargo"],
            Ecosystem::Npm => &["npm"],
            Ecosystem::Pip => &["uv"],
            Ecosystem::Go => &["go"],
            Ecosystem::Gradle => &["gradle"],
            Ecosystem::Nuget => &["dotnet"],
            Ecosystem::Swiftpm => &["swift", "git"],
        }
    }

    /// Whether a root-level file name is one of the ecosystem's manifests.
    fn manifest(self, name: &str) -> bool {
        match self {
            Ecosystem::Cargo => name == "Cargo.toml",
            Ecosystem::Npm => name == "package.json",
            Ecosystem::Pip => matches!(name, "pyproject.toml" | "uv.lock"),
            Ecosystem::Go => name == "go.mod",
            Ecosystem::Gradle => matches!(
                name,
                "build.gradle" | "build.gradle.kts" | "settings.gradle" | "settings.gradle.kts"
            ),
            Ecosystem::Nuget => name.ends_with(".csproj") || name.ends_with(".sln"),
            Ecosystem::Swiftpm => name == "Package.swift",
        }
    }
}

/// The ecosystems whose manifests are among `paths` (a project's files, `/`-separated; only the root's count).
pub fn detect(paths: &[String]) -> Vec<Ecosystem> {
    Ecosystem::ALL
        .into_iter()
        .filter(|eco| {
            paths
                .iter()
                .filter(|p| !p.contains('/'))
                .any(|p| eco.manifest(p))
        })
        .collect()
}

/// The program `name` on `path_var` (a PATH value), when it is an executable file there.
pub fn find_tool(name: &str, path_var: &str) -> Option<PathBuf> {
    std::env::split_paths(path_var)
        .filter(|dir| !dir.as_os_str().is_empty())
        .flat_map(|dir| candidates(&dir, name))
        .find(|candidate| is_executable(candidate))
}

/// The files `name` may be in `dir`: the name itself, and on Windows its usual program extensions.
fn candidates(dir: &Path, name: &str) -> Vec<PathBuf> {
    let mut files = vec![dir.join(name)];
    if cfg!(windows) {
        files.extend(["exe", "cmd", "bat"].map(|ext| dir.join(format!("{name}.{ext}"))));
    }
    files
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Windows has no execute bit: a file there is a program by its extension, which [`candidates`] chose.
#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Whether the ecosystem can run here: every one of its tools is on `path_var`. The reason names the first that is not.
pub fn available(eco: Ecosystem, path_var: &str) -> Result<(), String> {
    for tool in eco.tools() {
        if find_tool(tool, path_var).is_none() {
            return Err(format!(
                "{} is not run: `{tool}` is not on the hub's PATH",
                eco.name()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ecosystems_are_detected_from_root_manifests_only() {
        let paths: Vec<String> = [
            "Cargo.toml",
            "src/lib.rs",
            "web/package.json",
            "app.csproj",
            "go.mod",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            detect(&paths),
            vec![Ecosystem::Cargo, Ecosystem::Go, Ecosystem::Nuget]
        );
        assert!(detect(&["web/package.json".into()]).is_empty());
    }

    // The fake tools are shell scripts with the execute bit: Unix only.
    #[cfg(unix)]
    #[test]
    fn a_missing_tool_is_a_reason_and_a_present_one_is_found() {
        let dir = std::env::temp_dir().join(format!("apid-tools-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tool = dir.join("npm");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = dir.display().to_string();
        assert!(available(Ecosystem::Npm, &path).is_ok());
        let why = available(Ecosystem::Go, &path).unwrap_err();
        assert!(
            why.contains("go") && why.contains("not on the hub's PATH"),
            "{why}"
        );
        // A file without the execute bit is not a tool.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(find_tool("npm", &path).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
