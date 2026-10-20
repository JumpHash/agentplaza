//! The Vault screen's sections beyond the secrets, shown under its tabs when unsealed: the consumers
//! matrix (each consumer's entry on each node), the audit log with chain verify, and the `.env` import.
//!
//! The import reads a file here, sends the lines of the picked names once, and drops them. No value
//! is shown, logged or kept after its request. The audit log and the matrix hold only metadata.

use std::collections::BTreeMap;

use iced::widget::{button, checkbox, column, container, pick_list, row, text};
use iced::{Alignment, Element, Length, Task, Theme};
use serde::Deserialize;
use serde_json::{Value, json};

use super::forms::{ScopeMode, binding_label, valid_name};
use super::{
    ConsumerRef, Msg, NodeRef, State, ago, binding_name, cells, current_version, failure_line,
    field, flush_card, gate, layer, node_name, now_ms, table,
};
use crate::console::{Ctx, client, ui};
use crate::theme;

/// This hub's node id. The matrix shows it first, named "This hub".
const HUB: &str = "n0";
/// Audit rows per page, as the web console asks for them.
pub const PAGE: u64 = 50;

// ---- The audit log --------------------------------------------------------------------------

/// One row of `GET /v1/vault/audit`. Only what the screen shows; the chain fields stay with the hub.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AuditLine {
    pub seq: u64,
    pub ts_ms: f64,
    pub event: String,
    pub name: Option<String>,
    pub entry: Option<String>,
    pub actor: String,
    /// The consumer binding the event was for, when it had one.
    pub binding: Option<String>,
    pub node: Option<String>,
    pub cid: Option<u64>,
    pub detail: Option<String>,
}

/// One page of the audit log, and the chain answer when the request asked for a verify.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AuditPage {
    pub entries: Vec<AuditLine>,
    pub chain_ok: Option<bool>,
    /// The first broken entry, when the request verified a broken chain.
    pub first_broken: Option<BrokenEntry>,
    /// The entries that verified before the first broken one (all of them when intact).
    pub verified: Option<u64>,
}

/// The first broken entry of the chain as the hub reports it: its 0-based position, its seq when it has one,
/// and the expected and found hashes cut to 12 characters.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BrokenEntry {
    pub index: u64,
    pub seq: Option<u64>,
    pub expected: String,
    pub found: String,
}

/// The audit log as read so far: the pages loaded, where the next one starts, and the last verify.
#[derive(Debug, Clone, Default)]
pub struct AuditState {
    pub lines: Vec<AuditLine>,
    /// The seq of the last line loaded; the next page starts after it.
    pub after: u64,
    /// Whether the last page was full, so another may follow.
    pub more: bool,
    /// The last verify's answer.
    pub chain: Option<bool>,
    /// The first broken entry from the last verify, and the entries that verified before it.
    pub broken: Option<BrokenEntry>,
    pub verified: Option<u64>,
    pub loaded: bool,
    pub busy: bool,
    pub error: Option<String>,
}

