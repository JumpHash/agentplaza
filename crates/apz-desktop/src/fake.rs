//! An in-memory [`Desktop`] for tests of the daemon and the hub.
//!
//! Each session has a framebuffer that starts as a gradient. Input actions paint into it: a pointer
//! action paints a marker at the pointer, a drag or scroll paints a line, a typed text or a key paints a
//! cell along the bottom edge. The UI tree and the app list are fixed (two windows, Notes and Calc, and
//! three apps, one of which has no window). Every input, every UI action and every launch is logged, so a
//! test can check what the daemon asked the desktop to do.

use std::collections::BTreeMap;

use apz_station::{Bridge, InputAction, Selector, SessionId, UiAction};

use crate::desktop::{
    AppInfo, Desktop, DesktopError, Frame, Result, SessionSpec, UiNode, action_name,
};
use crate::keys::parse_combo;

/// The largest session side, the same bound `scale` applies.
const MAX_SIDE: u32 = 16384;

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const GREEN: [u8; 4] = [0, 200, 0, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];
const PURPLE: [u8; 4] = [160, 0, 200, 255];
const ORANGE: [u8; 4] = [255, 140, 0, 255];
const DARK: [u8; 4] = [40, 40, 40, 255];

/// One `launch` the fake was asked to perform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub session: SessionId,
    pub app: String,
    pub args: Vec<String>,
}

/// One `ui_act` the fake performed, with the selector and action as they were asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiActLog {
    pub session: SessionId,
    pub selector: Selector,
    pub action: UiAction,
}

/// The in-memory desktop. See the module documentation.
pub struct FakeDesktop {
    sessions: BTreeMap<SessionId, Session>,
    next: SessionId,
    apps: Vec<AppInfo>,
    root: UiNode,
    launches: Vec<Launch>,
    ui_acts: Vec<UiActLog>,
}

struct Session {
    frame: Frame,
    pointer: (i32, i32),
    owner_active: bool,
    inputs: Vec<InputAction>,
    cells: u32,
}

impl Default for FakeDesktop {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeDesktop {
    pub fn new() -> Self {
        Self {
            sessions: BTreeMap::new(),
            next: 1,
            apps: fixed_apps(),
            root: fixed_tree(),
            launches: Vec::new(),
            ui_acts: Vec::new(),
        }
    }

    /// Marks the owner as using (or no longer using) a session, as their input would.
    pub fn set_owner_active(&mut self, id: SessionId, active: bool) -> Result<()> {
        self.session_mut(id)?.owner_active = active;
        Ok(())
    }

    /// The inputs a session has received, in order. `None` for a session that is not open.
    pub fn inputs(&self, id: SessionId) -> Option<&[InputAction]> {
        self.sessions
            .get(&id)
            .map(|session| session.inputs.as_slice())
    }

    /// Every launch the fake performed.
    pub fn launches(&self) -> &[Launch] {
        &self.launches
    }

    /// Every UI action the fake performed.
    pub fn ui_acts(&self) -> &[UiActLog] {
        &self.ui_acts
    }

    fn session_mut(&mut self, id: SessionId) -> Result<&mut Session> {
        self.sessions
            .get_mut(&id)
            .ok_or(DesktopError::UnknownSession(id))
    }

    fn session(&self, id: SessionId) -> Result<&Session> {
        self.sessions
            .get(&id)
            .ok_or(DesktopError::UnknownSession(id))
    }

    /// The index path of the node a call starts from: the root, or one app's window.
    fn base(&self, app: Option<&str>) -> Result<Vec<usize>> {
        let Some(name) = app else {
            return Ok(Vec::new());
        };
        self.root
            .children
            .iter()
            .position(|child| child.role == "window" && child.name.as_deref() == Some(name))
            .map(|index| vec![index])
            .ok_or_else(|| DesktopError::NotFound(format!("a window of {name}")))
    }

