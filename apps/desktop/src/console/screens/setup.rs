//! Setup: the hub's checks (the web's Setup reads the same answers), the checks behind this console
//! (the hub, the node, the model keys, and this machine's requirements from the first-run checks),
//! and the guided setup and the tour again.

use iced::widget::{Space, column, row};
use iced::{Alignment, Element, Length, Task};
use serde_json::Value;

use super::Id;
use crate::console::onboard::{self, Status};
use crate::console::setup::Check;
use crate::console::ui::{self, Kind, Tone};
use crate::console::{Ctx, client};

#[derive(Debug, Default)]
pub struct State {
    pub status: Option<Status>,
    /// This machine's requirements; read once on the first open, then on "Check again".
    pub local: Option<Vec<Check>>,
    /// The hub's answers to the hub checks, each set when it arrives (None while it is in flight).
    overview: Option<Result<Value, String>>,
    identity: Option<Result<Value, String>>,
    node_link: Option<Result<Value, String>>,
    machine: Option<Result<Value, String>>,
    vault: Option<Result<Value, String>>,
    providers: Option<Result<Value, String>>,
    git: Option<Result<Value, String>>,
}

/// One of the hub reads behind the hub checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Overview,
    Identity,
    NodeLink,
    Machine,
    Vault,
    Providers,
    Git,
}

const SOURCES: [Source; 7] = [
    Source::Overview,
    Source::Identity,
    Source::NodeLink,
    Source::Machine,
    Source::Vault,
    Source::Providers,
    Source::Git,
];

#[derive(Debug, Clone)]
pub enum Msg {
    Check,
    Status(Status),
    Local(Vec<Check>),
    /// One of the hub reads behind the hub checks has answered.
    Fetched(Source, Result<Value, String>),
    /// Navigate to another screen (a check's fix).
    Go(Id),
    /// Run the setup wizard again (the console opens it).
    Wizard,
    /// Replay the tour (the console starts it).
    Tour,
}

pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let status = onboard::status_task(ctx).map(Msg::Status);
    let hub = fetch_all(ctx);
    if state.local.is_none() {
        Task::batch([status, onboard::local_task().map(Msg::Local), hub])
    } else {
        Task::batch([status, hub])
    }
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Check => {
            // The hub checks read "checking..." until their answers arrive again.
            for source in SOURCES.iter() {
                *slot_mut(state, *source) = None;
            }
            Task::batch([
                onboard::status_task(ctx).map(Msg::Status),
                onboard::local_task().map(Msg::Local),
                fetch_all(ctx),
            ])
        }
        Msg::Status(status) => {
            state.status = Some(status);
            Task::none()
        }
        Msg::Local(checks) => {
            state.local = Some(checks);
            Task::none()
        }
        Msg::Fetched(source, result) => {
            *slot_mut(state, source) = Some(result);
            Task::none()
        }
        // The console answers these (it owns the wizard and the tour), and `go` answers Go.
        Msg::Go(_) | Msg::Wizard | Msg::Tour => Task::none(),
    }
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let head = ui::view_head(
        onboard::copy_text("setup.title"),
        vec![ui::btn(
            onboard::copy_text("setup.check_again"),
            None,
            Kind::Ghost,
            Some(Msg::Check),
        )],
    );
    let intro = ui::faint(onboard::copy_text("setup.intro"));
    let rows = onboard::rows(state.status.as_ref(), state.local.as_deref())
        .into_iter()
        .map(|r| {
            row![
                ui::sans(r.label, 12.0, ui::INK_2),
                Space::new().width(Length::Fill),
                ui::badge(&r.detail, r.tone)
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into()
        });
    let checks = column(rows).spacing(10);
    let guided = row![
        ui::btn(
            onboard::copy_text("replay.setup"),
            None,
            Kind::Primary,
            Some(Msg::Wizard)
        ),
        ui::btn(
            onboard::copy_text("replay.tour"),
            None,
            Kind::Secondary,
            Some(Msg::Tour)
        ),
    ]
    .spacing(8);
    ui::sheet(
        head,
        column![
            intro,
            ui::card("Hub checks", hub_card(state)),
            ui::card(onboard::copy_text("wizard.steps.3.title"), checks.into()),
            guided
        ]
        .spacing(16)
        .into(),
    )
}

// ---- requests ----

fn path_of(source: Source) -> &'static str {
    match source {
        Source::Overview => "/v1/overview",
        Source::Identity => "/v1/hub/identity",
        Source::NodeLink => "/v1/node-link",
        Source::Machine => "/v1/machine",
        Source::Vault => "/v1/vault",
        Source::Providers => "/v1/providers",
        Source::Git => "/v1/git",
    }
}

