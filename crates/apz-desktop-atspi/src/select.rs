//! Which control a selector picks, by role and name. The app and the path are the tree walk's to check.

use apz_station::Selector;

/// Whether a control with this normalised role and name is one the selector picks. Every role or name the
/// selector gives must match. A selector with neither (a path alone) is not decided here.
pub(crate) fn selects(selector: &Selector, role: &str, name: Option<&str>) -> bool {
    if selector.role.is_none() && selector.name.is_none() {
        return false;
    }
    let role_matches = selector.role.as_deref().is_none_or(|wanted| wanted == role);
    let name_matches = selector
        .name
        .as_deref()
        .is_none_or(|wanted| name == Some(wanted));
    role_matches && name_matches
}

#[cfg(test)]
mod tests {
    use apz_desktop::UiNode;
    use apz_station::Selector;

    use super::selects;

    fn node(role: &str, name: Option<&str>, children: Vec<UiNode>) -> UiNode {
        UiNode {
            id: 0,
            role: role.into(),
            name: name.map(Into::into),
            value: None,
            bounds: (0, 0, 0, 0),
            actions: Vec::new(),
            children,
        }
    }

    /// The fixture: a window with a button, an entry and a label, in document order.
    fn fixture() -> UiNode {
        node(
            "application",
            Some("apz-test-app"),
            vec![node(
                "window",
                Some("Apz Test App"),
                vec![
                    node("button", Some("Ping"), Vec::new()),
                    node("text_field", Some("Name"), Vec::new()),
                    node("text", None, Vec::new()),
                ],
            )],
        )
    }

    /// The child-index path of the first control in document order that the selector picks.
    fn first_match(root: &UiNode, selector: &Selector) -> Option<Vec<u32>> {
        fn walk(node: &UiNode, selector: &Selector, trail: &mut Vec<u32>) -> Option<Vec<u32>> {
            if selects(selector, &node.role, node.name.as_deref()) {
                return Some(trail.clone());
            }
            for (index, child) in node.children.iter().enumerate() {
                trail.push(index as u32);
                if let Some(found) = walk(child, selector, trail) {
                    return Some(found);
                }
                trail.pop();
            }
            None
        }
        walk(root, selector, &mut Vec::new())
    }

    fn sel(role: Option<&str>, name: Option<&str>) -> Selector {
        Selector {
            role: role.map(Into::into),
            name: name.map(Into::into),
            ..Selector::default()
        }
    }

    #[test]
    fn selectors_match_on_role_and_name_in_document_order() {
        let tree = fixture();
        // (case, role, name, the path the selector should pick)
        for (case, role, name, want) in [
            (
                "a role and a name must both match",
                Some("button"),
                Some("Ping"),
                Some(vec![0, 0]),
            ),
            (
                "a role with another name does not match",
                Some("button"),
                Some("Name"),
                None,
            ),
            (
                "a name with another role does not match",
                Some("text_field"),
                Some("Ping"),
                None,
            ),
            (
                "a role alone picks the first match in document order",
                Some("text_field"),
                None,
                Some(vec![0, 1]),
            ),
            (
                "a name alone picks the first match in document order",
                None,
                Some("Apz Test App"),
                Some(vec![0]),
            ),
            (
                "a selector with neither role nor name picks nothing",
                None,
                None,
                None,
            ),
            (
                "an unnamed control is picked by its role",
                Some("text"),
                None,
                Some(vec![0, 2]),
            ),
            (
                "a named selector does not pick an unnamed control",
                Some("text"),
                Some("anything"),
                None,
            ),
        ] {
            assert_eq!(first_match(&tree, &sel(role, name)), want, "{case}");
        }
        // A selector with neither role nor name selects no control, even a named button.
        assert!(!selects(&sel(None, None), "button", Some("Ping")));
    }
}
