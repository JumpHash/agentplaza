//! The dash tab: the owner's pinned panels, each composed from its own tab and scope.

use super::{Ctx, panel};
use crate::model::{Body, Panel, PinRef, ViewQuery, starter_pins};
use crate::source::Source;
use crate::tabs;

/// The panels of the tab: an intro, then each pinned panel at half width. A pin whose tab or panel no longer
/// exists is skipped.
pub fn panels(ctx: &Ctx, src: &dyn Source) -> Vec<Panel> {
    let saved = src.saved();
    let pins = if saved.starter {
        starter_pins()
    } else {
        saved.pins
    };
    let mut text = String::from(
        "Pinned panels keep their own scope. Pin any panel with ⋯ → Pin to My dashboard.",
    );
    if pins.is_empty() {
        text.push_str(" Nothing pinned yet.");
    }
    let mut out = vec![panel(
        "intro",
        tabs::label("dash"),
        "",
        12,
        Body::Empty { text },
    )];
    for pin in pins {
        // A dashboard never shows itself, which also keeps the composition from recursing.
        if pin.tab == "dash" {
            continue;
        }
        let query = ViewQuery {
            tab: pin.tab.clone(),
            scope: pin.scope.clone(),
            ..ctx.q.clone()
        };
        let view = crate::compose::compose(&query, src);
        // A tab that does not apply to the pin's scope composes another tab instead; its panels are not the pin.
        if view.tab != pin.tab {
            continue;
        }
        let from = format!("{} · {}", tabs::label(&pin.tab), view.scope.label);
        let Some(mut found) = view.panels.into_iter().find(|p| p.id == pin.panel) else {
            continue;
        };
        found.span = 6;
        found.pinned = Some(PinRef {
            tab: pin.tab,
            scope: pin.scope,
            panel: pin.panel,
            from,
        });
        out.push(found);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::Fixture;

    #[test]
    fn dash_leads_with_the_intro_and_then_only_pins_whose_panels_exist_at_half_width() {
        let src = Fixture::new(1_760_600_000_000);
        let q = ViewQuery::new("dash", "global", "24h");
        let ctx = Ctx::new(&q, &src);
        let out = panels(&ctx, &src);
        assert_eq!(out[0].id, "intro");
        assert!(matches!(out[0].body, Body::Empty { .. }));
        for p in &out[1..] {
            assert_eq!(p.span, 6);
            assert!(p.pinned.is_some());
        }
        let saved = src.saved();
        let pins = if saved.starter {
            starter_pins()
        } else {
            saved.pins
        };
        assert!(out.len() - 1 <= pins.len());
    }
}
