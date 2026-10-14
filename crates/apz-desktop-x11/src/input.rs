//! Input through XTest: pointer motion, buttons, the wheel (buttons 4 to 7), keys, and typed text.
//!
//! Coordinates are screen points of the session. The keyboard is planned by [`crate::keymap`]; a character the
//! server's map lacks is typed through a scratch keycode remapped to it, and the mapping is restored before the
//! action returns, whether it succeeded or not.

use std::thread;
use std::time::Duration;

use apz_desktop::{DesktopError, Result, parse_combo};
use apz_station::{Button, InputAction};
use x11rb::NONE;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, Window};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use crate::capture::platform_error;
use crate::keymap::{KeyMap, Plan, Target, keysym_for_key, plan, plan_text};

/// The `XTest` event types: `KeyPress`, `KeyRelease`, `ButtonPress`, `ButtonRelease`, `MotionNotify`.
const KEY_PRESS: u8 = 2;
const KEY_RELEASE: u8 = 3;
const BUTTON_PRESS: u8 = 4;
const BUTTON_RELEASE: u8 = 5;
const MOTION_NOTIFY: u8 = 6;

/// The most clicks one action makes (a triple click is the last one a control counts).
const MAX_CLICKS: u8 = 3;
/// Screen points of wheel travel per notch. A scroll of `d` points makes `ceil(|d| / 40)` notches.
pub(crate) const POINTS_PER_NOTCH: u32 = 40;
/// The most notches one scroll action makes on one axis.
const MAX_NOTCHES: u32 = 200;
/// How long the server gets to tell applications about a scratch remap before the key that uses it.
const REMAP_SETTLE: Duration = Duration::from_millis(20);

/// The button a pointer button is on X: left 1, middle 2, right 3.
pub(crate) fn button_number(button: &Button) -> u8 {
    match button {
        Button::Left => 1,
        Button::Middle => 2,
        Button::Right => 3,
    }
}

/// The wheel buttons a scroll of `dx` and `dy` points presses, with their notch counts. Positive `dy` is
/// down (button 5), negative up (4); positive `dx` is right (7), negative left (6).
pub(crate) fn scroll_buttons(dx: i32, dy: i32) -> Vec<(u8, u32)> {
    let mut buttons = Vec::new();
    if dy != 0 {
        buttons.push((if dy > 0 { 5 } else { 4 }, notches(dy)));
    }
    if dx != 0 {
        buttons.push((if dx > 0 { 7 } else { 6 }, notches(dx)));
    }
    buttons
}

fn notches(delta: i32) -> u32 {
    delta.unsigned_abs().div_ceil(POINTS_PER_NOTCH)
}

/// Refuses a point outside the screen.
pub(crate) fn check_point(x: i32, y: i32, width: u32, height: u32) -> Result<()> {
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        return Err(DesktopError::Invalid(format!(
            "({x}, {y}) is off the {width}x{height} screen"
        )));
    }
    Ok(())
}

/// The server's keyboard map, from its setup range.
pub(crate) fn fetch_keymap(conn: &RustConnection) -> Result<KeyMap> {
    let setup = conn.setup();
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    let count = max - min + 1;
    let reply = conn
        .get_keyboard_mapping(min, count)
        .map_err(platform_error)?
        .reply()
        .map_err(platform_error)?;
    Ok(KeyMap::new(
        min,
        max,
        reply.keysyms_per_keycode,
        reply.keysyms,
    ))
}

/// Performs one action on the session whose root window is `root` and whose screen is `width` by `height`.
pub(crate) fn perform(
    conn: &RustConnection,
    root: Window,
    (width, height): (u32, u32),
    action: &InputAction,
) -> Result<()> {
    let input = Injector { conn, root };
    match action {
        InputAction::Move { x, y } => {
            check_point(*x, *y, width, height)?;
            input.move_to(*x, *y)
        }
        InputAction::Click {
            x,
            y,
            button,
            count,
        } => {
            check_point(*x, *y, width, height)?;
            if *count == 0 || *count > MAX_CLICKS {
                return Err(DesktopError::Invalid(format!(
                    "a click is 1 to {MAX_CLICKS} clicks, not {count}"
                )));
            }
            input.move_to(*x, *y)?;
            for _ in 0..*count {
                input.button(button_number(button), true)?;
                input.button(button_number(button), false)?;
            }
            Ok(())
        }
        InputAction::Down { button } => input.button(button_number(button), true),
        InputAction::Up { button } => input.button(button_number(button), false),
        InputAction::Scroll { x, y, dx, dy } => {
            check_point(*x, *y, width, height)?;
            let buttons = scroll_buttons(*dx, *dy);
            if let Some(&(_, count)) = buttons.iter().find(|(_, count)| *count > MAX_NOTCHES) {
                return Err(DesktopError::Invalid(format!(
                    "a scroll of {count} notches is more than {MAX_NOTCHES}"
                )));
            }
            input.move_to(*x, *y)?;
            for (button, count) in buttons {
                for _ in 0..count {
                    input.button(button, true)?;
                    input.button(button, false)?;
                }
            }
            Ok(())
        }
        InputAction::Drag { from, to } => {
            check_point(from.0, from.1, width, height)?;
            check_point(to.0, to.1, width, height)?;
            input.move_to(from.0, from.1)?;
            input.button(1, true)?;
            input.move_to(to.0, to.1)?;
            input.button(1, false)
        }
        InputAction::Type { text } => {
            let map = fetch_keymap(conn)?;
            let plans = plan_text(&map, text)?;
            run_plans(&input, &map, &plans)
        }
        InputAction::Key { combo } => {
            let combo = parse_combo(combo)?;
            let map = fetch_keymap(conn)?;
            let keypress = plan(&map, combo.mods, keysym_for_key(combo.key))?;
            run_plans(&input, &map, std::slice::from_ref(&keypress))
        }
    }
}

