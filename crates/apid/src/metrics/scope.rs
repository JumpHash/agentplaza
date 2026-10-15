//! Scopes on the hub: the tree the consoles show, the entities under a scope, and which series of a metric fall
//! under it. Built from the roster (agents, roles, placement), the nodes and their runs and VMs (the latest
//! reports), workstations, sandboxes, the providers and their models.
//!
//! Scope ids: `global`, `node:<id>`, `run:<id>`, `vm:<run>/<index>`, `ws:<id>`, `sb:<id>`, `agent:<cid>`,
//! `role:<role>`, `provider:<id>`, `model:<provider>/<model>`. Series entities: a node's id (`n1`), a run's id, a
//! VM's `<run>/<index>`, an agent's cid, a volume's `<node>:<mount>`, a service's `<node>:<name>`, a tool's or a
//! route's name.

use apz_metrics::SeriesKey;
use apz_metrics_view::ScopeNode;
use apz_metrics_view::source::{AgentInfo, Entity};

use crate::state::App;

/// The scope tree in order (Global; Nodes with their runs, VMs, workstations and sandboxes; Agents by role;
/// Providers with their models), with statuses (a node offline or a volume over 90 % is crit; an agent over 90 %
/// of its window is warn; an asleep VM is mute).
pub fn tree(app: &App) -> Vec<ScopeNode> {
    let _ = app;
    todo!()
}

/// The entities of `kind` under `scope` (kinds: node, run, vm, agent, role, provider, model, volume, service, tool,
/// route), with labels and, for VMs and agents, their state.
pub fn entities(app: &App, scope: &str, kind: &str) -> Vec<Entity> {
    let _ = (app, scope, kind);
    todo!()
}

/// What the view needs of an agent.
pub fn agent(app: &App, cid: u64) -> Option<AgentInfo> {
    let _ = (app, cid);
    todo!()
}

/// A filter for the series of a metric keyed by `kind` that fall under `scope`. Built once per read (it resolves
/// the scope's members up front), then called per series.
pub fn filter(
    app: &App,
    scope: &str,
    kind: apz_metrics::EntityKind,
) -> Box<dyn Fn(&SeriesKey) -> bool + Send> {
    let _ = (app, scope, kind);
    todo!()
}

/// The group a series falls in for an agent property (`role`, `run`, `node`, `engine`), with that group's scope id.
pub fn agent_prop(app: &App, key: &SeriesKey, prop: &str) -> Option<(String, Option<String>)> {
    let _ = (app, key, prop);
    todo!()
}

/// A label for an entity of a kind ("n1 · rack-a", "ada (4417)", "VM r12/3").
pub fn label(app: &App, kind: apz_metrics::EntityKind, entity: &str) -> String {
    let _ = (app, kind, entity);
    todo!()
}
