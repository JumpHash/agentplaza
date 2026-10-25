//! A scripted desktop: screens of controls, a pointer and a keyboard that move between them, and a picture per screen. It
//! stands in for an X session and its accessibility tree in the tests of the agent and of the hub's target over it.

use std::collections::{BTreeMap, BTreeSet};

use apz_desktop::{AppInfo, Desktop, DesktopError, Frame, Result, SessionSpec, UiNode};
use apz_station::{InputAction, Selector, SessionId, UiAction};

/// The size of a scripted screen's picture, in pixels.
const PICTURE_W: u32 = 160;
const PICTURE_H: u32 = 96;

/// One control of a screen: its id, its role (`button`, `link`, `textbox`, `text`), its label, and the screen a press of it
/// leads to, if any.
#[derive(Clone, Debug)]
pub struct Control {
    pub id: String,
    pub role: String,
    pub label: String,
    pub to: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Screen {
    pub id: String,
    pub title: String,
    pub controls: Vec<Control>,
}

/// The screens and the screen the session starts on.
#[derive(Clone, Debug)]
pub struct Script {
    pub start: String,
    pub screens: Vec<Screen>,
}

fn control(id: &str, role: &str, label: &str, to: Option<&str>) -> Control {
    Control {
        id: id.into(),
        role: role.into(),
        label: label.into(),
        to: to.map(str::to_string),
    }
}

/// The cart and its receipt. Pay leads to the receipt, or to the cart again when `buggy` (the seeded bug).
pub fn shop(buggy: bool) -> Script {
    Script {
        start: "cart".into(),
        screens: vec![
            Screen {
                id: "cart".into(),
                title: "Cart".into(),
                controls: vec![
                    control(
                        "pay",
                        "button",
                        "Pay",
                        Some(if buggy { "cart" } else { "receipt" }),
                    ),
                    control("note", "textbox", "Note", None),
                ],
            },
            Screen {
                id: "receipt".into(),
                title: "Receipt".into(),
                controls: vec![control("home", "link", "Back to shop", None)],
            },
        ],
    }
}

/// A desktop whose one session shows the script's screens: a pointer press on a control follows its link, a text field
/// takes the focus and what is typed into it.
pub struct ScriptDesk {
    script: Script,
    at: String,
    typed: BTreeMap<String, String>,
    focus: Option<String>,
    sessions: BTreeSet<SessionId>,
    next: SessionId,
    pointers: Vec<(i32, i32)>,
    keys: Vec<String>,
}

impl ScriptDesk {
    pub fn new(script: Script) -> ScriptDesk {
        ScriptDesk {
            at: script.start.clone(),
            script,
            typed: BTreeMap::new(),
            focus: None,
            sessions: BTreeSet::new(),
            next: 1,
            pointers: Vec::new(),
            keys: Vec::new(),
        }
    }

    /// The screen the session shows now.
    pub fn at(&self) -> &str {
        &self.at
    }

    /// The pointer presses and the key chords the session was sent, in order.
    pub fn pointers(&self) -> &[(i32, i32)] {
        &self.pointers
    }

    pub fn keys(&self) -> &[String] {
        &self.keys
    }

    fn current(&self) -> &Screen {
        self.script
            .screens
            .iter()
            .find(|screen| screen.id == self.at)
            .expect("the desktop shows a screen of its script")
    }

    fn check(&self, id: SessionId) -> Result<()> {
        if self.sessions.contains(&id) {
            Ok(())
        } else {
            Err(DesktopError::UnknownSession(id))
        }
    }

    /// The box of the control at `index`: a column of 300 by 40 points, 50 apart, under the window's top.
    fn bounds(index: usize) -> (i32, i32, u32, u32) {
        (20, 20 + 50 * i32::try_from(index).unwrap_or(0), 300, 40)
    }

    fn control_at(&self, x: i32, y: i32) -> Option<usize> {
        (0..self.current().controls.len()).find(|&i| {
            let (bx, by, bw, bh) = Self::bounds(i);
            x >= bx
                && x < bx + i32::try_from(bw).unwrap_or(0)
                && y >= by
                && y < by + i32::try_from(bh).unwrap_or(0)
        })
    }

