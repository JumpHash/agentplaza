//! The compatibility and install-and-upgrade matrices of a release (PLAN 22.9, 10.10). A project declares, per product
//! type, the axes its release must hold on (`.plaza/compatibility.toml`); the matrix expands into cells, one stage run
//! each; every cell is placed on a pool (a node) that has the capabilities the cell needs, and a cell no pool can run is
//! `not run` with its reason. The hub judges what the runs report (`apid`'s `releases::compat`).
//!
//! ```toml
//! [cli]
//! command = ["tool", "--version"]     # run in each shell, with the expected output below
//! load = "tool.sh"                    # optional: sourced in the shell first (a shell CLI's functions)
//! expect = "tool 1.2.0"
//! shells = ["bash", "zsh", "sh", "dash", "fish", "pwsh"]   # the default
//! systems = ["macos", "linux", "windows"]                  # the default
//!
//! [library]
//! methods = ["cargo-semver-checks"]   # the default
//!
//! [web]                               # browsers: chromium, firefox, webkit (the default)
//! [desktop]                           # platforms: macos, windows, linux (the default)
//! [mobile]                            # platforms: ios, android (the default)
//! ```
//!
//! The runners here are the stage runs a pool executes: a shell cell runs the declared command in its shell and compares
//! the output; an API cell runs `cargo semver-checks` against the previous release's revision. Cells of the other product
//! types have no runner in this build, so they are reported as planned, not run.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// The shells the CLI matrix names by default.
pub const SHELLS: [&str; 6] = ["bash", "zsh", "sh", "dash", "fish", "pwsh"];
/// The systems the CLI matrix names by default.
pub const SYSTEMS: [&str; 3] = ["macos", "linux", "windows"];
/// The browsers the web matrix names by default.
pub const BROWSERS: [&str; 3] = ["chromium", "firefox", "webkit"];
/// The platforms the desktop and mobile matrices name by default.
pub const DESKTOPS: [&str; 3] = ["macos", "windows", "linux"];
pub const MOBILES: [&str; 2] = ["ios", "android"];
/// The API checks the library matrix names by default.
pub const METHODS: [&str; 1] = ["cargo-semver-checks"];

/// The file a project declares its matrix in, in its Weft repository at main.
pub const FILE: &str = ".plaza/compatibility.toml";

/// How long a shell cell's command may run before it is a failure.
const TIMEOUT: Duration = Duration::from_secs(120);

