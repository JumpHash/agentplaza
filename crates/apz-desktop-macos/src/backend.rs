//! The macOS [`Desktop`]: sessions, and each call on CoreGraphics, the Accessibility API and the workspace.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::process::Command;
use std::time::Instant;

use apz_desktop::{
    AppInfo, Desktop, DesktopError, Frame, Result, SessionSpec, UiNode, action_name,
};
use apz_station::{Button, InputAction, Selector, SessionId, SessionKind, UiAction};
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::string::CFString;
use core_graphics::display::CGDisplay;
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, EventField,
    ScrollEventUnit,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use foreign_types::ForeignType;

use crate::ax::{self, AttrValue, AxStep};
use crate::input::{self, Step};
use crate::owner;
use crate::pixels;
use crate::tree::{self, Walk};
use crate::{apps, sys, workspace};

/// What a pixel session needs before this backend can open one. See the module documentation.
const PIXEL_SESSIONS_TODO: &str =
    "pixel sessions on macOS need the agent user's screen-sharing session; see the daemon";

const SCREEN_RECORDING: &str = "Screen Recording: allow the workstation daemon under System Settings > Privacy & Security > Screen & System Audio Recording, then restart it";

const ACCESSIBILITY: &str = "Accessibility: allow the workstation daemon under System Settings > Privacy & Security > Accessibility";

/// How many levels of the tree an action may search: a window's controls, its sheets, and their children.
const ACTION_LEVELS: u8 = 10;

/// The macOS desktop. See the module documentation for what each call uses.
#[derive(Debug)]
pub struct MacDesktop {
    sessions: BTreeMap<SessionId, Session>,
    next: SessionId,
}

/// A session: the shared console session, and when this backend last injected input into it.
#[derive(Debug)]
struct Session {
    last_injected: Option<Instant>,
}

impl Default for MacDesktop {
    fn default() -> Self {
        Self::new()
    }
}

