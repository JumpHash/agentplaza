//! `agentplaza stations`: the workstations enrolled on the hub (docs/design/workstations.md, section 7): the
//! list and one station, rename, drain, revoke, join codes, leases, the settings with the takeover policy, the station
//! audit, the recordings, the work folder's sync and files, and the live screen (a
//! screenshot, a watch), the owner's takeover of a session from the terminal (`take`, `input`) and the workstation
//! VMs (`vms`). The submodules hold most of those; the machine's own side (`agentplaza workstation join`) is not here.

pub mod control;
pub mod files;
pub mod recordings;
pub mod screen;
pub mod vms;

use std::path::PathBuf;

use apz_station::{AccountMode, StationSettings, Takeover};
use clap::{Subcommand, ValueEnum};
use serde_json::{Value, json};

use crate::ctx::{self, Ctx, confirm, seg};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// List them: id, name, status, OS, account, leases, last seen.
    List,
    /// One station: its facts, its settings and its leases.
    Show {
        /// The station's id.
        id: String,
    },
    /// Rename a station.
    Rename {
        /// The station's id.
        id: String,
        /// Its new name.
        name: String,
    },
    /// Stop new leases on a station (the ones running keep going).
    Drain {
        /// The station's id.
        id: String,
    },
    /// Put a drained station back in service.
    Undrain {
        /// The station's id.
        id: String,
    },
    /// Revoke a station: its token stops working and its link is ended. Asks first on a terminal unless --yes.
    Revoke {
        /// The station's id.
        id: String,
        /// Revoke without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Join codes for workstations. Without an action: the list.
    Codes {
        #[command(subcommand)]
        action: Option<CodesCmd>,
    },
    /// A station's leases in queue order (position 1 is next).
    Leases {
        /// The station's id.
        id: String,
    },
    /// End one lease on a station.
    Release {
        /// The station's id.
        id: String,
        /// The lease's id (from `agentplaza stations leases`).
        lease: String,
    },
    /// A station's settings. `settings ID set` changes the ones its flags name; the others keep their values.
    Settings {
        /// The station's id.
        id: String,
        #[command(subcommand)]
        action: Option<SettingsCmd>,
    },
    /// The station audit, newest first: joins, drains, revokes, leases, settings and the owner's takeovers.
    Audit {
        /// Only this station's rows (default: every station's).
        #[arg(long)]
        station: Option<String>,
        /// How many rows (the hub keeps at most 1000).
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },
    /// The recorded leases, newest first; or `recordings show|frames|delete ID`, or `recordings defaults`.
    Recordings {
        #[command(subcommand)]
        action: Option<recordings::Action>,
        /// Only this station's recordings (default: every station's).
        #[arg(long)]
        station: Option<String>,
        /// How many (1 to 200; 50 by default).
        #[arg(long)]
        limit: Option<u32>,
        /// Only the recordings that started before this time (milliseconds).
        #[arg(long, value_name = "MS")]
        before: Option<u64>,
    },
    /// A station's work-folder sync: its state. `sync ID resume` resumes a paused sync.
    Sync {
        /// The station's id.
        id: String,
        #[command(subcommand)]
        action: Option<files::SyncCmd>,
    },
    /// A station's work folder: its files (under PREFIX), or `files ID get|put|rm|deliver PATH`.
    Files {
        /// The station's id.
        id: String,
        /// Only the files under this prefix.
        prefix: Option<String>,
        #[command(subcommand)]
        action: Option<files::FilesCmd>,
    },
    /// One screenshot of a session (its first live frame), saved to a file.
    Screenshot {
        /// The station's id.
        id: String,
        /// The session: a number, or shared (the station's own screen).
        #[arg(long, default_value = screen::DEFAULT_SESSION)]
        session: String,
        /// The picture's scale, from 0.1 to 1 (1: full size).
        #[arg(long, default_value_t = 1.0)]
        scale: f32,
        /// The file to write (overwritten).
        #[arg(short, long, value_name = "FILE")]
        out: PathBuf,
    },
    /// Watch a session live: a status line per frame, and with --save every frame written into a folder.
    Watch {
        /// The station's id.
        id: String,
        /// The session: a number, or shared (the station's own screen).
        #[arg(long, default_value = screen::DEFAULT_SESSION)]
        session: String,
        /// Frames per second, from 1 to 15.
        #[arg(long, default_value_t = 5)]
        fps: u8,
        /// A folder to save every frame into.
        #[arg(long, value_name = "DIR")]
        save: Option<PathBuf>,
    },
    /// Take over a session: the station's policy applies to the agent meanwhile. Then the commands on stdin, one a line:
    /// click X Y [left|right|middle] [double], move X Y, type TEXT, key CHORD, scroll DX DY [X Y] (positive DY scrolls
    /// down), drag X1 Y1 X2 Y2, give-back. Give-back or the end of input gives the session back.
    Take {
        /// The station's id.
        id: String,
        /// The session: a number, or shared (the station's own screen).
        #[arg(long, default_value = screen::DEFAULT_SESSION)]
        session: String,
    },
    /// One input on a session: take it, do the command (the words of `take`'s commands), and give it back. For example
    /// `input w1 click 640 360 right`, or `input w1 key ctrl+s`.
    Input {
        /// The station's id.
        id: String,
        /// The session: a number, or shared (the station's own screen).
        #[arg(long, default_value = screen::DEFAULT_SESSION)]
        session: String,
        /// The command and its arguments.
        #[arg(required = true, num_args = 1.., allow_hyphen_values = true, trailing_var_arg = true)]
        command: Vec<String>,
    },
    /// The workstation VMs the plaza makes on its nodes. Without an action: the list. `vms create` and `vms delete`.
    Vms {
        #[command(subcommand)]
        action: Option<vms::VmsCmd>,
    },
}

