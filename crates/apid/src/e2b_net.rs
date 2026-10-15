//! An E2B sandbox's network rules (`network.allowOut`/`denyOut` at create, `PUT
//! /e2b/sandboxes/{id}/network` later), checked and matched against the host a connection names.
//!
//! The egress proxy sees the host of each connection as the guest asked for it (a name or an IP
//! literal), so: `allowOut` entries are a name (`example.com`), a wildcard (`*.example.com`: any
//! subdomain, not the apex), an IP or a CIDR block; `denyOut` entries are an IP or a CIDR block (E2B
//! takes no names there), and `0.0.0.0/0` (or `::/0`) denies every host, named or not. An allowed
//! entry wins over a denied one. Neither overrides the laws or the hub's blocklist of private
//! ranges: they only narrow or open what the sandbox's internet access would allow.

use std::net::IpAddr;

/// The most entries in each list.
pub const MAX_ENTRIES: usize = 256;

/// One checked entry.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Entry {
    Name(String),
    Wildcard(String),
    Block { addr: IpAddr, prefix: u8 },
}

fn parse_block(text: &str) -> Option<Entry> {
    let (addr, prefix) = match text.split_once('/') {
        Some((addr, prefix)) => (addr, Some(prefix.parse::<u8>().ok()?)),
        None => (text, None),
    };
    let addr: IpAddr = addr.parse().ok()?;
    let max = if addr.is_ipv4() { 32 } else { 128 };
    let prefix = prefix.unwrap_or(max);
    (prefix <= max).then_some(Entry::Block { addr, prefix })
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

fn parse(text: &str, names: bool) -> Result<Entry, String> {
    let text = text.trim().to_ascii_lowercase();
    if let Some(entry) = parse_block(&text) {
        return Ok(entry);
    }
    if !names {
        return Err(format!(
            "denyOut takes IP addresses and CIDR blocks, not {text:?}"
        ));
    }
    match text.strip_prefix("*.") {
        Some(suffix) if valid_name(suffix) => Ok(Entry::Wildcard(suffix.to_string())),
        None if valid_name(&text) => Ok(Entry::Name(text)),
        _ => Err(format!(
            "{text:?} is not a host name, a *.wildcard, an IP or a CIDR block"
        )),
    }
}

/// Checks a list as the API takes it (`names`: whether host names are allowed), returning it
/// normalised (trimmed, lower case).
pub fn check(list: &[String], what: &str, names: bool) -> Result<Vec<String>, String> {
    if list.len() > MAX_ENTRIES {
        return Err(format!("{what} has more than {MAX_ENTRIES} entries"));
    }
    list.iter()
        .map(|entry| {
            parse(entry, names)
                .map(|_| entry.trim().to_ascii_lowercase())
                .map_err(|why| format!("{what}: {why}"))
        })
        .collect()
}

fn in_block(addr: IpAddr, block: IpAddr, prefix: u8) -> bool {
    match (addr, block) {
        (IpAddr::V4(a), IpAddr::V4(b)) => {
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - prefix)
            };
            u32::from(a) & mask == u32::from(b) & mask
        }
        (IpAddr::V6(a), IpAddr::V6(b)) => {
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - prefix)
            };
            u128::from(a) & mask == u128::from(b) & mask
        }
        _ => false,
    }
}

fn matches(entry: &Entry, host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let host = host.trim_start_matches('[').trim_end_matches(']');
    match entry {
        Entry::Name(name) => host == name,
        Entry::Wildcard(suffix) => host
            .strip_suffix(suffix.as_str())
            .is_some_and(|rest| rest.ends_with('.') && rest.len() > 1),
        // A block of every address denies (or allows) every host, a name included: the name
        // resolves to some address in it.
        Entry::Block { prefix: 0, .. } => true,
        Entry::Block { addr, prefix } => host
            .parse::<IpAddr>()
            .is_ok_and(|ip| in_block(ip, *addr, *prefix)),
    }
}

/// What a sandbox's rules say about a connection to `host`: `Some(true)` allowed by `allow_out`,
/// `Some(false)` denied by `deny_out`, `None` when neither names it (its internet access decides).
pub fn verdict(allow_out: &[String], deny_out: &[String], host: &str) -> Option<bool> {
    let hit = |list: &[String], names: bool| {
        list.iter()
            .filter_map(|text| parse(text, names).ok())
            .any(|entry| matches(&entry, host))
    };
    if hit(allow_out, true) {
        Some(true)
    } else if hit(deny_out, false) {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn lists_are_checked() {
        assert_eq!(
            check(
                &list(&[" Example.COM ", "*.api.dev", "8.8.8.8", "10.0.0.0/8", "::1"]),
                "allowOut",
                true
            ),
            Ok(list(&[
                "example.com",
                "*.api.dev",
                "8.8.8.8",
                "10.0.0.0/8",
                "::1"
            ]))
        );
        assert!(
            check(&list(&["example.com"]), "denyOut", false).is_err(),
            "no names in denyOut"
        );
        assert!(check(&list(&["*"]), "allowOut", true).is_err());
        assert!(check(&list(&["1.2.3.4/33"]), "denyOut", false).is_err());
        assert!(check(&list(&["bad host!"]), "allowOut", true).is_err());
    }

    #[test]
    fn allow_wins_over_deny_and_wildcards_skip_the_apex() {
        let allow = list(&["*.example.com", "pypi.org", "1.1.1.0/24"]);
        let deny = list(&["0.0.0.0/0"]);
        assert_eq!(verdict(&allow, &deny, "files.example.com"), Some(true));
        assert_eq!(verdict(&allow, &deny, "a.b.example.com"), Some(true));
        assert_eq!(
            verdict(&allow, &deny, "example.com"),
            Some(false),
            "the apex is not a subdomain"
        );
        assert_eq!(verdict(&allow, &deny, "PyPI.org."), Some(true));
        assert_eq!(verdict(&allow, &deny, "1.1.1.1"), Some(true));
        assert_eq!(
            verdict(&allow, &deny, "github.com"),
            Some(false),
            "0.0.0.0/0 denies names too"
        );
        assert_eq!(verdict(&[], &list(&["8.8.8.0/24"]), "8.8.8.8"), Some(false));
        assert_eq!(verdict(&[], &list(&["8.8.8.0/24"]), "8.8.4.4"), None);
        assert_eq!(
            verdict(&[], &list(&["2001:db8::/32"]), "[2001:db8::1]"),
            Some(false)
        );
        assert_eq!(verdict(&[], &[], "anything"), None);
    }
}