fn slot(state: &State, source: Source) -> &Option<Result<Value, String>> {
    match source {
        Source::Overview => &state.overview,
        Source::Identity => &state.identity,
        Source::NodeLink => &state.node_link,
        Source::Machine => &state.machine,
        Source::Vault => &state.vault,
        Source::Providers => &state.providers,
        Source::Git => &state.git,
    }
}

fn slot_mut(state: &mut State, source: Source) -> &mut Option<Result<Value, String>> {
    match source {
        Source::Overview => &mut state.overview,
        Source::Identity => &mut state.identity,
        Source::NodeLink => &mut state.node_link,
        Source::Machine => &mut state.machine,
        Source::Vault => &mut state.vault,
        Source::Providers => &mut state.providers,
        Source::Git => &mut state.git,
    }
}

fn fetch(ctx: &Ctx, source: Source) -> Task<Msg> {
    let client = ctx.client.clone();
    let path = path_of(source);
    client::task(
        move || client.call("GET", path, None),
        move |result| Msg::Fetched(source, result),
    )
}

fn fetch_all(ctx: &Ctx) -> Task<Msg> {
    Task::batch(SOURCES.iter().map(|source| fetch(ctx, *source)))
}

// ---- the hub checks ----

/// A read's state: still in flight, failed, or answered.
enum Got<'a> {
    Pending,
    Failed(&'a str),
    Value(&'a Value),
}

fn got(slot: &Option<Result<Value, String>>) -> Got<'_> {
    match slot {
        None => Got::Pending,
        Some(Ok(value)) => Got::Value(value),
        Some(Err(message)) => Got::Failed(message.as_str()),
    }
}

