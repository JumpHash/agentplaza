//! `agents cancel`, `retire`, `wipe`, `sleep`/`wake`, and the VM checkpoints and forks: an agent's
//! running work, its place on the plaza, and its VM's state over time.

use serde_json::{Value, json};

use crate::ctx::{self, Ctx, confirm, seg};

/// What a wipe clears on top of the workspace, which is always reset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WipeWhat {
    pub memory: bool,
    pub transcript: bool,
    pub identity: bool,
}

/// Stops the agent's running work. The agent stays on the roster.
pub fn cancel(ctx: &Ctx, cid: u64) -> Result<(), String> {
    let v = ctx
        .client
        .post(&format!("/v1/agents/{cid}/cancel"), &json!({}))?;
    ctx.out(&v, |v| {
        if v.get("cancelled").and_then(Value::as_bool).unwrap_or(false) {
            println!("cancelled the running work of agent {cid}");
        } else {
            println!("agent {cid} had no work running");
        }
    });
    Ok(())
}

/// Retires an agent for good: its VM stops and it leaves the roster. Asks to confirm unless `yes`.
pub fn retire(ctx: &Ctx, cid: u64, yes: bool) -> Result<(), String> {
    if !confirm(
        yes,
        &format!("Retire agent {cid}? Its VM stops for good and it leaves the roster."),
    )? {
        return super::cancelled();
    }
    let v = ctx
        .client
        .post(&format!("/v1/agents/{cid}/retire"), &json!({}))?;
    ctx.out(&v, |_| println!("retired agent {cid}"));
    Ok(())
}

/// The wipe request body. At least one of memory, transcript or identity is needed: a wipe that
/// only resets the workspace is refused, so the owner names what else to clear.
pub fn wipe_body(what: WipeWhat) -> Result<Value, String> {
    if !(what.memory || what.transcript || what.identity) {
        return Err("a wipe needs at least one of --memory, --transcript, --identity (the workspace is always reset)".into());
    }
    Ok(json!({ "memory": what.memory, "transcript": what.transcript, "identity": what.identity }))
}

/// Wipes an agent's workspace and what `what` names. Asks to confirm unless `yes`.
pub fn wipe(ctx: &Ctx, cid: u64, what: WipeWhat, yes: bool) -> Result<(), String> {
    let body = wipe_body(what)?;
    let mut cleared = vec!["workspace"];
    if what.memory {
        cleared.push("memory");
    }
    if what.transcript {
        cleared.push("transcript");
    }
    if what.identity {
        cleared.push("identity (name, contacts and reputation)");
    }
    if !confirm(
        yes,
        &format!("Wipe agent {cid}: clear its {}?", cleared.join(", ")),
    )? {
        return super::cancelled();
    }
    let v = ctx.client.post(&format!("/v1/agents/{cid}/wipe"), &body)?;
    ctx.out(&v, |v| {
        let wiped: Vec<&str> = v["wiped"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        println!("wiped {} of agent {cid}", wiped.join(", "));
    });
    Ok(())
}

/// Puts an agent to sleep: its VM is paused with its memory kept, so it wakes where it was.
pub fn sleep(ctx: &Ctx, cid: u64) -> Result<(), String> {
    let v = ctx
        .client
        .post(&format!("/v1/agents/{cid}/sleep"), &json!({}))?;
    ctx.out(&v, |_| println!("agent {cid} is asleep"));
    Ok(())
}

/// Wakes a sleeping agent.
pub fn wake(ctx: &Ctx, cid: u64) -> Result<(), String> {
    let v = ctx
        .client
        .post(&format!("/v1/agents/{cid}/wake"), &json!({}))?;
    ctx.out(&v, |_| println!("agent {cid} is awake"));
    Ok(())
}

/// The agent's VM checkpoints collection.
fn checkpoints_path(cid: u64) -> String {
    format!("/v1/agents/{cid}/vm-checkpoints")
}

/// One checkpoint of the agent, by its name.
fn checkpoint_path(cid: u64, name: &str) -> String {
    format!("{}/{}", checkpoints_path(cid), seg(name))
}

/// The checkpoint request body: an optional label. A blank label is no label.
pub fn checkpoint_body(label: Option<&str>) -> Value {
    match label.map(str::trim).filter(|l| !l.is_empty()) {
        Some(label) => json!({ "label": label }),
        None => json!({}),
    }
}

/// Takes a checkpoint of the agent's whole VM (disk and memory) to roll back to later.
pub fn checkpoint(ctx: &Ctx, cid: u64, label: Option<&str>) -> Result<(), String> {
    let v = ctx
        .client
        .post(&checkpoints_path(cid), &checkpoint_body(label))?;
    ctx.out(&v, |c| {
        println!("checkpointed agent {cid} as {}", ctx::s(c, "name"))
    });
    Ok(())
}

/// A byte count for people ("512 B", "1.5 KiB", "3.0 MiB"); "—" when it is absent.
pub fn human_bytes(v: &Value) -> String {
    let Some(bytes) = v.as_f64() else {
        return "—".to_string();
    };
    let mut size = bytes.max(0.0);
    let units = ["B", "KiB", "MiB", "GiB"];
    let mut unit = 0;
    while size >= 1024.0 && unit < units.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} B", size as u64)
    } else {
        format!("{size:.1} {}", units[unit])
    }
}