pub fn parse_audit(value: Value) -> Result<AuditPage, String> {
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// The verify answer in words. The web console's chainMessage says the same thing.
pub fn chain_message(ok: bool, verified: Option<u64>, broken: Option<&BrokenEntry>) -> String {
    if ok {
        return match verified {
            Some(n) => format!("Chain intact: {n} entries verified"),
            None => "Chain intact".to_string(),
        };
    }
    match broken {
        None => "Chain broken".to_string(),
        Some(b) => {
            let at = b.seq.map(|s| format!(" (seq {s})")).unwrap_or_default();
            format!(
                "Chain broken at entry #{}{at}: {} entries verified before it",
                b.index,
                verified.unwrap_or(b.index)
            )
        }
    }
}

/// `/v1/vault/audit?after=&limit=`, with `verify=1` when the chain is to be checked.
pub fn audit_path(after: u64, verify: bool) -> String {
    let flag = if verify { "&verify=1" } else { "" };
    format!("/v1/vault/audit?after={after}&limit={PAGE}{flag}")
}

pub fn page_task(ctx: &Ctx, after: u64) -> Task<Msg> {
    let c = ctx.client.clone();
    let path = audit_path(after, false);
    client::task(
        move || c.call("GET", &path, None).and_then(parse_audit),
        Msg::AuditPage,
    )
}

pub fn verify_task(ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    let path = audit_path(0, true);
    client::task(
        move || c.call("GET", &path, None).and_then(parse_audit),
        Msg::AuditVerified,
    )
}

/// The audit tab: the chain's last answer and the Verify chain button in the head, then the log's rows (newest
/// first, as the hub sends them) with Load more while the hub has more. Locked, it says why.
pub fn audit_card<'a>(state: &'a State) -> Element<'a, Msg> {
    let audit = &state.audit;
    let locked = !matches!(&state.status, Some(status) if status.state == "unsealed");
    let now = now_ms();
    let mut actions: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(ok) = audit.chain {
        let message = chain_message(ok, audit.verified, audit.broken.as_ref());
        actions.push(ui::badge(
            &message,
            if ok { ui::Tone::Ok } else { ui::Tone::Danger },
        ));
    }
    actions.push(ui::btn(
        "Verify chain",
        None,
        ui::Kind::Secondary,
        if locked {
            None
        } else {
            gate(audit.busy, Msg::AuditVerify)
        },
    ));

    if locked {
        return flush_card(
            "Audit log",
            actions,
            container(ui::faint("Unseal the vault to read the audit log."))
                .padding(16)
                .width(Length::Fill)
                .into(),
        );
    }
    if let Some(error) = &audit.error {
        return flush_card(
            "Audit log",
            actions,
            container(ui::error_line(&format!(
                "Could not read the audit log. {error}"
            )))
            .padding(16)
            .width(Length::Fill)
            .into(),
        );
    }
    if !audit.loaded {
        return flush_card(
            "Audit log",
            actions,
            container(ui::faint("Loading the audit log."))
                .padding(16)
                .width(Length::Fill)
                .into(),
        );
    }
    if audit.lines.is_empty() {
        return flush_card(
            "Audit log",
            actions,
            ui::empty("file", "No audit events yet", ""),
        );
    }

    let head: [(u16, &str); 8] = [
        (2, "Time"),
        (3, "Event"),
        (2, "Actor"),
        (3, "Entry"),
        (2, "Binding"),
        (2, "Node"),
        (1, "Cid"),
        (4, "Detail"),
    ];
    let rows: Vec<Element<'a, Msg>> = audit
        .lines
        .iter()
        .map(|line| {
            let broken = audit.broken.as_ref().filter(|b| b.seq == Some(line.seq));
            audit_row(line, state, now, broken)
        })
        .collect();
    let mut body: Vec<Element<'a, Msg>> = vec![table(&head, rows)];
    if audit.more {
        body.push(
            container(ui::btn(
                "Load more",
                None,
                ui::Kind::Ghost,
                gate(audit.busy, Msg::AuditMore),
            ))
            .padding([12, 16])
            .into(),
        );
    }
    flush_card(
        "Audit log",
        actions,
        column(body).width(Length::Fill).into(),
    )
}

/// One audit row: when, the event (the broken entry gets its badge), the actor, the entry, its binding, the node, the
/// CID and the detail. The broken entry also shows the hashes that did not match.
fn audit_row<'a>(
    line: &AuditLine,
    state: &'a State,
    now: f64,
    broken: Option<&BrokenEntry>,
) -> Element<'a, Msg> {
    let p = theme::palette();
    let actor = if line.actor.is_empty() {
        "—".to_string()
    } else {
        line.actor.clone()
    };
    let entry = line
        .name
        .clone()
        .or_else(|| line.entry.clone())
        .unwrap_or_else(|| "—".to_string());
    let binding = line
        .binding
        .as_deref()
        .map(|b| binding_name(b, &state.consumers))
        .unwrap_or_else(|| "—".to_string());
    let node = line
        .node
        .as_deref()
        .map(|id| node_name(&state.nodes, id))
        .unwrap_or_else(|| "—".to_string());
    let cid = line
        .cid
        .map(|c| c.to_string())
        .unwrap_or_else(|| "—".to_string());
    let mut event_parts: Vec<Element<'a, Msg>> = vec![
        ui::sans(line.event.clone(), 13.0, p.ink).into(),
        ui::mono(format!("#{}", line.seq), 11.0, p.ink_3).into(),
    ];
    if broken.is_some() {
        event_parts.push(ui::badge("broken", ui::Tone::Danger));
    }
    let mut detail_parts: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(detail) = line.detail.as_deref().filter(|d| !d.is_empty()) {
        detail_parts.push(ui::faint(ui::ellipsize(&audit_detail(detail), 120)).into());
    }
    if let Some(b) = broken {
        detail_parts.push(ui::faint(format!("expected {}  found {}", b.expected, b.found)).into());
    }
    cells(vec![
        (2, ui::faint(ago(line.ts_ms, now)).into()),
        (
            3,
            row(event_parts)
                .spacing(8)
                .align_y(Alignment::Center)
                .into(),
        ),
        (2, ui::sans(actor, 13.0, p.ink).into()),
        (3, ui::mono(entry, 12.0, p.ink).into()),
        (2, ui::faint(binding).into()),
        (2, ui::faint(node).into()),
        (1, ui::faint(cid).into()),
        (4, column(detail_parts).spacing(4).into()),
    ])
}

