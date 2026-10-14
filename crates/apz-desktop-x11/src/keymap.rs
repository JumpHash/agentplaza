//! Keysyms and key plans: the X keysym of a key or a character, the server's keyboard map, and the plan that
//! presses a chord or types text on it. Pure: the map is passed in, so the plans are tested without a server.

use apz_desktop::{DesktopError, Key, Mods, Result};

/// `XK_Shift_L`.
pub const SHIFT_L: u32 = 0xffe1;
/// `XK_Control_L`.
pub const CONTROL_L: u32 = 0xffe3;
/// `XK_Alt_L`.
pub const ALT_L: u32 = 0xffe9;
/// `XK_Super_L`.
pub const SUPER_L: u32 = 0xffeb;

const BACKSPACE: u32 = 0xff08;
const TAB: u32 = 0xff09;
const RETURN: u32 = 0xff0d;
const ESCAPE: u32 = 0xff1b;
const HOME: u32 = 0xff50;
const LEFT: u32 = 0xff51;
const UP: u32 = 0xff52;
const RIGHT: u32 = 0xff53;
const DOWN: u32 = 0xff54;
const PAGE_UP: u32 = 0xff55;
const PAGE_DOWN: u32 = 0xff56;
const END: u32 = 0xff57;
const INSERT: u32 = 0xff63;
const DELETE: u32 = 0xffff;
/// `XK_F1`; `F<n>` is `XK_F1 + n - 1`.
const F1: u32 = 0xffbe;
const SPACE: u32 = 0x0020;

/// The keysym of a character: Latin-1 characters are their code point, other Unicode characters are
/// `0x01000000 + code point`, and line breaks and tabs are the Return and Tab keys. `None` for other control
/// characters, which no key types.
pub fn keysym_for_char(c: char) -> Option<u32> {
    match c {
        '\n' | '\r' => Some(RETURN),
        '\t' => Some(TAB),
        ' '..='~' | '\u{a0}'..='\u{ff}' => Some(c as u32),
        c if (c as u32) >= 0x100 => Some(0x0100_0000 | c as u32),
        _ => None,
    }
}

/// The keysym of a key of a chord. Letters, digits and punctuation are their ASCII code, as X spells them.
pub fn keysym_for_key(key: Key) -> u32 {
    match key {
        Key::Char(c) => c as u32,
        Key::F(n) => F1 + u32::from(n) - 1,
        Key::Enter => RETURN,
        Key::Tab => TAB,
        Key::Space => SPACE,
        Key::Escape => ESCAPE,
        Key::Backspace => BACKSPACE,
        Key::Delete => DELETE,
        Key::Insert => INSERT,
        Key::Home => HOME,
        Key::End => END,
        Key::PageUp => PAGE_UP,
        Key::PageDown => PAGE_DOWN,
        Key::Up => UP,
        Key::Down => DOWN,
        Key::Left => LEFT,
        Key::Right => RIGHT,
    }
}

/// Where a keysym is on the keyboard the server has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Found {
    pub keycode: u8,
    /// Whether the key makes the keysym only with Shift held (the second column of its map entry).
    pub shift: bool,
}

/// The server's keyboard map: for each keycode in `min..=max`, `per_keycode` keysyms, column 0 unshifted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyMap {
    min: u8,
    max: u8,
    per_keycode: usize,
    keysyms: Vec<u32>,
}

impl KeyMap {
    /// A map from a `GetKeyboardMapping` reply. `keysyms` holds `per_keycode` entries for each keycode in order.
    pub fn new(min: u8, max: u8, per_keycode: u8, keysyms: Vec<u32>) -> Self {
        Self {
            min,
            max,
            per_keycode: usize::from(per_keycode),
            keysyms,
        }
    }

    /// The keysyms of `keycode` (empty when it is out of range).
    pub fn keysyms_of(&self, keycode: u8) -> &[u32] {
        if self.per_keycode == 0 || keycode < self.min || keycode > self.max {
            return &[];
        }
        let start = usize::from(keycode - self.min) * self.per_keycode;
        self.keysyms
            .get(start..start + self.per_keycode)
            .unwrap_or(&[])
    }

    /// A key that makes `keysym`. An unshifted column wins over a shifted one.
    pub fn find(&self, keysym: u32) -> Option<Found> {
        for column in [0usize, 1] {
            for keycode in self.min..=self.max {
                if self.keysyms_of(keycode).get(column) == Some(&keysym) {
                    return Some(Found {
                        keycode,
                        shift: column == 1,
                    });
                }
            }
        }
        None
    }

    /// A keycode with no keysyms at all, from the top of the range down: the one a remap may use for a
    /// character the map lacks.
    pub fn scratch_keycode(&self) -> Option<u8> {
        (self.min..=self.max)
            .rev()
            .find(|&keycode| self.keysyms_of(keycode).iter().all(|&keysym| keysym == 0))
    }