/// The declared matrix of a project: the product types it ships and their axes. A product that is absent is not part of
/// the release's matrix.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Matrix {
    pub web: Option<Web>,
    pub cli: Option<Cli>,
    pub desktop: Option<Platforms>,
    pub mobile: Option<Platforms>,
    pub library: Option<Library>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Web {
    pub browsers: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cli {
    /// The command, as its argument vector, run in each shell.
    pub command: Vec<String>,
    /// A file sourced in the shell before the command (a shell CLI's functions); absent for a binary.
    #[serde(default)]
    pub load: Option<String>,
    /// The output the command must print (trailing newlines aside).
    pub expect: String,
    #[serde(default)]
    pub shells: Option<Vec<String>>,
    #[serde(default)]
    pub systems: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Platforms {
    pub platforms: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Library {
    pub methods: Option<Vec<String>>,
}

/// Parses a project's declaration. An unknown key is refused, as project.toml's are.
pub fn parse(text: &str) -> Result<Matrix, String> {
    let matrix: Matrix =
        toml::from_str(text).map_err(|e| format!("the matrix is not valid: {e}"))?;
    if matrix
        .cli
        .as_ref()
        .is_some_and(|cli| cli.command.is_empty())
    {
        return Err("[cli] needs the command it runs".into());
    }
    Ok(matrix)
}

/// What a cell runs and what it is judged by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Kind {
    /// The declared command in one shell on one system.
    Shell {
        shell: String,
        system: String,
        load: Option<String>,
        command: Vec<String>,
        expect: String,
    },
    /// An API compatibility check against the previous release.
    Api { method: String },
    /// A product type with no runner in this build (planned in 22.9): the reason it is not run.
    Planned { reason: String },
}

/// One cell of the matrix: a stage run's identity, the capabilities a pool must have to run it, and what it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Cell {
    /// The product type: web, cli, desktop, mobile or library.
    pub product: String,
    /// The cell's name within the matrix (`cli:bash:macos`).
    pub id: String,
    /// The capabilities a pool needs (`shell:bash`, `os:macos`, `tool:cargo-semver-checks`).
    pub needs: Vec<String>,
    pub kind: Kind,
}

/// Expands a declaration into its cells, in product order (web, cli, desktop, mobile, library).
pub fn expand(matrix: &Matrix) -> Vec<Cell> {
    let mut cells = Vec::new();
    if let Some(web) = &matrix.web {
        for browser in list(&web.browsers, &BROWSERS) {
            cells.push(Cell {
                product: "web".into(),
                id: format!("web:{browser}"),
                needs: vec![format!("browser:{browser}")],
                kind: Kind::Planned {
                    reason: planned("web"),
                },
            });
        }
    }
    if let Some(cli) = &matrix.cli {
        for shell in list(&cli.shells, &SHELLS) {
            for system in list(&cli.systems, &SYSTEMS) {
                cells.push(Cell {
                    product: "cli".into(),
                    id: format!("cli:{shell}:{system}"),
                    needs: vec![format!("shell:{shell}"), format!("os:{system}")],
                    kind: Kind::Shell {
                        shell: shell.clone(),
                        system: system.clone(),
                        load: cli.load.clone(),
                        command: cli.command.clone(),
                        expect: cli.expect.clone(),
                    },
                });
            }
        }
    }
    if let Some(desktop) = &matrix.desktop {
        for platform in list(&desktop.platforms, &DESKTOPS) {
            let signer = match platform.as_str() {
                "macos" => "codesign",
                "windows" => "signtool",
                _ => "gpg",
            };
            for stage in ["install", "upgrade", "signing"] {
                let mut needs = vec![format!("os:{platform}")];
                if stage == "signing" {
                    needs.push(format!("tool:{signer}"));
                }
                cells.push(Cell {
                    product: "desktop".into(),
                    id: format!("desktop:{platform}:{stage}"),
                    needs,
                    kind: Kind::Planned {
                        reason: planned("desktop"),
                    },
                });
            }
        }
    }
    if let Some(mobile) = &matrix.mobile {
        for platform in list(&mobile.platforms, &MOBILES) {
            let host = if platform == "ios" {
                "simulator"
            } else {
                "emulator"
            };
            for stage in ["install", "upgrade"] {
                cells.push(Cell {
                    product: "mobile".into(),
                    id: format!("mobile:{platform}:{stage}"),
                    needs: vec![host.to_string()],
                    kind: Kind::Planned {
                        reason: planned("mobile"),
                    },
                });
            }
        }
    }
    if let Some(library) = &matrix.library {
        for method in list(&library.methods, &METHODS) {
            cells.push(Cell {
                product: "library".into(),
                id: format!("api:{method}"),
                needs: vec![format!("tool:{method}")],
                kind: Kind::Api {
                    method: method.clone(),
                },
            });
        }
    }
    cells
}

fn list(given: &Option<Vec<String>>, default: &[&str]) -> Vec<String> {
    given
        .clone()
        .unwrap_or_else(|| default.iter().map(|s| (*s).to_string()).collect())
}

fn planned(product: &str) -> String {
    format!(
        "no runner for {product} cells in this build: they run on the pool that has the hardware, as 22.9 plans"
    )
}

/// A pool: a node and the capabilities it reports (`shell:bash`, `os:macos`, ...).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pool {
    pub name: String,
    pub capabilities: BTreeSet<String>,
}

/// Where a cell runs: on a pool, or not at all with the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    On(String),
    NotRun(String),
}

