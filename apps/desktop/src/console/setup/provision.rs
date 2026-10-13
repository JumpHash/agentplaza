//! The plaza's containers, Postgres and Forgejo, and their settings in apid's keys.env (see
//! `super`).
//!
//! Secrets live only in keys.env (`<home>/.config/agentplaza/keys.env`, mode 0600 on Unix) and in
//! the containers' own settings. They never reach the log: every command line shown to the user
//! has its secrets replaced by `***` ([`redact`]), and failures are reported redacted too.

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const POSTGRES: &str = "agentplaza-postgres";
const FORGEJO: &str = "agentplaza-forgejo";

/// Creates (or starts) `agentplaza-postgres` and records `APZ_DATABASE_URL` in keys.env.
///
/// An existing container is started if it is stopped; its password must already be in keys.env
/// (it is only written when the container is created). A new container takes the password from
/// keys.env if there is one, else a fresh random one.
pub fn postgres(log: &mut dyn FnMut(String)) -> Result<(), String> {
    match running_state(log, POSTGRES) {
        Some(running) => {
            if get_key("APZ_DATABASE_URL").is_none() {
                return Err(format!(
                    "the container exists but its password is not in keys.env: remove the container \
                     (docker rm -f {POSTGRES}) and set it up again"
                ));
            }
            if !running {
                run_docker(log, &["start", POSTGRES], &[])?;
            }
        }
        None => {
            let pw = match get_key("APZ_POSTGRES_PASSWORD") { Some(pw) => pw, None => secret()? };
            run_docker(log, &["volume", "create", "agentplaza-postgres"], &[])?;
            let password = format!("POSTGRES_PASSWORD={pw}");
            run_docker(
                log,
                &[
                    "run",
                    "-d",
                    "--name",
                    POSTGRES,
                    "--restart",
                    "unless-stopped",
                    "-p",
                    "127.0.0.1:5434:5432",
                    "-e",
                    "POSTGRES_USER=apz",
                    "-e",
                    password.as_str(),
                    "-e",
                    "POSTGRES_DB=apz",
                    "-v",
                    "agentplaza-postgres:/var/lib/postgresql/data",
                    "postgres:17",
                ],
                &[pw.as_str()],
            )?;
            let url = format!("postgres://apz:{pw}@127.0.0.1:5434/apz");
            set_keys(&[("APZ_POSTGRES_PASSWORD", pw.as_str()), ("APZ_DATABASE_URL", url.as_str())])?;
        }
    }
    wait_until(
        log,
        &["exec", POSTGRES, "pg_isready", "-U", "apz", "-q"],
        Duration::from_secs(60),
        Duration::from_secs(1),
    )?;
    log("Postgres is ready on 127.0.0.1:5434".to_string());
    Ok(())
}

