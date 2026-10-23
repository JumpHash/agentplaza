//! The base's warm language servers (PLAN 3.8), as the base recipe runs them in the VM. Two commands, neither the vsock
//! session:
//!
//! * `lsp-serve --id <server> --socket <path> --root <dir>` starts the server of the table with that id in `dir` and
//!   serves it on the socket ([`apz_lsp::bridge`]). The base runs it in the background, so it runs through the snapshot,
//!   and a fork's tools reach it on the same socket.
//! * `lsp-warm --id <server> --socket <path> --root <dir> --wait <seconds>` opens a file of the server's language and
//!   waits until the server has indexed the workspace, so the snapshot holds a server that answers without indexing.
//!
//! `lsp-serve` fails when the server ends; `lsp-warm` fails only when no server listens on the socket. A server that has
//! not indexed by its deadline is still warm: it is up, and the first query waits for it as on any cold server.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use apz_lsp::{Language, ServerSpec, Timeouts, bridge, servers};

/// Directories the warm-up does not look in for a file to open.
const SKIPPED_DIRS: [&str; 3] = ["target", "node_modules", "vendor"];
/// How deep under the root the warm-up looks for a file of the server's language.
const MAX_DEPTH: usize = 4;

/// `lsp-serve`: serves the server on its socket until it ends.
pub fn serve(args: &[String]) -> ExitCode {
    let outcome = flags(args).and_then(|flags| {
        let (spec, socket, root) = target(&flags)?;
        println!(
            "agent-harness: lsp-serve {}: serving on {}",
            spec.id,
            socket.display()
        );
        bridge::serve_installed(&socket, spec, &root, Timeouts::default().start)
            .map_err(|e| e.to_string())
    });
    report("lsp-serve", outcome.err().unwrap_or_default())
}

/// `lsp-warm`: waits until the server has indexed the workspace, or its `--wait` has passed.
pub fn warm(args: &[String]) -> ExitCode {
    let outcome = flags(args).and_then(|flags| {
        let (spec, socket, root) = target(&flags)?;
        let wait = flag(&flags, "--wait")?
            .parse::<u64>()
            .map_err(|_| "--wait is a number of seconds".to_string())?;
        let file = representative_file(&root, spec.id);
        let indexed = bridge::warm(
            &socket,
            file.as_deref(),
            Duration::from_secs(wait),
            Timeouts::default(),
        )
        .map_err(|e| e.to_string())?;
        Ok(if indexed {
            format!("lsp-warm {}: indexed", spec.id)
        } else {
            format!(
                "lsp-warm {}: not indexed within {wait} s; it stays up",
                spec.id
            )
        })
    });
    match outcome {
        Ok(line) => {
            println!("agent-harness: {line}");
            ExitCode::SUCCESS
        }
        Err(why) => report("lsp-warm", why),
    }
}

/// Prints a command's failure (an empty one is a clean end) and gives the exit status.
fn report(verb: &str, why: String) -> ExitCode {
    if why.is_empty() {
        return ExitCode::SUCCESS;
    }
    println!("agent-harness: {verb}: {why}");
    ExitCode::FAILURE
}

/// The `--name value` pairs of a command.
fn flags(args: &[String]) -> Result<BTreeMap<&str, &str>, String> {
    let mut map = BTreeMap::new();
    let mut parts = args.iter();
    while let Some(name) = parts.next() {
        let value = parts.next().ok_or_else(|| format!("{name} has no value"))?;
        map.insert(name.as_str(), value.as_str());
    }
    Ok(map)
}

fn flag<'a>(flags: &BTreeMap<&str, &'a str>, name: &str) -> Result<&'a str, String> {
    flags
        .get(name)
        .copied()
        .ok_or_else(|| format!("{name} is needed"))
}

/// The server a command names, its socket and the workspace it serves.
fn target(flags: &BTreeMap<&str, &str>) -> Result<(&'static ServerSpec, PathBuf, PathBuf), String> {
    let id = flag(flags, "--id")?;
    let spec =
        servers::by_id(id).ok_or_else(|| format!("no language server {id:?} in the table"))?;
    let socket = PathBuf::from(flag(flags, "--socket")?);
    let root = PathBuf::from(flag(flags, "--root")?);
    Ok((spec, socket, root))
}

/// The first file under `root` (sorted, a directory's files before its subdirectories) that the server of `id` serves.
fn representative_file(root: &Path, id: &str) -> Option<PathBuf> {
    find(root, id, 0)
}

fn find(dir: &Path, id: &str, depth: usize) -> Option<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    if let Some(file) = entries
        .iter()
        .find(|path| path.is_file() && serves(path, id))
    {
        return Some(file.clone());
    }
    if depth >= MAX_DEPTH {
        return None;
    }
    entries
        .iter()
        .filter(|path| path.is_dir() && !skipped(path))
        .find_map(|sub| find(sub, id, depth + 1))
}

fn serves(path: &Path, id: &str) -> bool {
    Language::from_path(&path.to_string_lossy()).is_some_and(|language| language.server().id == id)
}

fn skipped(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.') || SKIPPED_DIRS.contains(&name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("apz-warm-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn the_warm_up_opens_a_file_of_the_servers_language_outside_the_skipped_directories() {
        let root = scratch("file");
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("target").join("build.rs"), "").unwrap();
        fs::create_dir_all(root.join(".plaza")).unwrap();
        fs::write(root.join(".plaza").join("state.rs"), "").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("README.md"), "").unwrap();
        fs::write(root.join("src").join("lib.rs"), "").unwrap();
        assert_eq!(
            representative_file(&root, "rust-analyzer"),
            Some(root.join("src").join("lib.rs"))
        );
        assert_eq!(representative_file(&root, "gopls"), None);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_flags_are_pairs_and_each_command_names_its_server() {
        let args: Vec<String> = [
            "--id",
            "rust-analyzer",
            "--socket",
            "/s.sock",
            "--root",
            "/w",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let parsed = flags(&args).unwrap();
        let (spec, socket, root) = target(&parsed).unwrap();
        assert_eq!(spec.id, "rust-analyzer");
        assert_eq!(socket, PathBuf::from("/s.sock"));
        assert_eq!(root, PathBuf::from("/w"));
        assert!(flags(&["--id".to_string()]).is_err());
        let unknown_args: Vec<String> = vec!["--id".into(), "no-such-server".into()];
        let unknown = flags(&unknown_args).unwrap();
        assert!(target(&unknown).unwrap_err().contains("no language server"));
    }
}
