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
    fn advertised_actions_follow_the_control_facts() {
        let cases: &[(&str, &str, &[&str], bool, bool, bool, &[&str])] = &[
            (
                "a button with a click advertises invoke only",
                "button",
                &["click"],
                false,
                false,
                false,
                &["invoke"],
            ),
            (
                "a check box with a click also toggles",
                "check_box",
                &["click"],
                false,
                false,
                false,
                &["invoke", "toggle"],
            ),
            (
                "one expand or contract action serves expand and collapse",
                "tree_item",
                &["expand or contract"],
                false,
                false,
                false,
                &["expand", "collapse"],
            ),
            (
                "editable and component controls take set value and focus",
                "text_field",
                &[],
                true,
                true,
                false,
                &["set_value", "focus"],
            ),
            (
                "a selectable child advertises select without a select action",
                "list_item",
                &[],
                false,
                false,
                true,
                &["select"],
            ),
            (
                "a control with no actions advertises none",
                "text",
                &[],
                false,
                false,
                false,
                &[],
            ),
        ];
        for &(case, role, actions, editable, component, selectable, want) in cases {
            let actions = names(actions);
            let entry = Facts {
                editable,
                component,
                selectable,
                ..facts(role, &actions)
            };
            assert_eq!(advertised(&entry), want, "{case}");
        }
    }

    #[test]
    fn action_index_finds_the_named_action_for_each_wanted_action() {
        let cases: &[(&str, &[&str], &str, Option<usize>)] = &[
            (
                "invoke prefers the named actions over the first index",
                &["show menu", "press", "click"],
                "invoke",
                Some(2),
            ),
            (
                "toggle takes a toggle action before a click",
                &["click", "toggle"],
                "toggle",
                Some(1),
            ),
            (
                "toggle falls back to a click",
                &["click"],
                "toggle",
                Some(0),
            ),
            (
                "expand finds the expand or contract action",
                &["expand or contract"],
                "expand",
                Some(0),
            ),
            (
                "collapse finds the expand or contract action",
                &["expand or contract"],
                "collapse",
                Some(0),
            ),
            (
                "an unlisted select has no index",
                &["click"],
                "select",
                None,
            ),
            (
                "an unlisted set_value has no index",
                &["click"],
                "set_value",
                None,
            ),
        ];
        for &(case, actions, wanted, want) in cases {
            assert_eq!(action_index(&names(actions), wanted), want, "{case}");
        }
    }
}