/// Creates (or starts) `agentplaza-forgejo`, its admin and token, and records them in keys.env.
///
/// An existing container is started if it is stopped, and its token must already be in keys.env.
/// A new container gets an admin user `plaza` with a fresh password, and an access token; both
/// go to keys.env.
pub fn forgejo(log: &mut dyn FnMut(String)) -> Result<(), String> {
    let existed = running_state(log, FORGEJO);
    match existed {
        Some(running) => {
            if get_key("FORGEJO_TOKEN").is_none() {
                return Err(format!(
                    "the container exists but its token is not in keys.env: remove the container \
                     (docker rm -f {FORGEJO}) and set it up again"
                ));
            }
            if !running {
                run_docker(log, &["start", FORGEJO], &[])?;
            }
        }
        None => {
            run_docker(log, &["volume", "create", "agentplaza-forgejo"], &[])?;
            run_docker(
                log,
                &[
                    "run",
                    "-d",
                    "--name",
                    FORGEJO,
                    "--restart",
                    "unless-stopped",
                    "-p",
                    "127.0.0.1:3300:3000",
                    "-v",
                    "agentplaza-forgejo:/data",
                    "-e",
                    "FORGEJO__security__INSTALL_LOCK=true",
                    "-e",
                    "FORGEJO__server__ROOT_URL=http://127.0.0.1:3300/",
                    "-e",
                    "FORGEJO__server__HTTP_PORT=3000",
                    "-e",
                    "FORGEJO__service__DISABLE_REGISTRATION=true",
                    "codeberg.org/forgejo/forgejo:11",
                ],
                &[],
            )?;
        }
    }
    wait_until(
        log,
        &["exec", FORGEJO, "wget", "-q", "-O-", "http://127.0.0.1:3000/api/v1/version"],
        Duration::from_secs(120),
        Duration::from_secs(2),
    )?;
    if existed.is_none() {
        let pw = secret()?;
        let created = run_docker(
            log,
            &[
                "exec",
                "-u",
                "git",
                FORGEJO,
                "forgejo",
                "admin",
                "user",
                "create",
                "--admin",
                "--username",
                "plaza",
                "--password",
                pw.as_str(),
                "--email",
                "plaza@agentplaza.local",
                "--must-change-password=false",
            ],
            &[pw.as_str()],
        );
        // The admin's password is recorded only when this run created the admin: an existing one
        // keeps its own (the token below is what apid uses).
        let created = match created {
            Ok(_) => true,
            Err(e) if e.to_lowercase().contains("already exists") => {
                log("the admin user plaza already exists: continuing".to_string());
                false
            }
            Err(e) => return Err(e),
        };
        let token_name = format!("apid-{}", &secret()?[..8]);
        let token = run_docker(
            log,
            &[
                "exec",
                "-u",
                "git",
                FORGEJO,
                "forgejo",
                "admin",
                "user",
                "generate-access-token",
                "--username",
                "plaza",
                "--token-name",
                token_name.as_str(),
                "--scopes",
                "all",
                "--raw",
            ],
            &[],
        )?;
        if token.is_empty() {
            return Err("Forgejo did not return an access token".to_string());
        }
        let mut keys = vec![("FORGEJO_URL", "http://127.0.0.1:3300"), ("FORGEJO_TOKEN", token.as_str())];
        if created {
            keys.push(("FORGEJO_ADMIN_PASSWORD", pw.as_str()));
        }
        set_keys(&keys)?;
    }
    log("Forgejo is ready on http://127.0.0.1:3300 (admin: plaza)".to_string());
    Ok(())
}

/// Whether the container `name` exists and is running: `None` when it does not exist.
fn running_state(log: &mut dyn FnMut(String), name: &str) -> Option<bool> {
    run_docker(log, &["inspect", "-f", "{{.State.Running}}", name], &[])
        .ok()
        .map(|out| out == "true")
}

/// Runs `docker` with `args`, logging the command line with each of `secrets` shown as `***`.
/// Returns stdout, trimmed; a failure is an Err with the command's last stderr line.
fn run_docker(log: &mut dyn FnMut(String), args: &[&str], secrets: &[&str]) -> Result<String, String> {
    log(format!("$ docker {}", redact(&args.join(" "), secrets)));
    let out = Command::new("docker")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run docker: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(failure(&out, secrets))
    }
}

/// Runs `docker args` every `every` until it succeeds or `timeout` passes. The command is logged
/// once, not on every try.
fn wait_until(
    log: &mut dyn FnMut(String),
    args: &[&str],
    timeout: Duration,
    every: Duration,
) -> Result<(), String> {
    log(format!("$ docker {} (retried until it succeeds)", args.join(" ")));
    let started = Instant::now();
    loop {
        let last = match Command::new("docker").args(args).stdin(Stdio::null()).output() {
            Ok(out) if out.status.success() => return Ok(()),
            Ok(out) => failure(&out, &[]),
            Err(e) => format!("cannot run docker: {e}"),
        };
        if started.elapsed() >= timeout {
            return Err(format!("not ready after {} s: {last}", timeout.as_secs()));
        }
        thread::sleep(every);
    }
}

/// The last non-empty stderr line of a failed command (else of its stdout), redacted.
fn failure(out: &Output, secrets: &[&str]) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stderr
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .or_else(|| stdout.lines().rev().find(|l| !l.trim().is_empty()))
        .map(|l| l.trim().to_string())
        .unwrap_or_else(|| format!("exit status {}", out.status));
    redact(&line, secrets)
}

/// `text` with every occurrence of each secret replaced by `***`.
fn redact(text: &str, secrets: &[&str]) -> String {
    secrets
        .iter()
        .filter(|s| !s.is_empty())
        .fold(text.to_string(), |acc, s| acc.replace(*s, "***"))
}

/// A random secret: 32 hex characters (16 bytes from the OS random source).
fn secret() -> Result<String, String> {
    // No fallback: a password or token from a guessable source is worse than none.
    apz_os::random::hex(16).map_err(|e| format!("the system's random source failed: {e}"))
}

