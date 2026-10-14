//! `agentplaza stations`: the workstations enrolled on the hub (docs/design/workstations.md, section 7): the
//! list and one station, rename, drain, revoke, join codes, leases, the settings with the takeover policy, and
//! the station audit. The machine's own side (`agentplaza workstation join`) is not here.

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
    /// Frames per second a recording keeps.
    #[arg(long, value_name = "N")]
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

const LIST_HEAD: [&str; 7] = ["id", "name", "status", "os", "account", "leases", "last seen"];
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
            let station = ctx.client.patch(&format!("/v1/stations/{}", seg(&id)), &json!({ "name": name }))?;
            ctx.out(&station, |v| println!("{id} is now {}", name_or(v, &id)));
        }
        Cmd::Drain { id } => {
            let station = ctx.client.post(&format!("/v1/stations/{}/drain", seg(&id)), &json!({}))?;
            ctx.out(&station, |v| println!("{} is draining", name_or(v, &id)));
        }
        Cmd::Undrain { id } => {
            let station = ctx.client.post(&format!("/v1/stations/{}/undrain", seg(&id)), &json!({}))?;
            ctx.out(&station, |v| println!("{} is back in service", name_or(v, &id)));
        }
        Cmd::Revoke { id, yes } => {
            if !confirm(
                yes,
                &format!("Revoke workstation {id}? Its token stops working and its link is ended."),
            )? {
                println!("not revoked");
                return Ok(());
            }
            let station = ctx.client.post(&format!("/v1/stations/{}/revoke", seg(&id)), &json!({}))?;
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
                let made = ctx
                    .client
                    .post("/v1/stations/codes", &json!({ "label": label, "ttl_minutes": ttl }))?;
                ctx.out(&made, |v| {
                    let token = ctx::s(v, "token");
                    println!("join code for {label}, valid {ttl} minutes (shown once):");
                    println!("  {token}");
                    println!("on the machine: agentplaza workstation join {token}");
                });
            }
            CodesCmd::Revoke { id } => {
                let revoked = ctx.client.delete(&format!("/v1/stations/codes/{}", seg(&id)))?;
                ctx.out(&revoked, |_| println!("join code {id} revoked"));
            }
        },
        Cmd::Leases { id } => {
            let leases = ctx.client.get(&format!("/v1/stations/{}/leases", seg(&id)))?;
            ctx.out(&leases, |v| lease_table(items(v)));
        }
        Cmd::Release { id, lease } => {
            let path = format!("/v1/stations/{}/leases/{}", seg(&id), seg(&lease));
            let released = ctx.client.delete(&path)?;
            ctx.out(&released, |_| println!("lease {lease} released on {id}"));
        }
        Cmd::Settings { id, action: None } => {
            let settings = ctx.client.get(&format!("/v1/stations/{}/settings", seg(&id)))?;
            ctx.out(&settings, |v| ctx::fields(&settings_fields(v)));
        }
        Cmd::Settings {
            id,
            action: Some(SettingsCmd::Set(set)),
        } => {
            check_set(&set)?;
            let path = format!("/v1/stations/{}/settings", seg(&id));
            // The merge is done here, on the hub's current values: the PUT carries every setting.
            let current: StationSettings =
                serde_json::from_value(ctx.client.get(&path)?).map_err(|e| format!("the hub's settings: {e}"))?;
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
    if set.work_folder.as_deref().is_some_and(|folder| folder.trim().is_empty()) {
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
    let displays = f.get("displays").and_then(Value::as_array).map_or(0, Vec::len);
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
            dash(format!("{} {}", ctx::text(f, "os"), ctx::text(f, "os_version")).trim().to_string()),
        ),
        ("arch", dash(ctx::text(f, "arch"))),
        ("form", dash(ctx::text(f, "form"))),
        ("hostname", dash(ctx::text(f, "hostname"))),
        ("cpus", dash(ctx::text(f, "cpus"))),
        ("memory", format!("{} MiB", ctx::text(f, "ram_mb"))),
        ("gpus", if gpus.is_empty() { "none".to_string() } else { gpus.join(", ") }),
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
        ("retention", format!("{} days", ctx::text(s, "retention_days"))),
        ("record fps", ctx::text(s, "record_fps")),
        ("takeovers recorded", on_off(&s["record_takeovers"])),
        ("work folder", ctx::text(s, "work_folder")),
        ("snapshot reset", on_off(&s["snapshot_reset"])),
    ]
}

