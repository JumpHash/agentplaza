//! The CLI matrix run for real on this machine (PLAN 22.9, 10.10): the shells on the PATH run the declared command, and
//! the seeded shell incompatibilities fail where the shell cannot run them. Shells and tools that are not here are not
//! run, with the reason; nothing is installed.

use std::path::Path;

use apz_topology::compat::{self, Cell, Kind, Matrix, Outcome, Placement};

/// A POSIX CLI: one function, `tool --version` prints the version. Runs in bash, zsh, sh and dash.
const GOOD: &str = "tool() {\n  case \"$1\" in\n    --version) echo \"tool 1.2.0\" ;;\n    *) echo \"usage: tool --version\" >&2; return 2 ;;\n  esac\n}\n";

/// The same CLI with a bashism: `${1,,}` (lowercase expansion) is bash's; sh and dash say "bad substitution".
/// (`[[` is not a seeded case: this Mac's sh is bash in posix mode and accepts it.)
const BASHISM: &str =
    "tool() {\n  if [ \"${1,,}\" = \"--version\" ]; then echo \"tool 1.2.0\"; fi\n}\n";

fn matrix() -> Matrix {
    compat::parse(
        "[cli]\ncommand = [\"tool\", \"--version\"]\nload = \"tool.sh\"\nexpect = \"tool 1.2.0\"\n\
         systems = [\"macos\", \"linux\", \"windows\"]\n",
    )
    .unwrap()
}

fn run_all(dir: &Path, script: &str) -> Vec<(Cell, Placement, Option<Outcome>)> {
    std::fs::write(dir.join("tool.sh"), script).unwrap();
    let pools = [compat::local_pool("this-mac")];
    compat::expand(&matrix())
        .into_iter()
        .map(|cell| {
            let placement = compat::place(&cell, &pools);
            let outcome =
                matches!(placement, Placement::On(_)).then(|| compat::run_shell(&cell, dir));
            (cell, placement, outcome)
        })
        .collect()
}

fn find<'a>(
    rows: &'a [(Cell, Placement, Option<Outcome>)],
    id: &str,
) -> &'a (Cell, Placement, Option<Outcome>) {
    rows.iter()
        .find(|(cell, _, _)| cell.id == id)
        .expect("the cell is in the matrix")
}

fn on_this_mac(shell: &str) -> bool {
    compat::local_pool("probe")
        .capabilities
        .contains(&format!("shell:{shell}"))
}

#[test]
fn a_posix_cli_passes_in_every_shell_on_this_mac_and_the_rest_is_not_run() {
    let dir = tempfile::tempdir().unwrap();
    let rows = run_all(dir.path(), GOOD);
    for shell in ["bash", "zsh", "sh", "dash"] {
        let (_, placement, outcome) = find(&rows, &format!("cli:{shell}:macos"));
        if on_this_mac(shell) {
            assert_eq!(placement, &Placement::On("this-mac".into()));
            let outcome = outcome.as_ref().unwrap();
            assert!(
                outcome.ran && outcome.passed,
                "{shell}: {}",
                outcome.evidence
            );
        } else {
            println!("{shell} is not on this Mac: its cell is not run");
        }
    }
    if !on_this_mac("fish") {
        let (_, placement, outcome) = find(&rows, "cli:fish:macos");
        assert_eq!(
            placement,
            &Placement::NotRun("no pool has shell:fish".into())
        );
        assert!(outcome.is_none(), "a cell no pool can run is never run");
    }
    if !on_this_mac("pwsh") {
        let (_, placement, _) = find(&rows, "cli:pwsh:macos");
        assert_eq!(
            placement,
            &Placement::NotRun("no pool has shell:pwsh".into())
        );
    }
    // This Mac is not a Linux or Windows pool: those cells say so.
    let (_, placement, _) = find(&rows, "cli:bash:linux");
    assert_eq!(placement, &Placement::NotRun("no pool has os:linux".into()));
    let (_, placement, _) = find(&rows, "cli:sh:windows");
    assert_eq!(
        placement,
        &Placement::NotRun("no pool has os:windows".into())
    );
}

#[test]
fn a_bashism_fails_in_sh_and_dash_and_passes_in_bash() {
    let dir = tempfile::tempdir().unwrap();
    let rows = run_all(dir.path(), BASHISM);
    if on_this_mac("bash") {
        let (_, _, bash) = find(&rows, "cli:bash:macos");
        let bash = bash.as_ref().unwrap();
        assert!(bash.passed, "bash runs [[: {}", bash.evidence);
    }
    for shell in ["sh", "dash"] {
        if !on_this_mac(shell) {
            continue;
        }
        let (_, _, outcome) = find(&rows, &format!("cli:{shell}:macos"));
        let outcome = outcome.as_ref().unwrap();
        assert!(
            outcome.ran && !outcome.passed,
            "{shell} must refuse [[: {}",
            outcome.evidence
        );
        assert!(
            outcome.evidence.contains("expected"),
            "{}",
            outcome.evidence
        );
        assert!(
            outcome.evidence.to_lowercase().contains("bad substitution"),
            "{}",
            outcome.evidence
        );
    }
}

#[test]
fn a_missing_load_file_fails_the_cell_with_the_shells_error() {
    let dir = tempfile::tempdir().unwrap();
    let rows = run_all(dir.path(), GOOD);
    std::fs::remove_file(dir.path().join("tool.sh")).unwrap();
    let (cell, _, _) = find(&rows, "cli:bash:macos");
    let outcome = compat::run_shell(cell, dir.path());
    assert!(outcome.ran && !outcome.passed, "{}", outcome.evidence);
}

#[test]
fn the_library_check_is_not_run_here_without_cargo_semver_checks() {
    let cells = compat::expand(&compat::parse("[library]\n").unwrap());
    let pools = [compat::local_pool("this-mac")];
    let check = &cells[0];
    if compat::local_pool("probe")
        .capabilities
        .contains("tool:cargo-semver-checks")
    {
        println!("cargo-semver-checks is installed: the seeded break test below runs");
        return;
    }
    assert_eq!(
        compat::place(check, &pools),
        Placement::NotRun("no pool has tool:cargo-semver-checks".into())
    );
}

/// A library at version one with a public method, and at version two without it: a breaking change. Runs only when
/// cargo-semver-checks is installed (it is not on this Mac): otherwise the test prints why it did not run.
#[test]
fn a_seeded_breaking_change_is_caught_when_the_api_check_runs() {
    if !compat::local_pool("probe")
        .capabilities
        .contains("tool:cargo-semver-checks")
    {
        println!("not run: cargo-semver-checks is not installed on this Mac");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"seeded\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "pub fn greet() -> &'static str { \"hi\" }\npub fn bye() {}\n",
    )
    .unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .output()
            .unwrap()
    };
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "one"]);
    git(&["tag", "v1.0.0"]);
    std::fs::write(
        root.join("src/lib.rs"),
        "pub fn greet() -> &'static str { \"hi\" }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"seeded\"\nversion = \"2.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let cells = compat::expand(&compat::parse("[library]\n").unwrap());
    let outcome = compat::run_api(&cells[0], root, "v1.0.0");
    assert!(outcome.ran, "{}", outcome.reason);
    assert!(
        !outcome.passed,
        "a removed public fn is a breaking change: {}",
        outcome.evidence
    );
    assert!(matches!(cells[0].kind, Kind::Api { .. }));
}