    pub fn per_keycode(&self) -> usize {
        self.per_keycode
    }
}

/// What a key press targets: a key the map has, or a scratch keycode the executor remaps to the keysym first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Keycode(u8),
    Scratch(u32),
}

/// One key press: the modifier keycodes held down in order, then the target key tapped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub modifiers: Vec<u8>,
    pub target: Target,
}

/// Plans the press of `keysym` with `mods` held. Shift is added when the key only makes the keysym shifted.
pub fn plan(map: &KeyMap, mods: Mods, keysym: u32) -> Result<Plan> {
    let found = map.find(keysym);
    let shift = mods.shift || found.is_some_and(|key| key.shift);
    let mut modifiers = Vec::new();
    for (held, modifier) in [
        (mods.ctrl, CONTROL_L),
        (shift, SHIFT_L),
        (mods.alt, ALT_L),
        (mods.meta, SUPER_L),
    ] {
        if held {
            let key = map.find(modifier).ok_or_else(|| {
                DesktopError::Unsupported(format!(
                    "the keyboard has no key for modifier keysym 0x{modifier:x}"
                ))
            })?;
            modifiers.push(key.keycode);
        }
    }
    let target = match found {
        Some(key) => Target::Keycode(key.keycode),
        None => Target::Scratch(keysym),
    };
    Ok(Plan { modifiers, target })
}

