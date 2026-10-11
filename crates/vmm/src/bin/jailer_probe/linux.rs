// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// The jailer's probe body (Linux). Its dispatcher — and the reason the probe
// is a binary at all — is `../jailer_probe.rs`; read that first.
//
// This is the probe `tests/jailer.rs` drives. It exits 0 only when the jailer
// did exactly what it claims; every other outcome is a distinct exit code and a
// one-line verdict, so the test asserts the *behaviour* and not this file's
// text.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use vmm::jail::jailer::{Access, Entered, Jailer};

const USAGE: &str = "\
usage: jailer_probe <inside|inside-threaded|inside-off> <root> <bind-file> <bind-dir>

  inside           enter the jail, then prove the outside is unreachable
  inside-threaded  spawn a thread first: the jailer must report Unsupported
  inside-off       enter with APZ_VMM_JAILER=off (the caller sets it): the
                   process must be left exactly where it was

<root>       the jail root to build (removed and recreated by the probe)
<bind-file>  a file to bind read-only (the probe asserts it is reachable
             inside, at the same path)
<bind-dir>   a directory to bind read-write (the probe writes a file through
             the bind and reads it back)";

/// The verdict prefix, and the only thing `tests/jailer.rs` greps for.
const VERDICT: &str = "jailer-probe:";

pub(crate) fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => ExitCode::from(code),
    }
}

fn run(args: &[String]) -> Result<(), u8> {
    if args.len() != 4 {
        eprintln!("{USAGE}");
        return Err(2);
    }
    let what = args[0].as_str();
    let root = PathBuf::from(&args[1]);
    let bind_file = PathBuf::from(&args[2]);
    let bind_dir = PathBuf::from(&args[3]);

    // A stale root from an interrupted run would make `pivot_root` fail on a
    // path that is already a mount point; the probe owns this path.
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&bind_dir).map_err(|err| fail("cannot create the bind dir", err))?;

    // The read-only source's identity, taken *before* the jail: the probe
    // compares it against what it reads inside.
    let before = fs::metadata(&bind_file).map_err(|err| fail("cannot stat the bind file", err))?;
    let outside_hostname = fs::read("/etc/hostname").is_ok();

    if what == "inside-threaded" {
        // A live sibling thread is exactly what a user namespace refuses.
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(60));
        });
    }

    let mut plan = Jailer::new(&root);
    plan.bind(&bind_file, Access::ReadOnly);
    plan.bind_dir(&bind_dir, Access::ReadWrite);
    plan.bind("/dev/null", Access::ReadOnly);
    plan.tmpfs("/run");

    let entered = match plan.enter() {
        Ok(entered) => entered,
        Err(err) => {
            println!("{VERDICT} error {err}");
            return Err(4);
        }
    };

    match entered {
        Entered::Off => {
            // `off` must mean *nothing happened*: the outside is still there.
            if !outside_hostname || fs::read("/etc/hostname").is_err() {
                println!("{VERDICT} off-but-root-moved");
                return Err(4);
            }
            println!("{VERDICT} off");
            Ok(())
        }
        Entered::Unsupported(reason) => {
            println!("{VERDICT} unsupported {reason}");
            Err(3)
        }
        Entered::Inside { root, uid_map, .. } => {
            prove_inside(&root, &bind_file, &before, &bind_dir, uid_map.as_deref())
        }
    }
}

/// Everything the jail must be true of, asserted from inside it. Any failure
/// prints the verdict and exits non-zero; the test reads only the exit code and
/// the verdict line.
fn prove_inside(
    root: &Path,
    bind_file: &Path,
    before: &fs::Metadata,
    bind_dir: &Path,
    uid_map: Option<&str>,
) -> Result<(), u8> {
    // 1. A path outside the root is unreachable. `/etc/hostname` exists on this
    //    host and was readable a moment ago.
    match fs::read("/etc/hostname") {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        other => {
            println!("{VERDICT} outside-reachable {other:?}");
            return Err(4);
        }
    }

    // 2. `/` lists only what the jail put there — no `etc`, `usr`, `home`,
    //    `proc`, `var`, `bin`, `lib`.
    let entries: Vec<String> = fs::read_dir("/")
        .map_err(|err| fail("cannot list the root", err))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    for forbidden in ["etc", "usr", "home", "proc", "var", "bin", "sbin", "lib"] {
        if entries.iter().any(|entry| entry == forbidden) {
            println!("{VERDICT} root-has-{forbidden} {entries:?}");
            return Err(4);
        }
    }
    // The tmpfs, and the mirror of the read-only source, are there.
    if !entries.iter().any(|entry| entry == "run") {
        println!("{VERDICT} root-lacks-run {entries:?}");
        return Err(4);
    }

    // 3. The bind is the same inode: the file is readable inside, at the same
    //    path, with the same length.
    let inside = fs::metadata(bind_file).map_err(|err| fail("cannot stat the bind inside", err))?;
    if inside.len() != before.len() {
        println!(
            "{VERDICT} bind-differs {} != {}",
            inside.len(),
            before.len()
        );
        return Err(4);
    }

    // 4. A read-write bind really is writable, through the same path.
    let wrote = bind_dir.join("probe-wrote");
    fs::write(&wrote, b"jailer probe\n")
        .map_err(|err| fail("cannot write through the bind", err))?;
    if fs::read(&wrote).map_err(|err| fail("cannot read back the bind", err))? != b"jailer probe\n"
    {
        println!("{VERDICT} bind-readback-differs");
        return Err(4);
    }

    // 5. The uid map, when the jail made one, is the caller's own id at the
    //    namespace's 0.
    let mapping = match uid_map {
        Some(map) => {
            let fields: Vec<&str> = map.split_whitespace().collect();
            if fields.len() < 3 || fields[0] != "0" || fields[2] != "1" {
                println!("{VERDICT} uid-map-unexpected {map:?}");
                return Err(4);
            }
            format!("uid_map={}", fields.join(","))
        }
        None => "uid_map=none".to_string(),
    };
    println!(
        "{VERDICT} inside root={} {mapping} entries={entries:?}",
        root.display()
    );
    Ok(())
}

/// A probe failure that is the probe's own doing, reported the same way.
fn fail(what: &str, err: std::io::Error) -> u8 {
    println!("{VERDICT} {what}: {err}");
    4
}
