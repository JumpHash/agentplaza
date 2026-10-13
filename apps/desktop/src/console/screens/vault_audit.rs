//! The Vault screen's sections beyond the secrets, shown under its tabs when unsealed: the consumers
//! matrix (each consumer's entry on each node), the audit log with chain verify, and the `.env` import.
//!
//! The import reads a file here, sends the lines of the picked names once, and drops them. No value
//! is shown, logged or kept after its request. The audit log and the matrix hold only metadata.

use std::collections::BTreeMap;

use iced::widget::{checkbox, column, pick_list, row, text};
use iced::{Element, Length, Task};
use serde::Deserialize;
use serde_json::{json, Value};

use super::forms::{valid_name, ScopeMode};
use super::{ago, gate, node_name, now_ms, stack, ConsumerRef, Msg, NodeRef, State};
use crate::console::{client, ui, Ctx};

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
    pub node: Option<String>,
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
            format!("Chain broken at entry #{}{at}: {} entries verified before it", b.index, verified.unwrap_or(b.index))
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
    client::task(move || c.call("GET", &path, None).and_then(parse_audit), Msg::AuditPage)
}

pub fn verify_task(ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    let path = audit_path(0, true);
    client::task(move || c.call("GET", &path, None).and_then(parse_audit), Msg::AuditVerified)
}

pub fn audit_card<'a>(state: &'a State) -> Element<'a, Msg> {
    let audit = &state.audit;
    let now = now_ms();
    let mut head: Vec<Element<'a, Msg>> = vec![ui::btn_sm("Verify chain", None, ui::Kind::Secondary, gate(audit.busy, Msg::AuditVerify))];
    if let Some(ok) = audit.chain {
        let message = chain_message(ok, audit.verified, audit.broken.as_ref());
        head.push(ui::badge(&message, if ok { ui::Tone::Ok } else { ui::Tone::Danger }));
    }
    let mut items: Vec<Element<'a, Msg>> = vec![row(head).spacing(12).into()];
    if let Some(error) = &audit.error {
        items.push(ui::error_line(error));
    }
    if !audit.loaded && audit.error.is_none() {
        items.push(ui::faint("Loading the audit log.").into());
    } else if audit.loaded && audit.lines.is_empty() {
        items.push(ui::empty("key", "No audit events yet", "Events appear here as the vault is used."));
    }
    for (i, line) in audit.lines.iter().enumerate() {
        if i > 0 {
            items.push(ui::hairline());
        }
        let broken = audit.broken.as_ref().filter(|b| b.seq == Some(line.seq));
        items.push(audit_row(line, &state.nodes, now, broken));
    }
    if audit.more {
        items.push(row(vec![ui::btn("Load more", None, ui::Kind::Secondary, gate(audit.busy, Msg::AuditMore))]).into());
    }
    ui::card("Audit log", stack(items))
}

/// One audit row. The broken entry (when this row is it) gets a badge and the hashes that did not match.
fn audit_row<'a>(line: &AuditLine, nodes: &[NodeRef], now: f64, broken: Option<&BrokenEntry>) -> Element<'a, Msg> {
    let actor = if line.actor.is_empty() { "-".to_string() } else { line.actor.clone() };
    let entry = line.name.clone().or_else(|| line.entry.clone()).unwrap_or_else(|| "-".to_string());
    let node = line.node.as_deref().map(|id| node_name(nodes, id)).unwrap_or_else(|| "-".to_string());
    let event_text = format!("{}  #{}", line.event, line.seq);
    let event: Element<'a, Msg> = if broken.is_some() {
        let parts: Vec<Element<'a, Msg>> = vec![text(event_text).into(), ui::badge("broken", ui::Tone::Danger)];
        row(parts).spacing(8).width(Length::FillPortion(3)).into()
    } else {
        text(event_text).width(Length::FillPortion(3)).into()
    };
    let top: Element<'a, Msg> = row(vec![
        ui::faint(ago(line.ts_ms, now)).width(Length::FillPortion(2)).into(),
        event,
        text(actor).width(Length::FillPortion(2)).into(),
        text(entry).width(Length::FillPortion(3)).into(),
        text(node).width(Length::FillPortion(2)).into(),
    ])
    .spacing(12)
    .into();
    let mut rows: Vec<Element<'a, Msg>> = vec![top];
    if let Some(detail) = line.detail.as_deref().filter(|d| !d.is_empty()) {
        rows.push(ui::faint(ui::ellipsize(detail, 120)).into());
    }
    if let Some(b) = broken {
        rows.push(ui::faint(format!("expected {}  found {}", b.expected, b.found)).into());
    }
    if rows.len() == 1 {
        rows.remove(0)
    } else {
        column(rows).spacing(4).into()
    }
}

