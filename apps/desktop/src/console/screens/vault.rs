//! The Vault screen (first slice of the web console's Vault view): the vault's status (sealed or
//! unsealed, slots, entry count), the entry list (name, kind, scope, bindings, version, last used),
//! and a node selector that filters the list by scope. The hub never returns a secret value, and
//! this screen never shows, requests or sends one. Unsealing happens in the web console or with
//! `agentplaza vault unseal`; set and remove are not in this slice.

use std::fmt;

use iced::widget::{column, pick_list, row, text};
use iced::{Element, Length, Task};
use serde::Deserialize;

use super::Id;
use crate::console::{client, ui, Ctx};

// ---- The hub's shapes (as apps/web/src/api.ts types them, trimmed to what this screen shows) -----

/// `GET /v1/vault`: the state and the counts.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct VaultStatus {
    /// `uninitialized`, `sealed`, `unsealed` or `tampered`.
    pub state: String,
    pub slots: Vec<serde_json::Value>,
    pub entries: u64,
    pub audit_ok: bool,
    /// The reason the vault is tampered, when it is.
    pub tampered: Option<String>,
}

/// The scope of an entry: the whole plaza, or the listed nodes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum VaultScope {
    #[default]
    Plaza,
    Nodes { nodes: Vec<String> },
}

/// One entry from `GET /v1/vault/secrets` (metadata only; the value is never returned).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct VaultEntry {
    pub name: String,
    pub kind: String,
    pub scope: VaultScope,
    pub bindings: Vec<String>,
    pub current: u64,
    pub last_used_ms: f64,
    pub use_count: u64,
}

/// A machine from `GET /v1/nodes`, for the selector and the scope names.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct NodeRef {
    pub id: String,
    pub name: String,
}

// ---- Filtering --------------------------------------------------------------------------------

/// What the entry list shows: everything, the plaza-scoped entries, or the entries a node is in scope for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Selection {
    #[default]
    All,
    Plaza,
    Node(String),
}

/// Whether an entry is shown under a selection. A node sees only the entries that list it.
pub fn in_selection(entry: &VaultEntry, selection: &Selection) -> bool {
    match (selection, &entry.scope) {
        (Selection::All, _) => true,
        (Selection::Plaza, VaultScope::Plaza) => true,
        (Selection::Plaza, VaultScope::Nodes { .. }) => false,
        (Selection::Node(id), VaultScope::Nodes { nodes }) => nodes.iter().any(|n| n == id),
        (Selection::Node(_), VaultScope::Plaza) => false,
    }
}

/// The entries a selection shows, in their loaded order.
pub fn filter_entries<'a>(entries: &'a [VaultEntry], selection: &Selection) -> Vec<&'a VaultEntry> {
    entries.iter().filter(|e| in_selection(e, selection)).collect()
}

/// One option of the selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub selection: Selection,
    pub label: String,
}

impl fmt::Display for Choice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

/// All, Plaza, then each node.
fn choices(nodes: &[NodeRef]) -> Vec<Choice> {
    let mut out = vec![
        Choice { selection: Selection::All, label: "All".into() },
        Choice { selection: Selection::Plaza, label: "Plaza".into() },
    ];
    out.extend(nodes.iter().map(|n| Choice {
        selection: Selection::Node(n.id.clone()),
        label: if n.name.is_empty() { n.id.clone() } else { n.name.clone() },
    }));
    out
}

fn node_name(nodes: &[NodeRef], id: &str) -> String {
    nodes
        .iter()
        .find(|n| n.id == id)
        .map(|n| if n.name.is_empty() { n.id.clone() } else { n.name.clone() })
        .unwrap_or_else(|| id.to_string())
}

/// "never", or how long ago, from epoch milliseconds.
fn ago(ms: f64, now_ms: f64) -> String {
    if ms <= 0.0 {
        return "never".into();
    }
    let secs = ((now_ms - ms) / 1000.0).max(0.0) as u64;
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

// ---- State, messages, loading -----------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct State {
    /// None until the first status answer.
    pub status: Option<VaultStatus>,
    /// Loaded only while the vault is unsealed.
    pub entries: Vec<VaultEntry>,
    pub nodes: Vec<NodeRef>,
    pub selection: Selection,
    /// A load error, shown above the sections.
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Status(Result<VaultStatus, String>),
    Entries(Result<Vec<VaultEntry>, String>),
    Nodes(Result<Vec<NodeRef>, String>),
    Pick(Choice),
}

pub fn go(_msg: &Msg) -> Option<Id> {
    None
}

/// Loads the screen's data (called when it opens and on every poll tick).
pub fn load(_state: &mut State, ctx: &Ctx) -> Task<Msg> {
    Task::batch([fetch::<VaultStatus>(ctx, "/v1/vault", Msg::Status), fetch::<Vec<NodeRef>>(ctx, "/v1/nodes", Msg::Nodes)])
}