/// One checkpoint as a table row. `age` is how long ago it was taken, as the caller reads it.
pub fn checkpoint_row(c: &Value, age: &str) -> Vec<String> {
    let or_dash = |s: &str| {
        if s.is_empty() {
            "-".to_string()
        } else {
            s.to_string()
        }
    };
    vec![
        ctx::s(c, "name").to_string(),
        or_dash(ctx::s(c, "label")),
        age.to_string(),
        ctx::s(c, "kind").to_string(),
        human_bytes(&c["memory_written"]),
        human_bytes(&c["disk_bytes"]),
        match c["paused_ms"].as_f64() {
            Some(ms) => format!("{} ms", ms as u64),
            None => "—".to_string(),
        },
    ]
}

/// An agent's VM checkpoints, one row each, under the checkpoint table's header.
pub fn checkpoints(ctx: &Ctx, cid: u64) -> Result<(), String> {
    let v = ctx.client.get(&checkpoints_path(cid))?;
    ctx.out(&v, |v| {
        let rows: Vec<Vec<String>> = v["checkpoints"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|c| checkpoint_row(c, &ctx::ago(&c["taken_at_ms"])))
            .collect();
        ctx::table(
            &["name", "label", "age", "kind", "memory", "disk", "paused"],
            &rows,
        );
    });
    Ok(())
}

/// Rolls the agent's VM back to a checkpoint. The hub takes a safety checkpoint of the current
/// state first, so the rollback can itself be undone. Asks to confirm unless `yes`.
pub fn rollback(ctx: &Ctx, cid: u64, name: &str, yes: bool) -> Result<(), String> {
    if !confirm(
        yes,
        &format!(
            "Roll agent {cid} back to checkpoint {name}? Its VM's work since then is discarded. A safety checkpoint of the current state is taken first."
        ),
    )? {
        return super::cancelled();
    }
    let v = ctx.client.post(
        &format!("{}/restore", checkpoint_path(cid, name)),
        &json!({}),
    )?;
    ctx.out(&v, |v| match safety_name(&v["safety"]) {
        Some(safety) => {
            println!("rolled agent {cid} back to {name} (safety checkpoint {safety})")
        }
        None => println!("rolled agent {cid} back to {name}"),
    });
    Ok(())
}

/// The name of the safety checkpoint a rollback reports: a bare name or an object's `name`.
fn safety_name(v: &Value) -> Option<String> {
    v.as_str()
        .or_else(|| v.get("name").and_then(Value::as_str))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Deletes one VM checkpoint. Asks to confirm unless `yes`.
pub fn checkpoint_delete(ctx: &Ctx, cid: u64, name: &str, yes: bool) -> Result<(), String> {
    if !confirm(
        yes,
        &format!(
            "Delete checkpoint {name} of agent {cid}? It cannot be rolled back to afterwards."
        ),
    )? {
        return super::cancelled();
    }
    let v = ctx.client.delete(&checkpoint_path(cid, name))?;
    ctx.out(&v, |_| println!("deleted checkpoint {name} of agent {cid}"));
    Ok(())
}

/// The fork request body: how many copies to start.
pub fn fork_body(n: u64) -> Value {
    json!({ "n": n })
}

/// The forks as table rows: each new agent's cid, its label and run.
pub fn fork_rows(forks: &Value) -> Vec<Vec<String>> {
    forks
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|f| vec![ctx::text(f, "cid"), super::label_of(f), ctx::text(f, "run")])
        .collect()
}

