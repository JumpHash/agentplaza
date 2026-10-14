//! Key codes and modifier flags for macOS.
//!
//! The virtual key codes are the Carbon `kVK_*` values from HIToolbox's `Events.h`. They name a key by its position
//! on an ANSI keyboard, so a layout that moves letters still gets the same code for the same key; the characters
//! that come out depend on the layout, as they do for a person typing. The flags are the `CGEventFlags` masks.

use apz_desktop::{DesktopError, Key, KeyCombo, Mods, Result};

/// `CGEventFlags` for Shift.
pub const FLAG_SHIFT: u64 = 0x0002_0000;
/// `CGEventFlags` for Control.
pub const FLAG_CONTROL: u64 = 0x0004_0000;
/// `CGEventFlags` for Option (Alternate).
pub const FLAG_ALTERNATE: u64 = 0x0008_0000;
/// `CGEventFlags` for Command.
pub const FLAG_COMMAND: u64 = 0x0010_0000;

/// The flags for the modifiers a chord holds.
pub fn modifier_flags(mods: Mods) -> u64 {
    [
        (mods.shift, FLAG_SHIFT),
        (mods.ctrl, FLAG_CONTROL),
        (mods.alt, FLAG_ALTERNATE),
        (mods.meta, FLAG_COMMAND),
    ]
    .into_iter()
    .filter(|(held, _)| *held)
    .fold(0, |flags, (_, flag)| flags | flag)
}

/// The virtual key code and the flags a whole chord needs: the modifiers it holds, plus any the key itself needs
/// (an upper-case letter or `+` is typed with Shift).
pub fn chord(combo: &KeyCombo) -> Result<(u16, u64)> {
    let (code, key_flags) = key_code(combo.key)?;
    Ok((code, key_flags | modifier_flags(combo.mods)))
}

/// The virtual key code of a key, and the flags the key itself needs.
pub fn key_code(key: Key) -> Result<(u16, u64)> {
    let code = match key {
        Key::Char(c) => return char_code(c),
        Key::F(n) => return f_key_code(n),
        Key::Enter => 0x24,
        Key::Tab => 0x30,
        Key::Space => 0x31,
        Key::Escape => 0x35,
        Key::Backspace => 0x33,
        // kVK_ForwardDelete: the Mac's Delete key is backspace, and forward delete is Fn-Delete.
        Key::Delete => 0x75,
        // A Mac keyboard has no Insert; its Help key sits where a PC's Insert does.
        Key::Insert => 0x72,
        Key::Home => 0x73,
        Key::End => 0x77,
        Key::PageUp => 0x74,
        Key::PageDown => 0x79,
        Key::Up => 0x7E,
        Key::Down => 0x7D,
        Key::Left => 0x7B,
        Key::Right => 0x7C,
    };
    Ok((code, 0))
}

fn char_code(c: char) -> Result<(u16, u64)> {
    if c.is_ascii_uppercase() {
        let (code, flags) = char_code(c.to_ascii_lowercase())?;
        return Ok((code, flags | FLAG_SHIFT));
    }
    let code = match c {
        'a' => 0x00,
        's' => 0x01,
        'd' => 0x02,
        'f' => 0x03,
        'h' => 0x04,
        'g' => 0x05,
        'z' => 0x06,
        'x' => 0x07,
        'c' => 0x08,
        'v' => 0x09,
        'b' => 0x0B,
        'q' => 0x0C,
        'w' => 0x0D,
        'e' => 0x0E,
        'r' => 0x0F,
        'y' => 0x10,
        't' => 0x11,
        '1' => 0x12,
        '2' => 0x13,
        '3' => 0x14,
        '4' => 0x15,
        '6' => 0x16,
        '5' => 0x17,
        '=' => 0x18,
        '9' => 0x19,
        '7' => 0x1A,
        '-' => 0x1B,
        '8' => 0x1C,
        '0' => 0x1D,
        ']' => 0x1E,
        'o' => 0x1F,
        'u' => 0x20,
        '[' => 0x21,
        'i' => 0x22,
        'p' => 0x23,
        'l' => 0x25,
        'j' => 0x26,
        '\'' => 0x27,
        'k' => 0x28,
        ';' => 0x29,
        '\\' => 0x2A,
        ',' => 0x2B,
        '/' => 0x2C,
        'n' => 0x2D,
        'm' => 0x2E,
        '.' => 0x2F,
        '`' => 0x32,
        // `+` is Shift and `=` on a US layout.
        '+' => return Ok((0x18, FLAG_SHIFT)),
        other => {
            return Err(DesktopError::Unsupported(format!(
                "no key for \"{other}\" on macOS"
            )));
        }
    };
    Ok((code, 0))
}