fn fetch<T>(ctx: &Ctx, path: &str, map: fn(Result<T, String>) -> Msg) -> Task<Msg>
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    let client = ctx.client.clone();
    let path = path.to_string();
    client::task(move || client.get::<T>(&path), map)
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Status(Ok(status)) => {
            state.error = None;
            let unsealed = status.state == "unsealed";
            state.status = Some(status);
            if unsealed {
                fetch::<Vec<VaultEntry>>(ctx, "/v1/vault/secrets", Msg::Entries)
            } else {
                state.entries.clear();
                Task::none()
            }
        }
        Msg::Status(Err(error)) => {
            state.error = Some(error);
            Task::none()
        }
        Msg::Entries(Ok(entries)) => {
            state.entries = entries;
            Task::none()
        }
        Msg::Entries(Err(error)) => {
            state.error = Some(error);
            Task::none()
        }
        Msg::Nodes(Ok(nodes)) => {
            state.nodes = nodes;
            Task::none()
        }
        Msg::Nodes(Err(error)) => {
            state.error = Some(error);
            Task::none()
        }
        Msg::Pick(choice) => {
            state.selection = choice.selection;
            Task::none()
        }
    }
}

// ---- View -------------------------------------------------------------------------------------

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = vec![ui::faint("Values are write-only: the vault never shows or returns a secret.").into()];
    if let Some(error) = &state.error {
        parts.push(ui::error_line(error));
    }
    match &state.status {
        None => parts.push(ui::loading()),
        Some(status) => {
            parts.push(header(status));
            match status.state.as_str() {
                "unsealed" => {
                    parts.push(selector(state));
                    parts.push(entry_list(state, now_ms()));
                }
                "sealed" => parts.push(ui::card(
                    "Sealed",
                    ui::faint("The vault is sealed. Unseal in the web console or with `agentplaza vault unseal`.").into(),
                )),
                "tampered" => parts.push(ui::card(
                    "Tampered",
                    ui::error_line(status.tampered.as_deref().unwrap_or("The vault failed its integrity check.")),
                )),
                _ => parts.push(ui::card("Not set up", ui::faint("The vault has not been set up. Set it up in the web console.").into())),
            }
        }
    }
    ui::sheet(ui::view_head("Vault", vec![]), column(parts).spacing(16).into())
}

fn header<'a>(status: &VaultStatus) -> Element<'a, Msg> {
    let state = match status.state.as_str() {
        "unsealed" => "Unsealed",
        "sealed" => "Sealed",
        "uninitialized" => "Not set up",
        "tampered" => "Tampered",
        other => other,
    };
    let audit = if status.audit_ok { "audit ok" } else { "audit failed" };
    let line = format!("{state}  ·  {} key slots  ·  {} entries  ·  {audit}", status.slots.len(), status.entries);
    ui::card("Status", text(line).into())
}

fn selector<'a>(state: &'a State) -> Element<'a, Msg> {
    let options = choices(&state.nodes);
    let current = options.iter().find(|c| c.selection == state.selection).cloned().unwrap_or_else(|| options[0].clone());
    row(vec![ui::faint("Scope").into(), pick_list(options, Some(current), Msg::Pick).into()]).spacing(12).into()
}

fn entry_list<'a>(state: &'a State, now_ms: f64) -> Element<'a, Msg> {
    let shown = filter_entries(&state.entries, &state.selection);
    let mut rows: Vec<Element<'a, Msg>> = Vec::new();
    if shown.is_empty() {
        rows.push(ui::empty("key", "No entries", "Nothing in this scope yet."));
    }
    for (i, entry) in shown.iter().enumerate() {
        if i > 0 {
            rows.push(ui::hairline());
        }
        rows.push(entry_row(entry, &state.nodes, now_ms));
    }
    ui::card(&format!("Entries ({})", shown.len()), column(rows).spacing(8).into())
}

fn entry_row<'a>(entry: &VaultEntry, nodes: &[NodeRef], now_ms: f64) -> Element<'a, Msg> {
    let scope = match &entry.scope {
        VaultScope::Plaza => "Plaza".to_string(),
        VaultScope::Nodes { nodes: ids } => ids.iter().map(|id| node_name(nodes, id)).collect::<Vec<_>>().join(", "),
    };
    let bindings = if entry.bindings.is_empty() { "no bindings".to_string() } else { entry.bindings.join(", ") };
    let top: Element<'a, Msg> = row(vec![
        text(entry.name.clone()).width(Length::FillPortion(3)).into(),
        text(entry.kind.clone()).width(Length::FillPortion(2)).into(),
        text(scope).width(Length::FillPortion(3)).into(),
        text(format!("v{}", entry.current)).width(Length::FillPortion(1)).into(),
    ])
    .spacing(12)
    .into();
    let detail = ui::faint(format!("Bindings: {bindings}  ·  used {} times, last {}", entry.use_count, ago(entry.last_used_ms, now_ms)));
    column(vec![top, detail.into()]).spacing(4).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, scope: VaultScope) -> VaultEntry {
        VaultEntry { name: name.into(), scope, ..Default::default() }
    }

    fn names(entries: Vec<&VaultEntry>) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn vault_filter_by_node_selection() {
        let entries = vec![
            entry("plaza-key", VaultScope::Plaza),
            entry("n0-only", VaultScope::Nodes { nodes: vec!["n0".into()] }),
            entry("both", VaultScope::Nodes { nodes: vec!["n0".into(), "n1".into()] }),
        ];
        assert_eq!(names(filter_entries(&entries, &Selection::All)), vec!["plaza-key", "n0-only", "both"]);
        assert_eq!(names(filter_entries(&entries, &Selection::Plaza)), vec!["plaza-key"]);
        assert_eq!(names(filter_entries(&entries, &Selection::Node("n0".into()))), vec!["n0-only", "both"]);
        assert_eq!(names(filter_entries(&entries, &Selection::Node("n1".into()))), vec!["both"]);
        assert!(filter_entries(&entries, &Selection::Node("n9".into())).is_empty());
    }
}