/// Forks an agent into `n` copies. The hub takes a checkpoint of the agent's VM and starts each
/// copy from it, so the copies are new agents on the roster.
pub fn fork(ctx: &Ctx, cid: u64, n: u64) -> Result<(), String> {
    let v = ctx
        .client
        .post(&format!("/v1/agents/{cid}/fork"), &fork_body(n))?;
    ctx.out(&v, |v| {
        let rows = fork_rows(&v["forks"]);
        println!(
            "forked agent {cid} into {} agent{} from checkpoint {}",
            rows.len(),
            if rows.len() == 1 { "" } else { "s" },
            ctx::text(v, "checkpoint")
        );
        ctx::table(&["cid", "agent", "run"], &rows);
    });
    Ok(())
}

/// The move request body: the node the agent's VM goes to.
pub fn move_body(node: &str) -> Value {
    json!({ "node": node })
}

/// The result of a move as one line: the nodes it went between, the megabytes moved and the seconds it
/// took. A missing number reads as zero.
pub fn move_line(cid: u64, moved: &Value) -> String {
    let megabytes = moved["bytes"].as_f64().unwrap_or(0.0) / 1_000_000.0;
    let seconds = moved["ms"].as_f64().unwrap_or(0.0) / 1000.0;
    format!(
        "moved agent {cid} from {} to {}: {megabytes:.1} MB in {seconds:.1} s",
        ctx::text(moved, "from"),
        ctx::text(moved, "to")
    )
}

