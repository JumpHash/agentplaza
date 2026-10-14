//! Keys as the Windows keyboard sees them. A chord presses virtual keys (US layout codes; a layout that moves the
//! punctuation keys changes what a punctuation chord presses). Typed text presses a virtual key for newlines and
//! tabs and sends every other character as a UTF-16 unit.

use apz_desktop::{Key, KeyCombo};

pub const VK_BACK: u16 = 0x08;
pub const VK_TAB: u16 = 0x09;
pub const VK_RETURN: u16 = 0x0D;
pub const VK_SHIFT: u16 = 0x10;
pub const VK_CONTROL: u16 = 0x11;
pub const VK_MENU: u16 = 0x12;
pub const VK_ESCAPE: u16 = 0x1B;
pub const VK_SPACE: u16 = 0x20;
pub const VK_PRIOR: u16 = 0x21;
pub const VK_NEXT: u16 = 0x22;
pub const VK_END: u16 = 0x23;
pub const VK_HOME: u16 = 0x24;
pub const VK_LEFT: u16 = 0x25;
pub const VK_UP: u16 = 0x26;
pub const VK_RIGHT: u16 = 0x27;
pub const VK_DOWN: u16 = 0x28;
pub const VK_INSERT: u16 = 0x2D;
pub const VK_DELETE: u16 = 0x2E;
pub const VK_LWIN: u16 = 0x5B;
pub const VK_F1: u16 = 0x70;
pub const VK_OEM_PLUS: u16 = 0xBB;

/// One step of a chord: a virtual key goes down or comes up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyStep {
    Down(u16),
    Up(u16),
}

/// One unit of typed text: a virtual key press, or a UTF-16 unit sent with `KEYEVENTF_UNICODE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextStep {
    Vk(u16),
    Unit(u16),
}

/// The virtual-key code of a key, or `None` when the key has no code (a function key past F24, a character
/// with no key on the US layout).
pub fn key_vk(key: Key) -> Option<u16> {
    Some(match key {
        Key::Char(c) => char_vk(c)?,
        Key::F(n) if (1..=24).contains(&n) => VK_F1 + u16::from(n) - 1,
        Key::F(_) => return None,
        Key::Enter => VK_RETURN,
        Key::Tab => VK_TAB,
        Key::Space => VK_SPACE,
        Key::Escape => VK_ESCAPE,
        Key::Backspace => VK_BACK,
        Key::Delete => VK_DELETE,
        Key::Insert => VK_INSERT,
        Key::Home => VK_HOME,
        Key::End => VK_END,
        Key::PageUp => VK_PRIOR,
        Key::PageDown => VK_NEXT,
        Key::Up => VK_UP,
        Key::Down => VK_DOWN,
        Key::Left => VK_LEFT,
        Key::Right => VK_RIGHT,
    })
}

fn char_vk(c: char) -> Option<u16> {
    Some(match c {
        // Letters and digits: the virtual-key code is the upper-case ASCII code.
        'a'..='z' | 'A'..='Z' | '0'..='9' => c.to_ascii_uppercase() as u16,
        '+' | '=' => VK_OEM_PLUS,
        '-' => 0xBD,
        ',' => 0xBC,
        '.' => 0xBE,
        '/' => 0xBF,
        ';' => 0xBA,
        '`' => 0xC0,
        '[' => 0xDB,
        '\\' => 0xDC,
        ']' => 0xDD,
        '\'' => 0xDE,
        _ => return None,
    })
}

/// Whether a virtual key is one of the navigation keys that need the extended-key flag.
pub fn is_extended(vk: u16) -> bool {
    matches!(
        vk,
        VK_PRIOR
            | VK_NEXT
            | VK_END
            | VK_HOME
            | VK_LEFT
            | VK_UP
            | VK_RIGHT
            | VK_DOWN
            | VK_INSERT
            | VK_DELETE
    )
}

/// The steps of a chord: the modifiers go down in order (Ctrl, Shift, Alt, Windows), the key is pressed, and the
/// modifiers come up in reverse. `plus` needs Shift on the US layout, so it adds Shift unless Shift is already
/// in the chord. `None` when the key has no virtual-key code.
pub fn chord_steps(combo: KeyCombo) -> Option<Vec<KeyStep>> {
    let key = key_vk(combo.key)?;
    let mods = combo.mods;
    let mut held = Vec::with_capacity(4);
    if mods.ctrl {
        held.push(VK_CONTROL);
    }
    if mods.shift || combo.key == Key::Char('+') {
        held.push(VK_SHIFT);
    }
    if mods.alt {
        held.push(VK_MENU);
    }
    if mods.meta {
        held.push(VK_LWIN);
    }
    let mut steps: Vec<KeyStep> = held.iter().map(|&vk| KeyStep::Down(vk)).collect();
    steps.push(KeyStep::Down(key));
    steps.push(KeyStep::Up(key));
    steps.extend(held.iter().rev().map(|&vk| KeyStep::Up(vk)));
    Some(steps)
}

