//! Accessibility data decoded into the trait's UI tree.
//!
//! The macOS code reads the attributes of each `AXUIElement` into a [`RawElement`]. This module does the rest, so it
//! can be tested with fixtures on any host:
//!
//! - [`normalise_role`] turns `AXCheckBox` into `check_box`.
//! - [`decode_value`] turns the value (text, a number, or a boolean) into the text the tree shows.
//! - [`offered_actions`] lists the [`apz_desktop::UiNode::actions`] a control takes, from the AX action names and
//!   which attributes can be set.
//! - [`plan_action`] says which AX call performs a [`UiAction`].
//! - [`locate`] finds the control a [`Selector`] picks, by path or by role and name.

use apz_desktop::{DesktopError, Result, UiNode};
use apz_station::{Selector, UiAction};

/// An attribute value as the platform read it.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Raw {
    /// The attribute is missing, or has a type this backend does not read.
    #[default]
    Absent,
    Text(String),
    Number(f64),
    Boolean(bool),
}

/// The attributes of one accessibility element, as read from the platform.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RawElement {
    /// `AXRole`, such as `AXButton`.
    pub role: Option<String>,
    /// `AXTitle`.
    pub title: Option<String>,
    /// `AXDescription`, used as the name when there is no title.
    pub description: Option<String>,
    /// `AXValue`.
    pub value: Raw,
    /// `AXPosition` in screen points.
    pub position: Option<(f64, f64)>,
    /// `AXSize` in screen points.
    pub size: Option<(f64, f64)>,
    /// `AXActionNames`, such as `AXPress`.
    pub actions: Vec<String>,
    /// Whether `AXValue` can be set.
    pub value_settable: bool,
    /// Whether `AXFocused` can be set.
    pub focus_settable: bool,
    /// Whether `AXSelected` can be set.
    pub select_settable: bool,
}