/// Places a cell on the first pool that has every capability it needs (pure). When none does, the reason names the
/// capabilities no pool has, or the ones no single pool has together.
pub fn place(cell: &Cell, pools: &[Pool]) -> Placement {
    if let Some(pool) = pools.iter().find(|pool| {
        cell.needs
            .iter()
            .all(|need| pool.capabilities.contains(need))
    }) {
        return Placement::On(pool.name.clone());
    }
    if pools.is_empty() {
        return Placement::NotRun("no pool is in the fleet".into());
    }
    let unheld: Vec<&String> = cell
        .needs
        .iter()
        .filter(|need| !pools.iter().any(|pool| pool.capabilities.contains(*need)))
        .collect();
    let reason = if unheld.is_empty() {
        format!("no pool has all of {}", cell.needs.join(", "))
    } else {
        format!(
            "no pool has {}",
            unheld
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    Placement::NotRun(reason)
}

/// What a pool reports for one cell: whether it ran, whether it passed, the evidence, and the reason when it did not run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub cell: String,
    pub ran: bool,
    pub passed: bool,
    #[serde(default)]
    pub evidence: String,
    #[serde(default)]
    pub reason: String,
}

impl Outcome {
    /// A cell that ran and passed, or failed with its evidence.
    pub fn ran(cell: &str, passed: bool, evidence: String) -> Outcome {
        Outcome {
            cell: cell.into(),
            ran: true,
            passed,
            evidence,
            reason: String::new(),
        }
    }

    /// A cell that did not run, with its reason. It never counts as passed.
    pub fn not_run(cell: &str, reason: String) -> Outcome {
        Outcome {
            cell: cell.into(),
            ran: false,
            passed: false,
            evidence: String::new(),
            reason,
        }
    }
}

/// The script a shell runs for a cell: the load file sourced, then the command with each argument quoted.
pub fn shell_script(load: Option<&str>, command: &[String]) -> String {
    let words: Vec<String> = command.iter().map(|arg| quote(arg)).collect();
    match load {
        // A bare name is looked up on the PATH by `.` (zsh does so), so a file in the working directory is `./name`.
        Some(file) => {
            let path = if file.contains('/') {
                file.to_string()
            } else {
                format!("./{file}")
            };
            format!(". {} && {}", quote(&path), words.join(" "))
        }
        None => words.join(" "),
    }
}

/// A word in single quotes, as the POSIX shells read it.
fn quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// Runs a shell cell's command in its shell, in `dir`, and judges its output against the expectation. The shell is found
/// on the PATH; a shell that is not there is not run, with the reason.
pub fn run_shell(cell: &Cell, dir: &Path) -> Outcome {
    let Kind::Shell {
        shell,
        load,
        command,
        expect,
        ..
    } = &cell.kind
    else {
        return Outcome::not_run(&cell.id, "not a shell cell".into());
    };
    let script = shell_script(load.as_deref(), command);
    let child = Command::new(shell)
        .arg("-c")
        .arg(&script)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let child = match child {
        Ok(child) => child,
        Err(e) => return Outcome::not_run(&cell.id, format!("{shell} did not start: {e}")),
    };
    let output = match wait(child, TIMEOUT) {
        Ok(output) => output,
        Err(why) => return Outcome::ran(&cell.id, false, format!("{shell}: {why}")),
    };
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let got = stdout.trim_end();
    let passed = output.status.success() && got == expect.trim_end();
    let evidence = if passed {
        format!("{shell}: exit 0, printed {got:?}")
    } else {
        format!(
            "{shell}: exit {:?}, printed {got:?}, expected {:?}; stderr: {}",
            output.status.code(),
            expect.trim_end(),
            stderr.trim()
        )
    };
    Outcome::ran(&cell.id, passed, evidence)
}

/// Waits for a child up to `limit`, killing it past the limit.
fn wait(mut child: std::process::Child, limit: Duration) -> Result<std::process::Output, String> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().map_err(|e| e.to_string()),
            Ok(None) if started.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {}s", limit.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// Runs an API cell's check in the release's tree against the baseline revision. A check that is not installed is not
/// run, with the reason; a check that reports a breaking change fails with its output.
pub fn run_api(cell: &Cell, dir: &Path, baseline: &str) -> Outcome {
    let Kind::Api { method } = &cell.kind else {
        return Outcome::not_run(&cell.id, "not an API cell".into());
    };
    if method != "cargo-semver-checks" {
        return Outcome::not_run(&cell.id, format!("no runner for {method} in this build"));
    }
    if !on_path("cargo-semver-checks") {
        return Outcome::not_run(
            &cell.id,
            "cargo-semver-checks is not installed on this pool".into(),
        );
    }
    let output = Command::new("cargo")
        .args(["semver-checks", "check-release", "--baseline-rev", baseline])
        .current_dir(dir)
        .stdin(Stdio::null())
        .output();
    match output {
        Err(e) => Outcome::not_run(&cell.id, format!("cargo semver-checks did not start: {e}")),
        Ok(out) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            let lines: Vec<&str> = text.lines().collect();
            let tail = lines[lines.len().saturating_sub(12)..].join("\n");
            Outcome::ran(
                &cell.id,
                out.status.success(),
                format!("exit {:?}\n{tail}", out.status.code()),
            )
        }
    }
}

/// The system this machine is, as the matrix names it (`macos`, `linux`, `windows`): the `os:` capability it has.
pub fn local_system() -> &'static str {
    std::env::consts::OS
}