// ---- The consumers matrix -------------------------------------------------------------------

/// One row per consumer, one column per node: the entry that resolves there, or the gap.
pub fn consumers_card<'a>(state: &'a State) -> Element<'a, Msg> {
    if state.consumers.is_empty() {
        return ui::card("Consumers", ui::empty("key", "Nothing asks for a secret yet", "Providers, MCP servers and connectors list the secrets they want here."));
    }
    let nodes: &[NodeRef] = match state.status.as_ref().map(|s| s.nodes.as_slice()) {
        Some(nodes) if !nodes.is_empty() => nodes,
        _ => &state.nodes,
    };
    let legend: Element<'a, Msg> = row(vec![
        ui::badge("set", ui::Tone::Ok),
        ui::faint("resolves on this node").into(),
        ui::badge("missing", ui::Tone::Warn),
        ui::faint("nothing resolves; add it from here").into(),
        ui::badge("out of scope", ui::Tone::Muted),
        ui::faint("exists for other nodes only").into(),
    ])
    .spacing(8)
    .into();
    let mut head: Vec<Element<'a, Msg>> = vec![text("Binding").width(Length::FillPortion(3)).into()];
    for n in nodes {
        head.push(text(node_label(n)).width(Length::FillPortion(2)).into());
    }
    let mut items: Vec<Element<'a, Msg>> = vec![legend, row(head).spacing(12).into(), ui::hairline()];
    for c in &state.consumers {
        let title = if c.label.is_empty() { c.binding.clone() } else { c.label.clone() };
        let binding: Element<'a, Msg> = column(vec![text(title).into(), ui::faint(c.wants.clone()).into()]).width(Length::FillPortion(3)).into();
        let mut cells: Vec<Element<'a, Msg>> = vec![binding];
        for n in nodes {
            cells.push(column(vec![matrix_cell(state, c, n)]).width(Length::FillPortion(2)).into());
        }
        items.push(row(cells).spacing(12).into());
        items.push(ui::hairline());
    }
    ui::card("Consumers", stack(items))
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

/// One cell: the entry's name when it resolves, a button to add it when missing, or the scope note.
fn matrix_cell<'a>(state: &'a State, consumer: &ConsumerRef, node: &NodeRef) -> Element<'a, Msg> {
    match consumer.per_node.get(&node.id).map(String::as_str) {
        Some("missing") => row(vec![
            ui::badge("missing", ui::Tone::Warn),
            ui::btn_sm(
                "Add",
                None,
                ui::Kind::Ghost,
                gate(state.busy, Msg::OpenCreateFor(consumer.wants.clone(), consumer.binding.clone(), node.id.clone())),
            ),
        ])
        .spacing(8)
        .into(),
        Some("out_of_scope") => ui::badge("out of scope", ui::Tone::Muted),
        Some(id) => {
            let name = state.entries.iter().find(|e| e.id == id).map(|e| e.name.clone()).unwrap_or_else(|| "Unknown entry".to_string());
            ui::badge(&name, ui::Tone::Ok)
        }
        None => ui::faint("-").into(),
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
    let Some(file) = rfd::AsyncFileDialog::new().set_title("Choose a .env file").pick_file().await else {
        return Ok(None);
    };
    let name = file.file_name();
    std::fs::read_to_string(file.path()).map(|raw| Some((name, raw))).map_err(|_| "The file could not be read.".to_string())
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
pub fn import_body(raw: &str, names: &[String], scope: Value, bindings: &BTreeMap<String, Vec<String>>) -> Value {
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
            let nodes: Vec<&str> = form.nodes.iter().filter(|(_, on)| *on).map(|(id, _)| id.as_str()).collect();
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
    let picked: Vec<&str> = state.import.picked.iter().filter(|(_, on)| *on).map(|(n, _)| n.as_str()).collect();
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (binding, on) in &state.import.bindings {
        if !*on {
            continue;
        }
        let Some(c) = state.consumers.iter().find(|c| &c.binding == binding) else { continue };
        if picked.contains(&c.wants.as_str()) {
            map.entry(c.wants.clone()).or_default().push(binding.clone());
        }
    }
    map
}

fn noun(n: usize) -> &'static str {
    if n == 1 {
        "name"
    } else {
        "names"
    }
}