// ---- The per-node matrix --------------------------------------------------------------------

/// The Per node tab: one row per consumer binding, one column per node (this hub first). Each cell is the entry that
/// resolves there, a button to add the one that is missing, or the scope note.
pub fn consumers_card<'a>(state: &'a State) -> Element<'a, Msg> {
    let locked = !matches!(&state.status, Some(status) if status.state == "unsealed");
    if locked {
        return ui::card(
            "",
            ui::faint("Unseal the vault to see what resolves on each node.").into(),
        );
    }
    if state.consumers.is_empty() {
        return ui::card("", ui::empty("link", "Nothing asks for a secret yet", ""));
    }
    let nodes: &[NodeRef] = match state.status.as_ref().map(|s| s.nodes.as_slice()) {
        Some(nodes) if !nodes.is_empty() => nodes,
        _ => &state.nodes,
    };
    let legend = row![
        ui::badge("set", ui::Tone::Ok),
        ui::faint("The secret resolves on this node."),
        ui::badge("missing", ui::Tone::Warn),
        ui::faint("Nothing resolves. Select to add the secret."),
        ui::badge("out of scope", ui::Tone::Muted),
        ui::faint("The secret exists for other nodes only."),
    ]
    .spacing(16)
    .align_y(Alignment::Center);

    let labels: Vec<String> = std::iter::once("Binding".to_string())
        .chain(nodes.iter().map(node_label))
        .collect();
    let widths: Vec<u16> = std::iter::once(3).chain(nodes.iter().map(|_| 2)).collect();
    let head: Vec<(u16, &str)> = widths
        .iter()
        .copied()
        .zip(labels.iter().map(String::as_str))
        .collect();
    let rows: Vec<Element<'a, Msg>> = state
        .consumers
        .iter()
        .map(|consumer| consumer_row(state, consumer, nodes))
        .collect();
    flush_card(
        "",
        Vec::new(),
        column![
            container(legend).padding([12, 16]).width(Length::Fill),
            table(&head, rows),
        ]
        .width(Length::Fill)
        .into(),
    )
}

/// One consumer's row: its binding and the env name it wants, then a cell per node.
fn consumer_row<'a>(
    state: &'a State,
    consumer: &'a ConsumerRef,
    nodes: &[NodeRef],
) -> Element<'a, Msg> {
    let p = theme::palette();
    let binding: Element<'a, Msg> = column![
        text(binding_name(&consumer.binding, &state.consumers))
            .font(theme::SANS_SEMIBOLD)
            .size(theme::TYPE_UI)
            .color(p.ink),
        ui::mono(consumer.wants.clone(), 11.0, p.ink_3),
    ]
    .spacing(2)
    .into();
    let mut parts: Vec<(u16, Element<'a, Msg>)> = vec![(3, binding)];
    for node in nodes {
        parts.push((2, matrix_cell(state, consumer, node)));
    }
    cells(parts)
}

fn node_label(node: &NodeRef) -> String {
    if node.id == HUB {
        "This hub".to_string()
    } else if node.name.is_empty() {
        node.id.clone()
    } else {
        node.name.clone()
    }
}