/// The plans that type `text`, one per character.
pub fn plan_text(map: &KeyMap, text: &str) -> Result<Vec<Plan>> {
    text.chars()
        .map(|c| {
            let keysym = keysym_for_char(c)
                .ok_or_else(|| DesktopError::Invalid(format!("cannot type U+{:04X}", c as u32)))?;
            plan(map, Mods::default(), keysym)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_desktop::{KeyCombo, parse_combo};

    /// A small US-style map: keycode 8 is `a`/`A`, 9 is `1`/`!`, 10 is Shift, 11 is Control, 12 is Alt,
    /// 13 is Super, and 14 to 17 are empty. Min 8, max 17, two columns each.
    fn sample() -> KeyMap {
        let mut keysyms = vec![0u32; 10 * 2];
        keysyms[0] = u32::from(b'a');
        keysyms[1] = u32::from(b'A');
        keysyms[2] = u32::from(b'1');
        keysyms[3] = u32::from(b'!');
        keysyms[4] = SHIFT_L;
        keysyms[6] = CONTROL_L;
        keysyms[8] = ALT_L;
        keysyms[10] = SUPER_L;
        KeyMap::new(8, 17, 2, keysyms)
    }

    fn keycode(map: &KeyMap, keysym: u32) -> u8 {
        map.find(keysym).expect("in the map").keycode
    }

    #[test]
    fn the_keysyms_match_the_x_headers() {
        assert_eq!(SHIFT_L, 0xffe1);
        assert_eq!(CONTROL_L, 0xffe3);
        assert_eq!(ALT_L, 0xffe9);
        assert_eq!(SUPER_L, 0xffeb);
        assert_eq!(keysym_for_key(Key::Enter), 0xff0d);
        assert_eq!(keysym_for_key(Key::Tab), 0xff09);
        assert_eq!(keysym_for_key(Key::Escape), 0xff1b);
        assert_eq!(keysym_for_key(Key::Backspace), 0xff08);
        assert_eq!(keysym_for_key(Key::Delete), 0xffff);
        assert_eq!(keysym_for_key(Key::Insert), 0xff63);
        assert_eq!(keysym_for_key(Key::Home), 0xff50);
        assert_eq!(keysym_for_key(Key::End), 0xff57);
        assert_eq!(keysym_for_key(Key::PageUp), 0xff55);
        assert_eq!(keysym_for_key(Key::PageDown), 0xff56);
        assert_eq!(keysym_for_key(Key::Left), 0xff51);
        assert_eq!(keysym_for_key(Key::Up), 0xff52);
        assert_eq!(keysym_for_key(Key::Right), 0xff53);
        assert_eq!(keysym_for_key(Key::Down), 0xff54);
        assert_eq!(keysym_for_key(Key::Space), 0x0020);
    }

    #[test]
    fn function_keys_are_consecutive_from_f1() {
        assert_eq!(keysym_for_key(Key::F(1)), 0xffbe);
        assert_eq!(keysym_for_key(Key::F(12)), 0xffc9);
        assert_eq!(
            keysym_for_key(Key::F(13)),
            0xffca,
            "F13 is the header's XK_L3"
        );
        assert_eq!(
            keysym_for_key(Key::F(24)),
            0xffd5,
            "F24 is the header's XK_R4"
        );
    }

    #[test]
    fn characters_are_latin1_or_unicode() {
        assert_eq!(keysym_for_char('a'), Some(0x61));
        assert_eq!(keysym_for_char('A'), Some(0x41));
        assert_eq!(keysym_for_char(' '), Some(0x20));
        assert_eq!(keysym_for_char('é'), Some(0xe9));
        assert_eq!(keysym_for_char('€'), Some(0x0100_20ac));
        assert_eq!(keysym_for_char('\n'), Some(RETURN));
        assert_eq!(keysym_for_char('\t'), Some(TAB));
        assert_eq!(
            keysym_for_char('\u{1}'),
            None,
            "control characters have no key"
        );
    }

    #[test]
    fn a_keysym_is_found_unshifted_or_shifted() {
        let map = sample();
        assert_eq!(
            map.find(u32::from(b'a')),
            Some(Found {
                keycode: 8,
                shift: false
            })
        );
        assert_eq!(
            map.find(u32::from(b'A')),
            Some(Found {
                keycode: 8,
                shift: true
            })
        );
        assert_eq!(
            map.find(u32::from(b'!')),
            Some(Found {
                keycode: 9,
                shift: true
            })
        );
        assert_eq!(map.find(u32::from(b'z')), None);
    }

    #[test]
    fn an_unshifted_column_wins_over_a_shifted_one() {
        // Keycode 17 has `x` shifted; keycode 16 has `x` unshifted. The unshifted key is taken.
        let mut keysyms = vec![0u32; 10 * 2];
        keysyms[(17 - 8) * 2 + 1] = u32::from(b'x');
        keysyms[(16 - 8) * 2] = u32::from(b'x');
        let map = KeyMap::new(8, 17, 2, keysyms);
        assert_eq!(
            map.find(u32::from(b'x')),
            Some(Found {
                keycode: 16,
                shift: false
            })
        );
    }

    #[test]
    fn the_scratch_keycode_is_the_highest_empty_one() {
        assert_eq!(sample().scratch_keycode(), Some(17));
        let full = KeyMap::new(8, 9, 1, vec![1, 2]);
        assert_eq!(full.scratch_keycode(), None);
    }

    #[test]
    fn a_plain_key_is_one_tap_with_no_modifier() {
        let map = sample();
        let plan = plan(&map, Mods::default(), u32::from(b'a')).expect("plans");
        assert_eq!(
            plan,
            Plan {
                modifiers: vec![],
                target: Target::Keycode(8)
            }
        );
    }

    #[test]
    fn a_capital_is_shift_then_the_key() {
        let map = sample();
        let plans = plan_text(&map, "A").expect("plans");
        assert_eq!(
            plans,
            vec![Plan {
                modifiers: vec![10],
                target: Target::Keycode(8)
            }]
        );
    }

    #[test]
    fn a_chord_holds_its_modifiers_in_order_and_presses_its_key() {
        let map = sample();
        let combo: KeyCombo = parse_combo("ctrl+shift+a").expect("parses");
        let plan = plan(&map, combo.mods, keysym_for_key(combo.key)).expect("plans");
        // Ctrl (keycode 11) first, then Shift (keycode 10).
        assert_eq!(plan.modifiers, vec![11, 10]);
        assert_eq!(plan.target, Target::Keycode(8));
    }

    #[test]
    fn a_chord_with_a_shifted_key_adds_shift() {
        // `!` is shift-1 on this map: the key needs Shift even though the chord names only Ctrl.
        let map = sample();
        let ctrl = Mods {
            ctrl: true,
            ..Mods::default()
        };
        let plan = plan(&map, ctrl, u32::from(b'!')).expect("plans");
        assert_eq!(plan.modifiers, vec![11, 10]);
        assert_eq!(plan.target, Target::Keycode(9));
    }

    #[test]
    fn a_key_the_map_lacks_is_a_scratch_remap() {
        let map = sample();
        let plans = plan_text(&map, "é").expect("plans");
        assert_eq!(
            plans,
            vec![Plan {
                modifiers: vec![],
                target: Target::Scratch(0xe9)
            }]
        );
    }

    #[test]
    fn a_modifier_the_map_lacks_is_refused() {
        let map = KeyMap::new(8, 8, 1, vec![u32::from(b'a')]);
        let combo = parse_combo("cmd+a").expect("parses");
        assert!(plan(&map, combo.mods, keysym_for_key(combo.key)).is_err());
    }

    #[test]
    fn text_with_a_control_character_is_refused() {
        let map = sample();
        assert!(plan_text(&map, "a\u{7}").is_err());
        assert_eq!(plan_text(&map, "a\n").expect("plans").len(), 2);
        assert_eq!(keycode(&map, u32::from(b'1')), 9);
    }
}
