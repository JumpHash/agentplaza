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
    fn keys_have_their_virtual_key_codes() {
        for (case, key, want) in [
            (
                "letter a is its upper case code",
                Key::Char('a'),
                Some(0x41),
            ),
            (
                "letter z is its upper case code",
                Key::Char('z'),
                Some(0x5A),
            ),
            ("digit 0 is its code", Key::Char('0'), Some(0x30)),
            ("digit 9 is its code", Key::Char('9'), Some(0x39)),
            ("F1 is 0x70", Key::F(1), Some(0x70)),
            ("F12 is 0x7B", Key::F(12), Some(0x7B)),
            ("F24 is 0x87", Key::F(24), Some(0x87)),
            ("F0 has no code", Key::F(0), None),
            ("F25 has no code", Key::F(25), None),
            ("enter", Key::Enter, Some(0x0D)),
            ("tab", Key::Tab, Some(0x09)),
            ("space", Key::Space, Some(0x20)),
            ("escape", Key::Escape, Some(0x1B)),
            ("backspace", Key::Backspace, Some(0x08)),
            ("delete", Key::Delete, Some(0x2E)),
            ("insert", Key::Insert, Some(0x2D)),
            ("home", Key::Home, Some(0x24)),
            ("end", Key::End, Some(0x23)),
            ("page up", Key::PageUp, Some(0x21)),
            ("page down", Key::PageDown, Some(0x22)),
            ("left", Key::Left, Some(0x25)),
            ("up", Key::Up, Some(0x26)),
            ("right", Key::Right, Some(0x27)),
            ("down", Key::Down, Some(0x28)),
            ("minus has its OEM code", Key::Char('-'), Some(0xBD)),
            ("equal has its OEM code", Key::Char('='), Some(0xBB)),
            ("plus is the equal key", Key::Char('+'), Some(0xBB)),
            ("comma has its OEM code", Key::Char(','), Some(0xBC)),
            ("period has its OEM code", Key::Char('.'), Some(0xBE)),
            ("slash has its OEM code", Key::Char('/'), Some(0xBF)),
            ("semicolon has its OEM code", Key::Char(';'), Some(0xBA)),
            ("backtick has its OEM code", Key::Char('`'), Some(0xC0)),
            ("open bracket has its OEM code", Key::Char('['), Some(0xDB)),
            ("backslash has its OEM code", Key::Char('\\'), Some(0xDC)),
            ("close bracket has its OEM code", Key::Char(']'), Some(0xDD)),
            ("quote has its OEM code", Key::Char('\''), Some(0xDE)),
            ("hash has no code", Key::Char('#'), None),
        ] {
            assert_eq!(key_vk(key), want, "{case}");
        }
    }

    #[test]
    fn chords_become_down_and_up_steps() {
        for (case, text, want) in [
            (
                "modifiers go down, then the key, then release in reverse",
                "ctrl+shift+s",
                vec![
                    KeyStep::Down(0x11),
                    KeyStep::Down(0x10),
                    KeyStep::Down(0x53),
                    KeyStep::Up(0x53),
                    KeyStep::Up(0x10),
                    KeyStep::Up(0x11),
                ],
            ),
            (
                "meta is the windows key",
                "win+d",
                vec![
                    KeyStep::Down(0x5B),
                    KeyStep::Down(0x44),
                    KeyStep::Up(0x44),
                    KeyStep::Up(0x5B),
                ],
            ),
            (
                "plus presses shift with the equal key",
                "ctrl+plus",
                vec![
                    KeyStep::Down(0x11),
                    KeyStep::Down(0x10),
                    KeyStep::Down(0xBB),
                    KeyStep::Up(0xBB),
                    KeyStep::Up(0x10),
                    KeyStep::Up(0x11),
                ],
            ),
            (
                "plus with shift already held adds no second shift",
                "shift+plus",
                vec![
                    KeyStep::Down(0x10),
                    KeyStep::Down(0xBB),
                    KeyStep::Up(0xBB),
                    KeyStep::Up(0x10),
                ],
            ),
            (
                "a bare key is one press",
                "enter",
                vec![KeyStep::Down(0x0D), KeyStep::Up(0x0D)],
            ),
        ] {
            assert_eq!(chord(text), want, "{case}");
        }
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
    fn text_becomes_unit_and_key_steps() {
        for (case, text, want) in [
            (
                "newlines and tabs are keys, the rest are units",
                "a\tb\n",
                vec![
                    TextStep::Unit(0x61),
                    TextStep::Vk(VK_TAB),
                    TextStep::Unit(0x62),
                    TextStep::Vk(VK_RETURN),
                ],
            ),
            (
                "a crlf is one enter",
                "x\r\ny",
                vec![
                    TextStep::Unit(0x78),
                    TextStep::Vk(VK_RETURN),
                    TextStep::Unit(0x79),
                ],
            ),
            (
                "accented text is one unit per character",
                "é",
                vec![TextStep::Unit(0xE9)],
            ),
            (
                "characters outside the bmp are surrogate pairs",
                "\u{1F600}",
                vec![TextStep::Unit(0xD83D), TextStep::Unit(0xDE00)],
            ),
        ] {
            assert_eq!(text_steps(text), want, "{case}");
        }
    }
}