/// The AX call that performs a [`UiAction`].
#[derive(Debug, Clone, PartialEq)]
pub enum AxStep {
    /// `AXUIElementPerformAction` with this action name.
    Perform(&'static str),
    /// `AXUIElementSetAttributeValue` with this attribute and value.
    Set {
        attribute: &'static str,
        value: AttrValue,
    },
}

/// A value for [`AxStep::Set`].
#[derive(Debug, Clone, PartialEq)]
pub enum AttrValue {
    Bool(bool),
    Text(String),
}

/// The normalised role of an `AX` role name: the name without its `AX` prefix, in lower snake case. Unknown roles
/// keep their name in that form, so a role this table does not know still shows.
pub fn normalise_role(ax_role: &str) -> String {
    let name = ax_role.strip_prefix("AX").unwrap_or(ax_role);
    snake_case(name)
}

/// Lower snake case: a new word starts at each capital that follows a lower-case letter or a digit, so `PopUpButton`
/// becomes `pop_up_button` and an acronym stays together.
fn snake_case(name: &str) -> String {
    let mut out = String::new();
    let mut previous: Option<char> = None;
    for c in name.chars() {
        if !c.is_ascii_alphanumeric() {
            if !out.is_empty() && !out.ends_with('_') {
                out.push('_');
            }
            previous = Some('_');
            continue;
        }
        let starts_word = c.is_ascii_uppercase()
            && previous.is_some_and(|p| p.is_ascii_lowercase() || p.is_ascii_digit());
        if starts_word && !out.ends_with('_') {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
        previous = Some(c);
    }
    if out.is_empty() {
        "unknown".into()
    } else {
        out
    }
}

/// Whether a normalised role is a control with an on and off state.
pub fn checkbox_like(role: &str) -> bool {
    matches!(role, "check_box" | "radio_button" | "switch")
}

/// The text a value shows. A checkbox's number is 0 (off), 1 (on) or 2 (mixed); other numbers are written as they
/// are, with no fraction when they are whole.
pub fn decode_value(role: &str, raw: &Raw) -> Option<String> {
    match raw {
        Raw::Absent => None,
        Raw::Text(text) => Some(text.clone()),
        Raw::Number(number) if checkbox_like(role) => Some(match number.round() as i64 {
            0 => "off".to_string(),
            1 => "on".to_string(),
            2 => "mixed".to_string(),
            other => other.to_string(),
        }),
        Raw::Number(number) => Some(format_number(*number)),
        Raw::Boolean(on) if checkbox_like(role) => Some(if *on { "on" } else { "off" }.to_string()),
        Raw::Boolean(flag) => Some(flag.to_string()),
    }
}

fn format_number(number: f64) -> String {
    if number.fract() == 0.0 && number.abs() < 9.0e15 {
        (number as i64).to_string()
    } else {
        number.to_string()
    }
}

/// The actions a control takes, by the names the tree uses (`invoke`, `set_value`, `focus`, `select`, `toggle`,
/// `expand`, `collapse`), in a fixed order.
///
/// `AXPress` becomes `toggle` on a checkbox, `expand` and `collapse` on a disclosure triangle (both perform the same
/// press), and `invoke` otherwise. `focus` is offered when `AXFocused` can be set or the window can be raised.
pub fn offered_actions(element: &RawElement, role: &str) -> Vec<String> {
    let has = |name: &str| element.actions.iter().any(|offered| offered == name);
    let mut out: Vec<&str> = Vec::new();
    if has("AXPress") {
        if checkbox_like(role) {
            out.push("toggle");
        } else if role == "disclosure_triangle" {
            out.push("expand");
            out.push("collapse");
        } else {
            out.push("invoke");
        }
    }
    if element.value_settable {
        out.push("set_value");
    }
    if element.focus_settable || has("AXRaise") {
        out.push("focus");
    }
    if element.select_settable {
        out.push("select");
    }
    out.into_iter().map(String::from).collect()
}

/// The AX call for a UI action. `Toggle`, `Expand` and `Collapse` press the control, which is what the platform
/// does for them.
pub fn plan_action(action: &UiAction) -> AxStep {
    match action {
        UiAction::Invoke | UiAction::Toggle | UiAction::Expand | UiAction::Collapse => {
            AxStep::Perform("AXPress")
        }
        UiAction::SetValue { value } => AxStep::Set {
            attribute: "AXValue",
            value: AttrValue::Text(value.clone()),
        },
        UiAction::Focus => AxStep::Set {
            attribute: "AXFocused",
            value: AttrValue::Bool(true),
        },
        UiAction::Select => AxStep::Set {
            attribute: "AXSelected",
            value: AttrValue::Bool(true),
        },
    }
}

/// Builds the node for an element: its id, its normalised role, its name (the title, else the description), its
/// value, its bounds (`x`, `y`, `w`, `h` in screen points) and its offered actions. `children` are its children.
pub fn node_from_raw(id: u64, element: &RawElement, children: Vec<UiNode>) -> UiNode {
    let role = element
        .role
        .as_deref()
        .map_or_else(|| "unknown".to_string(), normalise_role);
    let name = [element.title.as_deref(), element.description.as_deref()]
        .into_iter()
        .flatten()
        .find(|text| !text.is_empty())
        .map(str::to_string);
    let value = decode_value(&role, &element.value);
    let actions = offered_actions(element, &role);
    UiNode {
        id,
        bounds: bounds(element.position, element.size),
        role,
        name,
        value,
        actions,
        children,
    }
}

fn bounds(position: Option<(f64, f64)>, size: Option<(f64, f64)>) -> (i32, i32, u32, u32) {
    let (x, y) = position.unwrap_or((0.0, 0.0));
    let (w, h) = size.unwrap_or((0.0, 0.0));
    (
        x.round() as i32,
        y.round() as i32,
        w.max(0.0).round() as u32,
        h.max(0.0).round() as u32,
    )
}

/// The control a selector picks, as its child-index path from `root`. The path is empty when `root` itself is picked.
///
/// A path picks that control, and the role and name, when given, must also match it. Without a path the first control
/// in document order whose role and name match is picked. The selector's `app` is not checked here: the caller
/// chooses `root` from it.
pub fn locate(root: &UiNode, selector: &Selector) -> Result<Vec<usize>> {
    if let Some(path) = &selector.path {
        let mut node = root;
        let mut trail = Vec::with_capacity(path.len());
        for &index in path {
            let index = index as usize;
            node = node
                .children
                .get(index)
                .ok_or_else(|| DesktopError::NotFound(format!("no control at path {path:?}")))?;
            trail.push(index);
        }
        if !selects(node, selector) {
            return Err(DesktopError::NotFound(format!(
                "the control at path {path:?} is not {selector:?}"
            )));
        }
        return Ok(trail);
    }
    if selector.role.is_none() && selector.name.is_none() {
        return Err(DesktopError::Invalid(
            "a selector needs a path, a role or a name".into(),
        ));
    }
    let mut trail = Vec::new();
    search(root, selector, &mut trail)
        .ok_or_else(|| DesktopError::NotFound(format!("no control matches {selector:?}")))
}

/// The node at a child-index path from `root`.
pub fn node_at<'a>(root: &'a UiNode, path: &[usize]) -> Option<&'a UiNode> {
    path.iter()
        .try_fold(root, |node, &index| node.children.get(index))
}

/// Whether the node's role and name are the ones the selector gives (an absent field matches anything).
fn selects(node: &UiNode, selector: &Selector) -> bool {
    selector
        .role
        .as_deref()
        .is_none_or(|role| role == node.role)
        && selector
            .name
            .as_deref()
            .is_none_or(|name| node.name.as_deref() == Some(name))
}