    /// The index path of the control a selector picks.
    fn locate(&self, selector: &Selector) -> Result<Vec<usize>> {
        let base = self.base(selector.app.as_deref())?;
        if let Some(path) = &selector.path {
            let mut full = base;
            full.extend(path.iter().map(|&index| index as usize));
            return match node_at(&self.root, &full) {
                Some(_) => Ok(full),
                None => Err(DesktopError::NotFound(format!(
                    "no control at path {path:?}"
                ))),
            };
        }
        if selector.role.is_none() && selector.name.is_none() {
            return Err(DesktopError::Invalid(
                "a selector needs a path, a role or a name".into(),
            ));
        }
        let start = node_at(&self.root, &base)
            .ok_or_else(|| DesktopError::NotFound("the window".into()))?;
        let mut trail = base.clone();
        let mut found = None;
        search(start, selector, &mut trail, &mut found);
        found.ok_or_else(|| DesktopError::NotFound(format!("no control matches {selector:?}")))
    }
}

impl Desktop for FakeDesktop {
    fn open_session(&mut self, spec: &SessionSpec) -> Result<SessionId> {
        if spec.width == 0 || spec.height == 0 || spec.width > MAX_SIDE || spec.height > MAX_SIDE {
            return Err(DesktopError::Invalid(format!(
                "a session of {}x{} is out of range",
                spec.width, spec.height
            )));
        }
        let id = self.next;
        self.next += 1;
        let frame = gradient(spec.width, spec.height);
        self.sessions.insert(
            id,
            Session {
                frame,
                pointer: (0, 0),
                owner_active: false,
                inputs: Vec::new(),
                cells: 0,
            },
        );
        Ok(id)
    }

    fn close_session(&mut self, id: SessionId) -> Result<()> {
        self.sessions
            .remove(&id)
            .map(|_| ())
            .ok_or(DesktopError::UnknownSession(id))
    }

    fn capture(&mut self, id: SessionId) -> Result<Frame> {
        Ok(self.session(id)?.frame.clone())
    }

    fn input(&mut self, id: SessionId, action: &InputAction) -> Result<()> {
        // A bad key chord is refused before anything is drawn or logged.
        if let InputAction::Key { combo } = action {
            parse_combo(combo)?;
        }
        let session = self.session_mut(id)?;
        let frame = &mut session.frame;
        match action {
            InputAction::Move { x, y } => {
                session.pointer = (*x, *y);
                paint(frame, *x, *y, RED);
            }
            InputAction::Click { x, y, .. } => {
                session.pointer = (*x, *y);
                paint(frame, *x, *y, BLUE);
            }
            InputAction::Down { .. } => {
                let (x, y) = session.pointer;
                paint(frame, x, y, GREEN);
            }
            InputAction::Up { .. } => {
                let (x, y) = session.pointer;
                paint(frame, x, y, YELLOW);
            }
            InputAction::Scroll { x, y, dx, dy } => {
                line(frame, (*x, *y), (x + dx, y + dy), PURPLE);
            }
            InputAction::Drag { from, to } => {
                session.pointer = *to;
                line(frame, *from, *to, ORANGE);
            }
            InputAction::Type { text } => {
                let color = text.bytes().fold(0u8, |sum, byte| sum.wrapping_add(byte));
                cell(frame, session.cells, [color, 255 - color, 128, 255]);
                session.cells += 1;
            }
            InputAction::Key { .. } => {
                cell(frame, session.cells, DARK);
                session.cells += 1;
            }
        }
        session.inputs.push(action.clone());
        Ok(())
    }

    fn ui_tree(&mut self, id: SessionId, app: Option<&str>, depth: u8) -> Result<UiNode> {
        self.session(id)?;
        let base = self.base(app)?;
        let node = node_at(&self.root, &base)
            .ok_or_else(|| DesktopError::NotFound("the window".into()))?;
        Ok(truncated(node.clone(), depth))
    }

    fn ui_act(&mut self, id: SessionId, selector: &Selector, action: &UiAction) -> Result<()> {
        self.session(id)?;
        let path = self.locate(selector)?;
        let node = node_at_mut(&mut self.root, &path)
            .ok_or_else(|| DesktopError::NotFound("the control".into()))?;
        let name = action_name(action);
        if !node.actions.iter().any(|offered| offered == name) {
            return Err(DesktopError::Unsupported(format!(
                "{} takes no {name}",
                describe(node)
            )));
        }
        match action {
            UiAction::SetValue { value } => node.value = Some(value.clone()),
            UiAction::Toggle => {
                let next = if node.value.as_deref() == Some("on") {
                    "off"
                } else {
                    "on"
                };
                node.value = Some(next.to_string());
            }
            UiAction::Invoke
            | UiAction::Focus
            | UiAction::Select
            | UiAction::Expand
            | UiAction::Collapse => {}
        }
        self.ui_acts.push(UiActLog {
            session: id,
            selector: selector.clone(),
            action: action.clone(),
        });
        Ok(())
    }

