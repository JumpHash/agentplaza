//! `agentplaza machines`: Machines: VMs, resources, runs.

use clap::Subcommand;
use serde_json::Value;

use crate::cmd::models::{confirm, items, none_if_empty};
use crate::ctx::{self, Ctx};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The machines the plaza runs on: state, load, runs.
    List,
    /// One machine (by id or name): hardware, load, the plaza's own usage, and its runs.
    Show {
        /// The machine id or name.
        id: String,
    },
    /// Kill a run and its VMs. Asks first unless --yes.
    Kill {
        /// The run id.
        run: String,
        /// Kill without asking.
        #[arg(long)]
        yes: bool,
    },
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List => {
            let list = ctx.client.get("/v1/machines")?;
            ctx.out(&list, |v| {
                let rows: Vec<Vec<String>> = items(v)
                    .map(|m| {
                        vec![
                            ctx::s(m, "name").to_string(),
                            ctx::s(m, "kind").to_string(),
                            ctx::s(m, "arch").to_string(),
                            reachable(m),
                            ctx::text(m, "cpus"),
                            format!("{}/{} MiB", figure(stat(m, "mem_used_mib"), ""), ctx::text(m, "memory_mib")),
                            figure(stat(m, "cpu_percent"), "%"),
                            figure(stat(m, "vms_running"), ""),
                            m.get("runs").and_then(Value::as_array).map_or(0, Vec::len).to_string(),
                        ]
                    })
                    .collect();
                ctx::table(&["name", "kind", "arch", "reachable", "cpus", "memory", "cpu", "vms", "runs"], &rows);
            });
            Ok(())
        }
        Cmd::Show { id } => {
            let list = ctx.client.get("/v1/machines")?;
            let node = items(&list)
                .find(|m| ctx::s(m, "id") == id.as_str() || ctx::s(m, "name") == id.as_str())
                .cloned()
                .ok_or_else(|| format!("no machine {id}: see `agentplaza machines list`"))?;
            ctx.out(&node, print_machine);
            Ok(())
        }
        Cmd::Kill { run, yes } => {
            confirm(&format!("Kill run {run}: its VMs stop."), yes)?;
            let killed = ctx.client.delete(&format!("/v1/runs/{}", ctx::seg(&run)))?;
            ctx.out(&killed, |_| println!("killed run {run}"));
            Ok(())
        }
    }
}

/// One figure from a machine's live stats (`-` when the node reports none).
fn stat<'a>(m: &'a Value, key: &str) -> Option<&'a Value> {
    m.get("stats").filter(|s| !s.is_null()).and_then(|s| s.get(key))
}

fn figure(v: Option<&Value>, unit: &str) -> String {
    match v.and_then(Value::as_f64) {
        Some(n) => format!("{n:.0}{unit}"),
        None => "-".into(),
    }
}

fn reachable(m: &Value) -> String {
    match m.get("reachable").and_then(Value::as_bool) {
        Some(true) => "yes".into(),
        Some(false) => "no".into(),
        None => "-".into(),
    }
}

fn print_machine(m: &Value) {
    let gpus: Vec<&str> = m.get("gpus").and_then(Value::as_array).into_iter().flatten().map(|g| ctx::s(g, "name")).collect();
    let load = stat(m, "load")
        .and_then(Value::as_array)
        .map(|l| l.iter().map(|n| format!("{:.2}", n.as_f64().unwrap_or(0.0))).collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|| "-".into());
    ctx::fields(&[
        ("id", ctx::s(m, "id").to_string()),
        ("name", ctx::s(m, "name").to_string()),
        ("kind", ctx::s(m, "kind").to_string()),
        ("arch", ctx::s(m, "arch").to_string()),
        ("backend", ctx::s(m, "backend").to_string()),
        ("cpus", ctx::text(m, "cpus")),
        ("memory", format!("{} MiB", ctx::text(m, "memory_mib"))),
        ("reachable", reachable(m)),
        ("gpus", if gpus.is_empty() { "none".into() } else { gpus.join(", ") }),
        ("cpu", figure(stat(m, "cpu_percent"), "%")),
        ("load", load),
        ("memory used", figure(stat(m, "mem_used_mib"), " MiB")),
        ("uptime", figure(stat(m, "uptime_s"), " s")),
        ("vms running", figure(stat(m, "vms_running"), "")),
        ("guest memory", figure(stat(m, "guest_mem_mib"), " MiB")),
    ]);
    if let Some(total) = stat(m, "plaza").and_then(|p| p.get("total")) {
        println!();
        ctx::fields(&[
            ("plaza cpu", figure(total.get("services_cpu_percent"), "%")),
            ("plaza memory", figure(total.get("services_rss_mib"), " MiB")),
            ("agents cpu", figure(total.get("vms_cpu_percent"), "%")),
            ("agents memory", figure(total.get("vms_rss_mib"), " MiB")),
        ]);
    }
    println!();
    let runs: Vec<Vec<String>> = m
        .get("runs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|r| {
            vec![
                ctx::s(r, "id").to_string(),
                ctx::s(r, "state").to_string(),
                none_if_empty(ctx::s(r, "role").to_string()),
                ctx::text(r, "vcpus"),
                ctx::text(r, "memory_mib"),
                r.get("vms").and_then(Value::as_array).map_or(0, Vec::len).to_string(),
            ]
        })
        .collect();
    ctx::table(&["run", "state", "role", "vcpus", "MiB", "vms"], &runs);
}