/// Pre-order search for the first node the selector matches, returning its path.
fn search(node: &UiNode, selector: &Selector, trail: &mut Vec<usize>) -> Option<Vec<usize>> {
    if selects(node, selector) {
        return Some(trail.clone());
    }
    for (index, child) in node.children.iter().enumerate() {
        trail.push(index);
        if let Some(found) = search(child, selector, trail) {
            return Some(found);
        }
        trail.pop();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(role: &str, title: Option<&str>) -> RawElement {
        RawElement {
            role: Some(role.into()),
            title: title.map(Into::into),
            ..RawElement::default()
        }
    }

    fn leaf(id: u64, role: &str, name: Option<&str>) -> UiNode {
        node_from_raw(id, &element(role, name), Vec::new())
    }

    #[test]
    fn ax_roles_normalise_to_snake_case() {
        let cases = [
            ("AXButton", "button"),
            ("AXCheckBox", "check_box"),
            ("AXRadioButton", "radio_button"),
            ("AXPopUpButton", "pop_up_button"),
            ("AXTextField", "text_field"),
            ("AXSecureTextField", "secure_text_field"),
            ("AXStaticText", "static_text"),
            ("AXWebArea", "web_area"),
            ("AXMenuBarItem", "menu_bar_item"),
            ("AXDisclosureTriangle", "disclosure_triangle"),
            ("AXWindow", "window"),
            ("AXApplication", "application"),
            ("AXURL", "url"),
            ("Unknown", "unknown"),
            ("", "unknown"),
        ];
        for (ax, normalised) in cases {
            assert_eq!(normalise_role(ax), normalised, "{ax}");
        }
    }

    #[test]
    fn a_checkbox_number_reads_as_on_off_or_mixed() {
        assert_eq!(
            decode_value("check_box", &Raw::Number(1.0)),
            Some("on".into())
        );
        assert_eq!(
            decode_value("check_box", &Raw::Number(0.0)),
            Some("off".into())
        );
        assert_eq!(
            decode_value("check_box", &Raw::Number(2.0)),
            Some("mixed".into())
        );
        assert_eq!(
            decode_value("check_box", &Raw::Boolean(true)),
            Some("on".into())
        );
    }

    #[test]
    fn other_values_read_as_their_text_or_number() {
        assert_eq!(
            decode_value("text_field", &Raw::Text("Draft".into())),
            Some("Draft".into())
        );
        assert_eq!(
            decode_value("slider", &Raw::Number(42.0)),
            Some("42".into())
        );
        assert_eq!(
            decode_value("slider", &Raw::Number(0.5)),
            Some("0.5".into())
        );
        assert_eq!(
            decode_value("text_field", &Raw::Boolean(false)),
            Some("false".into())
        );
        assert_eq!(decode_value("text_field", &Raw::Absent), None);
    }

    #[test]
    fn a_button_offers_invoke_and_a_checkbox_offers_toggle() {
        let button = RawElement {
            actions: vec!["AXPress".into(), "AXShowMenu".into()],
            ..element("AXButton", Some("Save"))
        };
        assert_eq!(offered_actions(&button, "button"), vec!["invoke"]);

        let checkbox = RawElement {
            actions: vec!["AXPress".into()],
            value_settable: true,
            focus_settable: true,
            ..element("AXCheckBox", Some("Wrap"))
        };
        assert_eq!(
            offered_actions(&checkbox, "check_box"),
            vec!["toggle", "set_value", "focus"]
        );
    }

    #[test]
    fn a_text_field_offers_set_value_and_focus_when_they_are_settable() {
        let field = RawElement {
            value_settable: true,
            focus_settable: true,
            ..element("AXTextField", Some("Title"))
        };
        assert_eq!(
            offered_actions(&field, "text_field"),
            vec!["set_value", "focus"]
        );
        assert!(offered_actions(&element("AXStaticText", None), "static_text").is_empty());
    }

    #[test]
    fn a_disclosure_triangle_offers_expand_and_collapse() {
        let triangle = RawElement {
            actions: vec!["AXPress".into()],
            ..element("AXDisclosureTriangle", None)
        };
        assert_eq!(
            offered_actions(&triangle, "disclosure_triangle"),
            vec!["expand", "collapse"]
        );
    }

    #[test]
    fn a_window_that_can_be_raised_offers_focus() {
        let window = RawElement {
            actions: vec!["AXRaise".into()],
            ..element("AXWindow", Some("Notes"))
        };
        assert_eq!(offered_actions(&window, "window"), vec!["focus"]);
    }

    #[test]
    fn each_ui_action_maps_to_its_ax_call() {
        assert_eq!(plan_action(&UiAction::Invoke), AxStep::Perform("AXPress"));
        assert_eq!(plan_action(&UiAction::Toggle), AxStep::Perform("AXPress"));
        assert_eq!(
            plan_action(&UiAction::SetValue {
                value: "Draft".into()
            }),
            AxStep::Set {
                attribute: "AXValue",
                value: AttrValue::Text("Draft".into()),
            }
        );
        assert_eq!(
            plan_action(&UiAction::Focus),
            AxStep::Set {
                attribute: "AXFocused",
                value: AttrValue::Bool(true),
            }
        );
        assert_eq!(
            plan_action(&UiAction::Select),
            AxStep::Set {
                attribute: "AXSelected",
                value: AttrValue::Bool(true),
            }
        );
    }

    #[test]
    fn a_fixture_window_decodes_into_a_tree_with_bounds_and_names() {
        let save = RawElement {
            position: Some((120.4, 119.6)),
            size: Some((80.0, 30.0)),
            actions: vec!["AXPress".into()],
            ..element("AXButton", None)
        };
        let save = RawElement {
            description: Some("Save".into()),
            ..save
        };
        let save_node = node_from_raw(3, &save, Vec::new());
        assert_eq!(
            save_node.name.as_deref(),
            Some("Save"),
            "description is the name"
        );
        assert_eq!(save_node.bounds, (120, 120, 80, 30));
        assert_eq!(save_node.actions, vec!["invoke"]);

        let title = RawElement {
            value: Raw::Text("Untitled".into()),
            value_settable: true,
            ..element("AXTextField", Some("Title"))
        };
        let title_node = node_from_raw(4, &title, Vec::new());
        assert_eq!(title_node.value.as_deref(), Some("Untitled"));
        assert_eq!(title_node.bounds, (0, 0, 0, 0), "missing geometry is zero");

        let window = node_from_raw(
            2,
            &RawElement {
                size: Some((-5.0, 10.0)),
                ..element("AXWindow", Some("Notes"))
            },
            vec![save_node, title_node],
        );
        assert_eq!(window.role, "window");
        assert_eq!(window.bounds.2, 0, "a negative size is clamped");
        assert_eq!(window.children.len(), 2);
    }

    fn tree() -> UiNode {
        let list = node_from_raw(
            6,
            &element("AXList", Some("Recent")),
            vec![leaf(7, "list_item", Some("Draft 1"))],
        );
        let window = node_from_raw(
            2,
            &element("AXWindow", Some("Notes")),
            vec![
                leaf(3, "button", Some("Save")),
                leaf(4, "text_field", Some("Title")),
                list,
            ],
        );
        let app = node_from_raw(1, &element("AXApplication", None), vec![window]);
        UiNode {
            name: Some("Notes".into()),
            ..app
        }
    }

    fn selector(role: Option<&str>, name: Option<&str>) -> Selector {
        Selector {
            role: role.map(Into::into),
            name: name.map(Into::into),
            path: None,
            app: None,
        }
    }

    #[test]
    fn a_selector_finds_the_first_match_in_document_order() {
        let root = tree();
        assert_eq!(
            locate(&root, &selector(Some("button"), Some("Save"))).expect("found"),
            vec![0, 0]
        );
        assert_eq!(
            locate(&root, &selector(None, Some("Draft 1"))).expect("found"),
            vec![0, 2, 0]
        );
        assert_eq!(
            locate(&root, &selector(Some("text_field"), None)).expect("found"),
            vec![0, 1]
        );
    }

    #[test]
    fn a_path_picks_its_control_and_must_match_the_given_role_and_name() {
        let root = tree();
        let by_path = Selector {
            path: Some(vec![0, 1]),
            ..Selector::default()
        };
        assert_eq!(locate(&root, &by_path).expect("found"), vec![0, 1]);
        let wrong_name = Selector {
            path: Some(vec![0, 1]),
            name: Some("Save".into()),
            ..Selector::default()
        };
        assert!(matches!(
            locate(&root, &wrong_name),
            Err(DesktopError::NotFound(_))
        ));
        let off_the_tree = Selector {
            path: Some(vec![9]),
            ..Selector::default()
        };
        assert!(matches!(
            locate(&root, &off_the_tree),
            Err(DesktopError::NotFound(_))
        ));
    }

    #[test]
    fn a_selector_with_nothing_to_match_or_no_match_is_an_error() {
        let root = tree();
        assert!(matches!(
            locate(&root, &Selector::default()),
            Err(DesktopError::Invalid(_))
        ));
        assert!(matches!(
            locate(&root, &selector(Some("slider"), None)),
            Err(DesktopError::NotFound(_))
        ));
    }

    #[test]
    fn node_at_follows_a_path() {
        let root = tree();
        assert_eq!(node_at(&root, &[0, 2, 0]).map(|n| n.id), Some(7));
        assert!(node_at(&root, &[5]).is_none());
    }
}
