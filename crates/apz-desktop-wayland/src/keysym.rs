//! Keys on the Wayland seat: the X keysym of each key, modifier and typed character, and the evdev code of
//! each pointer button. The RemoteDesktop portal takes keysyms and evdev codes, so these are the whole mapping.

use apz_desktop::{DesktopError, Key, Mods, Result};
use apz_station::Button;

/// Shift_L, Control_L, Alt_L and Super_L, the keysyms of the modifiers.
const SHIFT_L: u32 = 0xffe1;
const CONTROL_L: u32 = 0xffe3;
const ALT_L: u32 = 0xffe9;
const SUPER_L: u32 = 0xffeb;

/// Return and Tab, the keysyms a newline and a tab type.
const RETURN: u32 = 0xff0d;
const TAB: u32 = 0xff09;

/// F1; F2 to F24 follow it in order.
const F1: u32 = 0xffbe;

/// evdev codes of the pointer buttons (`BTN_LEFT`, `BTN_RIGHT`, `BTN_MIDDLE` in linux/input-event-codes.h).
const BTN_LEFT: i32 = 0x110;
const BTN_RIGHT: i32 = 0x111;
const BTN_MIDDLE: i32 = 0x112;

/// The evdev code of a pointer button.
pub fn button_code(button: Button) -> i32 {
    match button {
        Button::Left => BTN_LEFT,
        Button::Right => BTN_RIGHT,
        Button::Middle => BTN_MIDDLE,
    }
}

/// The keysyms of the modifiers held, in the order they are pressed (the release runs the list backwards).
pub fn modifier_keysyms(mods: Mods) -> Vec<u32> {
    let mut keysyms = Vec::with_capacity(4);
    if mods.ctrl {
        keysyms.push(CONTROL_L);
    }
    if mods.shift {
        keysyms.push(SHIFT_L);
    }
    if mods.alt {
        keysyms.push(ALT_L);
    }
    if mods.meta {
        keysyms.push(SUPER_L);
    }
    keysyms
}

/// The keysym of one key of a chord.
pub fn key_keysym(key: Key) -> u32 {
    match key {
        Key::Char(c) => char_keysym(c),
        Key::F(n) => F1 + u32::from(n).saturating_sub(1),
        Key::Enter => RETURN,
        Key::Tab => TAB,
        Key::Space => char_keysym(' '),
        Key::Escape => 0xff1b,
        Key::Backspace => 0xff08,
        Key::Delete => 0xffff,
        Key::Insert => 0xff63,
        Key::Home => 0xff50,
        Key::End => 0xff57,
        Key::PageUp => 0xff55,
        Key::PageDown => 0xff56,
        Key::Left => 0xff51,
        Key::Up => 0xff52,
        Key::Right => 0xff53,
        Key::Down => 0xff54,
    }
}

/// The keysym that types `c`: the code point itself for Latin-1 printable characters, and `0x01000000` plus the
/// code point for every other character (the X keysym encoding of Unicode).
pub fn char_keysym(c: char) -> u32 {
    let code = c as u32;
    if (0x20..=0x7e).contains(&code) || (0xa0..=0xff).contains(&code) {
        code
    } else {
        0x0100_0000 + code
    }
}