/// One row of the hub checks: its label, what it found, its tone, and the screen its fix opens.
struct CheckRow {
    label: &'static str,
    tone: Tone,
    detail: String,
    fix: Option<(&'static str, Id)>,
}

fn check(
    label: &'static str,
    tone: Tone,
    detail: impl Into<String>,
    fix: Option<(&'static str, Id)>,
) -> CheckRow {
    CheckRow {
        label,
        tone,
        detail: detail.into(),
        fix,
    }
}

fn pending(label: &'static str) -> CheckRow {
    check(label, Tone::Muted, "checking...", None)
}

fn str_at<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn uint(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// The hub checks, in the web's order and with its wording.
fn hub_checks(state: &State) -> Vec<CheckRow> {
    let mut rows = Vec::new();

    match got(slot(state, Source::Overview)) {
        Got::Pending => rows.push(pending("Hub reachable")),
        Got::Value(overview) => {
            let agents = overview
                .get("agents")
                .and_then(|a| a.get("total"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            rows.push(check(
                "Hub reachable",
                Tone::Ok,
                format!(
                    "apid answers, {} on this plaza",
                    plural(agents as usize, "agent")
                ),
                None,
            ));
        }
        Got::Failed(message) if message.contains("401") => {
            rows.push(check(
                "Hub reachable",
                Tone::Warn,
                "the hub wants the owner signed in",
                None,
            ));
        }
        Got::Failed(_) => rows.push(check(
            "Hub reachable",
            Tone::Danger,
            "apid does not answer",
            None,
        )),
    }

    match got(slot(state, Source::Identity)) {
        Got::Pending => rows.push(pending("Hub identity")),
        // `GET /v1/hub/identity` is the identity itself: { name, version, url, fingerprint, nodes }.
        Got::Value(v) if v.get("name").is_some_and(Value::is_string) => {
            let identity = &v;
            let fingerprint = str_at(identity, "fingerprint");
            let nodes = uint(identity, "nodes");
            let who = format!(
                "{} {}",
                str_at(identity, "name"),
                str_at(identity, "version")
            );
            if !fingerprint.is_empty() || nodes == 0 {
                let wire = if fingerprint.is_empty() {
                    "plain HTTP"
                } else {
                    "HTTPS"
                };
                rows.push(check(
                    "Hub identity",
                    Tone::Ok,
                    format!("{who}, {wire}"),
                    None,
                ));
            } else {
                rows.push(check(
                    "Hub identity",
                    Tone::Warn,
                    format!(
                        "{who}, plain HTTP with {} joined",
                        plural(nodes as usize, "node")
                    ),
                    Some(("Other machines should join over HTTPS", Id::Nodes)),
                ));
            }
        }
        _ => rows.push(check(
            "Hub identity",
            Tone::Danger,
            "the hub does not say who it is",
            None,
        )),
    }

    match got(slot(state, Source::Overview)) {
        Got::Pending => {
            rows.push(pending("Hub service"));
            rows.push(pending("Node here"));
        }
        Got::Value(overview) => {
            rows.push(if flag(overview, "hub") {
                check(
                    "Hub service",
                    Tone::Ok,
                    "the hub serves the console and the API",
                    None,
                )
            } else {
                check(
                    "Hub service",
                    Tone::Danger,
                    "the hub service is not running",
                    None,
                )
            });
            rows.push(if flag(overview, "noded") {
                check("Node here", Tone::Ok, "this machine runs agents", None)
            } else {
                check(
                    "Node here",
                    Tone::Warn,
                    "no node on this machine: agents cannot run here",
                    Some(("Add this machine as a node", Id::Nodes)),
                )
            });
        }
        Got::Failed(_) => {}
    }

    rows.push(match got(slot(state, Source::NodeLink)) {
        Got::Pending => pending("Network"),
        Got::Value(link) => {
            if !flag(link, "joined") {
                check("Network", Tone::Ok, "this hub runs its own network", None)
            } else {
                let running = flag(link, "running");
                let hub = link
                    .get("hub")
                    .and_then(Value::as_str)
                    .filter(|h| !h.is_empty())
                    .unwrap_or("another hub");
                let node = if running { "running" } else { "stopped" };
                let tone = if running { Tone::Ok } else { Tone::Warn };
                let fix = (!running).then_some(("Start the node", Id::Nodes));
                check("Network", tone, format!("joined {hub}, node {node}"), fix)
            }
        }
        Got::Failed(_) => check(
            "Network",
            Tone::Warn,
            "could not read the node link",
            Some(("Open Nodes", Id::Nodes)),
        ),
    });

    match got(slot(state, Source::Machine)) {
        Got::Pending => rows.push(pending("Machine and hypervisor")),
        Got::Value(v) => {
            // Without a machine the existing local checks cover this machine, so the row is left out.
            if let Some(machine) = v.get("machine").filter(|m| m.is_object()) {
                let backend = str_at(machine, "backend");
                let (tone, backend) = if backend.is_empty() {
                    (Tone::Warn, "no hypervisor backend")
                } else {
                    (Tone::Ok, backend)
                };
                let arch = str_at(machine, "arch");
                let cpus = uint(machine, "cpus");
                let mib = uint(machine, "memory_mib");
                rows.push(check(
                    "Machine and hypervisor",
                    tone,
                    format!(
                        "{backend} on {arch}, {}, {mib} MiB",
                        plural(cpus as usize, "CPU")
                    ),
                    None,
                ));
            }
        }
        Got::Failed(_) => {}
    }

    rows.push(match got(slot(state, Source::Vault)) {
        Got::Pending => pending("Vault"),
        Got::Value(v) if v.get("vault").is_some_and(Value::is_object) => vault_row(&v["vault"]),
        _ => check(
            "Vault",
            Tone::Warn,
            "could not read the vault",
            Some(("Open the Vault", Id::Vault)),
        ),
    });

    rows.push(match got(slot(state, Source::Providers)) {
        Got::Value(v) if v.is_array() => {
            providers_row(v.as_array().map(Vec::as_slice).unwrap_or_default())
        }
        Got::Pending => pending("Model providers"),
        _ => check(
            "Model providers",
            Tone::Warn,
            "could not list the providers",
            Some(("Open Models", Id::Models)),
        ),
    });

    match got(slot(state, Source::Overview)) {
        Got::Pending => rows.push(pending("Default model")),
        Got::Value(overview) => {
            let model = str_at(overview, "default_model");
            rows.push(if model.is_empty() {
                check(
                    "Default model",
                    Tone::Warn,
                    "none set: agents have no model by default",
                    Some(("Pick a default model", Id::Models)),
                )
            } else {
                check("Default model", Tone::Ok, model, None)
            });
        }
        Got::Failed(_) => {}
    }

    rows.push(match got(slot(state, Source::Git)) {
        Got::Pending => pending("Git host"),
        Got::Value(v) if v.get("git").is_some_and(Value::is_object) => {
            let git = &v["git"];
            if flag(git, "configured") {
                check(
                    "Git host",
                    Tone::Ok,
                    format!("org {}", str_at(git, "org")),
                    None,
                )
            } else {
                check(
                    "Git host",
                    Tone::Warn,
                    "the git host is not configured",
                    Some((
                        "Set FORGEJO_URL and FORGEJO_TOKEN for the hub, then restart apid",
                        Id::Git,
                    )),
                )
            }
        }
        _ => check(
            "Git host",
            Tone::Warn,
            "could not ask the git host",
            Some(("Open Git", Id::Git)),
        ),
    });

    rows
}

fn vault_row(vault: &Value) -> CheckRow {
    let state = str_at(vault, "state");
    let tampered = match vault.get("tampered") {
        Some(Value::String(why)) => why.clone(),
        Some(Value::Bool(true)) => "the records do not verify".to_string(),
        _ => String::new(),
    };
    if state == "uninitialized" {
        return check(
            "Vault",
            Tone::Warn,
            "not created yet: secrets have no encrypted home",
            Some(("Create the vault", Id::Vault)),
        );
    }
    if state == "sealed" {
        return check(
            "Vault",
            Tone::Warn,
            "sealed: agents cannot read secrets until it is unsealed",
            Some(("Unseal the vault", Id::Vault)),
        );
    }
    if state == "tampered" || !tampered.is_empty() {
        let why = if tampered.is_empty() {
            "the records do not verify"
        } else {
            tampered.as_str()
        };
        return check(
            "Vault",
            Tone::Danger,
            format!("tampered: {why}"),
            Some(("Read the audit log and recover", Id::Vault)),
        );
    }
    let env_keys = uint(vault, "env_file_keys");
    if env_keys > 0 {
        return check(
            "Vault",
            Tone::Warn,
            format!(
                "{} still in the env file, not in the vault",
                plural(env_keys as usize, "key")
            ),
            Some(("Import them into the vault", Id::Vault)),
        );
    }
    if vault.get("audit_ok").and_then(Value::as_bool) == Some(false) {
        return check(
            "Vault",
            Tone::Warn,
            "unsealed, but the audit chain does not verify",
            Some(("Check the audit log", Id::Vault)),
        );
    }
    let entries = uint(vault, "entries");
    let slots = vault
        .get("slots")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    check(
        "Vault",
        Tone::Ok,
        format!(
            "unsealed: {}, {}",
            plural(entries as usize, "secret"),
            plural(slots, "unlock slot")
        ),
        None,
    )
}

fn providers_row(list: &[Value]) -> CheckRow {
    if list.is_empty() {
        return check(
            "Model providers",
            Tone::Danger,
            "none: agents cannot call a model",
            Some(("Add a provider and its key", Id::Models)),
        );
    }
    let without: Vec<&str> = list
        .iter()
        .filter(|p| str_at(p, "credential") == "missing")
        .map(|p| str_at(p, "id"))
        .collect();
    if without.is_empty() {
        check(
            "Model providers",
            Tone::Ok,
            format!("{}, every key set", plural(list.len(), "provider")),
            None,
        )
    } else {
        check(
            "Model providers",
            Tone::Warn,
            format!(
                "{} without a key (of {})",
                without.join(", "),
                plural(list.len(), "provider")
            ),
            Some(("Set the key for each one", Id::Models)),
        )
    }
}

fn status_word(tone: Tone) -> &'static str {
    match tone {
        Tone::Ok => "OK",
        Tone::Warn => "WARNING",
        Tone::Danger => "FAILED",
        _ => "CHECKING",
    }
}

/// The hub checks card: a summary of the failures and the items to set up, then a row per check.
fn hub_card<'a>(state: &State) -> Element<'a, Msg> {
    let rows = hub_checks(state);
    let failures = rows.iter().filter(|r| r.tone == Tone::Danger).count();
    let items = rows.iter().filter(|r| r.tone == Tone::Warn).count();
    let summary = format!(
        "{failures} {}, {items} {} to set up",
        if failures == 1 { "failure" } else { "failures" },
        if items == 1 { "item" } else { "items" },
    );
    let lines: Vec<Element<'a, Msg>> = rows.into_iter().map(check_line).collect();
    column![ui::faint(summary), column(lines).spacing(10)]
        .spacing(14)
        .into()
}

fn check_line<'a>(item: CheckRow) -> Element<'a, Msg> {
    let fix: Element<'a, Msg> = match item.fix {
        Some((label, id)) if matches!(item.tone, Tone::Warn | Tone::Danger) => {
            ui::link(label, Msg::Go(id), false)
        }
        _ => Space::new().into(),
    };
    row![
        column![
            ui::sans(item.label, 12.0, ui::INK_2),
            ui::faint(item.detail)
        ]
        .spacing(4)
        .width(Length::Fill),
        ui::badge(status_word(item.tone), item.tone),
        fix,
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into()
}

/// The web's `plural`: "1 agent", "2 agents".
fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}
