//! Pointer, keyboard and UI-tree actions on a screen session.

use serde::{Deserialize, Serialize};

/// A mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Left,
    Right,
    Middle,
}

/// One pointer or keyboard action.
///
/// Coordinates (`x`, `y`, `from`, `to`) are in the pixels of the session's most recent screenshot or stream frame,
/// as the hub last saw them. The daemon maps each point to screen points with the scale of that image, so the hub
/// never needs the display's own size. Before any image has been sent for a session, points are screen points.
///
/// Scroll amounts (`dx`, `dy`) are notches (line counts), not pixels, so the daemon passes them unchanged. Their
/// sign is the content's direction: `dy > 0` scrolls the content down (the wheel turned down, toward the user),
/// `dy < 0` scrolls up; `dx > 0` scrolls right, `dx < 0` left. Every backend follows this.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum InputAction {
    Move {
        x: i32,
        y: i32,
    },
    /// `count` clicks in a row (2 is a double click).
    Click {
        x: i32,
        y: i32,
        button: Button,
        count: u8,
    },
    Down {
        button: Button,
    },
    Up {
        button: Button,
    },
    /// Moves to `(x, y)` and scrolls there by `dx` and `dy` notches. `dy > 0` scrolls content down, `dx > 0`
    /// scrolls content right (see the type's docs for the signs).
    Scroll {
        x: i32,
        y: i32,
        dx: i32,
        dy: i32,
    },
    /// Types text as keystrokes.
    Type {
        text: String,
    },
    /// A key chord such as `"ctrl+shift+s"`, parsed by the daemon with the cross-platform names.
    Key {
        combo: String,
    },
    Drag {
        from: (i32, i32),
        to: (i32, i32),
    },
}

/// Which control to act on. Every field given must match. `path` is the child-index path from the
/// root of the tree (from the app's window when `app` is given). Without `path`, the first control in
/// document order whose `role` and `name` match is taken.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selector {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<Vec<u32>>,
    /// Limits the search to one app's windows (the app's name, as `apps` lists it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

/// An accessibility action on a control. The control must list it in its `actions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "act", rename_all = "snake_case")]
pub enum UiAction {
    Invoke,
    SetValue { value: String },
    Focus,
    Select,
    Toggle,
    Expand,
    Collapse,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_wire;

    #[test]
    fn every_input_action_round_trips() {
        assert_wire(
            &InputAction::Move { x: 10, y: -4 },
            r#"{"action":"move","x":10,"y":-4}"#,
        );
        assert_wire(
            &InputAction::Click {
                x: 640,
                y: 360,
                button: Button::Right,
                count: 1,
            },
            r#"{"action":"click","x":640,"y":360,"button":"right","count":1}"#,
        );
        assert_wire(
            &InputAction::Down {
                button: Button::Middle,
            },
            r#"{"action":"down","button":"middle"}"#,
        );
        assert_wire(
            &InputAction::Up {
                button: Button::Left,
            },
            r#"{"action":"up","button":"left"}"#,
        );
        assert_wire(
            &InputAction::Scroll {
                x: 100,
                y: 200,
                dx: 0,
                dy: -3,
            },
            r#"{"action":"scroll","x":100,"y":200,"dx":0,"dy":-3}"#,
        );
        assert_wire(
            &InputAction::Type {
                text: "héllo".into(),
            },
            r#"{"action":"type","text":"héllo"}"#,
        );
        assert_wire(
            &InputAction::Key {
                combo: "ctrl+shift+s".into(),
            },
            r#"{"action":"key","combo":"ctrl+shift+s"}"#,
        );
        assert_wire(
            &InputAction::Drag {
                from: (5, 6),
                to: (50, 60),
            },
            r#"{"action":"drag","from":[5,6],"to":[50,60]}"#,
        );
    }

    #[test]
    fn buttons_are_lowercase_words() {
        for (button, json) in [
            (Button::Left, "left"),
            (Button::Right, "right"),
            (Button::Middle, "middle"),
        ] {
            assert_wire(&button, &format!(r#""{json}""#));
        }
    }

    #[test]
    fn a_selector_omits_the_fields_it_does_not_set() {
        assert_wire(&Selector::default(), "{}");
        assert_wire(
            &Selector {
                role: Some("button".into()),
                name: Some("Save".into()),
                path: None,
                app: Some("Notes".into()),
            },
            r#"{"role":"button","name":"Save","app":"Notes"}"#,
        );
        assert_wire(
            &Selector {
                path: Some(vec![1, 0, 2]),
                ..Selector::default()
            },
            r#"{"path":[1,0,2]}"#,
        );
    }

    #[test]
    fn every_ui_action_round_trips() {
        assert_wire(&UiAction::Invoke, r#"{"act":"invoke"}"#);
        assert_wire(
            &UiAction::SetValue {
                value: "Draft".into(),
            },
            r#"{"act":"set_value","value":"Draft"}"#,
        );
        assert_wire(&UiAction::Focus, r#"{"act":"focus"}"#);
        assert_wire(&UiAction::Select, r#"{"act":"select"}"#);
        assert_wire(&UiAction::Toggle, r#"{"act":"toggle"}"#);
        assert_wire(&UiAction::Expand, r#"{"act":"expand"}"#);
        assert_wire(&UiAction::Collapse, r#"{"act":"collapse"}"#);
    }
}