    fn apps(&mut self) -> Result<Vec<AppInfo>> {
        Ok(self.apps.clone())
    }

    fn launch(&mut self, id: SessionId, app: &str, args: &[String]) -> Result<()> {
        self.session(id)?;
        if !self.apps.iter().any(|known| known.name == app) {
            return Err(DesktopError::NotFound(format!("an app named {app}")));
        }
        self.launches.push(Launch {
            session: id,
            app: app.to_string(),
            args: args.to_vec(),
        });
        Ok(())
    }

    fn owner_active(&self, id: SessionId) -> bool {
        self.sessions
            .get(&id)
            .is_some_and(|session| session.owner_active)
    }
}

fn gradient(width: u32, height: u32) -> Frame {
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            rgba.push((u64::from(x) * 255 / u64::from(width)) as u8);
            rgba.push((u64::from(y) * 255 / u64::from(height)) as u8);
            rgba.push(128);
            rgba.push(255);
        }
    }
    Frame::new(width, height, rgba).expect("a gradient matches its size")
}

fn put(frame: &mut Frame, x: i32, y: i32, color: [u8; 4]) {
    if x < 0 || y < 0 || x as u32 >= frame.w || y as u32 >= frame.h {
        return;
    }
    let index = (y as usize * frame.w as usize + x as usize) * 4;
    frame.rgba[index..index + 4].copy_from_slice(&color);
}

/// A 5 by 5 marker centred on the point.
fn paint(frame: &mut Frame, x: i32, y: i32, color: [u8; 4]) {
    for dy in -2..=2 {
        for dx in -2..=2 {
            put(frame, x + dx, y + dy, color);
        }
    }
}

/// A straight line of pixels from one point to another.
fn line(frame: &mut Frame, from: (i32, i32), to: (i32, i32), color: [u8; 4]) {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let steps = dx.abs().max(dy.abs()).max(1);
    for step in 0..=steps {
        let x = from.0 + dx * step / steps;
        let y = from.1 + dy * step / steps;
        put(frame, x, y, color);
    }
}

/// A 6 by 6 cell in the bottom edge, the `index`-th along it.
fn cell(frame: &mut Frame, index: u32, color: [u8; 4]) {
    let x = (index * 8) % frame.w;
    let y = frame.h.saturating_sub(8);
    for dy in 0..6 {
        for dx in 0..6 {
            put(frame, (x + dx) as i32, (y + dy) as i32, color);
        }
    }
}

fn node_at<'a>(root: &'a UiNode, path: &[usize]) -> Option<&'a UiNode> {
    path.iter()
        .try_fold(root, |node, &index| node.children.get(index))
}

fn node_at_mut<'a>(root: &'a mut UiNode, path: &[usize]) -> Option<&'a mut UiNode> {
    let mut node = root;
    for &index in path {
        node = node.children.get_mut(index)?;
    }
    Some(node)
}

/// Pre-order search for the first control the selector's role and name match.
fn search(
    node: &UiNode,
    selector: &Selector,
    trail: &mut Vec<usize>,
    found: &mut Option<Vec<usize>>,
) {
    let role_ok = selector
        .role
        .as_deref()
        .is_none_or(|role| role == node.role);
    let name_ok = selector
        .name
        .as_deref()
        .is_none_or(|name| node.name.as_deref() == Some(name));
    if role_ok && name_ok && (selector.role.is_some() || selector.name.is_some()) {
        *found = Some(trail.clone());
        return;
    }
    for (index, child) in node.children.iter().enumerate() {
        trail.push(index);
        search(child, selector, trail, found);
        trail.pop();
        if found.is_some() {
            return;
        }
    }
}