impl MacDesktop {
    /// Runs `verb` on the app `id` names (its name, else its id), through AppleScript.
    fn script_for_app(&mut self, id: &str, verb: &str) -> Result<()> {
        if id.chars().any(char::is_control) {
            return Err(DesktopError::Invalid(
                "an app name has no control characters".into(),
            ));
        }
        let installed = workspace::installed_apps();
        let info = apps::find_app(&installed, id)
            .or_else(|| installed.iter().find(|app| app.id == id))
            .ok_or_else(|| DesktopError::NotFound(format!("no installed app named {id}")))?;
        let script = apps::running_app_script(&info.name, verb);
        let output = Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(&script)
            .output()
            .map_err(|e| DesktopError::Platform(format!("cannot run osascript: {e}")))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(DesktopError::Platform(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }

    pub fn new() -> Self {
        Self {
            sessions: BTreeMap::new(),
            next: 1,
        }
    }

    fn session(&self, id: SessionId) -> Result<&Session> {
        self.sessions
            .get(&id)
            .ok_or(DesktopError::UnknownSession(id))
    }

    fn session_mut(&mut self, id: SessionId) -> Result<&mut Session> {
        self.sessions
            .get_mut(&id)
            .ok_or(DesktopError::UnknownSession(id))
    }
}

impl Desktop for MacDesktop {
    fn open_session(&mut self, spec: &SessionSpec) -> Result<SessionId> {
        // TODO(daemon): a pixel session for a separate agent user needs that user's screen-sharing session on a
        // virtual display, created by the daemon. Until then only the owner's console session is offered.
        if spec.kind == SessionKind::Pixel {
            return Err(DesktopError::Unsupported(PIXEL_SESSIONS_TODO.into()));
        }
        let id = self.next;
        self.next += 1;
        self.sessions.insert(
            id,
            Session {
                last_injected: None,
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
        self.session(id)?;
        capture_main_display()
    }

    fn input(&mut self, id: SessionId, action: &InputAction) -> Result<()> {
        self.session(id)?;
        // Down and up happen where the pointer is now, which the owner may have moved.
        let (steps, _) = input::plan(action, cursor())?;
        require_accessibility()?;
        let source = event_source()?;
        self.session_mut(id)?.last_injected = Some(Instant::now());
        for step in &steps {
            post(&source, step)?;
        }
        Ok(())
    }

    fn ui_tree(&mut self, id: SessionId, app: Option<&str>, depth: u8) -> Result<UiNode> {
        self.session(id)?;
        require_accessibility()?;
        Ok(walk_for(app, depth)?.root)
    }

    fn ui_act(&mut self, id: SessionId, selector: &Selector, action: &UiAction) -> Result<()> {
        self.session(id)?;
        require_accessibility()?;
        let walk = walk_for(selector.app.as_deref(), ACTION_LEVELS)?;
        let path = ax::locate(&walk.root, selector)?;
        let node = ax::node_at(&walk.root, &path)
            .ok_or_else(|| DesktopError::NotFound("the control".into()))?;
        let name = action_name(action);
        if !node.actions.iter().any(|offered| offered == name) {
            return Err(DesktopError::Unsupported(format!(
                "{} takes no {name}",
                describe(node)
            )));
        }
        let element = walk
            .elements
            .get(node.id as usize)
            .and_then(Option::as_ref)
            .ok_or_else(|| DesktopError::NotFound("the control has no element".into()))?;
        perform(element, &ax::plan_action(action))
    }

    fn apps(&mut self) -> Result<Vec<AppInfo>> {
        Ok(workspace::installed_apps())
    }

    /// Brings a running app to the front (`activate`). The app is named by its name or its id. An app that is not
    /// running is left alone: the script never launches it.
    fn focus(&mut self, id: &str) -> Result<()> {
        self.script_for_app(id, "activate")
    }

    /// Quits a running app with `quit`, which lets it save. A forced quit is not built: the owner's app gets the
    /// chance to ask about unsaved work, so `force` is refused rather than faked.
    fn quit(&mut self, id: &str, force: bool) -> Result<()> {
        if force {
            return Err(DesktopError::Unsupported(
                "a forced quit is not built on macOS; quit the app without force".into(),
            ));
        }
        self.script_for_app(id, "quit")
    }

    fn launch(&mut self, id: SessionId, app: &str, args: &[String]) -> Result<()> {
        self.session(id)?;
        let installed = workspace::installed_apps();
        let info = apps::find_app(&installed, app)
            .ok_or_else(|| DesktopError::NotFound(format!("an app named {app}")))?;
        // `open -a` starts the app or brings it to the front, which is also how it is focused.
        let mut command = Command::new("/usr/bin/open");
        command.arg("-a").arg(&info.path);
        if !args.is_empty() {
            command.arg("--args").args(args);
        }
        let status = command
            .status()
            .map_err(|error| DesktopError::Platform(format!("could not run open: {error}")))?;
        if status.success() {
            Ok(())
        } else {
            Err(DesktopError::Platform(format!(
                "open could not start {app}: {status}"
            )))
        }
    }

    fn owner_active(&self, id: SessionId) -> bool {
        let Some(session) = self.sessions.get(&id) else {
            return false;
        };
        owner::owner_is_active(
            sys::seconds_since_input(),
            session
                .last_injected
                .map(|sent| sent.elapsed().as_secs_f64()),
        )
    }
}

/// The main display at its native pixel size.
///
/// This uses `CGDisplayCreateImage`, the simple CoreGraphics path. ScreenCaptureKit is the modern one, and recent
/// macOS versions deprecate the CoreGraphics capture calls in its favour. Both need the Screen Recording permission,
/// so the check is the same either way, and switching only changes this function.
///
/// The frame is at native pixels: on a Retina display a screen point is two pixels, so a point is the pixel divided
/// by the display's backing scale (the hub maps screenshot pixels to points, as it does for every backend).
fn capture_main_display() -> Result<Frame> {
    if !sys::screen_recording_granted() {
        return Err(DesktopError::NotPermitted(SCREEN_RECORDING.into()));
    }
    let image = CGDisplay::main().image().ok_or_else(|| {
        DesktopError::Platform("the main display gave no image; is Screen Recording on?".into())
    })?;
    // SAFETY: `image` is the CGImage that CGDisplayCreateImage returned under the create rule, and the local `image` owns
    // it until this function returns. The call only reads that image's bitmap info through the pointer, which stays
    // valid for the call.
    let bitmap_info = unsafe { sys::CGImageGetBitmapInfo(image.as_ptr().cast::<c_void>()) };
    let data = image.data();
    pixels::repack(
        data.bytes(),
        image.width(),
        image.height(),
        image.bytes_per_row(),
        pixels::layout(bitmap_info),
    )
}

/// The UI tree for `app` (its windows) or, without an app, for the desktop.
fn walk_for(app: Option<&str>, levels: u8) -> Result<Walk> {
    match app {
        None => tree::desktop(levels),
        Some(name) => {
            let pid = workspace::pid_of(name).ok_or_else(|| {
                DesktopError::NotFound(format!("an app named {name} is not running"))
            })?;
            tree::app(pid, name, levels)
        }
    }
}

fn require_accessibility() -> Result<()> {
    if sys::accessibility_trusted() {
        Ok(())
    } else {
        Err(DesktopError::NotPermitted(ACCESSIBILITY.into()))
    }
}

fn describe(node: &UiNode) -> String {
    match &node.name {
        Some(name) => format!("{} \"{name}\"", node.role),
        None => node.role.clone(),
    }
}

fn perform(element: &CFType, step: &AxStep) -> Result<()> {
    let error = match step {
        AxStep::Perform(action) => tree::perform_action(element, action),
        AxStep::Set { attribute, value } => {
            let value = match value {
                AttrValue::Bool(flag) => CFBoolean::from(*flag).as_CFType(),
                AttrValue::Text(text) => CFString::new(text).as_CFType(),
            };
            tree::set_attribute(element, attribute, &value)
        }
    };
    match error {
        sys::AX_ERROR_SUCCESS => Ok(()),
        sys::AX_ERROR_API_DISABLED => Err(DesktopError::NotPermitted(ACCESSIBILITY.into())),
        sys::AX_ERROR_ACTION_UNSUPPORTED
        | sys::AX_ERROR_ATTRIBUTE_UNSUPPORTED
        | sys::AX_ERROR_NOT_IMPLEMENTED => Err(DesktopError::Unsupported(format!(
            "the control does not accept that action (error {error})"
        ))),
        other => Err(DesktopError::Platform(format!(
            "the accessibility action failed (error {other})"
        ))),
    }
}

fn event_source() -> Result<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|()| DesktopError::Platform("could not open the event source".into()))
}

/// Where the pointer is now, in screen points. Zero when the system will not say.
fn cursor() -> (i32, i32) {
    event_source()
        .ok()
        .and_then(|source| CGEvent::new(source).ok())
        .map_or((0, 0), |event| {
            let point = event.location();
            (point.x.round() as i32, point.y.round() as i32)
        })
}

/// Posts one planned step to the HID event tap.
fn post(source: &CGEventSource, step: &Step) -> Result<()> {
    let event = match step {
        Step::Move { x, y } => mouse(source, CGEventType::MouseMoved, *x, *y, Button::Left)?,
        Step::Press {
            button,
            x,
            y,
            count,
        } => {
            let event = mouse(source, down_type(*button), *x, *y, *button)?;
            event.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, i64::from(*count));
            set_button(&event, *button);
            event
        }
        Step::Release {
            button,
            x,
            y,
            count,
        } => {
            let event = mouse(source, up_type(*button), *x, *y, *button)?;
            event.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, i64::from(*count));
            set_button(&event, *button);
            event
        }
        Step::Dragged { button, x, y } => {
            let event = mouse(source, drag_type(*button), *x, *y, *button)?;
            set_button(&event, *button);
            event
        }
        Step::Scroll { dx, dy, .. } => scroll(source, *dx, *dy)?,
        Step::Key { code, flags, down } => keyboard(source, *code, *flags, *down)?,
        Step::Text { units } => {
            // A text chunk is a key down and up that carry the characters, with no key code.
            for down in [true, false] {
                let event = keyboard(source, 0, 0, down)?;
                event.set_string_from_utf16_unchecked(units);
                event.post(CGEventTapLocation::HID);
            }
            return Ok(());
        }
    };
    event.post(CGEventTapLocation::HID);
    Ok(())
}

