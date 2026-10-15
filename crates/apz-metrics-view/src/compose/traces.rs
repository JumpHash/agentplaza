//! The traces tab.

use super::{Ctx, panel};
use crate::format::format;
use crate::model::{Body, LegendItem, Panel, SpanRow, Tone, TraceDetail, TraceRow, TracesBody};
use crate::source::{AgentInfo, Source};
use apz_metrics::catalog::Unit;
use apz_metrics::trace::{Keep, SpanKind, Trace};

/// The most kept turns the list shows.
const LIST_LIMIT: usize = 50;

/// The panels of the tab.
pub fn panels(ctx: &Ctx, src: &dyn Source) -> Vec<Panel> {
    let filter = ctx.q.filter.clone().unwrap_or_else(|| "all".into());
    let traces: Vec<Trace> = src
        .traces(&ctx.q.scope, ctx.from_ms, ctx.to_ms, LIST_LIMIT)
        .into_iter()
        .filter(|t| keeps(t, &filter))
        .collect();
    let list: Vec<TraceRow> = traces
        .iter()
        .map(|t| TraceRow {
            id: t.id.clone(),
            time: ctx.time(t.start_ms),
            agent: src
                .agent(t.cid)
                .map(|a| a.name)
                .unwrap_or_else(|| format!("agent {}", t.cid)),
            total: format(Unit::Ms, t.dur_ms),
            kept: kept_key(t).into(),
            kept_text: kept_text(t),
            error: !t.ok,
        })
        .collect();
    // The selected turn is the one the query names, or else the first in the list.
    let chosen = match ctx.q.trace.as_deref().and_then(|id| src.trace(id)) {
        Some(t) => Some(t),
        None => traces.first().cloned(),
    };
    let selected = chosen.map(|t| {
        let agent = src.agent(t.cid);
        detail(&t, agent.as_ref())
    });
    vec![panel(
        "traces",
        "Turns",
        "Kept turns in this window, newest first",
        12,
        Body::Traces(TracesBody {
            filter,
            list,
            selected,
        }),
    )]
}

/// Whether a kept trace passes the filter. `all`, and any name that is not a kept reason, pass everything.
fn keeps(t: &Trace, filter: &str) -> bool {
    match filter {
        "error" | "slow" | "sampled" | "open" => kept_key(t) == filter,
        _ => true,
    }
}

/// Why the turn is listed, as the filter names it: a failed turn is an error whatever its keep reason.
fn kept_key(t: &Trace) -> &'static str {
    if !t.ok {
        return "error";
    }
    match t.kept {
        Some(Keep::Error) => "error",
        Some(Keep::Slow) => "slow",
        Some(Keep::Open) => "open",
        Some(Keep::Sampled) | None => "sampled",
    }
}

/// The reason shown in the list.
fn kept_text(t: &Trace) -> String {
    match kept_key(t) {
        "error" => "error".into(),
        "slow" => format!("slow (> p95 for {})", t.role),
        "open" => "open in view".into(),
        _ => "sampled 1%".into(),
    }
}

/// The detail of one turn: its spans as rows, offset from the turn start, with a legend of the tones.
fn detail(t: &Trace, agent: Option<&AgentInfo>) -> TraceDetail {
    let who = agent
        .map(|a| a.name.clone())
        .unwrap_or_else(|| format!("agent {}", t.cid));
    let run = agent.map(|a| a.run.clone()).unwrap_or_else(|| "—".into());
    let total = format(Unit::Ms, t.dur_ms);
    let spans = t
        .spans
        .iter()
        .map(|s| {
            let offset = s.start_ms as f64 - t.start_ms as f64;
            let mut attrs = s.attrs.clone();
            attrs.push(("start".into(), format(Unit::Ms, offset)));
            attrs.push(("duration".into(), format(Unit::Ms, s.dur_ms)));
            SpanRow {
                name: s.name.clone(),
                depth: if s.parent.is_some() { 1 } else { 0 },
                tone: span_tone(s.kind),
                start_ms: offset,
                dur_ms: s.dur_ms,
                first_token_ms: s.first_token_ms,
                error: !s.ok,
                attrs,
            }
        })
        .collect();
    TraceDetail {
        id: t.id.clone(),
        title: format!("Turn {}", t.id),
        sub: format!("{who} ({}) · {} · run {run} · {total}", t.cid, t.role),
        error: !t.ok,
        total_ms: t.dur_ms,
        spans,
        legend: legend(),
    }
}

