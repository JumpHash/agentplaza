//! UI Automation control types and patterns, as the tree and the actions speak them.

/// The role name of a UI Automation control type (`UIA_*ControlTypeId`, 50000 to 50040).
pub fn role_name(control_type: i32) -> &'static str {
    match control_type {
        50000 => "button",
        50001 => "calendar",
        50002 => "check_box",
        50003 => "combo_box",
        50004 => "text_field",
        50005 => "link",
        50006 => "image",
        50007 => "list_item",
        50008 => "list",
        50009 => "menu",
        50010 => "menu_bar",
        50011 => "menu_item",
        50012 => "progress_bar",
        50013 => "radio_button",
        50014 => "scroll_bar",
        50015 => "slider",
        50016 => "spin_button",
        50017 => "status_bar",
        50018 => "tab_list",
        50019 => "tab",
        50020 => "static_text",
        50021 => "tool_bar",
        50022 => "tool_tip",
        50023 => "tree",
        50024 => "tree_item",
        50025 => "custom",
        50026 => "group",
        50027 => "thumb",
        50028 => "grid",
        50029 => "grid_item",
        50030 => "document",
        50031 => "split_button",
        50032 => "window",
        50033 => "pane",
        50034 => "header",
        50035 => "header_item",
        50036 => "table",
        50037 => "title_bar",
        50038 => "separator",
        50039 => "semantic_zoom",
        50040 => "app_bar",
        _ => "unknown",
    }
}

/// The state of an expandable control (`ExpandCollapseState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExpandState {
    Collapsed,
    Expanded,
    PartiallyExpanded,
    /// Nothing to expand: a leaf, or a control with no expand pattern.
    #[default]
    Leaf,
}

impl ExpandState {
    /// From the raw `ExpandCollapseState` value: 0 collapsed, 1 expanded, 2 partially expanded, 3 leaf.
    pub fn from_raw(state: i32) -> Self {
        match state {
            0 => Self::Collapsed,
            1 => Self::Expanded,
            2 => Self::PartiallyExpanded,
            _ => Self::Leaf,
        }
    }
}

/// The patterns a control offers and the state they report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub focusable: bool,
    pub invoke: bool,
    pub value_writable: bool,
    pub selectable: bool,
    pub toggle: bool,
    pub expand: ExpandState,
}

/// The actions a control takes, in the names `apz_desktop::action_name` gives them.
pub fn actions_for(caps: Capabilities) -> Vec<&'static str> {
    let mut actions = Vec::with_capacity(7);
    if caps.invoke {
        actions.push("invoke");
    }
    if caps.value_writable {
        actions.push("set_value");
    }
    if caps.focusable {
        actions.push("focus");
    }
    if caps.selectable {
        actions.push("select");
    }
    if caps.toggle {
        actions.push("toggle");
    }
    match caps.expand {
        ExpandState::Collapsed => actions.push("expand"),
        ExpandState::Expanded => actions.push("collapse"),
        ExpandState::PartiallyExpanded => {
            actions.push("expand");
            actions.push("collapse");
        }
        ExpandState::Leaf => {}
    }
    actions
}

/// The value a toggle reports, from `ToggleState`: 0 off, 1 on, 2 indeterminate.
pub fn toggle_value(state: i32) -> &'static str {
    match state {
        0 => "off",
        1 => "on",
        _ => "indeterminate",
    }
}

/// Whether a control matches a selector: each field the selector gives must equal the control's.
pub fn selector_matches(
    want_role: Option<&str>,
    want_name: Option<&str>,
    role: &str,
    name: Option<&str>,
) -> bool {
    want_role.is_none_or(|wanted| wanted == role)
        && want_name.is_none_or(|wanted| Some(wanted) == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_types_have_their_role_names() {
        assert_eq!(role_name(50000), "button");
        assert_eq!(role_name(50004), "text_field");
        assert_eq!(role_name(50032), "window");
        assert_eq!(role_name(50040), "app_bar");
        assert_eq!(role_name(0), "unknown");
        assert_eq!(role_name(50041), "unknown");
    }

    #[test]
    fn expand_states_map_from_their_raw_values() {
        assert_eq!(ExpandState::from_raw(0), ExpandState::Collapsed);
        assert_eq!(ExpandState::from_raw(1), ExpandState::Expanded);
        assert_eq!(ExpandState::from_raw(2), ExpandState::PartiallyExpanded);
        assert_eq!(ExpandState::from_raw(3), ExpandState::Leaf);
        assert_eq!(ExpandState::from_raw(9), ExpandState::Leaf);
    }

    #[test]
    fn actions_follow_the_capabilities_and_expand_direction() {
        for (case, caps, want) in [
            (
                "a plain control takes no actions",
                Capabilities::default(),
                vec![],
            ),
            (
                "every pattern offers its action, in order",
                Capabilities {
                    focusable: true,
                    invoke: true,
                    value_writable: true,
                    selectable: true,
                    toggle: true,
                    expand: ExpandState::PartiallyExpanded,
                },
                vec![
                    "invoke",
                    "set_value",
                    "focus",
                    "select",
                    "toggle",
                    "expand",
                    "collapse",
                ],
            ),
            (
                "a collapsed control offers only expand",
                Capabilities {
                    expand: ExpandState::Collapsed,
                    ..Capabilities::default()
                },
                vec!["expand"],
            ),
            (
                "an expanded control offers only collapse",
                Capabilities {
                    expand: ExpandState::Expanded,
                    ..Capabilities::default()
                },
                vec!["collapse"],
            ),
        ] {
            assert_eq!(actions_for(caps), want, "{case}");
        }
    }

    #[test]
    fn toggle_states_read_as_words() {
        assert_eq!(toggle_value(0), "off");
        assert_eq!(toggle_value(1), "on");
        assert_eq!(toggle_value(2), "indeterminate");
    }

    #[test]
    fn a_selector_matches_only_the_fields_it_gives() {
        assert!(selector_matches(None, None, "button", None));
        assert!(selector_matches(
            Some("button"),
            None,
            "button",
            Some("Save")
        ));
        assert!(selector_matches(None, Some("Save"), "button", Some("Save")));
        assert!(!selector_matches(
            Some("text_field"),
            None,
            "button",
            Some("Save")
        ));
        assert!(!selector_matches(None, Some("Save"), "button", None));
        assert!(!selector_matches(
            None,
            Some("Save"),
            "button",
            Some("Save as")
        ));
    }
}