/// One cell: the entry's name and fingerprint when it resolves; a button to add it when it is missing; the scope note.
fn matrix_cell<'a>(
    state: &'a State,
    consumer: &'a ConsumerRef,
    node: &NodeRef,
) -> Element<'a, Msg> {
    let p = theme::palette();
    match consumer.per_node.get(&node.id).map(String::as_str) {
        Some("missing") => {
            let add = gate(
                state.busy,
                Msg::OpenCreateFor(
                    consumer.wants.clone(),
                    consumer.binding.clone(),
                    node.id.clone(),
                ),
            );
            button(ui::badge("missing", ui::Tone::Warn))
                .padding(0)
                .style(button::text)
                .on_press_maybe(add)
                .into()
        }
        Some("out_of_scope") => ui::badge("out of scope", ui::Tone::Muted),
        Some(id) => match state.entries.iter().find(|e| e.id == id) {
            Some(entry) => {
                let fingerprint: String = current_version(entry)
                    .map(|v| v.fp.chars().take(14).collect())
                    .unwrap_or_default();
                column![
                    ui::badge(&entry.name, ui::Tone::Ok),
                    ui::mono(fingerprint, 11.0, p.ink_3),
                ]
                .spacing(4)
                .into()
            }
            None => ui::badge("Unknown entry", ui::Tone::Muted),
        },
        None => ui::faint("-").into(),
    }
}

/// An audit detail as the owner reads it: the epochs the hub writes into some details become local clock times.
/// "4 uses in minute 29864994" reads "4 uses at 14:32"; a bare 10-digit epoch in seconds or 13-digit one in ms too.
pub fn audit_detail(detail: &str) -> String {
    let words: Vec<&str> = detail.split_whitespace().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let word = words[i];
        let epoch_minute = |w: Option<&&str>| -> Option<i64> {
            w.filter(|d| (7..=9).contains(&d.len()) && d.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|d| d.parse::<i64>().ok())
        };
        if word == "in"
            && words.get(i + 1) == Some(&"minute")
            && let Some(minute) = epoch_minute(words.get(i + 2))
        {
            out.push(format!("at {}", clock_text(minute * 60_000)));
            i += 3;
            continue;
        }
        if word == "minute"
            && let Some(minute) = epoch_minute(words.get(i + 1))
        {
            out.push(format!("at {}", clock_text(minute * 60_000)));
            i += 2;
            continue;
        }
        let digits = word.bytes().all(|b| b.is_ascii_digit());
        if digits
            && word.len() == 13
            && let Ok(ms) = word.parse::<i64>()
        {
            out.push(clock_text(ms));
            i += 1;
            continue;
        }
        if digits
            && word.len() == 10
            && let Ok(secs) = word.parse::<i64>()
        {
            out.push(clock_text(secs * 1000));
            i += 1;
            continue;
        }
        out.push(word.to_string());
        i += 1;
    }
    out.join(" ")
}

/// A local clock time for epoch milliseconds: "14:32" today, "13 Oct 14:32" on another day.
fn clock_text(ms: i64) -> String {
    use chrono::{Datelike, Local, TimeZone, Timelike};
    let Some(when) = Local.timestamp_millis_opt(ms).single() else {
        return ms.to_string();
    };
    let time = format!("{:02}:{:02}", when.hour(), when.minute());
    if when.date_naive() == Local::now().date_naive() {
        time
    } else {
        format!("{} {} {time}", when.day(), when.format("%b"))
    }
}

