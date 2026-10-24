//! Responsive layout and localisation (PLAN 22.9): text that does not fit its box. A visible element that holds text of its own
//! fails `text-overflow` when its content is wider than its box (`scrollWidth` past `clientWidth` by more than a pixel) and
//! its overflow is not a scroll container's (`auto` and `scroll` are the scrolling the page asked for). The check runs at
//! each declared size and locale, so a translation that is longer than the English one is found as it is.

use crate::audit::short;
use crate::tree::Snapshot;
use crate::{Issue, Kind};

/// The text that overflows its box, as `text-overflow` issues.
pub fn overflow(snapshot: &Snapshot) -> Vec<Issue> {
    snapshot
        .elements
        .iter()
        .filter(|e| {
            e.visible
                && !e.text.trim().is_empty()
                && !matches!(e.overflow_x.as_str(), "auto" | "scroll")
                && e.scroll_width > e.client_width + 1.0
        })
        .map(|e| {
            Issue::new(
                Kind::Layout,
                "text-overflow",
                e.key.clone(),
                format!(
                    "\"{}\" is {:.0}px wide in a {:.0}px box, so its end is cut off or runs past the box",
                    short(&e.text),
                    e.scroll_width,
                    e.client_width
                ),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::Element;

    fn box_with(text: &str, overflow_x: &str, scroll: f64, client: f64) -> Element {
        Element {
            key: "dom-7".into(),
            tag: "div".into(),
            text: text.into(),
            visible: true,
            overflow_x: overflow_x.into(),
            scroll_width: scroll,
            client_width: client,
            ..Element::default()
        }
    }

    #[test]
    fn a_text_wider_than_its_box_is_found() {
        // The German label of a button that fits in English: 260px of text in a 120px box.
        let snapshot = Snapshot {
            elements: vec![box_with(
                "Zahlungsbestätigung senden",
                "visible",
                260.0,
                120.0,
            )],
            ..Snapshot::default()
        };
        let issues = overflow(&snapshot);
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].rule, "text-overflow");
        assert_eq!(issues[0].kind, Kind::Layout);
        assert!(issues[0].message.contains("260px"), "{}", issues[0].message);
    }

    #[test]
    fn clipped_text_is_found_too_and_a_scroller_is_not() {
        let clipped = Snapshot {
            elements: vec![box_with("A long title", "hidden", 300.0, 100.0)],
            ..Snapshot::default()
        };
        assert_eq!(overflow(&clipped).len(), 1);
        let scroller = Snapshot {
            elements: vec![box_with("A long title", "auto", 300.0, 100.0)],
            ..Snapshot::default()
        };
        assert!(overflow(&scroller).is_empty());
    }

    #[test]
    fn text_that_fits_and_boxes_without_text_pass() {
        let fits = Snapshot {
            elements: vec![
                box_with("Pay", "visible", 40.0, 120.0),
                // Within a pixel of its box: rounding, not overflow.
                box_with("Pay", "visible", 121.0, 120.0),
                // No text of its own: its children are checked where they are.
                box_with("  ", "visible", 400.0, 120.0),
                Element {
                    visible: false,
                    ..box_with("Hidden", "visible", 400.0, 120.0)
                },
            ],
            ..Snapshot::default()
        };
        assert!(overflow(&fits).is_empty(), "{:?}", overflow(&fits));
    }
}
