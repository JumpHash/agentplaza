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

/// The forks as table rows: each new agent's cid, name and run.
pub fn fork_rows(forks: &Value) -> Vec<Vec<String>> {
    forks
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|f| vec![ctx::text(f, "cid"), super::name_of(f), ctx::text(f, "run")])
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
        ctx::table(&["cid", "name", "run"], &rows);
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

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
    fn sleep_and_wake_take_the_agent_cid() {
        match parse(&["sleep", "4"]).unwrap() {
            super::super::Cmd::Sleep { cid } => assert_eq!(cid, 4),
            other => panic!("not a sleep: {other:?}"),
        }
        match parse(&["wake", "4"]).unwrap() {
            super::super::Cmd::Wake { cid } => assert_eq!(cid, 4),
            other => panic!("not a wake: {other:?}"),
        }
    }

    #[test]
    fn checkpoint_takes_an_optional_label() {
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

    #[test]
    fn rollback_and_checkpoint_delete_ask_unless_yes() {
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

    #[test]
    fn fork_makes_one_copy_unless_told_otherwise_and_at_most_eight() {
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
    fn human_bytes_scales_and_says_when_absent() {
        assert_eq!(human_bytes(&json!(512)), "512 B");
        assert_eq!(human_bytes(&json!(1536)), "1.5 KiB");
        assert_eq!(human_bytes(&json!(3 * 1024 * 1024)), "3.0 MiB");
        assert_eq!(human_bytes(&json!(5_497_558_138_880u64)), "5120.0 GiB");
        assert_eq!(human_bytes(&Value::Null), "—");
    }

    #[test]
    fn a_checkpoint_row_reads_each_field_in_the_table_order() {
        let c = json!({
            "name": "ckpt-7", "cid": 4, "label": "", "kind": "manual",
            "taken_at_ms": 1_700_000_000_000u64, "memory_written": 2048,
            "disk_bytes": 1_073_741_824u64, "paused_ms": 350
        });
        assert_eq!(
            checkpoint_row(&c, "3 h ago"),
            vec![
                "ckpt-7", "-", "3 h ago", "manual", "2.0 KiB", "1.0 GiB", "350 ms"
            ]
        );
        let bare = json!({ "name": "ckpt-8", "label": "pre" });
        let row = checkpoint_row(&bare, "just now");
        assert_eq!(row[1], "pre");
        assert_eq!(row[4], "—");
        assert_eq!(row[6], "—");
    }

    #[test]
    fn forks_list_each_new_agent_and_name_a_missing_one_by_its_cid() {
        let forks = json!([
            { "cid": 9, "run": "r1", "name": "ada-2" },
            { "cid": 10, "run": "r1", "name": "" }
        ]);
        assert_eq!(
            fork_rows(&forks),
            vec![vec!["9", "ada-2", "r1"], vec!["10", "agent 10", "r1"],]
        );
        assert!(fork_rows(&Value::Null).is_empty());
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