/// The keysyms that type `text`, in order. A newline or a carriage return presses Return and a tab presses Tab.
/// Any other control character is refused, and the whole text is refused with it, so a caller never types a
/// part of a text it asked for.
pub fn text_keysyms(text: &str) -> Result<Vec<u32>> {
    text.chars()
        .map(|c| match c {
            '\n' | '\r' => Ok(RETURN),
            '\t' => Ok(TAB),
            c if c.is_control() => Err(DesktopError::Invalid(format!(
                "cannot type the control character U+{:04X}",
                c as u32
            ))),
            c => Ok(char_keysym(c)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_keysyms_are_code_points_with_a_unicode_offset() {
        for (case, c, want) in [
            ("a lowercase letter is its own code point", 'a', 0x61),
            ("an uppercase letter is its own code point", 'A', 0x41),
            ("a digit is its own code point", '7', 0x37),
            ("a space is its own code point", ' ', 0x20),
            ("a latin-1 letter is its own code point", 'é', 0xe9),
            (
                "other unicode uses the keysym offset",
                '€',
                0x0100_0000 + 0x20ac,
            ),
            (
                "an astral char uses the keysym offset",
                '😀',
                0x0100_0000 + 0x1f600,
            ),
        ] {
            assert_eq!(char_keysym(c), want, "{case}");
        }
    }

    #[test]
    fn key_keysyms_cover_named_keys_function_keys_and_combo_chars() {
        for (case, key, want) in [
            ("Enter has the x keysym", Key::Enter, 0xff0d),
            ("Escape has the x keysym", Key::Escape, 0xff1b),
            ("Delete has the x keysym", Key::Delete, 0xffff),
            ("Left has the x keysym", Key::Left, 0xff51),
            ("Up has the x keysym", Key::Up, 0xff52),
            ("Right has the x keysym", Key::Right, 0xff53),
            ("Down has the x keysym", Key::Down, 0xff54),
            ("PageUp has the x keysym", Key::PageUp, 0xff55),
            ("PageDown has the x keysym", Key::PageDown, 0xff56),
            ("Home has the x keysym", Key::Home, 0xff50),
            ("End has the x keysym", Key::End, 0xff57),
            ("Insert has the x keysym", Key::Insert, 0xff63),
            ("Backspace has the x keysym", Key::Backspace, 0xff08),
            ("Tab has the x keysym", Key::Tab, 0xff09),
            ("Space has the x keysym", Key::Space, 0x20),
            ("F1 starts the function keys", Key::F(1), 0xffbe),
            ("F12 is the twelfth function key", Key::F(12), 0xffc9),
            ("F13 follows F12", Key::F(13), 0xffca),
            ("F24 is the last function key", Key::F(24), 0xffd5),
            ("a char in a combo is its keysym", Key::Char('s'), 0x73),
            ("a dash in a combo is its keysym", Key::Char('-'), 0x2d),
        ] {
            assert_eq!(key_keysym(key), want, "{case}");
        }
    }

    #[test]
    fn modifiers_press_in_a_fixed_order() {
        let all = Mods {
            ctrl: true,
            shift: true,
            alt: true,
            meta: true,
        };
        assert_eq!(modifier_keysyms(all), vec![0xffe3, 0xffe1, 0xffe9, 0xffeb]);
        assert_eq!(modifier_keysyms(Mods::default()), Vec::<u32>::new());
        let shift_only = Mods {
            shift: true,
            ..Mods::default()
        };
        assert_eq!(modifier_keysyms(shift_only), vec![0xffe1]);
    }

    #[test]
    fn pointer_buttons_are_the_evdev_codes() {
        assert_eq!(button_code(Button::Left), 0x110);
        assert_eq!(button_code(Button::Right), 0x111);
        assert_eq!(button_code(Button::Middle), 0x112);
    }

    #[test]
    fn text_types_newlines_and_tabs_as_keys() {
        assert_eq!(
            text_keysyms("a\n\tB").expect("types"),
            vec![0x61, 0xff0d, 0xff09, 0x42]
        );
        assert_eq!(text_keysyms("\r").expect("types"), vec![0xff0d]);
        assert_eq!(text_keysyms("").expect("types"), Vec::<u32>::new());
    }

    #[test]
    fn a_text_with_a_control_character_is_refused_whole() {
        assert!(matches!(
            text_keysyms("ok\u{7}"),
            Err(DesktopError::Invalid(_))
        ));
        assert!(matches!(
            text_keysyms("\u{0}"),
            Err(DesktopError::Invalid(_))
        ));
    }
}