/// The steps that type `text`. `\r\n`, `\r` and `\n` are one Enter; a tab is a Tab key; every other character
/// is one or two UTF-16 units (two for a character outside the Basic Multilingual Plane).
pub fn text_steps(text: &str) -> Vec<TextStep> {
    let mut steps = Vec::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                steps.push(TextStep::Vk(VK_RETURN));
            }
            '\n' => steps.push(TextStep::Vk(VK_RETURN)),
            '\t' => steps.push(TextStep::Vk(VK_TAB)),
            _ => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units).iter() {
                    steps.push(TextStep::Unit(*unit));
                }
            }
        }
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_desktop::parse_combo;

    fn chord(text: &str) -> Vec<KeyStep> {
        let combo = parse_combo(text).expect("the chord parses");
        chord_steps(combo).expect("the key has a code")
    }

    #[test]
    fn letters_and_digits_are_their_upper_case_codes() {
        assert_eq!(key_vk(Key::Char('a')), Some(0x41));
        assert_eq!(key_vk(Key::Char('z')), Some(0x5A));
        assert_eq!(key_vk(Key::Char('0')), Some(0x30));
        assert_eq!(key_vk(Key::Char('9')), Some(0x39));
    }

    #[test]
    fn function_keys_run_from_f1_to_f24() {
        assert_eq!(key_vk(Key::F(1)), Some(0x70));
        assert_eq!(key_vk(Key::F(12)), Some(0x7B));
        assert_eq!(key_vk(Key::F(24)), Some(0x87));
        assert_eq!(key_vk(Key::F(0)), None);
        assert_eq!(key_vk(Key::F(25)), None);
    }

    #[test]
    fn named_keys_have_their_codes() {
        let table = [
            (Key::Enter, 0x0D),
            (Key::Tab, 0x09),
            (Key::Space, 0x20),
            (Key::Escape, 0x1B),
            (Key::Backspace, 0x08),
            (Key::Delete, 0x2E),
            (Key::Insert, 0x2D),
            (Key::Home, 0x24),
            (Key::End, 0x23),
            (Key::PageUp, 0x21),
            (Key::PageDown, 0x22),
            (Key::Left, 0x25),
            (Key::Up, 0x26),
            (Key::Right, 0x27),
            (Key::Down, 0x28),
        ];
        for (key, vk) in table {
            assert_eq!(key_vk(key), Some(vk), "{key:?}");
        }
    }

    #[test]
    fn punctuation_has_its_oem_code() {
        let table = [
            ('-', 0xBD),
            ('=', 0xBB),
            ('+', 0xBB),
            (',', 0xBC),
            ('.', 0xBE),
            ('/', 0xBF),
            (';', 0xBA),
            ('`', 0xC0),
            ('[', 0xDB),
            ('\\', 0xDC),
            (']', 0xDD),
            ('\'', 0xDE),
        ];
        for (c, vk) in table {
            assert_eq!(key_vk(Key::Char(c)), Some(vk), "{c:?}");
        }
        assert_eq!(key_vk(Key::Char('#')), None);
    }

    #[test]
    fn a_chord_presses_modifiers_down_then_the_key_then_releases_in_reverse() {
        assert_eq!(
            chord("ctrl+shift+s"),
            vec![
                KeyStep::Down(0x11),
                KeyStep::Down(0x10),
                KeyStep::Down(0x53),
                KeyStep::Up(0x53),
                KeyStep::Up(0x10),
                KeyStep::Up(0x11),
            ]
        );
    }

    #[test]
    fn meta_is_the_windows_key() {
        assert_eq!(
            chord("win+d"),
            vec![
                KeyStep::Down(0x5B),
                KeyStep::Down(0x44),
                KeyStep::Up(0x44),
                KeyStep::Up(0x5B),
            ]
        );
    }

    #[test]
    fn plus_presses_shift_with_the_equal_key() {
        assert_eq!(
            chord("ctrl+plus"),
            vec![
                KeyStep::Down(0x11),
                KeyStep::Down(0x10),
                KeyStep::Down(0xBB),
                KeyStep::Up(0xBB),
                KeyStep::Up(0x10),
                KeyStep::Up(0x11),
            ]
        );
    }

    #[test]
    fn plus_with_shift_already_held_adds_no_second_shift() {
        assert_eq!(
            chord("shift+plus"),
            vec![
                KeyStep::Down(0x10),
                KeyStep::Down(0xBB),
                KeyStep::Up(0xBB),
                KeyStep::Up(0x10),
            ]
        );
    }

    #[test]
    fn a_bare_key_is_one_press() {
        assert_eq!(chord("enter"), vec![KeyStep::Down(0x0D), KeyStep::Up(0x0D)]);
    }

    #[test]
    fn only_navigation_keys_are_extended() {
        assert!(is_extended(VK_LEFT));
        assert!(is_extended(VK_DELETE));
        assert!(is_extended(VK_NEXT));
        assert!(!is_extended(VK_RETURN));
        assert!(!is_extended(VK_LWIN));
        assert!(!is_extended(0x41));
    }

    #[test]
    fn text_maps_newlines_and_tabs_to_keys_and_the_rest_to_units() {
        assert_eq!(
            text_steps("a\tb\n"),
            vec![
                TextStep::Unit(0x61),
                TextStep::Vk(VK_TAB),
                TextStep::Unit(0x62),
                TextStep::Vk(VK_RETURN),
            ]
        );
    }

    #[test]
    fn a_crlf_is_one_enter() {
        assert_eq!(
            text_steps("x\r\ny"),
            vec![
                TextStep::Unit(0x78),
                TextStep::Vk(VK_RETURN),
                TextStep::Unit(0x79),
            ]
        );
    }

    #[test]
    fn accented_text_is_one_unit_per_character() {
        assert_eq!(text_steps("é"), vec![TextStep::Unit(0xE9)]);
    }

    #[test]
    fn characters_outside_the_bmp_are_surrogate_pairs() {
        assert_eq!(
            text_steps("\u{1F600}"),
            vec![TextStep::Unit(0xD83D), TextStep::Unit(0xDE00)]
        );
    }
}
