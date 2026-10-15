//! The events tab.

use apz_metrics::{EVENT_KINDS, EntityKind, Event};

use super::{Ctx, panel};
use crate::model::{Body, EventRow, Panel, Status};
use crate::source::Source;

/// The panels of the tab: one list of what happened in the view's scope and window, newest first.
pub fn panels(ctx: &Ctx, src: &dyn Source) -> Vec<Panel> {
    let events = src.events(&ctx.q.scope, ctx.from_ms, ctx.to_ms, &[], 200);
    let kinds: Vec<String> = EVENT_KINDS
        .iter()
        .copied()
        .filter(|k| events.iter().any(|e| e.kind == *k))
        .map(String::from)
        .collect();
    let rows: Vec<EventRow> = events
        .iter()
        .map(|e| EventRow {
            t_ms: e.t_ms,
            time: ctx.time(e.t_ms),
            kind: e.kind.clone(),
            status: status_of_kind(&e.kind),
            subject: subject(e, src),
            scope: Some(scope_id(e)),
            detail: e.detail.clone(),
        })
        .collect();
    let count = rows.len();
    let mut events_panel = panel(
        "events",
        "Events",
        "What happened in this window, newest first",
        12,
        Body::Events { kinds, rows },
    );
    events_panel.right = format!("{count} events");
    vec![events_panel]
}

/// How serious an event kind reads: errors and alerts are critical, compactions informational, scaling, moves and
/// law changes warnings, and everything else muted.
fn status_of_kind(kind: &str) -> Status {
    match kind {
        "error" | "alert" => Status::Crit,
        "compaction" => Status::Info,
        "scale" | "move" | "law" => Status::Warn,
        _ => Status::Mute,
    }
}

/// Who or what the event is about: "ada (4417)" for an agent, "VM r12/3" for a VM, "node n1" for a node.
fn subject(e: &Event, src: &dyn Source) -> String {
    match e.entity_kind {
        EntityKind::Global => "Global".into(),
        EntityKind::Agent => match e.entity.parse::<u64>().ok().and_then(|cid| src.agent(cid)) {
            Some(info) => format!("{} ({})", info.name, info.cid),
            None => format!("agent ({})", e.entity),
        },
        EntityKind::Vm => format!("VM {}", e.entity),
        EntityKind::Node => format!("node {}", e.entity),
        kind => format!("{} {}", kind.prefix(), e.entity),
    }
}

/// The scope id the event belongs to (`node:n1`, `vm:r12/3`, `agent:4417` …), or `global`.
fn scope_id(e: &Event) -> String {
    match e.entity_kind {
        EntityKind::Global => "global".into(),
        kind => format!("{}:{}", kind.prefix(), e.entity),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::Fixture;
    use crate::model::ViewQuery;

    #[test]
    fn the_events_tab_is_one_events_panel_whose_body_is_the_event_list() {
        let src = Fixture::new(1_760_600_000_000);
        let q = ViewQuery::new("events", "global", "24h");
        let ctx = Ctx::new(&q, &src);
        let out = panels(&ctx, &src);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "events");
        assert!(matches!(out[0].body, Body::Events { .. }));
    }
}