/// The colour of a span by its kind.
fn span_tone(kind: SpanKind) -> Tone {
    match kind {
        SpanKind::Model => Tone::C1,
        SpanKind::Tool => Tone::C3,
        SpanKind::HubMcp => Tone::C4,
        SpanKind::Wake | SpanKind::Restore | SpanKind::Ready => Tone::C5,
    }
}

/// The legend of the span tones.
fn legend() -> Vec<LegendItem> {
    [
        ("waiting for first token", Tone::C1),
        ("model streaming", Tone::C1),
        ("tool", Tone::C3),
        ("hub MCP", Tone::C4),
        ("VM wake", Tone::C5),
    ]
    .into_iter()
    .map(|(label, tone)| LegendItem {
        label: label.into(),
        tone,
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::Fixture;
    use crate::model::ViewQuery;
    use apz_metrics::trace::Span;

    fn span(
        id: u32,
        parent: Option<u32>,
        kind: SpanKind,
        start_ms: u64,
        dur_ms: f64,
        ok: bool,
    ) -> Span {
        Span {
            id,
            parent,
            kind,
            name: format!("span {id}"),
            start_ms,
            dur_ms,
            first_token_ms: None,
            ok,
            attrs: Vec::new(),
        }
    }

    #[test]
    fn a_turn_becomes_a_detail_with_child_depths_tones_offsets_and_the_failed_span_flagged() {
        let mut model = span(2, Some(1), SpanKind::Model, 1_000_900, 2_000.0, true);
        model.first_token_ms = Some(350.0);
        let trace = Trace {
            id: "t-1".into(),
            cid: 4417,
            role: "builder".into(),
            start_ms: 1_000_000,
            dur_ms: 5_000.0,
            ok: false,
            spans: vec![
                span(1, None, SpanKind::Wake, 1_000_000, 800.0, true),
                model,
                span(3, Some(1), SpanKind::Tool, 1_003_000, 400.0, false),
            ],
            kept: Some(Keep::Error),
        };
        let agent = AgentInfo {
            cid: 4417,
            name: "forge".into(),
            role: "builder".into(),
            run: "r12".into(),
            ..Default::default()
        };

        let d = detail(&trace, Some(&agent));

        assert_eq!(d.id, "t-1");
        assert_eq!(d.title, "Turn t-1");
        assert_eq!(
            d.sub,
            format!(
                "forge (4417) · builder · run r12 · {}",
                format(Unit::Ms, 5_000.0)
            )
        );
        assert!(d.error);
        assert_eq!(d.total_ms, 5_000.0);
        assert_eq!(d.spans.len(), 3);
        assert_eq!(d.spans[0].depth, 0);
        assert_eq!(d.spans[1].depth, 1);
        assert_eq!(d.spans[2].depth, 1);
        assert_eq!(d.spans[0].tone, Tone::C5);
        assert_eq!(d.spans[1].tone, Tone::C1);
        assert_eq!(d.spans[1].first_token_ms, Some(350.0));
        assert_eq!(d.spans[2].tone, Tone::C3);
        assert!(!d.spans[1].error);
        assert!(d.spans[2].error);
        assert_eq!(d.spans[0].start_ms, 0.0);
        assert_eq!(d.spans[1].start_ms, 900.0);
        assert_eq!(d.spans[2].start_ms, 3_000.0);
    }

    #[test]
    fn composing_the_traces_tab_returns_one_traces_panel() {
        let fixture = Fixture::new(1_760_600_000_000);
        let view = crate::compose::compose(&ViewQuery::new("traces", "global", "24h"), &fixture);

        assert_eq!(view.tab, "traces");
        assert_eq!(view.panels.len(), 1);
        assert_eq!(view.panels[0].id, "traces");
        assert!(matches!(view.panels[0].body, Body::Traces(_)));
    }
}