fn mouse(
    source: &CGEventSource,
    kind: CGEventType,
    x: i32,
    y: i32,
    button: Button,
) -> Result<CGEvent> {
    CGEvent::new_mouse_event(
        source.clone(),
        kind,
        CGPoint::new(f64::from(x), f64::from(y)),
        cg_button(button),
    )
    .map_err(|()| DesktopError::Platform("could not create a mouse event".into()))
}

fn scroll(source: &CGEventSource, dx: i32, dy: i32) -> Result<CGEvent> {
    // The wheel reports vertical first, then horizontal, in pixels.
    CGEvent::new_scroll_event(source.clone(), ScrollEventUnit::PIXEL, 2, dy, dx, 0)
        .map_err(|()| DesktopError::Platform("could not create a scroll event".into()))
}

fn keyboard(source: &CGEventSource, code: u16, flags: u64, down: bool) -> Result<CGEvent> {
    let event = CGEvent::new_keyboard_event(source.clone(), code, down)
        .map_err(|()| DesktopError::Platform("could not create a key event".into()))?;
    event.set_flags(CGEventFlags::from_bits_truncate(flags));
    Ok(event)
}

fn set_button(event: &CGEvent, button: Button) {
    event.set_integer_value_field(
        EventField::MOUSE_EVENT_BUTTON_NUMBER,
        input::button_number(button),
    );
}

fn cg_button(button: Button) -> CGMouseButton {
    match button {
        Button::Left => CGMouseButton::Left,
        Button::Right => CGMouseButton::Right,
        Button::Middle => CGMouseButton::Center,
    }
}

fn down_type(button: Button) -> CGEventType {
    match button {
        Button::Left => CGEventType::LeftMouseDown,
        Button::Right => CGEventType::RightMouseDown,
        Button::Middle => CGEventType::OtherMouseDown,
    }
}

fn up_type(button: Button) -> CGEventType {
    match button {
        Button::Left => CGEventType::LeftMouseUp,
        Button::Right => CGEventType::RightMouseUp,
        Button::Middle => CGEventType::OtherMouseUp,
    }
}

fn drag_type(button: Button) -> CGEventType {
    match button {
        Button::Left => CGEventType::LeftMouseDragged,
        Button::Right => CGEventType::RightMouseDragged,
        Button::Middle => CGEventType::OtherMouseDragged,
    }
}
