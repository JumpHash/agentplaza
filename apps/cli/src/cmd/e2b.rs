//! `agentplaza e2b`: the hub's E2B-compatible sandbox API, for the owner. Its keys are what the E2B SDKs
//! sign in with; its sandboxes are the ones the hub runs for them.

use clap::Subcommand;
use serde_json::{Value, json};

use crate::ctx::{self, Ctx, confirm, seg};

/// The sandbox proxy the SDKs reach a sandbox through (`E2B_SANDBOX_URL`).
const SANDBOX_URL: &str = "http://127.0.0.1:49982";

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The E2B keys: list, create and revoke.
    #[command(subcommand)]
    Keys(KeysCmd),
    /// The sandboxes on the hub: state, template, size, and the agent whose VM runs them.
    Sandboxes,
    /// Kill a sandbox for good. Asks to confirm unless --yes.
    Kill {
        /// The sandbox's id, from `e2b sandboxes`.
        id: String,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Pause a sandbox. It keeps its state; `e2b resume` goes on from there.
    Pause {
        /// The sandbox's id, from `e2b sandboxes`.
        id: String,
    },
    /// Resume a paused sandbox.
    Resume {
        /// The sandbox's id, from `e2b sandboxes`.
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum KeysCmd {
    /// The keys: id, label and when each was made. A key is never shown again after it is created.
    List,
    /// Make a key. It is printed once, with the three environment lines an E2B SDK reads.
    Create {
        /// A label for the key, to find it by in `e2b keys list`.
        #[arg(long)]
        label: String,
    },
    /// Revoke a key, so the SDKs using it stop working. Asks to confirm unless --yes.
    Revoke {
        /// The key's id, from `e2b keys list`.
        id: String,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::Keys(sub) => keys(ctx, sub),
        Cmd::Sandboxes => sandboxes(ctx),
        Cmd::Kill { id, yes } => kill(ctx, &id, yes),
        Cmd::Pause { id } => pause_or_resume(ctx, &id, "pause"),
        Cmd::Resume { id } => pause_or_resume(ctx, &id, "resume"),
    }
}

fn keys(ctx: &Ctx, cmd: KeysCmd) -> Result<(), String> {
    match cmd {
        KeysCmd::List => keys_list(ctx),
        KeysCmd::Create { label } => key_create(ctx, &label),
        KeysCmd::Revoke { id, yes } => key_revoke(ctx, &id, yes),
    }
}

/// The request body for a new key.
pub fn key_body(label: &str) -> Value {
    json!({ "label": label })
}

/// The three environment lines an E2B SDK reads: the hub's `/e2b` API, the key, and the sandbox proxy.
pub fn env_lines(hub: &str, key: &str) -> Vec<String> {
    vec![
        format!("E2B_API_URL={}/e2b", hub.trim_end_matches('/')),
        format!("E2B_API_KEY={key}"),
        format!("E2B_SANDBOX_URL={SANDBOX_URL}"),
    ]
}

/// One key as a table row: id, label and when it was made.
pub fn key_row(k: &Value) -> Vec<String> {
    vec![
        ctx::text(k, "id"),
        ctx::text(k, "label"),
        ctx::when(&k["created_ms"]),
    ]
}

/// One sandbox as a table row: sandbox, template, state, cpus, memory in MB, start time and agent cid.
pub fn sandbox_row(s: &Value) -> Vec<String> {
    vec![
        ctx::text(s, "sandboxID"),
        ctx::text(s, "templateID"),
        ctx::text(s, "state"),
        ctx::text(s, "cpuCount"),
        ctx::text(s, "memoryMB"),
        ctx::text(s, "startedAt"),
        ctx::text(s, "cid"),
    ]
}

/// A sandbox's path on the hub, with an optional action after it (`pause`, `resume`).
fn sandbox_path(id: &str, action: Option<&str>) -> String {
    let base = format!("/v1/e2b/sandboxes/{}", seg(id));
    match action {
        Some(action) => format!("{base}/{action}"),
        None => base,
    }
}

fn keys_list(ctx: &Ctx) -> Result<(), String> {
    let v = ctx.client.get("/v1/e2b/keys")?;
    ctx.out(&v, |v| {
        let rows: Vec<Vec<String>> = v["keys"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(key_row)
            .collect();
        ctx::table(&["id", "label", "created"], &rows);
    });
    Ok(())
}

fn key_create(ctx: &Ctx, label: &str) -> Result<(), String> {
    let v = ctx.client.post("/v1/e2b/keys", &key_body(label))?;
    ctx.out(&v, |v| {
        println!(
            "created key {} ({}); the key is shown once, so keep these lines:",
            ctx::text(v, "id"),
            ctx::text(v, "label")
        );
        for line in env_lines(ctx.client.base(), ctx::s(v, "key")) {
            println!("{line}");
        }
    });
    Ok(())
}

fn key_revoke(ctx: &Ctx, id: &str, yes: bool) -> Result<(), String> {
    if !confirm(
        yes,
        &format!("Revoke E2B key {id}? The SDKs using it stop working."),
    )? {
        println!("cancelled");
        return Ok(());
    }
    let v = ctx.client.delete(&format!("/v1/e2b/keys/{}", seg(id)))?;
    ctx.out(&v, |_| println!("revoked E2B key {id}"));
    Ok(())
}

fn sandboxes(ctx: &Ctx) -> Result<(), String> {
    let v = ctx.client.get("/v1/e2b/sandboxes")?;
    ctx.out(&v, |v| {
        let rows: Vec<Vec<String>> = v["sandboxes"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(sandbox_row)
            .collect();
        ctx::table(
            &[
                "sandbox",
                "template",
                "state",
                "cpus",
                "memory MB",
                "started",
                "agent",
            ],
            &rows,
        );
    });
    Ok(())
}

fn kill(ctx: &Ctx, id: &str, yes: bool) -> Result<(), String> {
    if !confirm(
        yes,
        &format!("Kill sandbox {id}? It is destroyed and cannot be resumed."),
    )? {
        println!("cancelled");
        return Ok(());
    }
    let v = ctx.client.delete(&sandbox_path(id, None))?;
    ctx.out(&v, |_| println!("killed sandbox {id}"));
    Ok(())
}

fn pause_or_resume(ctx: &Ctx, id: &str, action: &str) -> Result<(), String> {
    let v = ctx
        .client
        .post(&sandbox_path(id, Some(action)), &json!({}))?;
    let done = if action == "pause" {
        "paused"
    } else {
        "resumed"
    };
    ctx.out(&v, |_| println!("{done} sandbox {id}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// The `e2b` command alone, so its subcommands parse the way the CLI parses them.
    #[derive(Parser, Debug)]
    struct Probe {
        #[command(subcommand)]
        cmd: Cmd,
    }

    /// `e2b ARGS`, parsed.
    fn parse(args: &[&str]) -> Result<Cmd, clap::Error> {
        let mut argv = vec!["e2b"];
        argv.extend_from_slice(args);
        Probe::try_parse_from(argv).map(|p| p.cmd)
    }

    #[test]
    fn keys_list_create_and_revoke_parse() {
        assert!(matches!(
            parse(&["keys", "list"]).unwrap(),
            Cmd::Keys(KeysCmd::List)
        ));
        match parse(&["keys", "create", "--label", "ci"]).unwrap() {
            Cmd::Keys(KeysCmd::Create { label }) => assert_eq!(label, "ci"),
            other => panic!("not a key create: {other:?}"),
        }
        assert!(parse(&["keys", "create"]).is_err(), "a key needs a label");
        match parse(&["keys", "revoke", "k1"]).unwrap() {
            Cmd::Keys(KeysCmd::Revoke { id, yes }) => assert_eq!((id.as_str(), yes), ("k1", false)),
            other => panic!("not a revoke: {other:?}"),
        }
        match parse(&["keys", "revoke", "k1", "--yes"]).unwrap() {
            Cmd::Keys(KeysCmd::Revoke { yes, .. }) => assert!(yes),
            other => panic!("not a revoke: {other:?}"),
        }
    }

    #[test]
    fn sandbox_commands_take_the_sandbox_id() {
        assert!(matches!(parse(&["sandboxes"]).unwrap(), Cmd::Sandboxes));
        match parse(&["kill", "sb1"]).unwrap() {
            Cmd::Kill { id, yes } => assert_eq!((id.as_str(), yes), ("sb1", false)),
            other => panic!("not a kill: {other:?}"),
        }
        match parse(&["kill", "sb1", "--yes"]).unwrap() {
            Cmd::Kill { yes, .. } => assert!(yes),
            other => panic!("not a kill: {other:?}"),
        }
        match parse(&["pause", "sb1"]).unwrap() {
            Cmd::Pause { id } => assert_eq!(id, "sb1"),
            other => panic!("not a pause: {other:?}"),
        }
        match parse(&["resume", "sb1"]).unwrap() {
            Cmd::Resume { id } => assert_eq!(id, "sb1"),
            other => panic!("not a resume: {other:?}"),
        }
    }

    #[test]
    fn a_key_body_names_the_label() {
        assert_eq!(key_body("ci"), json!({ "label": "ci" }));
    }

    #[test]
    fn the_env_lines_point_an_sdk_at_the_hub_and_the_sandbox_proxy() {
        assert_eq!(
            env_lines("http://127.0.0.1:7700", "e2b_abc"),
            [
                "E2B_API_URL=http://127.0.0.1:7700/e2b",
                "E2B_API_KEY=e2b_abc",
                "E2B_SANDBOX_URL=http://127.0.0.1:49982",
            ]
        );
        // A hub given with a trailing slash does not double it.
        assert_eq!(
            env_lines("https://hub.example/", "k")[0],
            "E2B_API_URL=https://hub.example/e2b"
        );
    }

    #[test]
    fn sandbox_paths_encode_the_id_and_add_the_action() {
        assert_eq!(sandbox_path("sb 1", None), "/v1/e2b/sandboxes/sb%201");
        assert_eq!(
            sandbox_path("sb1", Some("pause")),
            "/v1/e2b/sandboxes/sb1/pause"
        );
    }

    #[test]
    fn a_key_row_has_id_label_and_when() {
        let k = json!({ "id": "k1", "label": "ci", "created_ms": 1_700_000_000_000u64 });
        let row = key_row(&k);
        assert_eq!(row.len(), 3);
        assert_eq!(&row[..2], ["k1", "ci"]);
    }

    #[test]
    fn a_sandbox_row_reads_each_field_in_the_table_order() {
        let s = json!({
            "sandboxID": "sb1", "templateID": "base", "state": "running",
            "cpuCount": 2, "memoryMB": 512, "startedAt": "2026-10-15T10:00:00Z",
            "endAt": "2026-10-15T11:00:00Z", "cid": 4
        });
        assert_eq!(
            sandbox_row(&s),
            vec![
                "sb1",
                "base",
                "running",
                "2",
                "512",
                "2026-10-15T10:00:00Z",
                "4"
            ]
        );
    }
}