// ---- The .env import ------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ImportResult {
    pub imported: Vec<ImportedName>,
    pub skipped: Vec<SkippedName>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ImportedName {
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SkippedName {
    pub name: String,
    pub reason: String,
}

/// The import form. `raw` is the file's text, held from the pick until the one request is sent; it is
/// never shown. The names, their ticks and the bindings are what the screen displays.
#[derive(Debug, Clone, Default)]
pub struct ImportForm {
    pub source: Option<String>,
    pub raw: String,
    /// The valid names found, each ticked to import.
    pub picked: Vec<(String, bool)>,
    /// Names that are not valid, so they are skipped.
    pub invalid: Vec<String>,
    /// Lines with no name or no "=".
    pub skipped_lines: usize,
    pub scope: ScopeMode,
    /// Each node, ticked when the scope is the listed nodes.
    pub nodes: Vec<(String, bool)>,
    /// Each consumer binding, ticked to bind the imported names that it wants.
    pub bindings: Vec<(String, bool)>,
    pub result: Option<ImportResult>,
}

/// Reads a `.env` file the owner picks. `None` when the dialog is cancelled.
pub async fn pick_env_file() -> Result<Option<(String, String)>, String> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .set_title("Choose a .env file")
        .pick_file()
        .await
    else {
        return Ok(None);
    };
    let name = file.file_name();
    std::fs::read_to_string(file.path())
        .map(|raw| Some((name, raw)))
        .map_err(|_| "The file could not be read.".to_string())
}

/// The name a dotenv line sets: `NAME=...` or `export NAME=...`. None for a line with no name or no "=".
pub fn dotenv_name(line: &str) -> Option<&str> {
    let t = line.trim();
    if t.is_empty() || t.starts_with('#') {
        return None;
    }
    let body = t.strip_prefix("export ").map(str::trim_start).unwrap_or(t);
    let (name, _) = body.split_once('=')?;
    let name = name.trim();
    (!name.is_empty()).then_some(name)
}

/// The names a file sets, as (valid in file order without repeats, invalid, lines with no name).
pub fn scan_names(raw: &str) -> (Vec<String>, Vec<String>, usize) {
    let (mut valid, mut invalid, mut skipped) = (Vec::new(), Vec::new(), 0);
    for line in raw.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        match dotenv_name(line) {
            Some(name) if valid_name(name) => {
                if !valid.iter().any(|v| v == name) {
                    valid.push(name.to_string());
                }
            }
            Some(name) => {
                if !invalid.iter().any(|v| v == name) {
                    invalid.push(name.to_string());
                }
            }
            None => skipped += 1,
        }
    }
    (valid, invalid, skipped)
}

/// The body of `POST /v1/vault/import`: only the lines of the picked names, the scope, and the bindings.
pub fn import_body(
    raw: &str,
    names: &[String],
    scope: Value,
    bindings: &BTreeMap<String, Vec<String>>,
) -> Value {
    let text = raw
        .lines()
        .filter(|line| dotenv_name(line).is_some_and(|name| names.iter().any(|n| n == name)))
        .collect::<Vec<_>>()
        .join("\n");
    json!({ "format": "dotenv", "text": text, "scope": scope, "bindings": bindings })
}

/// The scope object: `{kind: "plaza"}`, or the ticked nodes.
pub fn import_scope(form: &ImportForm) -> Result<Value, String> {
    match form.scope {
        ScopeMode::Plaza => Ok(json!({ "kind": "plaza" })),
        ScopeMode::Nodes => {
            let nodes: Vec<&str> = form
                .nodes
                .iter()
                .filter(|(_, on)| *on)
                .map(|(id, _)| id.as_str())
                .collect();
            if nodes.is_empty() {
                Err("Pick at least one node, or choose Plaza.".into())
            } else {
                Ok(json!({ "kind": "nodes", "nodes": nodes }))
            }
        }
    }
}

/// Each ticked binding that wants a ticked name, mapped to that name.
pub fn import_bindings(state: &State) -> BTreeMap<String, Vec<String>> {
    let picked: Vec<&str> = state
        .import
        .picked
        .iter()
        .filter(|(_, on)| *on)
        .map(|(n, _)| n.as_str())
        .collect();
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (binding, on) in &state.import.bindings {
        if !*on {
            continue;
        }
        let Some(c) = state.consumers.iter().find(|c| &c.binding == binding) else {
            continue;
        };
        if picked.contains(&c.wants.as_str()) {
            map.entry(c.wants.clone())
                .or_default()
                .push(binding.clone());
        }
    }
    map
}

fn noun(n: usize) -> &'static str {
    if n == 1 { "name" } else { "names" }
}

