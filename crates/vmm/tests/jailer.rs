// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// The jailer's probes. The work itself happens in the `jailer_probe` binary
// (see the comment at the top of `src/bin/jailer_probe.rs`): `unshare(CLONE_NEWUSER)`
// is refused by a multi-threaded process, and `libtest` runs every test on a
// spawned thread, so a probe of the *inside* of the jail cannot be a `#[test]`
// in this binary. What is asserted here is the probe's behaviour end to end —
// the exit code and the one-line verdict — plus the two paths that do not need
// a single-threaded process: the `off` switch and the multi-threaded refusal.
#![cfg(target_os = "linux")]

mod common;

use std::path::{Path, PathBuf};
use std::process::Output;

/// The probe binary, built from this package (`src/bin/jailer_probe.rs`).
const PROBE: &str = env!("CARGO_BIN_EXE_jailer_probe");

/// A fresh working directory for one probe. The probe removes and recreates its
/// own jail root under this, so it must be private to the probe.
fn probe_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("apz-jailer-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a probe work directory");
    dir
}

/// Runs the probe binary and returns its output.
///
/// `env` is the caller's environment additions — which is how the switch is
/// probed, so what is tested is the wiring, not the parser (the crate's unit
/// tests cover the parsing).
fn probe(what: &str, root: &Path, file: &Path, dir: &Path, env: &[(&str, &str)]) -> Output {
    let mut command = common::command(Path::new(PROBE));
    command
        .arg(what)
        .arg(root)
        .arg(file)
        .arg(dir)
        .env_remove("APZ_VMM_JAILER")
        .env_remove("APZ_VMM_JAIL_ROOT")
        .envs(env.iter().copied());
    command.output().expect("the probe binary runs")
}

/// The probe's stdout, as text.
fn verdict(output: &Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

/// The one probe that needs a single-threaded process: after the jailer, the
/// process cannot name anything outside the per-VM root, the sources it was
/// given are reachable *at their own paths*, and (when the jail made a user
/// namespace) the `uid_map` is the caller's id at the namespace's 0.
///
/// A platform where the kernel refuses a user namespace reports `unsupported`,
/// which is the same honest "this machine cannot" the crate's other capability
/// probes report: nothing is asserted that the platform cannot do.
#[test]
fn the_jailer_leaves_the_outside_unreachable() {
    let base = probe_dir("inside");
    let file = std::env::current_exe().expect("the test binary's path");
    let dir = base.join("work");
    let root = base.join("root");

    let output = probe("inside", &root, &file, &dir, &[]);
    let text = verdict(&output);
    if text.contains("unsupported") {
        // The probe's own verdict: this host cannot build the jail (a user
        // namespace is refused). Written where a passing run shows it, so a
        // reader can tell a skip from a pass.
        common::note(format!(
            "soft-skip: the jailer probe cannot build its jail here: {text}"
        ));
        return;
    }
    assert!(output.status.success(), "the probe failed: {text}");
    assert!(text.contains("jailer-probe: inside"), "{text}");
    // The read-only bind of this test binary is the same inode inside, and the
    // writable bind round-trips — both asserted by the probe, whose verdict
    // names the root it built.
    assert!(text.contains(&root.display().to_string()), "{text}");
    let _ = std::fs::remove_dir_all(&base);
}

/// `APZ_VMM_JAILER=off` must mean *nothing happened*: the probe checks that the
/// host root is still the process's root (a path outside is still readable),
/// and reports `off`.
#[test]
fn the_off_switch_leaves_the_process_where_it_was() {
    let base = probe_dir("off");
    let file = std::env::current_exe().expect("the test binary's path");
    let dir = base.join("work");
    let root = base.join("root");

    let output = probe(
        "inside-off",
        &root,
        &file,
        &dir,
        &[("APZ_VMM_JAILER", "off")],
    );
    let text = verdict(&output);
    assert!(output.status.success(), "the probe failed: {text}");
    assert!(text.contains("jailer-probe: off"), "{text}");
    // The jail root was never built: `off` short-circuits before any mkdir.
    assert!(
        !root.exists(),
        "the jail root was built with the switch off"
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// A switch that names no mode is refused — a typo must stop the VM, not
/// silently pick one.
#[test]
fn a_mistyped_switch_is_refused() {
    let base = probe_dir("typo");
    let file = std::env::current_exe().expect("the test binary's path");
    let dir = base.join("work");
    let root = base.join("root");

    let output = probe("inside", &root, &file, &dir, &[("APZ_VMM_JAILER", "loud")]);
    let text = verdict(&output);
    assert!(!output.status.success(), "a typo must not succeed: {text}");
    assert!(text.contains("is not a mode"), "{text}");
    assert!(!text.contains("jailer-probe: inside"), "{text}");
    let _ = std::fs::remove_dir_all(&base);
}

/// A process with a sibling thread cannot be jailed by construction, and that
/// must be *reported* rather than silently downgraded to an unjailed VM — and
/// nothing may be moved.
#[test]
fn a_multi_threaded_process_is_reported_rather_than_jailed() {
    let base = probe_dir("threaded");
    let file = std::env::current_exe().expect("the test binary's path");
    let dir = base.join("work");
    let root = base.join("root");

    let output = probe("inside-threaded", &root, &file, &dir, &[]);
    let text = verdict(&output);
    assert!(!text.contains("jailer-probe: inside"), "{text}");
    assert!(text.contains("unsupported"), "{text}");
    assert!(text.contains("threads"), "{text}");
    let _ = std::fs::remove_dir_all(&base);
}
