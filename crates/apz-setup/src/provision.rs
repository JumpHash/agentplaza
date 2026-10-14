//! The plaza's containers, Postgres, Forgejo and Garage, and their settings in apid's keys.env (see
//! `lib.rs`).
//!
//! Secrets live only in keys.env (`<home>/.config/agentplaza/keys.env`, mode 0600 on Unix), in
//! Garage's config file beside it (also 0600), and in the containers' own settings. They never reach
//! the log: every command line shown to the user has its secrets replaced by `***` ([`redact`]),
//! and failures are reported redacted too.

use crate::runner::Output;
use crate::Env;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

const POSTGRES: &str = "agentplaza-postgres";
const FORGEJO: &str = "agentplaza-forgejo";
pub(crate) const GARAGE: &str = "agentplaza-garage";
/// The image of Garage, the S3 object store (its `garage` program runs inside the container).
const GARAGE_IMAGE: &str = "dxflrs/garage:v1.0.1";
/// The bucket apid keeps the library's files in, and the key that may read and write it.
const GARAGE_BUCKET: &str = "agentplaza";
const GARAGE_KEY_NAME: &str = "agentplaza-apid";
const GARAGE_ENDPOINT: &str = "http://127.0.0.1:3900";
const GARAGE_REGION: &str = "garage";
/// The settings apid reads Garage's address and keys from (its vault takes the two keys from here).
pub(crate) const GARAGE_KEYS: [&str; 5] = [
    "APZ_S3_ENDPOINT",
    "APZ_S3_BUCKET",
    "APZ_S3_REGION",
    "APZ_S3_ACCESS_KEY",
    "APZ_S3_SECRET_KEY",
];