/// The file apid reads its settings from: `<home>/.config/agentplaza/keys.env`.
fn keys_path() -> Result<PathBuf, String> {
    let home = apz_os::dirs::home().ok_or("the home directory is not set")?;
    Ok(home.join(".config").join("agentplaza").join("keys.env"))
}

/// The value of `name` in keys.env; `None` when the file or the key is missing, or the value empty.
fn get_key(name: &str) -> Option<String> {
    let text = fs::read_to_string(keys_path().ok()?).ok()?;
    lookup(&text, name)
}

/// Writes `values` into keys.env, keeping its other lines (the user's model keys) as they are.
fn set_keys(values: &[(&str, &str)]) -> Result<(), String> {
    let path = keys_path()?;
    let dir = path.parent().ok_or("keys.env has no directory")?;
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let existing = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    write_private(&path, &merge(&existing, values))
}

/// Writes `text` to `path` through a temp file in the same directory, then renames it over `path`.
/// On Unix the file is 0600 from its creation.
fn write_private(path: &Path, text: &str) -> Result<(), String> {
    let tmp = path.with_file_name(format!("keys.env.{}.tmp", std::process::id()));
    let _ = fs::remove_file(&tmp);
    let written = (|| -> std::io::Result<()> {
        let mut file = create_private(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    written.map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("cannot write {}: {e}", path.display())
    })
}

/// Creates `path` for writing, failing if it exists; private (0600) on Unix.
fn create_private(path: &Path) -> std::io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// `existing` with `values` merged in: a line for a key in `values` is replaced (the first one
/// kept, later duplicates dropped), every other line is kept as it is, and keys not present are
/// appended.
fn merge(existing: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::new();
    let mut written = vec![false; values.len()];
    for line in existing.lines() {
        let slot = line
            .split_once('=')
            .and_then(|(key, _)| values.iter().position(|(name, _)| *name == key.trim()));
        match slot {
            Some(i) if written[i] => {}
            Some(i) => {
                written[i] = true;
                push_line(&mut out, values[i]);
            }
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    for (i, value) in values.iter().enumerate() {
        if !written[i] {
            push_line(&mut out, *value);
        }
    }
    out
}

fn push_line(out: &mut String, (name, value): (&str, &str)) {
    out.push_str(name);
    out.push('=');
    out.push_str(value);
    out.push('\n');
}

/// The value of `name` in the KEY=VALUE lines of `text`; `None` when missing or empty.
fn lookup(text: &str, name: &str) -> Option<String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .find(|(key, _)| key.trim() == name)
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_replaces_appends_and_keeps_other_lines() {
        let existing = "# keys\nANTHROPIC_API_KEY=sk-1\nAPZ_DATABASE_URL=old\n\nOTHER=x\n";
        let merged = merge(existing, &[("APZ_DATABASE_URL", "new"), ("FORGEJO_TOKEN", "t")]);
        assert_eq!(
            merged,
            "# keys\nANTHROPIC_API_KEY=sk-1\nAPZ_DATABASE_URL=new\n\nOTHER=x\nFORGEJO_TOKEN=t\n"
        );
    }

    #[test]
    fn merge_into_an_empty_file() {
        assert_eq!(merge("", &[("A", "1"), ("B", "2")]), "A=1\nB=2\n");
    }

    #[test]
    fn merge_keeps_one_line_per_key() {
        assert_eq!(merge("A=1\nA=2\n", &[("A", "3")]), "A=3\n");
    }

    #[test]
    fn lookup_reads_values_and_skips_empty_and_commented_ones() {
        let text = "# A=0\nFORGEJO_TOKEN= \nA=1\n";
        assert_eq!(lookup(text, "A"), Some("1".to_string()));
        assert_eq!(lookup(text, "FORGEJO_TOKEN"), None);
        assert_eq!(lookup(text, "MISSING"), None);
    }

    #[test]
    fn redact_hides_each_secret() {
        assert_eq!(
            redact("$ docker run -e POSTGRES_PASSWORD=abc123 --password abc123", &["abc123"]),
            "$ docker run -e POSTGRES_PASSWORD=*** --password ***"
        );
        assert_eq!(redact("nothing here", &[""]), "nothing here");
    }

    #[test]
    fn secret_is_32_hex_characters() {
        let s = secret().unwrap();
        assert_eq!(s.len(), 32);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