/// The major version of the `bash` on the PATH, or None when there is none. The Mac's `/bin/bash` is 3.2, which has no
/// bash-4 expansion such as `${1,,}`: a test that expects one to pass in bash reads the version first.
pub fn bash_major() -> Option<u32> {
    let out = Command::new("bash")
        .args(["-c", "echo \"${BASH_VERSINFO[0]}\""])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// The pool this machine is, as far as its shells, OS and tools go (the tests' pool, and a probe a node can report).
/// It looks only: PATH and the OS, nothing is run.
pub fn local_pool(name: &str) -> Pool {
    let mut capabilities = BTreeSet::new();
    capabilities.insert(format!("os:{}", local_system()));
    for shell in SHELLS {
        if on_path(shell) {
            capabilities.insert(format!("shell:{shell}"));
        }
    }
    for tool in ["codesign", "signtool", "gpg", "cargo-semver-checks"] {
        if on_path(tool) {
            capabilities.insert(format!("tool:{tool}"));
        }
    }
    Pool {
        name: name.into(),
        capabilities,
    }
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLI: &str =
        "[cli]\ncommand = [\"tool\", \"--version\"]\nload = \"tool.sh\"\nexpect = \"tool 1.2.0\"\n";

    fn pool(name: &str, caps: &[&str]) -> Pool {
        Pool {
            name: name.into(),
            capabilities: caps.iter().map(|c| (*c).to_string()).collect(),
        }
    }

    #[test]
    fn the_cli_matrix_expands_to_shells_times_systems_with_their_needs() {
        let matrix = parse(CLI).unwrap();
        let cells = expand(&matrix);
        assert_eq!(cells.len(), SHELLS.len() * SYSTEMS.len());
        let bash = cells.iter().find(|c| c.id == "cli:bash:macos").unwrap();
        assert_eq!(bash.needs, ["shell:bash", "os:macos"]);
        assert!(matches!(&bash.kind, Kind::Shell { load: Some(l), .. } if l == "tool.sh"));
    }

    #[test]
    fn a_cell_is_placed_on_a_pool_with_every_capability_or_not_run_with_the_reason() {
        let cells = expand(&parse(CLI).unwrap());
        let pools = [
            pool("mac", &["shell:bash", "os:macos"]),
            pool("lnx", &["shell:fish", "os:linux"]),
        ];
        let bash = cells.iter().find(|c| c.id == "cli:bash:macos").unwrap();
        assert_eq!(place(bash, &pools), Placement::On("mac".into()));
        let fish = cells.iter().find(|c| c.id == "cli:fish:linux").unwrap();
        assert_eq!(place(fish, &pools), Placement::On("lnx".into()));
        let pwsh = cells.iter().find(|c| c.id == "cli:pwsh:macos").unwrap();
        assert_eq!(
            place(pwsh, &pools),
            Placement::NotRun("no pool has shell:pwsh".into())
        );
        let split = cells.iter().find(|c| c.id == "cli:fish:macos").unwrap();
        assert_eq!(
            place(split, &pools),
            Placement::NotRun("no pool has all of shell:fish, os:macos".into())
        );
        assert_eq!(
            place(bash, &[]),
            Placement::NotRun("no pool is in the fleet".into())
        );
    }

    #[test]
    fn a_web_desktop_or_mobile_cell_is_planned_and_placed_only_where_its_hardware_is() {
        let matrix =
            parse("[web]\n[desktop]\nplatforms = [\"macos\"]\n[mobile]\nplatforms = [\"ios\"]\n")
                .unwrap();
        let cells = expand(&matrix);
        let chromium = cells.iter().find(|c| c.id == "web:chromium").unwrap();
        assert_eq!(chromium.needs, ["browser:chromium"]);
        assert!(matches!(chromium.kind, Kind::Planned { .. }));
        let signing = cells
            .iter()
            .find(|c| c.id == "desktop:macos:signing")
            .unwrap();
        assert_eq!(signing.needs, ["os:macos", "tool:codesign"]);
        let ios = cells.iter().find(|c| c.id == "mobile:ios:install").unwrap();
        assert_eq!(ios.needs, ["simulator"]);
        assert_eq!(
            place(ios, &[pool("mac", &["os:macos"])]),
            Placement::NotRun("no pool has simulator".into())
        );
    }

    #[test]
    fn the_library_matrix_names_its_check_as_a_tool_a_pool_must_have() {
        let cells = expand(&parse("[library]\n").unwrap());
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].id, "api:cargo-semver-checks");
        assert_eq!(cells[0].needs, ["tool:cargo-semver-checks"]);
    }

    #[test]
    fn the_declaration_refuses_unknown_keys_and_a_cli_without_a_command() {
        assert!(parse("[cli]\nexpect = \"x\"\n").is_err());
        assert!(parse("[cli]\ncommand = [\"x\"]\nexpect = \"x\"\nshel = [\"bash\"]\n").is_err());
        assert!(parse("[nope]\n").is_err());
    }

    #[test]
    fn the_script_sources_the_load_file_then_quotes_each_word() {
        assert_eq!(
            shell_script(
                Some("tool.sh"),
                &["tool".into(), "--version".into(), "a'b".into()]
            ),
            ". './tool.sh' && 'tool' '--version' 'a'\\''b'"
        );
        assert_eq!(shell_script(None, &["tool".into()]), "'tool'");
    }
}