/// Creates (or starts) `agentplaza-postgres` and records `APZ_DATABASE_URL` in keys.env.
///
/// An existing container is started if it is stopped; its password must already be in keys.env
/// (it is only written when the container is created). A new container takes the password from
/// keys.env if there is one, else a fresh random one.
pub fn postgres(env: &Env, log: &mut dyn FnMut(String)) -> Result<(), String> {
    // Without keys.env there is nowhere to record the password: say so before anything runs.
    keys_file(env)?;
    match running_state(env, log, POSTGRES) {
        Some(running) => {
            if get_key(env, "APZ_DATABASE_URL").is_none() {
                return Err(format!(
                    "the container exists but its password is not in keys.env: remove the container \
                     (docker rm -f {POSTGRES}) and set it up again"
                ));
            }
            if !running {
                run_docker(env, log, &["start", POSTGRES], &[])?;
            }
        }
        None => {
            let pw = match get_key(env, "APZ_POSTGRES_PASSWORD") {
                Some(pw) => pw,
                None => secret()?,
            };
            run_docker(env, log, &["volume", "create", "agentplaza-postgres"], &[])?;
            let password = format!("POSTGRES_PASSWORD={pw}");
            run_docker(
                env,
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
            set_keys(
                env,
                &[("APZ_POSTGRES_PASSWORD", pw.as_str()), ("APZ_DATABASE_URL", url.as_str())],
            )?;
        }
    }
    wait_until(
        env,
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
pub fn forgejo(env: &Env, log: &mut dyn FnMut(String)) -> Result<(), String> {
    keys_file(env)?;
    let existed = running_state(env, log, FORGEJO);
    match existed {
        Some(running) => {
            if get_key(env, "FORGEJO_TOKEN").is_none() {
                return Err(format!(
                    "the container exists but its token is not in keys.env: remove the container \
                     (docker rm -f {FORGEJO}) and set it up again"
                ));
            }
            if !running {
                run_docker(env, log, &["start", FORGEJO], &[])?;
            }
        }
        None => {
            run_docker(env, log, &["volume", "create", "agentplaza-forgejo"], &[])?;
            run_docker(
                env,
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
        env,
        log,
        &["exec", FORGEJO, "wget", "-q", "-O-", "http://127.0.0.1:3000/api/v1/version"],
        Duration::from_secs(120),
        Duration::from_secs(2),
    )?;
    if existed.is_none() {
        let pw = secret()?;
        let created = run_docker(
            env,
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
            env,
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
        set_keys(env, &keys)?;
    }
    log("Forgejo is ready on http://127.0.0.1:3300 (admin: plaza)".to_string());
    Ok(())
}

/// Creates (or starts) `agentplaza-garage`, the S3 object store apid keeps the library's files in.
///
/// Garage reads its settings from a config file that holds a fresh random RPC secret: the file is
/// written beside keys.env (0600) and mounted read-only at `/etc/garage.toml`. A new container is a
/// single-node cluster with one layout, the bucket `agentplaza`, and a key that may read and write
/// it. The endpoint, the bucket and the key go to keys.env under the names apid reads.
///
/// An existing container is started if it is stopped, and its keys must already be in keys.env.
pub fn garage(env: &Env, log: &mut dyn FnMut(String)) -> Result<(), String> {
    keys_file(env)?;
    let existed = running_state(env, log, GARAGE);
    match existed {
        Some(running) => {
            if !keys_set(env, &GARAGE_KEYS) {
                return Err(format!(
                    "the container exists but its keys are not in keys.env: remove the container \
                     (docker rm -f {GARAGE}) and set it up again"
                ));
            }
            if !running {
                run_docker(env, log, &["start", GARAGE], &[])?;
            }
        }
        None => {
            let config = garage_config(env)?;
            let mount = format!("{}:/etc/garage.toml:ro", config.display());
            run_docker(env, log, &["volume", "create", "agentplaza-garage-meta"], &[])?;
            run_docker(env, log, &["volume", "create", "agentplaza-garage-data"], &[])?;
            run_docker(
                env,
                log,
                &[
                    "run",
                    "-d",
                    "--name",
                    GARAGE,
                    "--restart",
                    "unless-stopped",
                    "-p",
                    "127.0.0.1:3900:3900",
                    "-v",
                    "agentplaza-garage-meta:/var/lib/garage/meta",
                    "-v",
                    "agentplaza-garage-data:/var/lib/garage/data",
                    "-v",
                    mount.as_str(),
                    GARAGE_IMAGE,
                ],
                &[],
            )?;
        }
    }
    wait_until(
        env,
        log,
        &["exec", GARAGE, "/garage", "status"],
        Duration::from_secs(120),
        Duration::from_secs(2),
    )?;
    if existed.is_none() {
        let node = garage_cli(env, log, &["node", "id", "-q"])?;
        let node = node
            .lines()
            .find(|line| !line.trim().is_empty())
            .and_then(|line| line.split('@').next())
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or("Garage did not return its node id")?
            .to_string();
        garage_cli(env, log, &["layout", "assign", "-z", "plaza", "-c", "1G", node.as_str()])?;
        garage_cli(env, log, &["layout", "apply", "--version", "1"])?;
        garage_cli(env, log, &["bucket", "create", GARAGE_BUCKET])?;
        let created = garage_cli(env, log, &["key", "create", GARAGE_KEY_NAME])?;
        let (id, key) = garage_key(&created).ok_or("Garage did not return the key")?;
        garage_cli(
            env,
            log,
            &["bucket", "allow", "--read", "--write", "--owner", GARAGE_BUCKET, "--key", GARAGE_KEY_NAME],
        )?;
        set_keys(
            env,
            &[
                ("APZ_S3_ENDPOINT", GARAGE_ENDPOINT),
                ("APZ_S3_BUCKET", GARAGE_BUCKET),
                ("APZ_S3_REGION", GARAGE_REGION),
                ("APZ_S3_ACCESS_KEY", id.as_str()),
                ("APZ_S3_SECRET_KEY", key.as_str()),
            ],
        )?;
    }
    log(format!("Garage is ready on {GARAGE_ENDPOINT} (bucket {GARAGE_BUCKET})"));
    Ok(())
}

/// Runs the `garage` program inside the container with `args`; its stdout, trimmed.
fn garage_cli(env: &Env, log: &mut dyn FnMut(String), args: &[&str]) -> Result<String, String> {
    let mut full = vec!["exec", GARAGE, "/garage"];
    full.extend_from_slice(args);
    run_docker(env, log, &full, &[])
}

/// The access key ID and the secret key of `garage key create`'s answer (`Key ID: …`,
/// `Secret key: …`); `None` when either is missing.
fn garage_key(text: &str) -> Option<(String, String)> {
    let field = |label: &str| {
        text.lines()
            .find_map(|line| line.trim().strip_prefix(label))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    Some((field("Key ID:")?, field("Secret key:")?))
}

/// Writes Garage's config file beside keys.env, with a fresh random RPC secret (64 hex characters),
/// and returns its path. One node, no admin API: apid and the container's own `garage` program talk
/// to it through the S3 port and the RPC port only.
fn garage_config(env: &Env) -> Result<PathBuf, String> {
    let path = keys_file(env)?.with_file_name("garage.toml");
    let dir = path.parent().ok_or("garage.toml has no directory")?;
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let rpc_secret = format!("{}{}", secret()?, secret()?);
    let text = format!(
        "metadata_dir = \"/var/lib/garage/meta\"\n\
         data_dir = \"/var/lib/garage/data\"\n\
         db_engine = \"lmdb\"\n\
         replication_factor = 1\n\
         rpc_bind_addr = \"0.0.0.0:3901\"\n\
         rpc_public_addr = \"127.0.0.1:3901\"\n\
         rpc_secret = \"{rpc_secret}\"\n\
         \n\
         [s3_api]\n\
         s3_region = \"{GARAGE_REGION}\"\n\
         api_bind_addr = \"0.0.0.0:3900\"\n"
    );
    write_private(&path, &text)?;
    Ok(path)
}

/// Whether the container `name` exists and is running: `None` when it does not exist.
fn running_state(env: &Env, log: &mut dyn FnMut(String), name: &str) -> Option<bool> {
    run_docker(env, log, &["inspect", "-f", "{{.State.Running}}", name], &[])
        .ok()
        .map(|out| out == "true")
}

/// Runs `docker` with `args` through the runner, logging the command line with each of `secrets`
/// shown as `***`. Returns stdout, trimmed; a failure is an Err with the command's last stderr line.
fn run_docker(env: &Env, log: &mut dyn FnMut(String), args: &[&str], secrets: &[&str]) -> Result<String, String> {
    log(format!("$ docker {}", redact(&args.join(" "), secrets)));
    let out = env
        .runner
        .output("docker", args)
        .map_err(|e| format!("cannot run docker: {e}"))?;
    if out.success {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(failure(&out, secrets))
    }
}

/// Runs `docker args` every `every` until it succeeds or `timeout` passes. The command is logged
/// once, not on every try.
fn wait_until(
    env: &Env,
    log: &mut dyn FnMut(String),
    args: &[&str],
    timeout: Duration,
    every: Duration,
) -> Result<(), String> {
    log(format!("$ docker {} (retried until it succeeds)", args.join(" ")));
    let started = Instant::now();
    loop {
        let last = match env.runner.output("docker", args) {
            Ok(out) if out.success => return Ok(()),
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
        .unwrap_or_else(|| match out.code {
            Some(code) => format!("exit status {code}"),
            None => "exit status unknown".to_string(),
        });
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

/// The file apid reads its settings from, or an error when there is no home directory.
fn keys_file(env: &Env) -> Result<PathBuf, String> {
    env.keys.clone().ok_or_else(|| "the home directory is not set".to_string())
}

/// The value of `name` in keys.env; `None` when the file or the key is missing, or the value empty.
fn get_key(env: &Env, name: &str) -> Option<String> {
    let text = fs::read_to_string(env.keys.as_ref()?).ok()?;
    lookup(&text, name)
}

/// Whether keys.env holds a value for every name in `names`.
pub(crate) fn keys_set(env: &Env, names: &[&str]) -> bool {
    let Some(text) = env.keys.as_ref().and_then(|path| fs::read_to_string(path).ok()) else {
        return false;
    };
    names.iter().all(|name| lookup(&text, name).is_some())
}

/// Writes `values` into keys.env, keeping its other lines (the user's model keys) as they are.
fn set_keys(env: &Env, values: &[(&str, &str)]) -> Result<(), String> {
    let path = keys_file(env)?;
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
    let tmp = path.with_file_name(format!("{}.{}.tmp", file_name(path), std::process::id()));
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

fn file_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default()
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
    use crate::runner::mock::{scratch, Mock};

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

    #[test]
    fn garage_key_reads_the_id_and_the_secret_of_the_answer() {
        let answer = "Key name: agentplaza-apid\nKey ID: GK0123456789abcdef\nSecret key: 9f8e7d6c\nCan create buckets: false\n";
        assert_eq!(garage_key(answer), Some(("GK0123456789abcdef".to_string(), "9f8e7d6c".to_string())));
        assert_eq!(garage_key("Key ID: GK1\n"), None, "no secret, no key");
    }

    /// A docker host that answers each command from `answer` (the arguments after `docker`), and
    /// succeeds with no output when `answer` has nothing to say.
    fn docker_says(answer: impl Fn(&[&str]) -> Option<Output> + 'static) -> Mock {
        Mock::new(move |_program, args| answer(args).unwrap_or_else(|| Output::ok("")))
    }

    /// The first recorded command that contains `needle`.
    fn call_with(calls: &[String], needle: &str) -> String {
        calls.iter().find(|c| c.contains(needle)).cloned().unwrap_or_else(|| panic!("no call with {needle}"))
    }

    /// The keys.env in `dir`, as text (empty when there is none).
    fn keys_text(dir: &Path) -> String {
        fs::read_to_string(dir.join("keys.env")).unwrap_or_default()
    }

    fn inspect_absent(args: &[&str]) -> Option<Output> {
        (args.first() == Some(&"inspect")).then(|| Output::failed("Error: No such object"))
    }

    #[test]
    fn postgres_creates_the_container_and_records_its_url_redacted() {
        let dir = scratch("provision-postgres");
        let mock = docker_says(inspect_absent);
        let env = Env { runner: &mock, keys: Some(dir.join("keys.env")), user: None };
        let mut log = Vec::new();
        postgres(&env, &mut |l| log.push(l)).unwrap();

        let keys = keys_text(&dir);
        let password = lookup(&keys, "APZ_POSTGRES_PASSWORD").expect("the password is recorded");
        assert_eq!(
            lookup(&keys, "APZ_DATABASE_URL"),
            Some(format!("postgres://apz:{password}@127.0.0.1:5434/apz"))
        );
        let calls = mock.calls();
        assert_eq!(calls[0], "docker inspect -f {{.State.Running}} agentplaza-postgres");
        assert_eq!(calls[1], "docker volume create agentplaza-postgres");
        assert!(call_with(&calls, "docker run -d --name agentplaza-postgres").contains(&format!("POSTGRES_PASSWORD={password}")));
        assert_eq!(calls.last().unwrap(), "docker exec agentplaza-postgres pg_isready -U apz -q");
        // The password never reaches the log; the command is shown with it hidden.
        assert!(log.iter().all(|line| !line.contains(&password)), "{log:?}");
        assert!(log.iter().any(|line| line.contains("POSTGRES_PASSWORD=***")), "{log:?}");
        assert!(log.last().unwrap().starts_with("Postgres is ready"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_existing_stopped_postgres_is_only_started() {
        let dir = scratch("provision-postgres-stopped");
        fs::write(dir.join("keys.env"), "APZ_DATABASE_URL=postgres://apz:x@127.0.0.1:5434/apz\n").unwrap();
        let mock = docker_says(|args| {
            (args.first() == Some(&"inspect")).then(|| Output::ok("false\n"))
        });
        let env = Env { runner: &mock, keys: Some(dir.join("keys.env")), user: None };
        postgres(&env, &mut |_| {}).unwrap();
        let calls = mock.calls();
        assert_eq!(calls.len(), 3, "inspect, start, then wait: nothing is created");
        assert_eq!(calls[1], "docker start agentplaza-postgres");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_existing_postgres_without_its_url_is_an_error_that_says_what_to_do() {
        let dir = scratch("provision-postgres-nokeys");
        let mock = docker_says(|args| (args.first() == Some(&"inspect")).then(|| Output::ok("true\n")));
        let env = Env { runner: &mock, keys: Some(dir.join("keys.env")), user: None };
        let err = postgres(&env, &mut |_| {}).unwrap_err();
        assert!(err.contains("docker rm -f agentplaza-postgres"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn forgejo_creates_the_admin_and_token_and_records_them_redacted() {
        let dir = scratch("provision-forgejo");
        let mock = docker_says(|args| {
            if args.first() == Some(&"inspect") {
                Some(Output::failed("No such object"))
            } else if args.contains(&"generate-access-token") {
                Some(Output::ok("tok-forgejo-1\n"))
            } else {
                None
            }
        });
        let env = Env { runner: &mock, keys: Some(dir.join("keys.env")), user: None };
        let mut log = Vec::new();
        forgejo(&env, &mut |l| log.push(l)).unwrap();

        let keys = keys_text(&dir);
        let admin_pw = lookup(&keys, "FORGEJO_ADMIN_PASSWORD").expect("the admin password is recorded");
        assert_eq!(lookup(&keys, "FORGEJO_TOKEN"), Some("tok-forgejo-1".to_string()));
        assert_eq!(lookup(&keys, "FORGEJO_URL"), Some("http://127.0.0.1:3300".to_string()));
        assert!(log.iter().all(|line| !line.contains(&admin_pw)), "{log:?}");
        assert!(log.iter().any(|line| line.contains("--password ***")), "{log:?}");
        let calls = mock.calls();
        assert!(call_with(&calls, "admin user create").contains(&format!("--password {admin_pw}")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn garage_creates_the_bucket_and_key_and_records_them_redacted() {
        let dir = scratch("provision-garage");
        let mock = docker_says(|args| {
            if args.first() == Some(&"inspect") {
                Some(Output::failed("No such object"))
            } else if args.contains(&"node") {
                Some(Output::ok("6e1f2a@127.0.0.1:3901\n"))
            } else if args.contains(&"key") {
                Some(Output::ok(
                    "Key name: agentplaza-apid\nKey ID: GK0123456789abcdef\nSecret key: 9f8e7d6c5b4a\nCan create buckets: false\n",
                ))
            } else {
                None
            }
        });
        let env = Env { runner: &mock, keys: Some(dir.join("keys.env")), user: None };
        let mut log = Vec::new();
        garage(&env, &mut |l| log.push(l)).unwrap();

        let calls = mock.calls();
        assert_eq!(calls[0], "docker inspect -f {{.State.Running}} agentplaza-garage");
        assert_eq!(calls[1], "docker volume create agentplaza-garage-meta");
        assert_eq!(calls[2], "docker volume create agentplaza-garage-data");
        let run = call_with(&calls, "docker run -d --name agentplaza-garage");
        assert!(run.contains("dxflrs/garage:v1.0.1"), "{run}");
        assert!(run.contains(&format!("{}:/etc/garage.toml:ro", dir.join("garage.toml").display())), "{run}");
        assert_eq!(
            calls[4..].to_vec(),
            vec![
                "docker exec agentplaza-garage /garage status",
                "docker exec agentplaza-garage /garage node id -q",
                "docker exec agentplaza-garage /garage layout assign -z plaza -c 1G 6e1f2a",
                "docker exec agentplaza-garage /garage layout apply --version 1",
                "docker exec agentplaza-garage /garage bucket create agentplaza",
                "docker exec agentplaza-garage /garage key create agentplaza-apid",
                "docker exec agentplaza-garage /garage bucket allow --read --write --owner agentplaza --key agentplaza-apid",
            ]
        );
        let keys = keys_text(&dir);
        assert_eq!(lookup(&keys, "APZ_S3_ENDPOINT"), Some("http://127.0.0.1:3900".to_string()));
        assert_eq!(lookup(&keys, "APZ_S3_BUCKET"), Some("agentplaza".to_string()));
        assert_eq!(lookup(&keys, "APZ_S3_REGION"), Some("garage".to_string()));
        assert_eq!(lookup(&keys, "APZ_S3_ACCESS_KEY"), Some("GK0123456789abcdef".to_string()));
        assert_eq!(lookup(&keys, "APZ_S3_SECRET_KEY"), Some("9f8e7d6c5b4a".to_string()));
        assert!(log.iter().all(|line| !line.contains("9f8e7d6c5b4a")), "{log:?}");
        assert!(log.last().unwrap().starts_with("Garage is ready"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn garage_config_has_a_64_hex_rpc_secret_and_is_private() {
        let dir = scratch("provision-garage-config");
        let mock = Mock::new(|_, _| Output::ok(""));
        let env = Env { runner: &mock, keys: Some(dir.join("keys.env")), user: None };
        let path = garage_config(&env).unwrap();
        assert_eq!(path, dir.join("garage.toml"));
        let text = fs::read_to_string(&path).unwrap();
        let rpc = text
            .lines()
            .find_map(|line| line.strip_prefix("rpc_secret = \""))
            .and_then(|value| value.strip_suffix('"'))
            .expect("an rpc_secret line");
        assert_eq!(rpc.len(), 64);
        assert!(rpc.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(text.contains("replication_factor = 1"));
        assert!(text.contains("s3_region = \"garage\""));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_existing_garage_without_its_keys_is_an_error_that_says_what_to_do() {
        let dir = scratch("provision-garage-nokeys");
        let mock = docker_says(|args| (args.first() == Some(&"inspect")).then(|| Output::ok("true\n")));
        let env = Env { runner: &mock, keys: Some(dir.join("keys.env")), user: None };
        let err = garage(&env, &mut |_| {}).unwrap_err();
        assert!(err.contains("docker rm -f agentplaza-garage"), "{err}");
        assert_eq!(mock.calls().len(), 1, "nothing else runs");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_docker_command_is_reported_without_its_password() {
        let dir = scratch("provision-redacted-failure");
        fs::write(dir.join("keys.env"), "APZ_POSTGRES_PASSWORD=hunter2-secret\n").unwrap();
        // The daemon refuses the run and echoes the password it was given.
        let mock = Mock::new(|_, args| match args.first() {
            Some(&"inspect") => Output::failed("No such object"),
            Some(&"run") => Output::failed("Error: invalid POSTGRES_PASSWORD=hunter2-secret value"),
            _ => Output::ok(""),
        });
        let env = Env { runner: &mock, keys: Some(dir.join("keys.env")), user: None };
        let err = postgres(&env, &mut |_| {}).unwrap_err();
        assert!(!err.contains("hunter2-secret"), "{err}");
        assert!(err.contains("invalid POSTGRES_PASSWORD=*** value"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn wait_until_gives_up_after_its_timeout() {
        let mock = Mock::new(|_, _| Output::failed("not yet"));
        let env = Env { runner: &mock, keys: None, user: None };
        let err = wait_until(&env, &mut |_| {}, &["exec", "x", "true"], Duration::ZERO, Duration::from_millis(1))
            .unwrap_err();
        assert!(err.contains("not yet"), "{err}");
        assert_eq!(mock.calls().len(), 1);
    }

    #[test]
    fn without_a_home_directory_no_container_is_created() {
        let mock = docker_says(|_| None);
        let env = Env { runner: &mock, keys: None, user: None };
        assert_eq!(postgres(&env, &mut |_| {}).unwrap_err(), "the home directory is not set");
        assert_eq!(forgejo(&env, &mut |_| {}).unwrap_err(), "the home directory is not set");
        assert_eq!(garage(&env, &mut |_| {}).unwrap_err(), "the home directory is not set");
        assert!(mock.calls().is_empty());
    }
}