/// The Import .env dialog: the warning, the file, the names it holds (each ticked to import), the scope, and the
/// bindings for the names. The file is read here for names only; its values are never shown. Once imported, the
/// dialog shows what came in and what was skipped.
pub fn import_layer<'a>(state: &'a State) -> Element<'a, Msg> {
    let p = theme::palette();
    let imp = &state.import;
    let picked = imp.picked.iter().filter(|(_, on)| *on).count();
    let warn_soft = p.warn_soft;
    let mut body: Vec<Element<'a, Msg>> = vec![
        container(
            column![
                ui::faint(".env files are discouraged. A file can be copied, synced or kept in backups. Delete it after the import."),
                ui::faint("The clean path is apz vault import FILE.env --shred, which overwrites and deletes the file after a verified import."),
            ]
            .spacing(4),
        )
        .padding([12, 16])
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(warn_soft.into()),
            border: iced::Border::default().rounded(theme::skin().radius),
            ..Default::default()
        })
        .into(),
    ];

    if let Some(result) = &imp.result {
        let names: Vec<&str> = result.imported.iter().map(|i| i.name.as_str()).collect();
        let line = if names.is_empty() {
            "Nothing was imported.".to_string()
        } else {
            format!(
                "Imported {} {}: {}.",
                names.len(),
                noun(names.len()),
                names.join(", ")
            )
        };
        body.push(ui::faint(line).into());
        for skipped in &result.skipped {
            body.push(ui::faint(format!("Skipped {}: {}", skipped.name, skipped.reason)).into());
        }
    } else {
        let choose = ui::btn(
            "Choose a .env file",
            None,
            ui::Kind::Secondary,
            gate(state.busy, Msg::ImportPick),
        );
        let chosen: Element<'a, Msg> = match &imp.source {
            Some(source) => row![choose, ui::faint(source.clone())]
                .spacing(12)
                .align_y(Alignment::Center)
                .into(),
            None => choose,
        };
        body.push(field(
            ".env file",
            chosen,
            Some("Read on this device. Nothing is sent until you import."),
        ));
        match &imp.source {
            None => body
                .push(ui::faint("Choose a file to see its names. Values are never shown.").into()),
            Some(source) => {
                let total = imp.picked.len();
                body.push(
                    ui::faint(format!(
                        "{total} {} found from {source}. Values are not shown.",
                        noun(total)
                    ))
                    .into(),
                );
                if !imp.invalid.is_empty() {
                    body.push(
                        ui::faint(format!(
                            "Not valid names, so skipped: {}",
                            imp.invalid.join(", ")
                        ))
                        .into(),
                    );
                }
                if imp.skipped_lines > 0 {
                    body.push(
                        ui::faint(format!(
                            "Skipped {} {} with no name, or no \"=\".",
                            imp.skipped_lines,
                            if imp.skipped_lines == 1 {
                                "line"
                            } else {
                                "lines"
                            }
                        ))
                        .into(),
                    );
                }
                if total > 0 {
                    let names: Vec<Element<'a, Msg>> = imp
                        .picked
                        .iter()
                        .enumerate()
                        .map(|(i, (name, on))| {
                            checkbox(*on)
                                .label(name.clone())
                                .on_toggle(move |v| Msg::ImportName(i, v))
                                .into()
                        })
                        .collect();
                    body.push(column(names).spacing(6).into());
                }
            }
        }
    }

    if imp.result.is_none() {
        let mut scope: Vec<Element<'a, Msg>> = vec![
            pick_list(
                vec![ScopeMode::Plaza, ScopeMode::Nodes],
                Some(imp.scope),
                Msg::ImportScope,
            )
            .style(theme::pick_list)
            .into(),
        ];
        if imp.scope == ScopeMode::Nodes {
            for (i, (id, on)) in imp.nodes.iter().enumerate() {
                scope.push(
                    checkbox(*on)
                        .label(node_name(&state.nodes, id))
                        .on_toggle(move |v| Msg::ImportNode(i, v))
                        .into(),
                );
            }
        }
        body.push(field("Scope", column(scope).spacing(6).into(), None));
        if picked > 0 {
            let bindings: Vec<Element<'a, Msg>> = imp
                .bindings
                .iter()
                .enumerate()
                .map(|(i, (binding, on))| {
                    checkbox(*on)
                        .label(binding_label(binding, &state.consumers))
                        .on_toggle(move |v| Msg::ImportBinding(i, v))
                        .into()
                })
                .collect();
            let list: Element<'a, Msg> = if bindings.is_empty() {
                ui::faint("Nothing asks for a secret yet. The names import without bindings.")
                    .into()
            } else {
                column(bindings).spacing(6).into()
            };
            body.push(field(
                "Bindings",
                column![
                    ui::faint("A name gets the ticked bindings that want it."),
                    list
                ]
                .spacing(6)
                .into(),
                None,
            ));
        }
    }
    body.extend(failure_line(state));

    let actions: Vec<Element<'a, Msg>> = if imp.result.is_some() {
        vec![ui::btn(
            "Done",
            None,
            ui::Kind::Primary,
            Some(Msg::CloseImport),
        )]
    } else {
        let mut acts: Vec<Element<'a, Msg>> = Vec::new();
        if imp.source.is_some() {
            acts.push(ui::btn(
                "Clear",
                None,
                ui::Kind::Ghost,
                Some(Msg::ImportClear),
            ));
        }
        acts.push(ui::btn(
            "Cancel",
            None,
            ui::Kind::Ghost,
            Some(Msg::CloseImport),
        ));
        let label = if picked == 0 {
            "Import".to_string()
        } else {
            format!("Import {picked} {}", noun(picked))
        };
        acts.push(ui::btn(
            &label,
            None,
            ui::Kind::Primary,
            gate(state.busy || picked == 0, Msg::ImportSubmit),
        ));
        acts
    };
    layer("Import .env", body, actions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_audit_page_reads_entries_and_the_chain_answer() {
        let page = parse_audit(json!({
            "entries": [{ "seq": 7, "ts_ms": 1000.0, "event": "secret.read", "name": "API_KEY", "actor": "owner", "node": "n1", "cid": 3, "prev": "p", "mac": "m" }],
            "chain_ok": false
        }))
        .unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].seq, 7);
        assert_eq!(page.entries[0].name.as_deref(), Some("API_KEY"));
        assert_eq!(page.entries[0].node.as_deref(), Some("n1"));
        assert_eq!(page.chain_ok, Some(false));
        assert_eq!(
            parse_audit(json!({ "entries": [] })).unwrap().chain_ok,
            None
        );
    }

    #[test]
    fn vault_audit_paths_page_and_verify() {
        assert_eq!(audit_path(50, false), "/v1/vault/audit?after=50&limit=50");
        assert_eq!(
            audit_path(0, true),
            "/v1/vault/audit?after=0&limit=50&verify=1"
        );
    }

    #[test]
    fn vault_consumers_read_each_nodes_state() {
        let consumers: Vec<super::super::ConsumerRef> = serde_json::from_value(json!([{
            "binding": "mcp:fs", "label": "Files", "wants": "API_KEY",
            "per_node": { "n0": "e1", "n1": "missing", "n2": "out_of_scope" }
        }]))
        .unwrap();
        let per = &consumers[0].per_node;
        assert_eq!(per.get("n0").map(String::as_str), Some("e1"));
        assert_eq!(per.get("n1").map(String::as_str), Some("missing"));
        assert_eq!(per.get("n2").map(String::as_str), Some("out_of_scope"));
    }

    #[test]
    fn vault_import_scans_names_and_sends_only_the_picked_lines() {
        let raw = "# keys\nexport API_KEY=abc\nlower=2\nNOEQ\nAPI_KEY=dup\n\nDB_URL=postgres://x\n";
        let (valid, invalid, skipped) = scan_names(raw);
        assert_eq!(valid, vec!["API_KEY".to_string(), "DB_URL".to_string()]);
        assert_eq!(invalid, vec!["lower".to_string()]);
        assert_eq!(skipped, 1);
        let body = import_body(
            raw,
            &["DB_URL".to_string()],
            json!({ "kind": "plaza" }),
            &BTreeMap::new(),
        );
        assert_eq!(body["format"], "dotenv");
        assert_eq!(body["text"], "DB_URL=postgres://x");
        assert_eq!(body["scope"]["kind"], "plaza");
    }
}