#[derive(Subcommand, Debug)]
pub enum CodesCmd {
    /// The join codes: state (active, used, expired, revoked), when made and when they end, and who used them.
    List,
    /// Make a join code (shown once). On the machine: `agentplaza workstation join <code>`.
    Create {
        /// What the workstation is called here.
        #[arg(long, default_value = "workstation")]
        label: String,
        /// Minutes the code stays valid.
        #[arg(long, default_value_t = 15)]
        ttl: u64,
    },
    /// Revoke a join code nobody has used yet: it can no longer join a machine.
    Revoke {
        /// The code's id (from `agentplaza stations codes`).
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum SettingsCmd {
    /// Change the settings the flags name; the others keep their values.
    Set(SetArgs),
}

/// The settings flags. Each one is optional: a flag not given keeps the station's value.
#[derive(clap::Args, Debug, Default, Clone, PartialEq, Eq)]
pub struct SetArgs {
    /// What happens when the owner takes over a pixel session.
    #[arg(long, value_enum)]
    pub takeover: Option<TakeoverArg>,
    /// Whether screen sessions are recorded.
    #[arg(long, value_enum)]
    pub record: Option<OnOff>,
    /// How many days recordings are kept.
    #[arg(long, value_name = "DAYS")]
    pub retention_days: Option<u32>,
    /// Frames per second a recording keeps, from 1 to 5.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u8).range(1..=5))]
    pub fps: Option<u8>,
    /// Whether the owner's takeovers are recorded too.
    #[arg(long, value_enum)]
    pub record_takeovers: Option<OnOff>,
    /// The work folder on the machine, under the agent's account, synced with the hub both ways.
    #[arg(long, value_name = "PATH")]
    pub work_folder: Option<String>,
    /// For VMs: restore a clean snapshot after each lease.
    #[arg(long, value_enum)]
    pub snapshot_reset: Option<OnOff>,
}

/// The takeover policy, as the flags spell it (`--takeover agent-waits`).
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakeoverArg {
    /// The agent waits and resumes when the owner gives the session back.
    AgentWaits,
    /// The agent's lease is released and its task goes back to the queue.
    ReleaseLease,
    /// The agent keeps working through shells, runs, files and the UI tree; only its pointer pauses.
    KeepWorking,
}

impl From<TakeoverArg> for Takeover {
    fn from(arg: TakeoverArg) -> Takeover {
        match arg {
            TakeoverArg::AgentWaits => Takeover::AgentWaits,
            TakeoverArg::ReleaseLease => Takeover::ReleaseLease,
            TakeoverArg::KeepWorking => Takeover::KeepWorkingPointerPaused,
        }
    }
}

/// A switch flag: `on` or `off`.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnOff {
    On,
    Off,
}

impl OnOff {
    fn is_on(self) -> bool {
        self == OnOff::On
    }
}

const LIST_HEAD: [&str; 7] = [
    "id",
    "name",
    "status",
    "os",
    "account",
    "leases",
    "last seen",
];
const CODE_HEAD: [&str; 6] = ["id", "label", "state", "created", "expires", "used"];
const LEASE_HEAD: [&str; 7] = ["id", "agent", "kind", "state", "queue", "task", "started"];
const AUDIT_HEAD: [&str; 6] = ["at", "actor", "station", "agent", "kind", "detail"];
/// The audit row's own columns; the rest of a row is its detail.
const AUDIT_COLUMNS: [&str; 5] = ["at_ms", "actor", "station", "agent", "kind"];

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List => {
            let stations = ctx.client.get("/v1/stations")?;
            ctx.out(&stations, |v| {
                let list = items(v);
                let rows: Vec<Vec<String>> = list.iter().map(row).collect();
                ctx::table(&LIST_HEAD, &rows);
                for station in list {
                    if let Some(warning) = account_warning(station) {
                        println!("{}  {warning}", ctx::s(station, "id"));
                    }
                }
            });
        }
        Cmd::Show { id } => {
            let station = ctx.client.get(&format!("/v1/stations/{}", seg(&id)))?;
            ctx.out(&station, |s| {
                if let Some(warning) = account_warning(s) {
                    println!("{warning}\n");
                }
                ctx::fields(&station_fields(s));
                println!();
                ctx::fields(&settings_fields(&s["settings"]));
                println!();
                println!("leases");
                lease_table(leases(s));
            });
        }
        Cmd::Rename { id, name } => {
            let name = name.trim().to_string();
            if name.is_empty() {
                return Err("the name is empty".into());
            }
            let station = ctx.client.patch(
                &format!("/v1/stations/{}", seg(&id)),
                &json!({ "name": name }),
            )?;
            ctx.out(&station, |v| println!("{id} is now {}", name_or(v, &id)));
        }
        Cmd::Drain { id } => {
            let station = ctx
                .client
                .post(&format!("/v1/stations/{}/drain", seg(&id)), &json!({}))?;
            ctx.out(&station, |v| println!("{} is draining", name_or(v, &id)));
        }
        Cmd::Undrain { id } => {
            let station = ctx
                .client
                .post(&format!("/v1/stations/{}/undrain", seg(&id)), &json!({}))?;
            ctx.out(&station, |v| {
                println!("{} is back in service", name_or(v, &id))
            });
        }
        Cmd::Revoke { id, yes } => {
            if !confirm(
                yes,
                &format!("Revoke workstation {id}? Its token stops working and its link is ended."),
            )? {
                println!("not revoked");
                return Ok(());
            }
            let station = ctx
                .client
                .post(&format!("/v1/stations/{}/revoke", seg(&id)), &json!({}))?;
            ctx.out(&station, |v| println!("{} revoked", name_or(v, &id)));
        }
        Cmd::Codes { action } => match action.unwrap_or(CodesCmd::List) {
            CodesCmd::List => {
                let codes = ctx.client.get("/v1/stations/codes")?;
                ctx.out(&codes, |v| {
                    let rows: Vec<Vec<String>> = items(v).iter().map(code_row).collect();
                    ctx::table(&CODE_HEAD, &rows);
                });
            }
            CodesCmd::Create { label, ttl } => {
                let made = ctx.client.post(
                    "/v1/stations/codes",
                    &json!({ "label": label, "ttl_minutes": ttl }),
                )?;
                ctx.out(&made, |v| {
                    let token = ctx::s(v, "token");
                    println!("join code for {label}, valid {ttl} minutes (shown once):");
                    println!("  {token}");
                    println!("on the machine: agentplaza workstation join {token}");
                });
            }
            CodesCmd::Revoke { id } => {
                let revoked = ctx
                    .client
                    .delete(&format!("/v1/stations/codes/{}", seg(&id)))?;
                ctx.out(&revoked, |_| println!("join code {id} revoked"));
            }
        },
        Cmd::Leases { id } => {
            let leases = ctx
                .client
                .get(&format!("/v1/stations/{}/leases", seg(&id)))?;
            ctx.out(&leases, |v| lease_table(items(v)));
        }
        Cmd::Release { id, lease } => {
            let path = format!("/v1/stations/{}/leases/{}", seg(&id), seg(&lease));
            let released = ctx.client.delete(&path)?;
            ctx.out(&released, |_| println!("lease {lease} released on {id}"));
        }
        Cmd::Settings { id, action: None } => {
            let settings = ctx
                .client
                .get(&format!("/v1/stations/{}/settings", seg(&id)))?;
            ctx.out(&settings, |v| ctx::fields(&settings_fields(v)));
        }
        Cmd::Settings {
            id,
            action: Some(SettingsCmd::Set(set)),
        } => {
            check_set(&set)?;
            let path = format!("/v1/stations/{}/settings", seg(&id));
            // The merge is done here, on the hub's current values: the PUT carries every setting.
            let current: StationSettings = serde_json::from_value(ctx.client.get(&path)?)
                .map_err(|e| format!("the hub's settings: {e}"))?;
            let body = serde_json::to_value(merged(current, &set)).map_err(|e| e.to_string())?;
            let answer = ctx.client.put(&path, &body)?;
            ctx.out(&answer, |a| {
                ctx::fields(&settings_fields(&a["settings"]));
                let mut pushed = vec![("pushed", yes_no(&a["pushed"]))];
                let error = ctx::text(a, "push_error");
                if !error.is_empty() {
                    pushed.push(("push error", error));
                }
                println!();
                ctx::fields(&pushed);
            });
        }
        Cmd::Audit { station, limit } => {
            let query = match &station {
                Some(station) => format!("station={}&limit={limit}", seg(station)),
                None => format!("limit={limit}"),
            };
            let rows = ctx.client.get(&format!("/v1/stations/audit?{query}"))?;
            ctx.out(&rows, |v| {
                let rows: Vec<Vec<String>> = items(v).iter().map(audit_row).collect();
                ctx::table(&AUDIT_HEAD, &rows);
            });
        }
        Cmd::Recordings {
            action,
            station,
            limit,
            before,
        } => {
            recordings::run(ctx, station.as_deref(), limit, before, action)?;
        }
        Cmd::Sync { id, action } => files::sync(ctx, &id, action)?,
        Cmd::Files { id, prefix, action } => files::files(ctx, &id, prefix.as_deref(), action)?,
        Cmd::Screenshot {
            id,
            session,
            scale,
            out,
        } => {
            screen::check_session(&session)?;
            screen::check_scale(scale)?;
            screen::screenshot(ctx, &id, &session, scale, &out)?;
        }
        Cmd::Watch {
            id,
            session,
            fps,
            save,
        } => {
            screen::check_session(&session)?;
            screen::check_fps(fps)?;
            screen::watch(ctx, &id, &session, fps, save.as_deref())?;
        }
        Cmd::Take { id, session } => control::take(ctx, &id, &session)?,
        Cmd::Input {
            id,
            session,
            command,
        } => control::input(ctx, &id, &session, &command.join(" "))?,
        Cmd::Vms { action } => vms::run(ctx, action)?,
    }
    Ok(())
}

