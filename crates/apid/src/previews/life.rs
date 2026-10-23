//! The lifecycle of a preview (PLAN 18.1): started by the CI recipe's `preview` step, asleep when idle, awake on the
//! next request, and closed when its lane has ended and the project's grace has passed. Every VM call is noded's, made
//! through a [`Host`] (the hub's own calls in production, a stand-in in tests).

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use crate::base_recipe;
use crate::base_vm::{Fork, Host, Live};
use crate::nodes;
use crate::previews::{ASLEEP, AWAKE, CLOSED, Preview, Store};
use crate::project_stack::{self, DEFAULT_GRACE_HOURS, Stack, Target, TargetKind};
use crate::state::App;
use crate::{base_builds, placement};

/// A preview sleeps after this long without a request (15 minutes).
pub const IDLE_SLEEP_MS: u64 = 15 * 60 * 1000;
/// How often the lifecycle sweep runs.
pub const SWEEP_EVERY: Duration = Duration::from_secs(60);
const HOUR_MS: u64 = 3_600_000;

/// What the lifecycle and the routes need: the preview store, the nodes (their tunnels) and noded's calls.
pub struct Ctx<'a, H: Host> {
    pub store: &'a Store,
    pub nodes: &'a nodes::Store,
    pub host: &'a H,
}

/// A lane whose preview starts: its project, its change, the revision built and the repository it is checked out from.
pub struct Lane<'a> {
    pub project: &'a str,
    pub lane: &'a str,
    pub rev: &'a str,
    pub repo: &'a str,
}

/// The CI recipe's preview step for `lane`, on the project's preview base. It picks a node of the project's pool,
/// starts the lane's app there, and returns the summary the step records. A project with no preview target has nothing
/// to start.
pub fn start(app: &App, lane: &Lane, stack: &Stack, now: u64) -> Result<String, String> {
    let Some(target) = stack.targets.iter().find(|t| t.kind == TargetKind::Preview) else {
        return Ok("no preview target: nothing to start".to_string());
    };
    let pool = base_builds::pool_keys(stack)
        .first()
        .cloned()
        .unwrap_or_default();
    let node = placement::base_node(app, lane.project, &pool, &base_recipe::volumes(stack))
        .ok_or_else(|| format!("no node in the pool {pool} has room for a preview"))?;
    let host = Live(app);
    let ctx = Ctx {
        store: &app.previews,
        nodes: &app.nodes,
        host: &host,
    };
    start_on(&ctx, &node, lane, stack, target, now)
}

/// Starts the lane's preview on `node`: the preview base's steps in a fork (the app built at the lane's revision,
/// started, listening), its port registered with noded, the lane's earlier previews closed, and the new preview recorded
/// awake.
pub fn start_on<H: Host>(
    ctx: &Ctx<H>,
    node: &str,
    lane: &Lane,
    stack: &Stack,
    target: &Target,
    now: u64,
) -> Result<String, String> {
    let (project, name) = (lane.project, lane.lane);
    let (serve, port) = match (
        target
            .serve
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty()),
        target.port.filter(|port| *port != 0),
    ) {
        (Some(serve), Some(port)) => (serve, port),
        _ => {
            return Err(format!(
                "preview target {} needs a serve command and a port",
                target.name
            ));
        }
    };
    let steps = base_recipe::preview_steps(stack, lane.repo, lane.rev, serve, port);
    let fork = Fork::start(ctx.host, node.to_string(), project.to_string(), &steps)?;
    let Some((run, cid)) = fork.location() else {
        return Err("the preview's fork has no VM".to_string());
    };
    let registered = ctx.host.noded(
        node,
        "POST",
        &format!("/v1/runs/{run}/ports"),
        Some(&json!({"cid": cid, "port": port})),
    );
    match registered {
        Ok((200, _)) => {}
        other => {
            let _ = fork.retire();
            return Err(format!(
                "noded did not register the preview's port: {other:?}"
            ));
        }
    }
    for mut earlier in ctx
        .store
        .previews(Some(project))
        .into_iter()
        .filter(|p| p.lane == name && p.status != CLOSED)
    {
        if let Err(why) = close(ctx, &mut earlier) {
            eprintln!(
                "previews: closing the lane's earlier preview {}: {why}",
                earlier.id
            );
        }
    }
    let preview = ctx
        .store
        .add_preview(project, name, node, &run, cid, u32::from(port), now)
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "preview {} of lane {name} serves port {port} on {node} (run {run})",
        preview.id
    ))
}

/// Sleeps an awake preview: noded takes a snapshot of its VM and stops it. A preview already asleep stays so.
pub fn sleep<H: Host>(ctx: &Ctx<H>, preview: &mut Preview) -> Result<(), String> {
    if preview.status != AWAKE {
        return Ok(());
    }
    vm_call(ctx, preview, "sleep")?;
    preview.status = ASLEEP.to_string();
    ctx.store.save_preview(preview).map_err(|e| e.to_string())
}

