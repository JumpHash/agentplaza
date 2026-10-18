//! The `weft` binary end to end: `weft serve` on a fresh store, a node cache in front of it, and a research lane
//! written through the CLI (checkout, snapshots per turn, an intent, done), read back with show, tree and log, and
//! a snapshot that waits in the outbox while the hub is out of reach.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const WEFT: &str = env!("CARGO_BIN_EXE_weft");

struct Run {
    dir: PathBuf,
    env: Vec<(String, String)>,
}

impl Run {
    fn weft(&self, cwd: &Path, args: &[&str], extra: &[(&str, &str)]) -> Result<String, String> {
        let mut cmd = Command::new(WEFT);
        cmd.args(args)
            .current_dir(cwd)
            .env_remove("WEFT_HUB")
            .env_remove("WEFT_KEY")
            .env_remove("WEFT_AS")
            .env("WEFT_CACHE", self.dir.join("cli-cache"));
        for (k, v) in &self.env {
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

    fn ok(&self, cwd: &Path, args: &[&str], extra: &[(&str, &str)]) -> String {
        self.weft(cwd, args, extra)
            .unwrap_or_else(|e| panic!("weft {}: {e}", args.join(" ")))
    }
}

fn serve(args: &[&str], port_file: &Path) -> (Kill, u16) {
    let child = Kill(
        Command::new(WEFT)
            .args(args)
            .arg("--port-file")
            .arg(port_file)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let start = Instant::now();
    loop {
        if let Some(p) = std::fs::read_to_string(port_file)
            .ok()
            .and_then(|s| s.trim().parse().ok())
        {
            return (child, p);
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "weft serve did not start"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn a_research_lane_through_the_cli_and_a_node_cache() {
    let dir = std::env::temp_dir().join(format!("weft-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut run = Run {
        dir: dir.clone(),
        env: vec![],
    };
    let k = |n: &str| dir.join(format!("{n}.key"));
    for n in ["owner", "hub", "ada"] {
        run.ok(&dir, &["keys", "new", k(n).to_str().unwrap()], &[]);
    }
    let ada_pub = run
        .ok(&dir, &["keys", "show", k("ada").to_str().unwrap()], &[])
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_string();
    let state = dir.join("state");
    run.ok(
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
    );
    let token = dir.join("internal.token");
    std::fs::write(&token, "s3cret").unwrap();
    let (hub, port) = serve(
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
    let _hub = hub;
    let hub_url = format!("http://127.0.0.1:{port}/v1");
    run.env = vec![
        ("WEFT_HUB".into(), hub_url.clone()),
        ("WEFT_TOKEN_FILE".into(), token.to_str().unwrap().into()),
    ];
    let owner_key = k("owner");
    let owner = [("WEFT_KEY", owner_key.to_str().unwrap())];

    // Without the token, nothing answers.
    assert!(
        run.weft(&dir, &["repos"], &[("WEFT_TOKEN_FILE", "/nonexistent")])
            .is_err()
    );

    // The owner sets up: a repo, the researcher's certificate, the Markdown pack.
    run.ok(
        &dir,
        &[
            "repo",
            "create",
            "research",
            "--project",
            "pr5",
            "--description",
            "briefs and data",
        ],
        &owner,
    );
    run.ok(
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
        &owner,
    );
    let md = Path::new(env!("CARGO_MANIFEST_DIR")).join("packs/markdown");
    run.ok(&dir, &["pack", "add", md.to_str().unwrap()], &owner);
    assert!(run.ok(&dir, &["packs"], &[]).contains("markdown"));

    // The hub opens Ada's lane when she claims t61.
    let change = run
        .ok(
            &dir,
            &[
                "lane",
                "open",
                "--repo",
                "research",
                "--title",
                "Edge caching brief",
                "--author",
                "agent:115",
                "--task",
                "t61",
            ],
            &[],
        )
        .trim()
        .to_string();
    assert_eq!(change, "w-1");

    // Ada works in her VM: checkout, two turns, an intent, done.
    let ada_key = k("ada");
    let ada = [("WEFT_KEY", ada_key.to_str().unwrap())];
    let lane = dir.join("ada-lane");
    run.ok(
        &dir,
        &["checkout", "w-1", "--dir", lane.to_str().unwrap()],
        &ada,
    );
    std::fs::create_dir_all(lane.join("briefs")).unwrap();
    std::fs::write(
        lane.join("briefs/edge-caching.md"),
        "# Edge caching\n\n## Question\n\nShould nodes cache objects?\n",
    )
    .unwrap();
    std::fs::write(lane.join("notes.txt"), "scratch\n").unwrap();
    let s1 = run.ok(
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
        &ada,
    );
    assert!(s1.contains("w-1.d1"), "{s1}");
    std::fs::write(lane.join("briefs/edge-caching.md"), "# Edge caching\n\n## Question\n\nShould nodes cache objects?\n\n## Findings\n\n- 83% of reads repeat within a task.\n").unwrap();
    std::fs::remove_file(lane.join("notes.txt")).unwrap();
    let status = run.ok(&lane, &["status"], &ada);
    assert!(
        status.contains("briefs/edge-caching.md") && status.contains("not snapshotted"),
        "{status}"
    );
    run.ok(
        &lane,
        &[
            "snapshot",
            "--tool-call",
            "ide_write#2",
            "--turn",
            "2",
            "--model",
            "claude-opus-5-5",
        ],
        &ada,
    );
    assert!(run.ok(&lane, &["snapshot"], &ada).contains("nothing new"));
    run.ok(
        &lane,
        &[
            "describe",
            "--intent",
            "Measure edge caching",
            "--kind",
            "research",
            "briefs/**",
        ],
        &ada,
    );
    let done = run.ok(&lane, &["done"], &ada);
    assert!(done.contains("w-1.r1 sealed"), "{done}");

    // Read it back.
    let show = run.ok(&dir, &["show", "w-1"], &[]);
    assert!(
        show.contains("w-1.d1")
            && show.contains("w-1.d2")
            && show.contains("w-1.r1")
            && show.contains("Measure edge caching"),
        "{show}"
    );
    let tree = run.ok(&dir, &["tree", "w-1.r1"], &[]);
    assert!(
        tree.contains("section Edge caching › section Findings") && tree.contains("w-1.d2"),
        "{tree}"
    );
    assert!(
        !tree.contains("notes.txt"),
        "a file added and removed again is not in the change: {tree}"
    );
    let log = run.ok(
        &dir,
        &["log", "by(agent:115) & sealed()", "--repo", "research"],
        &[],
    );
    assert!(
        log.contains("w-1") && log.contains("Edge caching brief"),
        "{log}"
    );
    assert!(
        run.ok(&dir, &["log", "landed()", "--repo", "research"], &[])
            .contains("0 of 0")
    );
    let verify = run.ok(&dir, &["verify", "w-1"], &[]);
    assert!(
        verify.contains("verified") && verify.contains("Ada, researcher"),
        "{verify}"
    );
    assert!(
        run.ok(&dir, &["verify", "--deep"], &[])
            .contains("the log verifies")
    );

    // Another VM on the same node checks the lane out through the node's cache: the second read is all hits.
    let (cache, cport) = serve(
        &[
            "serve",
            "--cache",
            "--dir",
            dir.join("cache").to_str().unwrap(),
            "--upstream",
            &hub_url,
            "--upstream-token-file",
            token.to_str().unwrap(),
        ],
        &dir.join("cache.port"),
    );
    let _cache = cache;
    let via_cache = format!("http://127.0.0.1:{cport}/v1");
    let other = dir.join("other-vm");
    run.ok(
        &dir,
        &["checkout", "w-1", "--dir", other.to_str().unwrap()],
        &[("WEFT_HUB", &via_cache), ("WEFT_TOKEN_FILE", "")],
    );
    assert!(
        std::fs::read_to_string(other.join("briefs/edge-caching.md"))
            .unwrap()
            .contains("83%")
    );
    let third = dir.join("third-vm");
    run.ok(
        &dir,
        &["checkout", "w-1", "--dir", third.to_str().unwrap()],
        &[("WEFT_HUB", &via_cache), ("WEFT_TOKEN_FILE", "")],
    );
    let stats: serde_json::Value = serde_json::from_slice(
        &apz_http::client::send(cport, "GET", "/v1/cache", None, Duration::from_secs(5))
            .unwrap()
            .body,
    )
    .unwrap();
    assert!(
        stats["hits"].as_u64().unwrap() >= 1 && stats["objects"].as_u64().unwrap() >= 1,
        "{stats}"
    );

    // Rework: the hub is out of reach, so the snapshot waits in the outbox; the next command sends it.
    std::fs::write(lane.join("briefs/edge-caching.md"), "# Edge caching\n\n## Question\n\nShould nodes cache objects?\n\n## Findings\n\n- 83% of reads repeat within a task.\n\n## Recommendation\n\nCache on every node.\n").unwrap();
    let off = run.ok(
        &lane,
        &["snapshot", "--turn", "3"],
        &[
            ("WEFT_KEY", ada_key.to_str().unwrap()),
            ("WEFT_HUB", "http://127.0.0.1:9/v1"),
        ],
    );
    assert!(off.contains("outbox"), "{off}");
    assert!(
        run.ok(&lane, &["status"], &ada)
            .contains("waiting in the outbox")
    );
    std::fs::write(lane.join("data.csv"), "task,reads\nt1,120\n").unwrap();
    run.ok(&lane, &["snapshot", "--turn", "4"], &ada);
    let show = run.ok(&dir, &["show", "w-1"], &[]);
    assert!(
        show.contains("w-1.d3") && show.contains("w-1.d4"),
        "the outbox went first: {show}"
    );
    std::fs::remove_dir_all(&dir).ok();
}