fn describe(node: &UiNode) -> String {
    match &node.name {
        Some(name) => format!("{} \"{name}\"", node.role),
        None => node.role.clone(),
    }
}

/// The tree down to `depth` levels below `node`.
fn truncated(node: UiNode, depth: u8) -> UiNode {
    let children = if depth == 0 {
        Vec::new()
    } else {
        node.children
            .iter()
            .cloned()
            .map(|child| truncated(child, depth - 1))
            .collect()
    };
    UiNode { children, ..node }
}

fn fixed_apps() -> Vec<AppInfo> {
    vec![
        AppInfo {
            id: "com.example.notes".into(),
            name: "Notes".into(),
            path: "/Applications/Notes.app".into(),
            scripting: vec![Bridge::AppleScript],
        },
        AppInfo {
            id: "com.example.calc".into(),
            name: "Calc".into(),
            path: "/Applications/Calc.app".into(),
            scripting: vec![],
        },
        AppInfo {
            id: "org.blender.Blender".into(),
            name: "Blender".into(),
            path: "/Applications/Blender.app".into(),
            scripting: vec![Bridge::Cli, Bridge::AppScripting],
        },
    ]
}

fn node(
    id: u64,
    role: &str,
    name: Option<&str>,
    value: Option<&str>,
    bounds: (i32, i32, u32, u32),
    actions: &[&str],
    children: Vec<UiNode>,
) -> UiNode {
    UiNode {
        id,
        role: role.into(),
        name: name.map(Into::into),
        value: value.map(Into::into),
        bounds,
        actions: actions.iter().map(|action| (*action).into()).collect(),
        children,
    }
}