    /// A press of the control at `index`: a button or a link leads where it points, a text field takes the focus.
    fn press(&mut self, index: usize) {
        let control = self.current().controls[index].clone();
        self.focus = None;
        match control.role.as_str() {
            "button" | "link" => {
                if let Some(to) = control.to {
                    self.at = to;
                }
            }
            "textbox" => self.focus = Some(control.id),
            _ => {}
        }
    }

    fn picture(&self) -> Result<Frame> {
        let title = self.current().title.as_bytes();
        let shade = title
            .iter()
            .fold(7u8, |h, b| h.wrapping_mul(31).wrapping_add(*b));
        let pixels: Vec<u8> = (0..PICTURE_W * PICTURE_H)
            .flat_map(|_| [shade, 255 - shade, 128, 255])
            .collect();
        Frame::new(PICTURE_W, PICTURE_H, pixels)
    }

    fn node(&self) -> UiNode {
        let screen = self.current();
        let children = screen
            .controls
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let (role, actions) = match c.role.as_str() {
                    "textbox" => ("text_field", vec!["set_value", "focus"]),
                    "button" | "link" => (c.role.as_str(), vec!["invoke"]),
                    other => (other, vec![]),
                };
                UiNode {
                    id: i as u64 + 1,
                    role: role.into(),
                    name: Some(c.label.clone()),
                    value: self.typed.get(&c.id).cloned(),
                    bounds: Self::bounds(i),
                    actions: actions.into_iter().map(str::to_string).collect(),
                    children: Vec::new(),
                }
            })
            .collect();
        UiNode {
            id: 0,
            role: "window".into(),
            name: Some(screen.title.clone()),
            value: None,
            bounds: (0, 0, 800, 600),
            actions: Vec::new(),
            children,
        }
    }
}

impl Desktop for ScriptDesk {
    fn open_session(&mut self, _spec: &SessionSpec) -> Result<SessionId> {
        self.at = self.script.start.clone();
        self.typed.clear();
        self.focus = None;
        let id = self.next;
        self.next += 1;
        self.sessions.insert(id);
        Ok(id)
    }

    fn close_session(&mut self, id: SessionId) -> Result<()> {
        if self.sessions.remove(&id) {
            Ok(())
        } else {
            Err(DesktopError::UnknownSession(id))
        }
    }

    fn capture(&mut self, id: SessionId) -> Result<Frame> {
        self.check(id)?;
        self.picture()
    }

    fn input(&mut self, id: SessionId, action: &InputAction) -> Result<()> {
        self.check(id)?;
        match action {
            InputAction::Click { x, y, .. } => {
                self.pointers.push((*x, *y));
                if let Some(index) = self.control_at(*x, *y) {
                    self.press(index);
                }
            }
            InputAction::Type { text } => {
                let focus = self
                    .focus
                    .clone()
                    .ok_or_else(|| DesktopError::Invalid("nothing has the focus".into()))?;
                self.typed.entry(focus).or_default().push_str(text);
            }
            InputAction::Key { combo } => self.keys.push(combo.clone()),
            _ => {}
        }
        Ok(())
    }

    fn ui_tree(&mut self, id: SessionId, _app: Option<&str>, _depth: u8) -> Result<UiNode> {
        self.check(id)?;
        Ok(self.node())
    }

    fn ui_act(&mut self, id: SessionId, selector: &Selector, action: &UiAction) -> Result<()> {
        self.check(id)?;
        let index = selector
            .path
            .as_ref()
            .and_then(|path| path.first())
            .map(|i| *i as usize)
            .filter(|i| *i < self.current().controls.len())
            .ok_or_else(|| DesktopError::NotFound("no control at that path".into()))?;
        match action {
            UiAction::Invoke => self.press(index),
            UiAction::SetValue { value } => {
                let id = self.current().controls[index].id.clone();
                self.typed.insert(id, value.clone());
            }
            UiAction::Focus => self.focus = Some(self.current().controls[index].id.clone()),
            _ => {}
        }
        Ok(())
    }

    fn apps(&mut self) -> Result<Vec<AppInfo>> {
        Ok(Vec::new())
    }

    fn launch(&mut self, _id: SessionId, _app: &str, _args: &[String]) -> Result<()> {
        Err(DesktopError::Unsupported("the script has no apps".into()))
    }

    fn owner_active(&self, _id: SessionId) -> bool {
        false
    }
}