/// `F1` to `F20`. Apple's keyboards stop at F20, so `F21` to `F24` are refused.
fn f_key_code(n: u8) -> Result<(u16, u64)> {
    const CODES: [u16; 20] = [
        0x7A, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D, 0x67, 0x6F, 0x69, 0x6B, 0x71,
        0x6A, 0x40, 0x4F, 0x50, 0x5A,
    ];
    usize::from(n)
        .checked_sub(1)
        .and_then(|index| CODES.get(index))
        .map(|code| (*code, 0))
        .ok_or_else(|| DesktopError::Unsupported(format!("F{n} is not on a Mac keyboard")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_desktop::parse_combo;

    fn code_of(c: char) -> u16 {
        char_code(c).expect("a key").0
    }

    #[test]
    fn letters_have_their_ansi_positions() {
        assert_eq!(code_of('a'), 0x00);
        assert_eq!(code_of('s'), 0x01);
        assert_eq!(code_of('q'), 0x0C);
        assert_eq!(code_of('z'), 0x06);
        assert_eq!(code_of('m'), 0x2E);
    }

    #[test]
    fn all_letters_and_digits_have_distinct_codes() {
        let mut seen = std::collections::BTreeSet::new();
        for c in ('a'..='z').chain('0'..='9') {
            assert!(seen.insert(code_of(c)), "{c} repeats a key code");
        }
        assert_eq!(seen.len(), 36);
    }

    #[test]
    fn upper_case_and_plus_need_shift() {
        assert_eq!(char_code('A'), Ok((0x00, FLAG_SHIFT)));
        assert_eq!(char_code('+'), Ok((0x18, FLAG_SHIFT)));
        assert_eq!(char_code('='), Ok((0x18, 0)));
    }

    #[test]
    fn punctuation_has_its_key() {
        assert_eq!(code_of('-'), 0x1B);
        assert_eq!(code_of(','), 0x2B);
        assert_eq!(code_of('`'), 0x32);
        assert_eq!(code_of('\\'), 0x2A);
    }

    #[test]
    fn named_keys_map_to_their_codes() {
        assert_eq!(key_code(Key::Enter), Ok((0x24, 0)));
        assert_eq!(key_code(Key::Escape), Ok((0x35, 0)));
        assert_eq!(key_code(Key::Backspace), Ok((0x33, 0)));
        assert_eq!(key_code(Key::Delete), Ok((0x75, 0)));
        assert_eq!(key_code(Key::Left), Ok((0x7B, 0)));
        assert_eq!(key_code(Key::Right), Ok((0x7C, 0)));
        assert_eq!(key_code(Key::Up), Ok((0x7E, 0)));
        assert_eq!(key_code(Key::Down), Ok((0x7D, 0)));
        assert_eq!(key_code(Key::PageUp), Ok((0x74, 0)));
        assert_eq!(key_code(Key::PageDown), Ok((0x79, 0)));
        assert_eq!(key_code(Key::Home), Ok((0x73, 0)));
        assert_eq!(key_code(Key::End), Ok((0x77, 0)));
    }

    #[test]
    fn function_keys_stop_at_f20() {
        assert_eq!(key_code(Key::F(1)), Ok((0x7A, 0)));
        assert_eq!(key_code(Key::F(12)), Ok((0x6F, 0)));
        assert_eq!(key_code(Key::F(20)), Ok((0x5A, 0)));
        assert!(matches!(
            key_code(Key::F(21)),
            Err(DesktopError::Unsupported(_))
        ));
    }

    #[test]
    fn modifiers_map_to_their_flags() {
        assert_eq!(modifier_flags(Mods::default()), 0);
        let all = Mods {
            ctrl: true,
            shift: true,
            alt: true,
            meta: true,
        };
        assert_eq!(
            modifier_flags(all),
            FLAG_SHIFT | FLAG_CONTROL | FLAG_ALTERNATE | FLAG_COMMAND
        );
        assert_eq!(
            modifier_flags(Mods {
                meta: true,
                ..Mods::default()
            }),
            FLAG_COMMAND
        );
    }

    #[test]
    fn a_chord_combines_its_key_and_modifiers() {
        let save = parse_combo("cmd+s").expect("parses");
        assert_eq!(chord(&save), Ok((0x01, FLAG_COMMAND)));
        let upper = parse_combo("ctrl+shift+a").expect("parses");
        assert_eq!(chord(&upper), Ok((0x00, FLAG_CONTROL | FLAG_SHIFT)));
        let plus = parse_combo("cmd+plus").expect("parses");
        assert_eq!(chord(&plus), Ok((0x18, FLAG_COMMAND | FLAG_SHIFT)));
    }

    #[test]
    fn a_key_with_no_code_is_refused_not_guessed() {
        assert!(matches!(char_code('é'), Err(DesktopError::Unsupported(_))));
    }
}