/// Moves an agent's VM to another node. The hub copies it over and reports what it moved.
pub fn move_to(ctx: &Ctx, cid: u64, node: &str) -> Result<(), String> {
    let v = ctx
        .client
        .post(&format!("/v1/agents/{cid}/move"), &move_body(node))?;
    ctx.out(&v, |v| println!("{}", move_line(cid, v)));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn agent_commands_parse_their_arguments() {
        // `move` takes the agent cid and the node, and needs the node.
        {
            match parse(&["move", "4", "n2"]).unwrap() {
                super::super::Cmd::Move { cid, node } => {
                    assert_eq!((cid, node.as_str()), (4, "n2"))
                }
                other => panic!("not a move: {other:?}"),
            }
            assert!(parse(&["move", "4"]).is_err(), "a move needs a node");
        }
        // `sleep` and `wake` take the agent cid.
        {
            match parse(&["sleep", "4"]).unwrap() {
                super::super::Cmd::Sleep { cid } => assert_eq!(cid, 4),
                other => panic!("not a sleep: {other:?}"),
            }
            match parse(&["wake", "4"]).unwrap() {
                super::super::Cmd::Wake { cid } => assert_eq!(cid, 4),
                other => panic!("not a wake: {other:?}"),
            }
        }
        // `checkpoint` takes an optional label.
        {
            match parse(&["checkpoint", "4", "--label", "before upgrade"]).unwrap() {
                super::super::Cmd::Checkpoint { cid, label } => {
                    assert_eq!((cid, label.as_deref()), (4, Some("before upgrade")))
                }
                other => panic!("not a checkpoint: {other:?}"),
            }
            match parse(&["checkpoint", "4"]).unwrap() {
                super::super::Cmd::Checkpoint { label, .. } => assert_eq!(label, None),
                other => panic!("not a checkpoint: {other:?}"),
            }
        }
        // `rollback` and `checkpoint-delete` ask for confirmation unless `--yes` is given.
        {
            match parse(&["rollback", "4", "snap-1"]).unwrap() {
                super::super::Cmd::Rollback {
                    cid,
                    checkpoint,
                    yes,
                } => assert_eq!((cid, checkpoint.as_str(), yes), (4, "snap-1", false)),
                other => panic!("not a rollback: {other:?}"),
            }
            match parse(&["rollback", "4", "snap-1", "--yes"]).unwrap() {
                super::super::Cmd::Rollback { yes, .. } => assert!(yes),
                other => panic!("not a rollback: {other:?}"),
            }
            match parse(&["checkpoint-delete", "4", "snap-1", "--yes"]).unwrap() {
                super::super::Cmd::CheckpointDelete {
                    cid,
                    checkpoint,
                    yes,
                } => assert_eq!((cid, checkpoint.as_str(), yes), (4, "snap-1", true)),
                other => panic!("not a checkpoint-delete: {other:?}"),
            }
        }
        // `fork` makes one copy unless told otherwise, and at most eight.
        {
            match parse(&["fork", "4"]).unwrap() {
                super::super::Cmd::Fork { n, .. } => assert_eq!(n, 1),
                other => panic!("not a fork: {other:?}"),
            }
            match parse(&["fork", "4", "--n", "8"]).unwrap() {
                super::super::Cmd::Fork { cid, n } => assert_eq!((cid, n), (4, 8)),
                other => panic!("not a fork: {other:?}"),
            }
            for bad in ["0", "9"] {
                assert!(
                    parse(&["fork", "4", "--n", bad]).is_err(),
                    "--n {bad} should be refused"
                );
            }
        }
    }

    #[test]
    fn a_move_body_names_the_node() {
        assert_eq!(move_body("n2"), json!({ "node": "n2" }));
    }

    /// The `agents` command alone, so its subcommands parse the way the CLI parses them.
    #[derive(Parser, Debug)]
    struct Probe {
        #[command(subcommand)]
        cmd: super::super::Cmd,
    }

    /// `agents ARGS`, parsed.
    fn parse(args: &[&str]) -> Result<super::super::Cmd, clap::Error> {
        let mut argv = vec!["agents"];
        argv.extend_from_slice(args);
        Probe::try_parse_from(argv).map(|p| p.cmd)
    }

    #[test]
    fn checkpoint_paths_name_the_agent_and_percent_encode_the_name() {
        assert_eq!(checkpoints_path(4), "/v1/agents/4/vm-checkpoints");
        assert_eq!(
            checkpoint_path(4, "snap 1/x"),
            "/v1/agents/4/vm-checkpoints/snap%201%2Fx"
        );
    }

    #[test]
    fn a_checkpoint_body_carries_a_label_only_when_it_has_one() {
        assert_eq!(checkpoint_body(None), json!({}));
        assert_eq!(checkpoint_body(Some("   ")), json!({}));
        assert_eq!(checkpoint_body(Some(" pre ")), json!({ "label": "pre" }));
    }

    #[test]
    fn a_fork_body_names_the_copies() {
        assert_eq!(fork_body(3), json!({ "n": 3 }));
    }

    #[test]
    fn a_safety_checkpoint_reads_as_a_name_or_an_object() {
        assert_eq!(safety_name(&json!("safety-1")), Some("safety-1".into()));
        assert_eq!(
            safety_name(&json!({ "name": "safety-2" })),
            Some("safety-2".into())
        );
        assert_eq!(safety_name(&Value::Null), None);
        assert_eq!(safety_name(&json!("")), None);
    }

    #[test]
    fn a_wipe_needs_at_least_one_flag() {
        assert!(wipe_body(WipeWhat::default()).is_err());
    }

    #[test]
    fn a_wipe_body_names_each_flag() {
        let body = wipe_body(WipeWhat {
            memory: true,
            transcript: false,
            identity: true,
        })
        .unwrap();
        assert_eq!(
            body,
            json!({ "memory": true, "transcript": false, "identity": true })
        );
        let body = wipe_body(WipeWhat {
            transcript: true,
            ..WipeWhat::default()
        })
        .unwrap();
        assert_eq!(body["transcript"], true);
        assert_eq!(body["memory"], false);
    }
}