/// Wakes a sleeping preview: noded restores its VM from the snapshot. An awake preview stays awake.
pub fn wake<H: Host>(ctx: &Ctx<H>, preview: &mut Preview) -> Result<(), String> {
    if preview.status != ASLEEP {
        return Ok(());
    }
    vm_call(ctx, preview, "wake")?;
    preview.status = AWAKE.to_string();
    ctx.store.save_preview(preview).map_err(|e| e.to_string())
}

/// Closes a preview for good: its port is unregistered with noded, its VM stopped, and the row marked closed.
pub fn close<H: Host>(ctx: &Ctx<H>, preview: &mut Preview) -> Result<(), String> {
    if preview.status == CLOSED {
        return Ok(());
    }
    let node = preview.node.clone();
    let unregistered = ctx.host.noded(
        &node,
        "DELETE",
        &format!(
            "/v1/runs/{}/ports/{}/{}",
            preview.run, preview.cid, preview.port
        ),
        None,
    );
    let stopped = ctx.host.noded(
        &node,
        "POST",
        &format!("/v1/runs/{}/vms/0/stop", preview.run),
        None,
    );
    preview.status = CLOSED.to_string();
    ctx.store.save_preview(preview).map_err(|e| e.to_string())?;
    if let Err(why) = unregistered {
        eprintln!("previews: unregistering {}: {why}", preview.id);
    }
    match stopped {
        Ok((status, _)) if status < 300 => Ok(()),
        Ok((status, reply)) => Err(format!(
            "noded: status {status} stopping the preview: {reply}"
        )),
        Err(why) => Err(why),
    }
}

fn vm_call<H: Host>(ctx: &Ctx<H>, preview: &Preview, action: &str) -> Result<(), String> {
    let path = format!("/v1/runs/{}/vms/0/{action}", preview.run);
    let (status, reply) = ctx.host.noded(&preview.node, "POST", &path, None)?;
    if status >= 300 {
        return Err(format!("noded: status {status} on {action}: {reply}"));
    }
    Ok(())
}

/// One pass of the lifecycle over the open previews. A preview whose lane has ended (landed or abandoned) gets its end:
/// the lane's end plus the project's grace. A preview whose end has come is closed. An awake preview idle for
/// [`IDLE_SLEEP_MS`] sleeps. Returns what it did, one line each, for the log.
pub fn settle<H: Host>(
    ctx: &Ctx<H>,
    now: u64,
    lane_ended: &dyn Fn(&str, &str) -> bool,
    grace_hours: &dyn Fn(&str) -> u64,
) -> Vec<String> {
    let mut notes = Vec::new();
    for mut preview in ctx.store.open_previews() {
        if preview.ends_ms.is_none() && lane_ended(&preview.project, &preview.lane) {
            preview.ends_ms = Some(now + grace_hours(&preview.project) * HOUR_MS);
            match ctx.store.save_preview(&preview) {
                Ok(()) => notes.push(format!(
                    "preview {}: lane {} has ended; it ends after the grace",
                    preview.id, preview.lane
                )),
                Err(why) => notes.push(format!("preview {}: {why}", preview.id)),
            }
        }
        if preview.ends_ms.is_some_and(|at| now >= at) {
            match close(ctx, &mut preview) {
                Ok(()) => notes.push(format!("preview {}: closed", preview.id)),
                Err(why) => notes.push(format!("preview {}: closing: {why}", preview.id)),
            }
        } else if preview.status == AWAKE && now.saturating_sub(preview.used_ms) >= IDLE_SLEEP_MS {
            match sleep(ctx, &mut preview) {
                Ok(()) => notes.push(format!("preview {}: asleep when idle", preview.id)),
                Err(why) => notes.push(format!("preview {}: sleeping: {why}", preview.id)),
            }
        }
    }
    notes
}

/// The hub's lifecycle thread: one pass every [`SWEEP_EVERY`].
pub fn spawn_sweeper(app: Arc<App>) {
    std::thread::spawn(move || {
        loop {
            sweep(&app);
            std::thread::sleep(SWEEP_EVERY);
        }
    });
}

fn sweep(app: &App) {
    let host = Live(app);
    let ctx = Ctx {
        store: &app.previews,
        nodes: &app.nodes,
        host: &host,
    };
    let ended = |_: &str, lane: &str| lane_ended(app, lane);
    let grace = |project: &str| grace_hours(app, project);
    for note in settle(&ctx, apz_plaza::now_ms(), &ended, &grace) {
        eprintln!("previews: {note}");
    }
}

/// Whether lane `lane` has landed or been abandoned in Weft.
pub fn lane_ended(app: &App, lane: &str) -> bool {
    app.weft
        .get(&format!("/changes/{lane}"))
        .is_ok_and(|detail| detail["state"] == "landed" || detail["state"] == "abandoned")
}

/// The grace a project's previews keep after their lane ends: the preview target's `grace_hours`, 24 by default.
pub fn grace_hours(app: &App, project: &str) -> u64 {
    project_stack::read(app, project)
        .ok()
        .flatten()
        .and_then(|stack| {
            stack
                .targets
                .iter()
                .find(|t| t.kind == TargetKind::Preview)
                .and_then(|t| t.grace_hours)
        })
        .unwrap_or(DEFAULT_GRACE_HOURS)
}