/// The settings with the flags applied: a flag that is not given keeps the station's value.
pub fn merged(mut current: StationSettings, set: &SetArgs) -> StationSettings {
    if let Some(policy) = set.takeover {
        current.takeover = policy.into();
    }
    if let Some(on) = set.record {
        current.record = on.is_on();
    }
    if let Some(days) = set.retention_days {
        current.retention_days = days;
    }
    if let Some(fps) = set.fps {
        current.record_fps = fps;
    }
    if let Some(on) = set.record_takeovers {
        current.record_takeovers = on.is_on();
    }
    if let Some(folder) = &set.work_folder {
        current.work_folder = folder.trim().to_string();
    }
    if let Some(on) = set.snapshot_reset {
        current.snapshot_reset = on.is_on();
    }
    current
}

/// Refuses a `settings set` that changes nothing or names an empty work folder, before the hub is asked.
pub fn check_set(set: &SetArgs) -> Result<(), String> {
    if *set == SetArgs::default() {
        return Err("nothing to set: give at least one setting flag (see `agentplaza stations settings --help`)".into());
    }
    if set
        .work_folder
        .as_deref()
        .is_some_and(|folder| folder.trim().is_empty())
    {
        return Err("the work folder is empty".into());
    }
    Ok(())
}

/// The station's identity, machine and account, as `stations show` prints them.
pub fn station_fields(s: &Value) -> Vec<(&'static str, String)> {
    let f = &s["facts"];
    let gpus: Vec<&str> = f
        .get("gpus")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|g| g.get("name").and_then(Value::as_str))
        .collect();
    let displays = f
        .get("displays")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    vec![
        ("id", ctx::text(s, "id")),
        ("name", ctx::text(s, "name")),
        ("status", ctx::text(s, "status")),
        ("address", dash(ctx::text(s, "address"))),
        ("version", dash(ctx::text(s, "version"))),
        ("code", dash(ctx::text(s, "code"))),
        ("joined", ctx::when(&s["joined_ms"])),
        ("last seen", last_seen(s)),
        (
            "os",
            dash(
                format!("{} {}", ctx::text(f, "os"), ctx::text(f, "os_version"))
                    .trim()
                    .to_string(),
            ),
        ),
        ("arch", dash(ctx::text(f, "arch"))),
        ("form", dash(ctx::text(f, "form"))),
        ("hostname", dash(ctx::text(f, "hostname"))),
        ("cpus", dash(ctx::text(f, "cpus"))),
        ("memory", format!("{} MiB", ctx::text(f, "ram_mb"))),
        (
            "gpus",
            if gpus.is_empty() {
                "none".to_string()
            } else {
                gpus.join(", ")
            },
        ),
        ("displays", displays.to_string()),
        ("account", account(s)),
        ("apps", dash(ctx::text(f, "apps"))),
        ("virtual displays", yes_no(&f["virtual_displays"])),
        ("ui tree", yes_no(&f["ui_tree"])),
    ]
}

