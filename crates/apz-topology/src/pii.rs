//! Personal data patterns (PLAN 22.7, 22.5's scan): an email address, a phone number, a card number (13 to 19 digits that
//! pass the Luhn check), a national id in the US social security form (`123-45-6789`). Pure: no I/O. The scrubber (22.5)
//! scans with the same patterns before a copy reaches the lab.

/// The kinds of personal data a text holds, each once, in the order they are checked.
pub fn kinds_in(text: &str) -> Vec<&'static str> {
    let mut kinds = Vec::new();
    if has_email(text) {
        kinds.push("email");
    }
    let numbers = numbers_in(text);
    if numbers.iter().any(|n| is_phone(n)) {
        kinds.push("phone");
    }
    if numbers.iter().any(|n| is_card(n)) {
        kinds.push("card");
    }
    if has_ssn(text) {
        kinds.push("national-id");
    }
    kinds
}

/// Whether a text holds an email address: a local part, an `@`, and a domain with a dot and a letters-only top level.
fn has_email(text: &str) -> bool {
    let bytes = text.as_bytes();
    for (at, byte) in bytes.iter().enumerate() {
        if *byte != b'@' {
            continue;
        }
        let local = bytes[..at]
            .iter()
            .rev()
            .take_while(|b| is_local(**b))
            .count();
        let domain: String = bytes[at + 1..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || **b == b'.' || **b == b'-')
            .map(|b| *b as char)
            .collect();
        let labels: Vec<&str> = domain.split('.').collect();
        if local > 0
            && labels.len() >= 2
            && labels.iter().all(|l| !l.is_empty())
            && labels
                .last()
                .is_some_and(|tld| tld.len() >= 2 && tld.bytes().all(|b| b.is_ascii_alphabetic()))
        {
            return true;
        }
    }
    false
}

fn is_local(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'%' | b'+' | b'-')
}

/// The maximal runs of digits with the separators people write them with (spaces, dashes, dots, parentheses, a plus
/// that starts one), each run ending with a digit.
fn numbers_in(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut run = String::new();
    for c in text.chars() {
        let separator = matches!(c, ' ' | '-' | '.' | '(' | ')' | '+');
        // A plus starts a run too (an international number), and the others continue one.
        if c.is_ascii_digit() || (separator && (!run.is_empty() || c == '+')) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
        }
    }
    flush(&mut run, &mut out);
    out
}

fn flush(run: &mut String, out: &mut Vec<String>) {
    let trimmed = run.trim_end_matches(|c: char| !c.is_ascii_digit());
    if trimmed.chars().any(|c| c.is_ascii_digit()) {
        out.push(trimmed.to_string());
    }
    run.clear();
}

fn digits(number: &str) -> String {
    number.chars().filter(char::is_ascii_digit).collect()
}

/// A phone number: `+` and 8 to 15 digits, or ten digits in the 3-3-4 groups people write (`555-010-7788`,
/// `(555) 010-7788`), or eleven with a leading 1. A date does not read as one.
fn is_phone(number: &str) -> bool {
    let count = digits(number).len();
    let groups: Vec<usize> = number
        .split(|c: char| !c.is_ascii_digit())
        .filter(|g| !g.is_empty())
        .map(str::len)
        .collect();
    (number.starts_with('+') && (8..=15).contains(&count))
        || matches!(groups.as_slice(), [3, 3, 4] | [1, 3, 3, 4])
}

/// A card number: 13 to 19 digits that pass the Luhn check.
fn is_card(number: &str) -> bool {
    let d = digits(number);
    (13..=19).contains(&d.len()) && luhn(&d)
}

/// The Luhn check of a string of digits.
pub fn luhn(digits: &str) -> bool {
    let mut sum = 0u32;
    for (i, c) in digits.chars().rev().enumerate() {
        let Some(mut n) = c.to_digit(10) else {
            return false;
        };
        if i % 2 == 1 {
            n *= 2;
            if n > 9 {
                n -= 9;
            }
        }
        sum += n;
    }
    !digits.is_empty() && sum.is_multiple_of(10)
}

/// Whether the text holds `NNN-NN-NNNN` with the digits bounded by non-digits.
fn has_ssn(text: &str) -> bool {
    let bytes = text.as_bytes();
    let shape = |s: &[u8]| {
        s.len() == 11
            && s[..3].iter().all(u8::is_ascii_digit)
            && s[3] == b'-'
            && s[4..6].iter().all(u8::is_ascii_digit)
            && s[6] == b'-'
            && s[7..].iter().all(u8::is_ascii_digit)
    };
    bytes.windows(11).enumerate().any(|(at, window)| {
        shape(window)
            && (at == 0 || !bytes[at - 1].is_ascii_digit())
            && bytes.get(at + 11).is_none_or(|b| !b.is_ascii_digit())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_email_address_is_found_and_a_handle_is_not() {
        assert_eq!(kinds_in("user ana.k@example.com logged in"), ["email"]);
        assert!(kinds_in("mention @here and an @ alone").is_empty());
    }

    #[test]
    fn a_phone_number_is_found_with_separators_or_a_plus() {
        assert_eq!(kinds_in("call +44 20 7946 0958 now"), ["phone"]);
        assert_eq!(kinds_in("tel 555-010-7788"), ["phone"]);
        assert!(kinds_in("took 2026-10-24 12:30").is_empty());
    }

    #[test]
    fn a_card_number_is_found_only_when_it_passes_luhn() {
        assert!(luhn("4539578763621486"));
        assert_eq!(kinds_in("card 4539 5787 6362 1486 charged"), ["card"]);
        assert!(!luhn("4539578763621487"));
        assert!(kinds_in("order 4539 5787 6362 1487").is_empty());
    }

    #[test]
    fn a_national_id_is_found_in_its_shape_only() {
        assert_eq!(kinds_in("ssn 123-45-6789 on file"), ["national-id"]);
        assert!(kinds_in("ref 2026-10-24-7").is_empty());
    }

    #[test]
    fn plain_logs_hold_no_personal_data() {
        assert!(kinds_in("GET /items 200 in 12 ms, request 4417").is_empty());
    }
}