pub fn import_card<'a>(state: &'a State) -> Element<'a, Msg> {
    let imp = &state.import;
    let picked = imp.picked.iter().filter(|(_, on)| *on).count();
    let mut items: Vec<Element<'a, Msg>> = vec![
        ui::faint(".env files are discouraged. A file can be copied, synced or kept in backups. Delete it after the import.").into(),
        ui::faint("The clean path is apz vault import FILE.env --shred, which overwrites and deletes the file after a verified import.").into(),
        ui::btn("Choose a .env file", None, ui::Kind::Secondary, gate(state.busy, Msg::ImportPick)),
    ];
    match &imp.source {
        None => items.push(ui::faint("Choose a file to see its names. Values are never shown.").into()),
        Some(source) => {
            let total = imp.picked.len();
            items.push(ui::faint(format!("{source}: {total} {} found. Values are not shown.", noun(total))).into());
            if !imp.invalid.is_empty() {
                items.push(ui::faint(format!("Not valid names, so skipped: {}", imp.invalid.join(", "))).into());
            }
            if imp.skipped_lines > 0 {
                let lines = if imp.skipped_lines == 1 { "line" } else { "lines" };
                items.push(ui::faint(format!("{} unreadable {lines} skipped (no name, or no \"=\").", imp.skipped_lines)).into());
            }
            for (i, (name, on)) in imp.picked.iter().enumerate() {
                items.push(checkbox(*on).label(name.clone()).on_toggle(move |v| Msg::ImportName(i, v)).into());
            }
            items.push(
                row(vec![
                    ui::label("Scope").into(),
                    pick_list(vec![ScopeMode::Plaza, ScopeMode::Nodes], Some(imp.scope), Msg::ImportScope).into(),
                ])
                .spacing(12)
                .into(),
            );
            if imp.scope == ScopeMode::Nodes {
                for (i, (id, on)) in imp.nodes.iter().enumerate() {
                    items.push(checkbox(*on).label(node_name(&state.nodes, id)).on_toggle(move |v| Msg::ImportNode(i, v)).into());
                }
            }
            items.push(ui::label("Bindings").into());
            if imp.bindings.is_empty() {
                items.push(ui::faint("Nothing asks for a secret yet. The names import without bindings.").into());
            }
            for (i, (binding, on)) in imp.bindings.iter().enumerate() {
                let label = match state.consumers.iter().find(|c| &c.binding == binding) {
                    Some(c) if !c.wants.is_empty() => format!("{}  ·  {}", c.label, c.wants),
                    Some(c) => c.label.clone(),
                    None => binding.clone(),
                };
                items.push(checkbox(*on).label(label).on_toggle(move |v| Msg::ImportBinding(i, v)).into());
            }
            items.push(
                row(vec![
                    ui::btn("Clear", None, ui::Kind::Ghost, Some(Msg::ImportClear)),
                    ui::btn(&format!("Import {picked} {}", noun(picked)), None, ui::Kind::Primary, gate(state.busy || picked == 0, Msg::ImportSubmit)),
                ])
                .spacing(12)
                .into(),
            );
        }
    }
    if let Some(result) = &imp.result {
        let names: Vec<&str> = result.imported.iter().map(|i| i.name.as_str()).collect();
        let line = if names.is_empty() {
            "Nothing was imported.".to_string()
        } else {
            format!("Imported {} {}: {}.", names.len(), noun(names.len()), names.join(", "))
        };
        items.push(ui::faint(line).into());
        for s in &result.skipped {
            items.push(ui::faint(format!("Skipped {}: {}", s.name, s.reason)).into());
        }
    }
    ui::card(".env import", stack(items))
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
        assert_eq!(parse_audit(json!({ "entries": [] })).unwrap().chain_ok, None);
    }

    #[test]
    fn vault_audit_chain_message_names_the_broken_entry() {
        assert_eq!(chain_message(true, Some(5), None), "Chain intact: 5 entries verified");
        let broken = BrokenEntry { index: 2, seq: Some(3), expected: "a".into(), found: "b".into() };
        assert_eq!(chain_message(false, Some(2), Some(&broken)), "Chain broken at entry #2 (seq 3): 2 entries verified before it");
    }

    #[test]
    fn vault_audit_paths_page_and_verify() {
        assert_eq!(audit_path(50, false), "/v1/vault/audit?after=50&limit=50");
        assert_eq!(audit_path(0, true), "/v1/vault/audit?after=0&limit=50&verify=1");
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
        let body = import_body(raw, &["DB_URL".to_string()], json!({ "kind": "plaza" }), &BTreeMap::new());
        assert_eq!(body["format"], "dotenv");
        assert_eq!(body["text"], "DB_URL=postgres://x");
        assert_eq!(body["scope"]["kind"], "plaza");
    }
}