/// The station's settings as `settings` prints them: the takeover policy in its flag's spelling.
pub fn settings_fields(s: &Value) -> Vec<(&'static str, String)> {
    vec![
        ("takeover", takeover_word(ctx::s(s, "takeover")).to_string()),
        ("record", on_off(&s["record"])),
        (
            "retention",
            format!("{} days", ctx::text(s, "retention_days")),
        ),
        ("record fps", ctx::text(s, "record_fps")),
        ("takeovers recorded", on_off(&s["record_takeovers"])),
        ("work folder", ctx::text(s, "work_folder")),
        ("snapshot reset", on_off(&s["snapshot_reset"])),
    ]
}

/// The warning shown wherever a station's agents act as the owner's own account: "agents act as <name>".
pub fn account_warning(s: &Value) -> Option<String> {
    match account_mode(s)? {
        AccountMode::Owner { name } => Some(format!(
            "warning: agents act as {name} (the owner's own account)"
        )),
        AccountMode::AgentUser { .. } => None,
    }
}

/// One station's row in the list: id, name, status, OS, account, lease count, last seen.
pub fn row(s: &Value) -> Vec<String> {
    vec![
        ctx::text(s, "id"),
        ctx::text(s, "name"),
        ctx::text(s, "status"),
        dash(ctx::text(&s["facts"], "os")),
        account(s),
        leases(s).len().to_string(),
        last_seen(s),
    ]
}

