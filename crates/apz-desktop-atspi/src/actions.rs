//! Which accessibility actions a control takes, and which AT-SPI action performs each one.
//!
//! An AT-SPI control lists its actions by name (`click`, `expand or contract`, ...). The names here are the apz
//! action names (`apz_desktop::action_name`). A control advertises an action only when it can really perform it.

/// The AT-SPI actions that invoke a control, in order of preference.
const INVOKE: &[&str] = &["click", "press", "activate", "invoke"];
/// The AT-SPI actions that toggle a control, in order of preference.
const TOGGLE: &[&str] = &["toggle", "click", "press", "activate"];
const SELECT: &[&str] = &["select"];
/// AT-SPI names one action for both states, so either name can expand a control.
const EXPAND: &[&str] = &["expand or contract", "expand"];
const COLLAPSE: &[&str] = &["expand or contract", "collapse"];
/// Roles whose click toggles their state, so they advertise `toggle` when they have a click.
const TOGGLE_ROLES: &[&str] = &[
    "check_box",
    "toggle_button",
    "radio_button",
    "check_menu_item",
    "radio_menu_item",
];

/// What a control implements and lists, as the advertised actions are worked out from.
pub(crate) struct Facts<'a> {
    /// The normalised role (`button`, `check_box`, ...).
    pub(crate) role: &'a str,
    /// The AT-SPI action names the control lists, in index order.
    pub(crate) actions: &'a [String],
    /// Implements EditableText.
    pub(crate) editable: bool,
    /// Implements Value.
    pub(crate) value: bool,
    /// Implements Component, so it can take focus.
    pub(crate) component: bool,
    /// Carries the Selectable state, so its parent can select it by index.
    pub(crate) selectable: bool,
}

/// The apz action names the control advertises, in a fixed order.
pub(crate) fn advertised(facts: &Facts<'_>) -> Vec<&'static str> {
    let lists = |names: &[&str]| {
        facts
            .actions
            .iter()
            .any(|action| names.contains(&action.as_str()))
    };
    let mut out = Vec::new();
    if lists(INVOKE) {
        out.push("invoke");
    }
    let toggles_by_click =
        TOGGLE_ROLES.contains(&facts.role) && lists(&["click", "press", "activate"]);
    if lists(&["toggle"]) || toggles_by_click {
        out.push("toggle");
    }
    if lists(SELECT) || facts.selectable {
        out.push("select");
    }
    if lists(EXPAND) {
        out.push("expand");
    }
    if lists(COLLAPSE) {
        out.push("collapse");
    }
    if facts.editable || facts.value {
        out.push("set_value");
    }
    if facts.component {
        out.push("focus");
    }
    out
}

/// The index of the AT-SPI action that performs the apz action `wanted` (`invoke`, `toggle`, `select`, `expand`
/// or `collapse`), by the preference order of its AT-SPI names.
pub(crate) fn action_index(actions: &[String], wanted: &str) -> Option<usize> {
    let names: &[&str] = match wanted {
        "invoke" => INVOKE,
        "toggle" => TOGGLE,
        "select" => SELECT,
        "expand" => EXPAND,
        "collapse" => COLLAPSE,
        _ => return None,
    };
    names
        .iter()
        .find_map(|name| actions.iter().position(|action| action == name))
}

#[cfg(test)]
mod tests {
    use super::{Facts, action_index, advertised};

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_owned()).collect()
    }

    fn facts<'a>(role: &'a str, actions: &'a [String]) -> Facts<'a> {
        Facts {
            role,
            actions,
            editable: false,
            value: false,
            component: false,
            selectable: false,
        }
    }

    #[test]
    fn a_button_with_a_click_advertises_invoke_only() {
        let actions = names(&["click"]);
        assert_eq!(advertised(&facts("button", &actions)), vec!["invoke"]);
    }

    #[test]
    fn a_check_box_with_a_click_also_toggles() {
        let actions = names(&["click"]);
        assert_eq!(
            advertised(&facts("check_box", &actions)),
            vec!["invoke", "toggle"]
        );
    }

    #[test]
    fn one_expand_or_contract_action_serves_expand_and_collapse() {
        let actions = names(&["expand or contract"]);
        assert_eq!(
            advertised(&facts("tree_item", &actions)),
            vec!["expand", "collapse"]
        );
    }

    #[test]
    fn editable_and_component_controls_take_set_value_and_focus() {
        let actions = Vec::new();
        let entry = Facts {
            editable: true,
            component: true,
            ..facts("text_field", &actions)
        };
        assert_eq!(advertised(&entry), vec!["set_value", "focus"]);
    }

    #[test]
    fn a_selectable_child_advertises_select_without_a_select_action() {
        let actions = Vec::new();
        let item = Facts {
            selectable: true,
            ..facts("list_item", &actions)
        };
        assert_eq!(advertised(&item), vec!["select"]);
    }

    #[test]
    fn a_control_with_no_actions_advertises_none() {
        let actions = Vec::new();
        assert!(advertised(&facts("text", &actions)).is_empty());
    }

    #[test]
    fn invoke_prefers_the_named_actions_over_the_first_index() {
        let actions = names(&["show menu", "press", "click"]);
        assert_eq!(action_index(&actions, "invoke"), Some(2));
    }

    #[test]
    fn toggle_takes_a_toggle_action_before_a_click() {
        let actions = names(&["click", "toggle"]);
        assert_eq!(action_index(&actions, "toggle"), Some(1));
        assert_eq!(action_index(&names(&["click"]), "toggle"), Some(0));
    }

    #[test]
    fn expand_and_collapse_find_the_expand_or_contract_action() {
        let actions = names(&["expand or contract"]);
        assert_eq!(action_index(&actions, "expand"), Some(0));
        assert_eq!(action_index(&actions, "collapse"), Some(0));
    }

    #[test]
    fn an_action_the_control_does_not_list_has_no_index() {
        let actions = names(&["click"]);
        assert_eq!(action_index(&actions, "select"), None);
        assert_eq!(action_index(&actions, "set_value"), None);
    }
}
