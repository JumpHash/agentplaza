//! `weft symbols` and `weft names` through a running hub: a lane's Rust file is snapshotted, and the code queries
//! answer for the lane's revision. These are the fallback the code-intelligence tools use where no language server
//! runs (PLAN 15.3).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

const WEFT: &str = env!("CARGO_BIN_EXE_weft");

/// Runs the CLI in `cwd` with the hub's address and token, plus `extra` variables; its stdout, or its stderr on failure.
fn weft(
    cwd: &Path,
    args: &[&str],
    hub: &[(&str, String)],
    extra: &[(&str, &str)],
) -> Result<String, String> {
    let mut cmd = Command::new(WEFT);
    cmd.args(args)
        .current_dir(cwd)
        .env_remove("WEFT_HUB")
        .env_remove("WEFT_KEY")
        .env_remove("WEFT_AS")
        .env("WEFT_CACHE", npm_cache());
    for (k, v) in hub {
        cmd.env(k, v);
    }
    for (k, v) in extra {
        cmd.env(k, v);
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    if out.status.success() {
        Ok(text)
    } else {
        Err(format!("{}{}", text, String::from_utf8_lossy(&out.stderr)))
    }
}

/// The grammar cache the packs tests share, so a grammar is fetched once.
fn npm_cache() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("weft-npm")
}

struct Hub(Child);

impl Drop for Hub {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn serve(state_args: &[&str], port_file: &Path) -> (Hub, u16) {
    // Wrapped at once, so the hub is killed and reaped on every path out, a failed start included.
    let hub = Hub(Command::new(WEFT)
        .args(state_args)
        .arg("--port-file")
        .arg(port_file)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap());
    let start = Instant::now();
    loop {
        if let Some(port) = std::fs::read_to_string(port_file)
            .ok()
            .and_then(|s| s.trim().parse().ok())
        {
            return (hub, port);
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "weft serve did not start"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_lanes_revision_answers_symbols_and_names_through_the_hub() {
    let dir = std::env::temp_dir().join(format!("weft-code-queries-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let k = |n: &str| dir.join(format!("{n}.key"));
    for n in ["owner", "hub", "ada"] {
        cli(&dir, &["keys", "new", k(n).to_str().unwrap()], &[], &[]);
    }
    let ada_pub = cli(
        &dir,
        &["keys", "show", k("ada").to_str().unwrap()],
        &[],
        &[],
    )
    .split_whitespace()
    .nth(1)
    .unwrap()
    .to_string();
    let state = dir.join("state");
    cli(
        &dir,
        &[
            "init",
            "--state",
            state.to_str().unwrap(),
            "--plaza",
            "test",
            "--owner-key",
            k("owner").to_str().unwrap(),
            "--hub-key",
            k("hub").to_str().unwrap(),
        ],
        &[],
        &[],
    );
    let token = dir.join("internal.token");
    std::fs::write(&token, "s3cret").unwrap();
    let (_hub, port) = serve(
        &[
            "serve",
            "--state",
            state.to_str().unwrap(),
            "--hub-key",
            k("hub").to_str().unwrap(),
            "--token-file",
            token.to_str().unwrap(),
        ],
        &dir.join("hub.port"),
    );
    let hub = vec![
        ("WEFT_HUB", format!("http://127.0.0.1:{port}/v1")),
        ("WEFT_TOKEN_FILE", token.to_str().unwrap().to_string()),
    ];
    let owner_key = k("owner");
    let owner = [("WEFT_KEY", owner_key.to_str().unwrap())];
    let ada_key = k("ada");
    let ada = [("WEFT_KEY", ada_key.to_str().unwrap())];

    cli(
        &dir,
        &[
            "repo",
            "create",
            "lane",
            "--project",
            "pr5",
            "--description",
            "code",
        ],
        &hub,
        &owner,
    );
    cli(
        &dir,
        &[
            "keys",
            "certify",
            &ada_pub,
            "--actor",
            "agent:115",
            "--name",
            "Ada",
            "--role",
            "researcher",
            "--project",
            "pr5",
        ],
        &hub,
        &owner,
    );
    let rust_pack = Path::new(env!("CARGO_MANIFEST_DIR")).join("packs/rust");
    cli(
        &dir,
        &["pack", "add", rust_pack.to_str().unwrap()],
        &hub,
        &owner,
    );
    let change = cli(
        &dir,
        &[
            "lane",
            "open",
            "--repo",
            "lane",
            "--title",
            "Code queries",
            "--author",
            "agent:115",
            "--task",
            "t1",
        ],
        &hub,
        &[],
    )
    .trim()
    .to_string();

    // Ada checks the lane out, writes the fixture's Rust file, and snapshots it.
    let lane = dir.join("ada-lane");
    cli(
        &dir,
        &["checkout", &change, "--dir", lane.to_str().unwrap()],
        &hub,
        &ada,
    );
    std::fs::create_dir_all(lane.join("src")).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../apz-lsp/testdata/repo/src/lib.rs");
    std::fs::copy(&fixture, lane.join("src/lib.rs")).unwrap();
    cli(
        &lane,
        &[
            "snapshot",
            "--tool-call",
            "ide_write#1",
            "--turn",
            "1",
            "--model",
            "claude-opus-5-5",
        ],
        &hub,
        &ada,
    );

    // The file's symbols: the two functions, with their lines.
    let symbols: Value =
        serde_json::from_str(&cli(&lane, &["symbols", "src/lib.rs", "--json"], &hub, &[])).unwrap();
    let names: Vec<&str> = symbols["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["greet", "welcome"], "{symbols}");
    assert_eq!(symbols["symbols"][0]["start_line"], 1);
    assert_eq!(symbols["pack"], "rust");
    assert_eq!(symbols["error"], false);

    // A name: its definition, and the call in `welcome`.
    let greet: Value =
        serde_json::from_str(&cli(&lane, &["names", "greet", "--json"], &hub, &[])).unwrap();
    assert_eq!(greet["definitions"][0]["path"], "src/lib.rs", "{greet}");
    assert_eq!(greet["definitions"][0]["start_line"], 1);
    assert!(
        greet["references"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["line"] == 6 && r["path"] == "src/lib.rs"),
        "{greet}"
    );
    // Names containing a text, without case.
    let found: Value = serde_json::from_str(&cli(
        &lane,
        &["names", "--find", "ELC", "--json"],
        &hub,
        &[],
    ))
    .unwrap();
    assert_eq!(found["definitions"][0]["name"], "welcome", "{found}");
    // An explicit revision answers the same way.
    let at = format!("{change}.d1");
    let explicit: Value = serde_json::from_str(&cli(
        &dir,
        &["names", "greet", "--rev", &at, "--json"],
        &hub,
        &[],
    ))
    .unwrap();
    assert_eq!(explicit["revision"], at);
    std::fs::remove_dir_all(&dir).ok();
}

/// Runs the CLI and asserts it succeeded, returning its stdout.
fn cli(cwd: &Path, args: &[&str], hub: &[(&str, String)], extra: &[(&str, &str)]) -> String {
    weft(cwd, args, hub, extra).unwrap_or_else(|e| panic!("weft {}: {e}", args.join(" ")))
}