/// The desktop's tree: a window for Notes (a button, a text field, a check box, a list) and one for Calc
/// (two buttons). Blender has no window.
fn fixed_tree() -> UiNode {
    let notes = node(
        2,
        "window",
        Some("Notes"),
        None,
        (100, 100, 800, 600),
        &["focus"],
        vec![
            node(
                3,
                "button",
                Some("Save"),
                None,
                (120, 120, 80, 30),
                &["invoke", "focus"],
                vec![],
            ),
            node(
                4,
                "text_field",
                Some("Title"),
                Some("Untitled"),
                (120, 160, 400, 30),
                &["focus", "set_value"],
                vec![],
            ),
            node(
                5,
                "check_box",
                Some("Wrap"),
                Some("off"),
                (120, 200, 120, 24),
                &["toggle", "focus"],
                vec![],
            ),
            node(
                6,
                "list",
                Some("Recent"),
                None,
                (120, 240, 400, 300),
                &["focus"],
                vec![node(
                    7,
                    "list_item",
                    Some("Draft 1"),
                    None,
                    (120, 240, 400, 24),
                    &["select"],
                    vec![],
                )],
            ),
        ],
    );
    let calc = node(
        8,
        "window",
        Some("Calc"),
        None,
        (950, 100, 300, 400),
        &["focus"],
        vec![
            node(
                9,
                "button",
                Some("7"),
                None,
                (960, 120, 60, 40),
                &["invoke"],
                vec![],
            ),
            node(
                10,
                "button",
                Some("+"),
                None,
                (1030, 120, 60, 40),
                &["invoke"],
                vec![],
            ),
        ],
    );
    node(
        1,
        "desktop",
        Some("Desktop"),
        None,
        (0, 0, 1920, 1080),
        &[],
        vec![notes, calc],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_station::{Button, SessionKind};

    fn spec() -> SessionSpec {
        SessionSpec {
            agent: 3,
            kind: SessionKind::Pixel,
            width: 200,
            height: 100,
        }
    }

    fn pixel(frame: &Frame, x: u32, y: u32) -> [u8; 4] {
        let index = ((y * frame.w + x) * 4) as usize;
        [
            frame.rgba[index],
            frame.rgba[index + 1],
            frame.rgba[index + 2],
            frame.rgba[index + 3],
        ]
    }

    fn click(x: i32, y: i32) -> InputAction {
        InputAction::Click {
            x,
            y,
            button: Button::Left,
            count: 1,
        }
    }

    fn sel(role: Option<&str>, name: Option<&str>, app: Option<&str>) -> Selector {
        Selector {
            role: role.map(Into::into),
            name: name.map(Into::into),
            path: None,
            app: app.map(Into::into),
        }
    }

    #[test]
    fn sessions_get_ids_and_capture_at_their_size() {
        let mut desk = FakeDesktop::new();
        let first = desk.open_session(&spec()).expect("opens");
        let second = desk.open_session(&spec()).expect("opens");
        assert_ne!(first, second);
        let frame = desk.capture(first).expect("captures");
        assert_eq!((frame.w, frame.h), (200, 100));
        assert_eq!(frame.rgba.len(), 200 * 100 * 4);
    }

    #[test]
    fn a_zero_sized_session_is_refused() {
        let mut desk = FakeDesktop::new();
        let mut bad = spec();
        bad.width = 0;
        assert!(matches!(
            desk.open_session(&bad),
            Err(DesktopError::Invalid(_))
        ));
    }

    #[test]
    fn a_closed_session_is_unknown() {
        let mut desk = FakeDesktop::new();
        let id = desk.open_session(&spec()).expect("opens");
        desk.close_session(id).expect("closes");
        assert_eq!(desk.capture(id), Err(DesktopError::UnknownSession(id)));
        assert_eq!(
            desk.close_session(id),
            Err(DesktopError::UnknownSession(id))
        );
        assert_eq!(desk.inputs(id), None);
    }

    #[test]
    fn a_click_paints_a_marker_and_is_logged() {
        let mut desk = FakeDesktop::new();
        let id = desk.open_session(&spec()).expect("opens");
        desk.input(id, &click(50, 40)).expect("clicks");
        let frame = desk.capture(id).expect("captures");
        assert_eq!(pixel(&frame, 50, 40), [0, 0, 255, 255]);
        assert_eq!(desk.inputs(id), Some(&[click(50, 40)][..]));
    }

    #[test]
    fn a_drag_paints_a_line_and_moves_the_pointer_for_later_presses() {
        let mut desk = FakeDesktop::new();
        let id = desk.open_session(&spec()).expect("opens");
        desk.input(
            id,
            &InputAction::Drag {
                from: (10, 10),
                to: (30, 10),
            },
        )
        .expect("drags");
        let frame = desk.capture(id).expect("captures");
        assert_eq!(pixel(&frame, 20, 10), [255, 140, 0, 255]);
        desk.input(
            id,
            &InputAction::Down {
                button: Button::Left,
            },
        )
        .expect("presses");
        let frame = desk.capture(id).expect("captures");
        assert_eq!(
            pixel(&frame, 30, 10),
            [0, 200, 0, 255],
            "the press lands where the drag ended"
        );
    }

    #[test]
    fn typing_and_keys_paint_cells_along_the_bottom_edge() {
        let mut desk = FakeDesktop::new();
        let id = desk.open_session(&spec()).expect("opens");
        let before = desk.capture(id).expect("captures");
        desk.input(
            id,
            &InputAction::Key {
                combo: "ctrl+s".into(),
            },
        )
        .expect("presses a chord");
        let after = desk.capture(id).expect("captures");
        assert_eq!(pixel(&after, 2, 94), [40, 40, 40, 255]);
        assert_ne!(before, after);
    }

    #[test]
    fn a_bad_chord_is_refused_and_not_logged() {
        let mut desk = FakeDesktop::new();
        let id = desk.open_session(&spec()).expect("opens");
        assert!(
            desk.input(
                id,
                &InputAction::Key {
                    combo: "ctrl+nope".into()
                }
            )
            .is_err()
        );
        assert_eq!(desk.inputs(id), Some(&[][..]));
    }

    #[test]
    fn the_owner_flag_is_per_session() {
        let mut desk = FakeDesktop::new();
        let id = desk.open_session(&spec()).expect("opens");
        assert!(!desk.owner_active(id));
        desk.set_owner_active(id, true).expect("sets");
        assert!(desk.owner_active(id));
        assert!(!desk.owner_active(id + 100));
        assert!(desk.set_owner_active(id + 100, true).is_err());
    }

    #[test]
    fn the_tree_is_whole_or_one_app_and_truncated_by_depth() {
        let mut desk = FakeDesktop::new();
        let id = desk.open_session(&spec()).expect("opens");
        let whole = desk.ui_tree(id, None, 5).expect("reads");
        assert_eq!(whole.role, "desktop");
        assert_eq!(whole.children.len(), 2);
        let notes = desk.ui_tree(id, Some("Notes"), 1).expect("reads");
        assert_eq!(notes.name.as_deref(), Some("Notes"));
        assert_eq!(
            notes.children.len(),
            4,
            "depth 1 keeps the window's controls"
        );
        assert!(notes.children[0].children.is_empty());
        let shallow = desk.ui_tree(id, None, 0).expect("reads");
        assert!(shallow.children.is_empty());
    }

    #[test]
    fn an_app_without_a_window_is_not_found() {
        let mut desk = FakeDesktop::new();
        let id = desk.open_session(&spec()).expect("opens");
        assert!(matches!(
            desk.ui_tree(id, Some("Blender"), 2),
            Err(DesktopError::NotFound(_))
        ));
    }

    #[test]
    fn a_selector_finds_a_control_by_role_and_name_or_by_path() {
        let mut desk = FakeDesktop::new();
        let id = desk.open_session(&spec()).expect("opens");
        desk.ui_act(
            id,
            &sel(Some("button"), Some("Save"), Some("Notes")),
            &UiAction::Invoke,
        )
        .expect("invokes Save");
        let by_path = Selector {
            path: Some(vec![0, 1]),
            ..Selector::default()
        };
        desk.ui_act(
            id,
            &by_path,
            &UiAction::SetValue {
                value: "Draft 2".into(),
            },
        )
        .expect("sets the title by path");
        let tree = desk.ui_tree(id, Some("Notes"), 1).expect("reads");
        assert_eq!(tree.children[1].value.as_deref(), Some("Draft 2"));
        assert_eq!(desk.ui_acts().len(), 2);
        assert_eq!(
            desk.ui_acts()[0].selector,
            sel(Some("button"), Some("Save"), Some("Notes"))
        );
    }

    #[test]
    fn toggling_flips_the_check_box_and_unoffered_actions_are_refused() {
        let mut desk = FakeDesktop::new();
        let id = desk.open_session(&spec()).expect("opens");
        let wrap = sel(Some("check_box"), Some("Wrap"), Some("Notes"));
        desk.ui_act(id, &wrap, &UiAction::Toggle).expect("toggles");
        let tree = desk.ui_tree(id, Some("Notes"), 1).expect("reads");
        assert_eq!(tree.children[2].value.as_deref(), Some("on"));
        assert!(matches!(
            desk.ui_act(
                id,
                &sel(Some("button"), Some("Save"), None),
                &UiAction::SetValue { value: "x".into() }
            ),
            Err(DesktopError::Unsupported(_))
        ));
    }

    #[test]
    fn a_selector_that_matches_nothing_or_nothing_at_all_is_an_error() {
        let mut desk = FakeDesktop::new();
        let id = desk.open_session(&spec()).expect("opens");
        assert!(matches!(
            desk.ui_act(id, &sel(Some("slider"), None, None), &UiAction::Focus),
            Err(DesktopError::NotFound(_))
        ));
        assert!(matches!(
            desk.ui_act(id, &Selector::default(), &UiAction::Focus),
            Err(DesktopError::Invalid(_))
        ));
    }

    #[test]
    fn apps_are_listed_and_launches_are_checked_and_logged() {
        let mut desk = FakeDesktop::new();
        let id = desk.open_session(&spec()).expect("opens");
        let apps = desk.apps().expect("lists");
        assert_eq!(apps.len(), 3);
        assert_eq!(apps[2].scripting, vec![Bridge::Cli, Bridge::AppScripting]);
        desk.launch(id, "Blender", &["-b".into(), "scene.blend".into()])
            .expect("launches");
        assert_eq!(
            desk.launches(),
            &[Launch {
                session: id,
                app: "Blender".into(),
                args: vec!["-b".into(), "scene.blend".into()],
            }]
        );
        assert!(matches!(
            desk.launch(id, "Nope", &[]),
            Err(DesktopError::NotFound(_))
        ));
    }
}