/// The XTest events of one session.
struct Injector<'a> {
    conn: &'a RustConnection,
    root: Window,
}

impl Injector<'_> {
    fn fake(&self, kind: u8, detail: u8, root: Window, x: i16, y: i16) -> Result<()> {
        self.conn
            .xtest_fake_input(kind, detail, 0, root, x, y, 0)
            .map_err(platform_error)?
            .check()
            .map_err(platform_error)
    }

    /// Moves the pointer to an absolute screen point.
    fn move_to(&self, x: i32, y: i32) -> Result<()> {
        // A motion with detail 0 is absolute; the root window names the screen.
        self.fake(MOTION_NOTIFY, 0, self.root, x as i16, y as i16)
    }

    fn button(&self, button: u8, press: bool) -> Result<()> {
        let kind = if press { BUTTON_PRESS } else { BUTTON_RELEASE };
        self.fake(kind, button, NONE, 0, 0)
    }

    fn key(&self, keycode: u8, press: bool) -> Result<()> {
        let kind = if press { KEY_PRESS } else { KEY_RELEASE };
        self.fake(kind, keycode, NONE, 0, 0)
    }
}

/// The scratch keycode a remap currently uses, if any, so it can be restored.
#[derive(Default)]
struct Scratch {
    keycode: Option<u8>,
}

fn run_plans(input: &Injector, map: &KeyMap, plans: &[Plan]) -> Result<()> {
    let mut scratch = Scratch::default();
    let result = plans
        .iter()
        .try_for_each(|plan| press(input, map, plan, &mut scratch));
    let restored = restore(input, map, &mut scratch);
    result.and(restored)
}

fn press(input: &Injector, map: &KeyMap, plan: &Plan, scratch: &mut Scratch) -> Result<()> {
    let keycode = match plan.target {
        Target::Keycode(keycode) => keycode,
        Target::Scratch(keysym) => {
            let keycode = match scratch.keycode {
                Some(keycode) => keycode,
                None => {
                    let keycode = map.scratch_keycode().ok_or_else(|| {
                        DesktopError::Unsupported(
                            "the keyboard has no spare keycode to type this with".into(),
                        )
                    })?;
                    scratch.keycode = Some(keycode);
                    keycode
                }
            };
            remap(input.conn, keycode, keysym, map.per_keycode())?;
            thread::sleep(REMAP_SETTLE);
            keycode
        }
    };
    for &modifier in &plan.modifiers {
        input.key(modifier, true)?;
    }
    input.key(keycode, true)?;
    input.key(keycode, false)?;
    for &modifier in plan.modifiers.iter().rev() {
        input.key(modifier, false)?;
    }
    Ok(())
}

/// Maps `keycode` to `keysym` in the first column and nothing else.
fn remap(conn: &RustConnection, keycode: u8, keysym: u32, per_keycode: usize) -> Result<()> {
    let columns = per_keycode.max(1);
    let mut keysyms = vec![0u32; columns];
    keysyms[0] = keysym;
    conn.change_keyboard_mapping(1, keycode, columns as u8, &keysyms)
        .map_err(platform_error)?
        .check()
        .map_err(platform_error)
}

/// Clears a scratch keycode back to no keysyms.
fn restore(input: &Injector, map: &KeyMap, scratch: &mut Scratch) -> Result<()> {
    let Some(keycode) = scratch.keycode.take() else {
        return Ok(());
    };
    let columns = map.per_keycode().max(1);
    let keysyms = vec![0u32; columns];
    input
        .conn
        .change_keyboard_mapping(1, keycode, columns as u8, &keysyms)
        .map_err(platform_error)?
        .check()
        .map_err(platform_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_buttons_are_left_middle_right() {
        assert_eq!(button_number(&Button::Left), 1);
        assert_eq!(button_number(&Button::Middle), 2);
        assert_eq!(button_number(&Button::Right), 3);
    }

    #[test]
    fn a_scroll_is_wheel_notches_per_axis() {
        assert_eq!(scroll_buttons(0, 0), vec![]);
        assert_eq!(scroll_buttons(0, 40), vec![(5, 1)], "down");
        assert_eq!(scroll_buttons(0, -41), vec![(4, 2)], "up, rounded up");
        assert_eq!(scroll_buttons(-80, 0), vec![(6, 2)], "left");
        assert_eq!(scroll_buttons(10, 120), vec![(5, 3), (7, 1)]);
    }

    #[test]
    fn points_outside_the_screen_are_refused() {
        assert!(check_point(0, 0, 10, 10).is_ok());
        assert!(check_point(9, 9, 10, 10).is_ok());
        assert!(check_point(10, 0, 10, 10).is_err());
        assert!(check_point(0, -1, 10, 10).is_err());
    }
}
