//! Key chords: `"ctrl+shift+s"` parsed into modifiers and one key, with names every platform understands.

use crate::desktop::{DesktopError, Result};

/// The modifiers held with the key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// Command on macOS, the Windows key on Windows, Super on Linux.
    pub meta: bool,
}

/// The one key a chord presses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A letter (lower case), a digit, or a punctuation character.
    Char(char),
    /// `f1` to `f24`.
    F(u8),
    Enter,
    Tab,
    Space,
    Escape,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
}

/// A parsed chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyCombo {
    pub mods: Mods,
    pub key: Key,
}

/// Parses a chord: modifiers and one key joined by `+`, case-insensitive, spaces around parts ignored.
///
/// Modifiers: `ctrl`/`control`; `shift`; `alt`/`option`; `cmd`/`command`/`super`/`win`/`windows`/`meta`
/// (all mean `meta`). Keys: a letter, a digit, `f1`..`f24`, `enter`/`return`, `tab`, `space`,
/// `esc`/`escape`, `backspace`, `delete`/`del`, `insert`/`ins`, `home`, `end`, `pageup`/`pgup`,
/// `pagedown`/`pgdn`, the arrows `up`/`down`/`left`/`right` (also `arrowup` and so on), and the
/// punctuation keys (`- = [ ] ; ' \ , . / `` ` ``) by their character or by name (`minus`, `equal`,
/// `bracketleft`, `bracketright`, `semicolon`, `quote`, `backslash`, `comma`, `period`, `slash`,
/// `grave`, `plus`).
pub fn parse_combo(text: &str) -> Result<KeyCombo> {
    let parts: Vec<String> = text
        .split('+')
        .map(|part| part.trim().to_ascii_lowercase())
        .collect();
    let (key_name, modifier_names) = parts.split_last().ok_or_else(|| invalid(text, "no key"))?;
    let mut mods = Mods::default();
    for name in modifier_names {
        match name.as_str() {
            "ctrl" | "control" => mods.ctrl = true,
            "shift" => mods.shift = true,
            "alt" | "option" => mods.alt = true,
            "cmd" | "command" | "super" | "win" | "windows" | "meta" => mods.meta = true,
            "" => return Err(invalid(text, "an empty part")),
            other => return Err(invalid(text, &format!("unknown modifier \"{other}\""))),
        }
    }
    let key =
        parse_key(key_name).ok_or_else(|| invalid(text, &format!("unknown key \"{key_name}\"")))?;
    Ok(KeyCombo { mods, key })
}

fn parse_key(name: &str) -> Option<Key> {
    let mut chars = name.chars();
    if let (Some(only), None) = (chars.next(), chars.next()) {
        return (only.is_ascii_alphanumeric() || PUNCTUATION.contains(&only))
            .then_some(Key::Char(only));
    }
    if let Some(digits) = name.strip_prefix('f')
        && !digits.is_empty()
        && digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return digits
            .parse::<u8>()
            .ok()
            .filter(|n| (1..=24).contains(n))
            .map(Key::F);
    }
    Some(match name {
        "enter" | "return" => Key::Enter,
        "tab" => Key::Tab,
        "space" => Key::Space,
        "esc" | "escape" => Key::Escape,
        "backspace" | "bksp" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "insert" | "ins" => Key::Insert,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "pgup" => Key::PageUp,
        "pagedown" | "pgdn" => Key::PageDown,
        "up" | "arrowup" => Key::Up,
        "down" | "arrowdown" => Key::Down,
        "left" | "arrowleft" => Key::Left,
        "right" | "arrowright" => Key::Right,
        "minus" => Key::Char('-'),
        "equal" | "equals" => Key::Char('='),
        "bracketleft" => Key::Char('['),
        "bracketright" => Key::Char(']'),
        "semicolon" => Key::Char(';'),
        "quote" => Key::Char('\''),
        "backslash" => Key::Char('\\'),
        "comma" => Key::Char(','),
        "period" => Key::Char('.'),
        "slash" => Key::Char('/'),
        "grave" | "backquote" => Key::Char('`'),
        "plus" => Key::Char('+'),
        _ => return None,
    })
}

/// The punctuation keys that can be named by their character. `+` is only named `plus`, since it
/// separates the parts of a chord.
const PUNCTUATION: [char; 11] = ['-', '=', '[', ']', ';', '\'', '\\', ',', '.', '/', '`'];

fn invalid(text: &str, reason: &str) -> DesktopError {
    DesktopError::Invalid(format!("key chord \"{text}\": {reason}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combo(text: &str) -> KeyCombo {
        parse_combo(text).unwrap_or_else(|error| panic!("{text} should parse: {error}"))
    }

    #[test]
    fn chords_parse_and_bad_chords_are_refused() {
        // a_chord_has_its_modifiers_and_one_key
        {
            assert_eq!(
                combo("ctrl+shift+s"),
                KeyCombo {
                    mods: Mods {
                        ctrl: true,
                        shift: true,
                        ..Mods::default()
                    },
                    key: Key::Char('s'),
                }
            );
            assert_eq!(combo("s").mods, Mods::default());
        }
        // the_names_of_each_platform_map_to_the_same_modifier
        {
            for name in ["cmd", "command", "super", "win", "windows", "meta"] {
                assert!(combo(&format!("{name}+a")).mods.meta, "{name}");
            }
            for name in ["ctrl", "control"] {
                assert!(combo(&format!("{name}+a")).mods.ctrl, "{name}");
            }
            for name in ["alt", "option"] {
                assert!(combo(&format!("{name}+a")).mods.alt, "{name}");
            }
        }
        // case_and_spaces_do_not_matter
        {
            assert_eq!(combo("Ctrl + Shift + S"), combo("ctrl+shift+s"));
        }
        // named_keys_parse
        for (text, key) in [
            ("enter", Key::Enter),
            ("return", Key::Enter),
            ("esc", Key::Escape),
            ("pgdn", Key::PageDown),
            ("arrowleft", Key::Left),
            ("up", Key::Up),
            ("del", Key::Delete),
            ("space", Key::Space),
            ("tab", Key::Tab),
            ("backspace", Key::Backspace),
        ] {
            assert_eq!(combo(text).key, key, "{text}");
        }
        // function_keys_run_from_one_to_twenty_four
        for (text, n) in [("f1", 1), ("cmd+F12", 12), ("f24", 24)] {
            assert_eq!(combo(text).key, Key::F(n), "{text}");
        }
        assert!(parse_combo("f0").is_err());
        assert!(parse_combo("f25").is_err());
        // digits_letters_and_punctuation_parse
        for (text, c) in [
            ("7", '7'),
            ("ctrl+-", '-'),
            ("cmd+,", ','),
            ("cmd+minus", '-'),
            ("alt+`", '`'),
            ("ctrl+plus", '+'),
            ("ctrl+bracketleft", '['),
        ] {
            assert_eq!(combo(text).key, Key::Char(c), "{text}");
        }
        // bad_chords_are_refused
        {
            assert!(parse_combo("").is_err());
            assert!(parse_combo("ctrl+").is_err());
            assert!(parse_combo("ctrl+nosuchkey").is_err());
            assert!(parse_combo("hyper+a").is_err());
            assert!(
                parse_combo("ctrl").is_err(),
                "a modifier alone is not a chord"
            );
            assert!(parse_combo("ctrl+++").is_err());
        }
    }
}
