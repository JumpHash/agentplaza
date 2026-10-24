//! Axe-style accessibility rules over a page's snapshot (PLAN 22.8, 22.9). Each rule names what it checks and the element
//! it is about; a page passes when no rule fails.
//!
//! - `html-has-lang`: the document declares its language.
//! - `control-name`: a control a person acts on (button, link, field, tick box, …) has an accessible name.
//! - `image-alt`: an image that is not hidden from assistive technology has a text alternative.
//! - `color-contrast`: text is at least 4.5:1 against its background (3:1 for large text: 24px, or bold from 18.66px).
//! - `duplicate-id`: no id is used by two elements.
//! - `focus-order`: no element takes focus before the ones that precede it in the page (a positive `tabindex`), and focus
//!   does not jump back up the page within one column.
//! - `focus-hidden`: a control a keyboard reaches is not hidden from assistive technology (the tab stop is skipped).

use std::collections::BTreeMap;

use crate::tree::{Element, Snapshot};
use crate::{Issue, Kind};

/// The roles a person acts on: each needs a name a screen reader can say.
const CONTROLS: &[&str] = &[
    "button",
    "checkbox",
    "combobox",
    "link",
    "listbox",
    "menuitem",
    "radio",
    "searchbox",
    "slider",
    "switch",
    "tab",
    "textbox",
];

/// The roles of images that need a text alternative.
const IMAGES: &[&str] = &["img", "image"];

