//! Pointer and keyboard input through SendInput. Each action is one SendInput call, so its events are not
//! interleaved with another action's. Pointer positions are screen points over the virtual screen.

use apz_desktop::{DesktopError, Result, parse_combo};
use apz_station::{Button, InputAction};
use windows::Win32::Foundation::E_ACCESSDENIED;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSE_EVENT_FLAGS,
    MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput,
    VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use super::WindowsDesktop;
use crate::geometry::{absolute_axis, between, horizontal_wheel, vertical_wheel};
use crate::keys::{KeyStep, TextStep, chord_steps, is_extended, text_steps};

/// Points a drag passes through between its ends, so the target sees motion.
const DRAG_STEPS: i32 = 8;

impl WindowsDesktop {
    /// Performs one input action and records when it was injected, for [`WindowsDesktop::owner_active`].
    pub(super) fn send_action(&mut self, action: &InputAction) -> Result<()> {
        let mut events = Vec::new();
        let mut pointer = self.pointer;
        match action {
            InputAction::Move { x, y } => {
                events.push(move_event(*x, *y));
                pointer = (*x, *y);
            }
            InputAction::Click {
                x,
                y,
                button,
                count,
            } => {
                events.push(move_event(*x, *y));
                pointer = (*x, *y);
                for _ in 0..(*count).max(1) {
                    events.push(mouse(0, 0, 0, button_down(*button)));
                    events.push(mouse(0, 0, 0, button_up(*button)));
                }
            }
            InputAction::Down { button } => events.push(mouse(0, 0, 0, button_down(*button))),
            InputAction::Up { button } => events.push(mouse(0, 0, 0, button_up(*button))),
            InputAction::Scroll { x, y, dx, dy } => {
                events.push(move_event(*x, *y));
                pointer = (*x, *y);
                if *dy != 0 {
                    events.push(mouse(0, 0, vertical_wheel(*dy), MOUSEEVENTF_WHEEL));
                }
                if *dx != 0 {
                    events.push(mouse(0, 0, horizontal_wheel(*dx), MOUSEEVENTF_HWHEEL));
                }
            }
            InputAction::Drag { from, to } => {
                events.push(move_event(from.0, from.1));
                events.push(mouse(0, 0, 0, MOUSEEVENTF_LEFTDOWN));
                for step in 1..=DRAG_STEPS {
                    let (x, y) = between(*from, *to, step, DRAG_STEPS);
                    events.push(move_event(x, y));
                }
                events.push(mouse(0, 0, 0, MOUSEEVENTF_LEFTUP));
                pointer = *to;
            }
            InputAction::Type { text } => {
                for step in text_steps(text) {
                    match step {
                        TextStep::Vk(vk) => {
                            events.push(key_event(vk, false));
                            events.push(key_event(vk, true));
                        }
                        TextStep::Unit(unit) => {
                            events.push(unit_event(unit, false));
                            events.push(unit_event(unit, true));
                        }
                    }
                }
            }
            InputAction::Key { combo } => {
                let combo = parse_combo(combo)?;
                let steps = chord_steps(combo).ok_or_else(|| {
                    DesktopError::Invalid(format!("no key code for {:?}", combo.key))
                })?;
                for step in steps {
                    match step {
                        KeyStep::Down(vk) => events.push(key_event(vk, false)),
                        KeyStep::Up(vk) => events.push(key_event(vk, true)),
                    }
                }
            }
        }
        send(&events)?;
        self.pointer = pointer;
        // SAFETY: GetTickCount takes no pointers. It is read after the injection, so it is never earlier than the
        // system's stamp on our input.
        self.last_injected = Some(unsafe { GetTickCount() });
        Ok(())
    }
}

/// The events of an absolute pointer move to a screen point.
fn move_event(x: i32, y: i32) -> INPUT {
    // SAFETY: the metrics take no pointers.
    let (left, top, width, height) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    mouse(
        absolute_axis(x, left, width),
        absolute_axis(y, top, height),
        0,
        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
    )
}

fn button_down(button: Button) -> MOUSE_EVENT_FLAGS {
    match button {
        Button::Left => MOUSEEVENTF_LEFTDOWN,
        Button::Right => MOUSEEVENTF_RIGHTDOWN,
        Button::Middle => MOUSEEVENTF_MIDDLEDOWN,
    }
}

fn button_up(button: Button) -> MOUSE_EVENT_FLAGS {
    match button {
        Button::Left => MOUSEEVENTF_LEFTUP,
        Button::Right => MOUSEEVENTF_RIGHTUP,
        Button::Middle => MOUSEEVENTF_MIDDLEUP,
    }
}

fn mouse(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// A virtual-key event. The extended flag goes on the navigation keys, which share codes with the numpad.
fn key_event(vk: u16, up: bool) -> INPUT {
    let mut flags = if up {
        KEYEVENTF_KEYUP
    } else {
        KEYBD_EVENT_FLAGS(0)
    };
    if is_extended(vk) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    keyboard(VIRTUAL_KEY(vk), 0, flags)
}

/// A UTF-16 unit event, for a character that has no key.
fn unit_event(unit: u16, up: bool) -> INPUT {
    let flags = if up {
        KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
    } else {
        KEYEVENTF_UNICODE
    };
    keyboard(VIRTUAL_KEY(0), unit, flags)
}

fn keyboard(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Sends the events in one call. When Windows injects none, the usual cause is UIPI: the window that has focus
/// runs elevated, and an unelevated process may not send it input.
fn send(events: &[INPUT]) -> Result<()> {
    // SAFETY: the slice holds `events.len()` INPUT structs of the size passed.
    let sent = unsafe { SendInput(events, std::mem::size_of::<INPUT>() as i32) } as usize;
    if sent == events.len() {
        return Ok(());
    }
    let error = windows::core::Error::from_win32();
    if error.code() == E_ACCESSDENIED {
        Err(DesktopError::NotPermitted(
            "Windows refused the input: the focused window runs elevated (UIPI) or the desktop is locked".into(),
        ))
    } else {
        Err(DesktopError::Platform(format!(
            "SendInput sent {sent} of {} events: {error}",
            events.len()
        )))
    }
}
