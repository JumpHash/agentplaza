//! How many columns a character takes: 0 for combining marks and format characters, 2 for the East
//! Asian wide and fullwidth ranges and the common emoji blocks, 1 for the rest. A small table on
//! purpose: the crate's one dependency is the parser, and a full Unicode width table is not worth a
//! second one. The ranges are the wide ones of Unicode's EastAsianWidth data.

/// Ranges of zero-width characters (combining marks, zero-width joiners, variation selectors).
const ZERO: &[(u32, u32)] = &[
    (0x0300, 0x036F),
    (0x0483, 0x0489),
    (0x0591, 0x05BD),
    (0x0610, 0x061A),
    (0x064B, 0x065F),
    (0x0E31, 0x0E31),
    (0x0E34, 0x0E3A),
    (0x1AB0, 0x1AFF),
    (0x1DC0, 0x1DFF),
    (0x200B, 0x200F),
    (0x20D0, 0x20FF),
    (0xFE00, 0xFE0F),
    (0xFE20, 0xFE2F),
    (0xE0100, 0xE01EF),
];

/// Ranges of double-width characters.
const WIDE: &[(u32, u32)] = &[
    (0x1100, 0x115F),
    (0x231A, 0x231B),
    (0x2329, 0x232A),
    (0x23E9, 0x23EC),
    (0x25FD, 0x25FE),
    (0x2614, 0x2615),
    (0x2648, 0x2653),
    (0x267F, 0x267F),
    (0x2693, 0x2693),
    (0x26A1, 0x26A1),
    (0x26AA, 0x26AB),
    (0x26BD, 0x26BE),
    (0x26C4, 0x26C5),
    (0x26CE, 0x26CE),
    (0x26D4, 0x26D4),
    (0x26EA, 0x26EA),
    (0x26F2, 0x26F3),
    (0x26F5, 0x26F5),
    (0x26FA, 0x26FA),
    (0x26FD, 0x26FD),
    (0x2705, 0x2705),
    (0x270A, 0x270B),
    (0x2728, 0x2728),
    (0x274C, 0x274C),
    (0x274E, 0x274E),
    (0x2753, 0x2755),
    (0x2757, 0x2757),
    (0x2795, 0x2797),
    (0x27B0, 0x27B0),
    (0x27BF, 0x27BF),
    (0x2B1B, 0x2B1C),
    (0x2B50, 0x2B50),
    (0x2B55, 0x2B55),
    (0x2E80, 0x303E),
    (0x3041, 0x33FF),
    (0x3400, 0x4DBF),
    (0x4E00, 0x9FFF),
    (0xA000, 0xA4CF),
    (0xA960, 0xA97F),
    (0xAC00, 0xD7A3),
    (0xF900, 0xFAFF),
    (0xFE10, 0xFE19),
    (0xFE30, 0xFE6F),
    (0xFF00, 0xFF60),
    (0xFFE0, 0xFFE6),
    (0x1F004, 0x1F004),
    (0x1F0CF, 0x1F0CF),
    (0x1F18E, 0x1F18E),
    (0x1F191, 0x1F19A),
    (0x1F200, 0x1F251),
    (0x1F300, 0x1F64F),
    (0x1F680, 0x1F6FF),
    (0x1F7E0, 0x1F7EB),
    (0x1F90C, 0x1F9FF),
    (0x1FA70, 0x1FAFF),
    (0x20000, 0x3FFFD),
];

fn within(table: &[(u32, u32)], code: u32) -> bool {
    table
        .iter()
        .any(|&(low, high)| (low..=high).contains(&code))
}

/// The columns `c` takes on screen: 0, 1 or 2.
pub fn char_width(c: char) -> usize {
    let code = c as u32;
    if code < 0x20 || (0x7F..0xA0).contains(&code) {
        return 0;
    }
    if code < 0x300 {
        return 1;
    }
    if within(ZERO, code) {
        0
    } else if within(WIDE, code) {
        2
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::char_width;

    #[test]
    fn cjk_and_fullwidth_take_two_and_combining_marks_none() {
        assert_eq!(char_width('漢'), 2);
        assert_eq!(char_width('字'), 2);
        assert_eq!(char_width('한'), 2);
        assert_eq!(char_width('Ａ'), 2);
        assert_eq!(char_width('\u{0301}'), 0, "combining acute");
        assert_eq!(char_width('\u{200D}'), 0, "zero-width joiner");
        assert_eq!(char_width('\u{FE0F}'), 0, "variation selector");
        assert_eq!(char_width('\u{7}'), 0, "a control");
    }
}