/// The luminance of an opaque colour (WCAG 2.x, relative luminance of sRGB 0 to 255 channels).
pub fn luminance(colour: [f64; 3]) -> f64 {
    let linear = |v: f64| {
        let s = v / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(colour[0]) + 0.7152 * linear(colour[1]) + 0.0722 * linear(colour[2])
}

/// The contrast ratio of two opaque colours, from 1 to 21 (WCAG 2.x).
pub fn contrast(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// The contrast the text needs: 3:1 for large text, 4.5:1 otherwise.
pub fn needed_contrast(element: &Element) -> f64 {
    if element.font_px >= 24.0 || (element.bold && element.font_px >= 18.66) {
        3.0
    } else {
        4.5
    }
}

/// The audit of a snapshot: every rule's failures, in the order the rules run.
pub fn audit(snapshot: &Snapshot) -> Vec<Issue> {
    let mut out = Vec::new();
    names(snapshot, &mut out);
    if snapshot.lang.trim().is_empty() {
        out.push(Issue::new(
            Kind::Accessibility,
            "html-has-lang",
            "html",
            "the document declares no language, so a screen reader cannot pick its voice",
        ));
    }
    contrast_of_text(snapshot, &mut out);
    duplicate_ids(snapshot, &mut out);
    focus(snapshot, &mut out);
    out
}

/// A node's element key, or its role when it has none (what an issue names).
fn target(key: &str, role: &str) -> String {
    if key.is_empty() {
        role.to_string()
    } else {
        key.to_string()
    }
}

/// The rules over names: `control-name` and `image-alt`.
fn names(snapshot: &Snapshot, out: &mut Vec<Issue>) {
    for node in snapshot.nodes.iter().filter(|n| !n.ignored) {
        if node.name.trim().is_empty() {
            if CONTROLS.contains(&node.role.as_str()) {
                out.push(Issue::new(
                    Kind::Accessibility,
                    "control-name",
                    target(&node.key, &node.role),
                    format!(
                        "a {} has no accessible name, so a screen reader says only \"{}\"",
                        node.role, node.role
                    ),
                ));
            } else if IMAGES.contains(&node.role.as_str()) {
                out.push(Issue::new(
                    Kind::Accessibility,
                    "image-alt",
                    target(&node.key, &node.role),
                    "an image has no text alternative",
                ));
            }
        }
    }
}

/// `color-contrast`: the text of each visible element, against the colour behind it.
fn contrast_of_text(snapshot: &Snapshot, out: &mut Vec<Issue>) {
    for element in &snapshot.elements {
        let (Some(fg), Some(bg)) = (element.fg, element.bg) else {
            continue;
        };
        if !element.visible || element.text.trim().is_empty() {
            continue;
        }
        let ratio = contrast(fg, bg);
        let needed = needed_contrast(element);
        if ratio < needed {
            out.push(Issue::new(
                Kind::Accessibility,
                "color-contrast",
                element.key.clone(),
                format!(
                    "\"{}\" is {ratio:.2}:1 against its background; {needed}:1 is needed",
                    short(&element.text)
                ),
            ));
        }
    }
}

/// `duplicate-id`: each id that more than one element carries.
fn duplicate_ids(snapshot: &Snapshot, out: &mut Vec<Issue>) {
    let mut ids: BTreeMap<&str, usize> = BTreeMap::new();
    for element in snapshot.elements.iter().filter(|e| !e.id.is_empty()) {
        *ids.entry(element.id.as_str()).or_default() += 1;
    }
    for (id, count) in ids.into_iter().filter(|(_, n)| *n > 1) {
        out.push(Issue::new(
            Kind::Accessibility,
            "duplicate-id",
            format!("#{id}"),
            format!(
                "the id \"{id}\" is on {count} elements; a label that names it finds the wrong one"
            ),
        ));
    }
}

/// `focus-order` and `focus-hidden`: the keyboard's order against the page's.
fn focus(snapshot: &Snapshot, out: &mut Vec<Issue>) {
    for element in snapshot
        .elements
        .iter()
        .filter(|e| e.focusable && e.visible)
    {
        if let Some(index) = element.tabindex
            && index > 0
        {
            out.push(Issue::new(
                Kind::Accessibility,
                "focus-order",
                element.key.clone(),
                format!(
                    "tabindex {index} takes focus out of the page's order: it is reached before the controls that precede it"
                ),
            ));
        }
        if snapshot.ignored(&element.key) {
            out.push(Issue::new(
                Kind::Accessibility,
                "focus-hidden",
                element.key.clone(),
                "a control a keyboard reaches is hidden from assistive technology",
            ));
        }
    }
    let stops: Vec<&Element> = snapshot
        .elements
        .iter()
        .filter(|e| e.focusable && e.visible && e.tabindex.unwrap_or(0) == 0)
        .collect();
    for pair in stops.windows(2) {
        let (prev, next) = (pair[0], pair[1]);
        let [px, py, pw, ph] = prev.rect;
        let [nx, ny, nw, _] = next.rect;
        let same_column = nx < px + pw && px < nx + nw;
        // Up by more than a whole control: the focus went back up the page.
        if same_column && ny + ph < py {
            out.push(Issue::new(
                Kind::Accessibility,
                "focus-order",
                next.key.clone(),
                format!(
                    "focus jumps back up the page, from a control at y={py:.0} to one at y={ny:.0}"
                ),
            ));
        }
    }
}

/// The first 40 characters of a text, with an ellipsis when cut.
pub fn short(text: &str) -> String {
    let mut out: String = text.chars().take(40).collect();
    if text.chars().count() > 40 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::AxNode;

    fn node(key: &str, role: &str, name: &str) -> AxNode {
        AxNode {
            key: key.into(),
            role: role.into(),
            name: name.into(),
            ignored: false,
        }
    }

    /// A visible element with `text` in `fg` on `bg`, 16 px.
    fn text(key: &str, text: &str, fg: [f64; 3], bg: [f64; 3]) -> Element {
        Element {
            key: key.into(),
            tag: "p".into(),
            text: text.into(),
            visible: true,
            fg: Some(fg),
            bg: Some(bg),
            font_px: 16.0,
            rect: [0.0, 0.0, 200.0, 20.0],
            overflow_x: "visible".into(),
            client_width: 200.0,
            scroll_width: 200.0,
            ..Element::default()
        }
    }

    fn rules(issues: &[Issue]) -> Vec<&'static str> {
        issues.iter().map(|i| i.rule).collect()
    }

    /// A page that passes every rule: a language, a named button and link, a labelled image, readable text.
    fn clean() -> Snapshot {
        Snapshot {
            lang: "en".into(),
            nodes: vec![
                node("dom-1", "RootWebArea", "Shop"),
                node("dom-2", "button", "Pay"),
                node("dom-3", "link", "Back to shop"),
                node("dom-4", "img", "A receipt icon"),
                node("dom-5", "StaticText", "Your order"),
            ],
            elements: vec![
                text(
                    "dom-5",
                    "Your order",
                    [0.0, 0.0, 0.0],
                    [255.0, 255.0, 255.0],
                ),
                Element {
                    key: "dom-2".into(),
                    tag: "button".into(),
                    focusable: true,
                    visible: true,
                    rect: [0.0, 40.0, 60.0, 30.0],
                    tabindex: None,
                    ..Element::default()
                },
                Element {
                    key: "dom-3".into(),
                    tag: "a".into(),
                    focusable: true,
                    visible: true,
                    rect: [80.0, 40.0, 90.0, 30.0],
                    tabindex: None,
                    ..Element::default()
                },
            ],
        }
    }

    #[test]
    fn a_clean_page_passes_every_rule() {
        let issues = audit(&clean());
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn a_seeded_missing_label_is_found() {
        // The fixture a page with an icon-only button and no label would give: the button's name is empty.
        let mut page = clean();
        page.nodes[1].name = String::new();
        let issues = audit(&page);
        assert_eq!(rules(&issues), vec!["control-name"], "{issues:?}");
        assert_eq!(issues[0].kind, Kind::Accessibility);
        assert_eq!(issues[0].element, "dom-2");
        assert!(
            issues[0].message.contains("button"),
            "{}",
            issues[0].message
        );
    }

    #[test]
    fn an_image_with_no_alternative_and_a_link_with_no_name_are_found() {
        let mut page = clean();
        page.nodes[2].name = "  ".into();
        page.nodes[3].name = String::new();
        let found = rules(&audit(&page));
        assert_eq!(found, vec!["control-name", "image-alt"], "{found:?}");
    }

    #[test]
    fn a_decorative_image_hidden_from_assistive_technology_is_not_an_issue() {
        let mut page = clean();
        page.nodes[3].name = String::new();
        page.nodes[3].ignored = true;
        assert!(audit(&page).is_empty());
    }

    #[test]
    fn low_contrast_text_is_found_and_large_text_is_judged_by_its_own_bar() {
        // #bbb on white is 1.92:1: it fails the 4.5:1 bar for body text.
        let mut page = clean();
        page.elements[0] = text(
            "dom-5",
            "Faint",
            [187.0, 187.0, 187.0],
            [255.0, 255.0, 255.0],
        );
        let issues = audit(&page);
        assert_eq!(rules(&issues), vec!["color-contrast"], "{issues:?}");
        assert!(issues[0].message.contains("1.9"), "{}", issues[0].message);
        // The same colour at 24 px is large text, which needs 3:1: 1.92 still fails.
        page.elements[0].font_px = 24.0;
        assert_eq!(rules(&audit(&page)), vec!["color-contrast"]);
        // A grey of 4.6:1 passes the body bar: #767676 on white is 4.54:1.
        page.elements[0] = text(
            "dom-5",
            "Quiet",
            [118.0, 118.0, 118.0],
            [255.0, 255.0, 255.0],
        );
        assert!(audit(&page).is_empty(), "{:?}", audit(&page));
    }

    #[test]
    fn contrast_is_the_wcag_ratio() {
        let black_on_white = contrast([0.0, 0.0, 0.0], [255.0, 255.0, 255.0]);
        assert!((black_on_white - 21.0).abs() < 1e-9, "{black_on_white}");
        let same = contrast([10.0, 20.0, 30.0], [10.0, 20.0, 30.0]);
        assert!((same - 1.0).abs() < 1e-9);
    }

    #[test]
    fn text_over_an_unknown_background_is_not_judged() {
        let mut page = clean();
        let mut faint = text("dom-5", "On a photo", [187.0, 187.0, 187.0], [0.0; 3]);
        faint.bg = None;
        page.elements[0] = faint;
        assert!(audit(&page).is_empty());
    }

    #[test]
    fn a_missing_language_and_a_duplicate_id_are_found() {
        let mut page = clean();
        page.lang = String::new();
        page.elements[1].id = "pay".into();
        page.elements[2].id = "pay".into();
        let found = rules(&audit(&page));
        assert_eq!(found, vec!["html-has-lang", "duplicate-id"], "{found:?}");
    }

    #[test]
    fn a_positive_tabindex_and_a_jump_back_up_the_page_are_found() {
        let mut page = clean();
        page.elements[1].tabindex = Some(2);
        let found = audit(&page);
        assert_eq!(rules(&found), vec!["focus-order"], "{found:?}");

        // A control below, then one above it in the same column: focus jumps back up.
        let mut back = clean();
        back.elements[1].rect = [0.0, 200.0, 60.0, 30.0];
        back.elements[2].rect = [0.0, 40.0, 60.0, 30.0];
        let found = audit(&back);
        assert_eq!(rules(&found), vec!["focus-order"], "{found:?}");
        assert!(found[0].message.contains("back up"), "{}", found[0].message);
        // The same two in different columns are a grid, not a jump.
        let mut grid = back.clone();
        grid.elements[2].rect = [300.0, 40.0, 60.0, 30.0];
        assert!(audit(&grid).is_empty());
    }

    #[test]
    fn a_focusable_control_hidden_from_assistive_technology_is_found() {
        let mut page = clean();
        page.nodes[1].ignored = true;
        let issues = audit(&page);
        assert_eq!(rules(&issues), vec!["focus-hidden"], "{issues:?}");
    }
}