/// The warning shown wherever a station's agents act as the owner's own account: "agents act as <name>".
pub fn account_warning(s: &Value) -> Option<String> {
    match account_mode(s)? {
        AccountMode::Owner { name } => Some(format!("warning: agents act as {name} (the owner's own account)")),
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
    s.get("leases").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
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
    let Some(map) = r.as_object() else { return String::new() };
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
    if name.is_empty() { id.to_string() } else { name.to_string() }
}

fn dash(text: String) -> String {
    if text.is_empty() { "-".to_string() } else { text }
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
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    /// The subcommand alone, so each test reads as `agentplaza stations <args>`.
    #[derive(Parser, Debug)]
    struct Probe {
        #[command(subcommand)]
        cmd: Cmd,
    }

    fn parse(args: &[&str]) -> Result<Cmd, String> {
        Probe::try_parse_from(std::iter::once("stations").chain(args.iter().copied()))
            .map(|p| p.cmd)
            .map_err(|e| e.to_string())
    }

    fn parsed(args: &[&str]) -> Cmd {
        parse(args).unwrap_or_else(|e| panic!("{args:?} should parse: {e}"))
    }

    fn all_settings() -> SetArgs {
        SetArgs {
            takeover: Some(TakeoverArg::KeepWorking),
            record: Some(OnOff::Off),
            retention_days: Some(7),
            fps: Some(1),
            record_takeovers: Some(OnOff::On),
            work_folder: Some("/srv/work".into()),
            snapshot_reset: Some(OnOff::On),
        }
    }

    #[test]
    fn the_command_definition_is_valid() {
        Probe::command().debug_assert();
    }

    #[test]
    fn list_show_rename_drain_undrain_and_revoke_parse() {
        assert!(matches!(parsed(&["list"]), Cmd::List));
        let Cmd::Show { id } = parsed(&["show", "w1"]) else {
            panic!("show")
        };
        assert_eq!(id, "w1");
        let Cmd::Rename { id, name } = parsed(&["rename", "w1", "Lab box"]) else {
            panic!("rename")
        };
        assert_eq!((id.as_str(), name.as_str()), ("w1", "Lab box"));
        let Cmd::Drain { id } = parsed(&["drain", "w1"]) else {
            panic!("drain")
        };
        assert_eq!(id, "w1");
        let Cmd::Undrain { id } = parsed(&["undrain", "w1"]) else {
            panic!("undrain")
        };
        assert_eq!(id, "w1");
        let Cmd::Revoke { id, yes } = parsed(&["revoke", "w1", "--yes"]) else {
            panic!("revoke --yes")
        };
        assert_eq!((id.as_str(), yes), ("w1", true));
        let Cmd::Revoke { yes, .. } = parsed(&["revoke", "w1"]) else {
            panic!("revoke")
        };
        assert!(!yes);
    }

    #[test]
    fn codes_parse_with_list_as_the_default() {
        assert!(matches!(parsed(&["codes"]), Cmd::Codes { action: None }));
        assert!(matches!(
            parsed(&["codes", "list"]),
            Cmd::Codes {
                action: Some(CodesCmd::List)
            }
        ));
        let Cmd::Codes {
            action: Some(CodesCmd::Create { label, ttl }),
        } = parsed(&["codes", "create", "--label", "gpu-box", "--ttl", "30"])
        else {
            panic!("create")
        };
        assert_eq!((label.as_str(), ttl), ("gpu-box", 30));
        let Cmd::Codes {
            action: Some(CodesCmd::Create { label, ttl }),
        } = parsed(&["codes", "create"])
        else {
            panic!("create defaults")
        };
        assert_eq!((label.as_str(), ttl), ("workstation", 15));
        let Cmd::Codes {
            action: Some(CodesCmd::Revoke { id }),
        } = parsed(&["codes", "revoke", "j3"])
        else {
            panic!("revoke")
        };
        assert_eq!(id, "j3");
    }

    #[test]
    fn leases_and_release_parse() {
        let Cmd::Leases { id } = parsed(&["leases", "w1"]) else {
            panic!("leases")
        };
        assert_eq!(id, "w1");
        let Cmd::Release { id, lease } = parsed(&["release", "w1", "l4"]) else {
            panic!("release")
        };
        assert_eq!((id.as_str(), lease.as_str()), ("w1", "l4"));
    }

    #[test]
    fn settings_show_and_set_parse_every_flag() {
        let Cmd::Settings { id, action: None } = parsed(&["settings", "w1"]) else {
            panic!("settings show")
        };
        assert_eq!(id, "w1");
        let Cmd::Settings {
            id,
            action: Some(SettingsCmd::Set(set)),
        } = parsed(&[
            "settings",
            "w1",
            "set",
            "--takeover",
            "keep-working",
            "--record",
            "off",
            "--retention-days",
            "7",
            "--fps",
            "1",
            "--record-takeovers",
            "on",
            "--work-folder",
            "/srv/work",
            "--snapshot-reset",
            "on",
        ])
        else {
            panic!("settings set")
        };
        assert_eq!(id, "w1");
        assert_eq!(set, all_settings());
    }

    #[test]
    fn each_takeover_policy_has_its_flag_spelling() {
        for (word, policy) in [
            ("agent-waits", TakeoverArg::AgentWaits),
            ("release-lease", TakeoverArg::ReleaseLease),
            ("keep-working", TakeoverArg::KeepWorking),
        ] {
            let Cmd::Settings {
                action: Some(SettingsCmd::Set(set)),
                ..
            } = parsed(&["settings", "w1", "set", "--takeover", word])
            else {
                panic!("takeover {word}")
            };
            assert_eq!(set.takeover, Some(policy));
        }
        assert!(parse(&["settings", "w1", "set", "--takeover", "bogus"]).is_err());
    }

    #[test]
    fn audit_parses_with_its_defaults_and_filters() {
        let Cmd::Audit { station, limit } = parsed(&["audit"]) else {
            panic!("audit")
        };
        assert_eq!((station, limit), (None, 100));
        let Cmd::Audit { station, limit } = parsed(&["audit", "--station", "w1", "--limit", "5"]) else {
            panic!("audit filters")
        };
        assert_eq!((station.as_deref(), limit), (Some("w1"), 5));
    }

    #[test]
    fn ids_are_required_where_a_command_names_one() {
        let cases: [&[&str]; 5] = [
            &["show"],
            &["rename", "w1"],
            &["release", "w1"],
            &["codes", "revoke"],
            &["settings"],
        ];
        for args in cases {
            assert!(parse(args).is_err(), "{args:?} needs its id");
        }
    }

    #[test]
    fn flags_merge_onto_the_hubs_settings_and_leave_the_rest() {
        let current = StationSettings {
            record: true,
            retention_days: 30,
            ..StationSettings::default()
        };
        let set = SetArgs {
            takeover: Some(TakeoverArg::ReleaseLease),
            record: Some(OnOff::Off),
            fps: Some(3),
            ..SetArgs::default()
        };
        let merged = merged(current, &set);
        assert_eq!(merged.takeover, Takeover::ReleaseLease);
        assert!(!merged.record);
        assert_eq!(merged.record_fps, 3);
        assert_eq!(merged.retention_days, 30, "a flag not given keeps the station's value");
        assert_eq!(merged.work_folder, "~/AgentPlaza");
    }

    #[test]
    fn the_merged_settings_encode_as_the_hub_reads_them() {
        let body = serde_json::to_value(merged(StationSettings::default(), &all_settings())).unwrap();
        assert_eq!(
            body,
            json!({
                "takeover": "keep_working_pointer_paused",
                "record": false,
                "retention_days": 7,
                "record_fps": 1,
                "record_takeovers": true,
                "work_folder": "/srv/work",
                "snapshot_reset": true
            })
        );
    }

    #[test]
    fn a_set_with_no_flag_or_an_empty_folder_is_refused_before_the_hub() {
        assert!(check_set(&SetArgs::default()).is_err());
        let blank = SetArgs {
            work_folder: Some("  ".into()),
            ..SetArgs::default()
        };
        assert_eq!(check_set(&blank).unwrap_err(), "the work folder is empty");
        assert!(check_set(&all_settings()).is_ok());
    }

    #[test]
    fn a_station_row_shows_its_account_and_lease_count() {
        let station = json!({
            "id": "w1", "name": "lab", "status": "online", "last_seen_ms": 0,
            "facts": { "os": "linux", "account": { "mode": "owner", "name": "irfan" } },
            "leases": [{ "id": "l1" }, { "id": "l2" }]
        });
        assert_eq!(row(&station), ["w1", "lab", "online", "linux", "irfan (owner)", "2", "never"]);
        assert_eq!(
            account_warning(&station).as_deref(),
            Some("warning: agents act as irfan (the owner's own account)")
        );
        let agent = json!({ "facts": { "os": "macos", "account": { "mode": "agent_user", "name": "agentws" } } });
        assert_eq!(account(&agent), "agentws");
        assert_eq!(account_warning(&agent), None);
        assert_eq!(account(&json!({})), "-");
    }

    #[test]
    fn lease_and_code_rows_keep_the_hubs_words() {
        let lease = json!({ "id": "l4", "agent": 12, "kind": "pixel", "state": "queued", "position": 2, "task": null, "started_ms": 0 });
        assert_eq!(lease_row(&lease)[..5], ["l4", "12", "pixel", "queued", "2"]);
        assert_eq!(lease_row(&lease)[5], "-");
        let active = json!({ "id": "l5", "agent": 3, "kind": "shared", "state": "active" });
        assert_eq!(lease_row(&active)[4], "-", "an active lease has no queue position");
        let code = json!({ "id": "j1", "label": "gpu", "state": "used", "created_ms": 0, "expires_ms": 0, "used_by": "lab-1" });
        assert_eq!(code_row(&code)[2], "used");
        assert_eq!(code_row(&code)[5], "lab-1");
    }

    #[test]
    fn audit_rows_put_the_extra_fields_in_the_detail() {
        let row =
            json!({ "at_ms": 0, "actor": "owner", "station": "w1", "agent": null, "kind": "lease_granted", "lease": "l2", "session": 9 });
        let cells = audit_row(&row);
        assert_eq!(cells[1], "owner");
        assert_eq!(cells[3], "-");
        assert_eq!(cells[4], "lease_granted");
        assert_eq!(cells[5], "lease=l2 session=9");
    }

    #[test]
    fn settings_read_the_takeover_in_its_flag_spelling() {
        let fields = settings_fields(&json!({ "takeover": "release_lease", "record": true, "retention_days": 30, "record_fps": 2 }));
        assert_eq!(fields[0], ("takeover", "release-lease".to_string()));
        assert_eq!(fields[1], ("record", "on".to_string()));
        assert_eq!(fields[2], ("retention", "30 days".to_string()));
        assert_eq!(takeover_word("keep_working_pointer_paused"), "keep-working");
    }
}