/// One lease's cells: id, agent (its CID), kind, state, queue position, task, started.
pub fn lease_row(l: &Value) -> Vec<String> {
    vec![
        ctx::text(l, "id"),
        ctx::text(l, "agent"),
        ctx::text(l, "kind"),
        ctx::text(l, "state"),
        l.get("position")
            .and_then(Value::as_u64)
            .map_or_else(|| "-".to_string(), |p| p.to_string()),
        dash(ctx::text(l, "task")),
        ctx::when(&l["started_ms"]),
    ]
}

/// A station's leases as the hub lists them (empty when it has none).
pub fn leases(s: &Value) -> &[Value] {
    s.get("leases")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn lease_table(leases: &[Value]) {
    let rows: Vec<Vec<String>> = leases.iter().map(lease_row).collect();
    ctx::table(&LEASE_HEAD, &rows);
}

fn code_row(c: &Value) -> Vec<String> {
    vec![
        ctx::text(c, "id"),
        ctx::text(c, "label"),
        ctx::text(c, "state"),
        ctx::when(&c["created_ms"]),
        ctx::when(&c["expires_ms"]),
        dash(ctx::text(c, "used_by")),
    ]
}

fn audit_row(r: &Value) -> Vec<String> {
    vec![
        ctx::when(&r["at_ms"]),
        ctx::text(r, "actor"),
        dash(ctx::text(r, "station")),
        dash(ctx::text(r, "agent")),
        ctx::text(r, "kind"),
        detail(r),
    ]
}

/// An audit row's fields beyond its columns, as `key=value` words.
fn detail(r: &Value) -> String {
    let Some(map) = r.as_object() else {
        return String::new();
    };
    map.iter()
        .filter(|(key, value)| !AUDIT_COLUMNS.contains(&key.as_str()) && !value.is_null())
        .map(|(key, value)| format!("{key}={}", plain(value)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn plain(v: &Value) -> String {
    match v {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Whose account the station's agents act as, typed (None when the facts carry none the crate can read).
fn account_mode(s: &Value) -> Option<AccountMode> {
    serde_json::from_value(s.get("facts")?.get("account")?.clone()).ok()
}

/// Whose account the station's agents act as: the agents' own account, or the owner's (marked).
fn account(s: &Value) -> String {
    match account_mode(s) {
        Some(AccountMode::AgentUser { name }) => name,
        Some(AccountMode::Owner { name }) => format!("{name} (owner)"),
        None => "-".to_string(),
    }
}

/// "never" while the station has not spoken; else how long ago its link last did.
fn last_seen(s: &Value) -> String {
    match s.get("last_seen_ms").and_then(Value::as_u64) {
        None | Some(0) => "never".to_string(),
        Some(_) => ctx::ago(&s["last_seen_ms"]),
    }
}

/// The hub's takeover word as the flag spells it.
fn takeover_word(hub: &str) -> &str {
    match hub {
        "agent_waits" => "agent-waits",
        "release_lease" => "release-lease",
        "keep_working_pointer_paused" => "keep-working",
        other => other,
    }
}

fn items(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn name_or(v: &Value, id: &str) -> String {
    let name = ctx::s(v, "name");
    if name.is_empty() {
        id.to_string()
    } else {
        name.to_string()
    }
}

fn dash(text: String) -> String {
    if text.is_empty() {
        "-".to_string()
    } else {
        text
    }
}

fn on_off(v: &Value) -> String {
    let word = match v.as_bool() {
        Some(true) => "on",
        Some(false) => "off",
        None => "-",
    };
    word.to_string()
}

fn yes_no(v: &Value) -> String {
    let word = match v.as_bool() {
        Some(true) => "yes",
        Some(false) => "no",
        None => "-",
    };
    word.to_string()
}

#[cfg(test)]
mod tests;
