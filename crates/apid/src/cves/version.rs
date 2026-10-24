//! Versions as each ecosystem orders them (PLAN 24.3): semver for Cargo, npm and Go modules (a leading `v` is dropped,
//! build metadata is ignored, a pre-release sorts before its release), and PEP 440 for PyPI (epochs, pre-, post- and
//! development releases). Only what the advisory feeds write is read: a text that is not a version orders by its digits.

use std::cmp::Ordering;

use crate::upgrades::ecosystem::Ecosystem;

/// Orders two versions of an ecosystem.
pub fn compare(eco: Ecosystem, a: &str, b: &str) -> Ordering {
    match eco {
        Ecosystem::Pip => pep440(a).cmp(&pep440(b)),
        _ => semver(a).cmp(&semver(b)),
    }
}

/// One pre-release identifier: numbers sort before text, and numbers by value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Ident {
    Num(u64),
    Text(String),
}

/// A semver key: the numeric core (padded with zeros when compared) and the pre-release identifiers, if any.
#[derive(Debug)]
struct Semver {
    core: Vec<u64>,
    pre: Option<Vec<Ident>>,
}

fn number(text: &str) -> u64 {
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().unwrap_or(0)
}

fn semver(text: &str) -> Semver {
    let text = text.trim().trim_start_matches(['v', 'V']);
    let text = text.split('+').next().unwrap_or("");
    let (core, pre) = match text.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (text, None),
    };
    let core = core.split('.').map(number).collect();
    let pre = pre.map(|pre| {
        pre.split('.')
            .map(|id| match id.parse::<u64>() {
                Ok(n) => Ident::Num(n),
                Err(_) => Ident::Text(id.to_string()),
            })
            .collect()
    });
    Semver { core, pre }
}

impl Ord for Semver {
    fn cmp(&self, other: &Self) -> Ordering {
        let len = self.core.len().max(other.core.len());
        let pad = |core: &[u64], i: usize| core.get(i).copied().unwrap_or(0);
        for i in 0..len {
            match pad(&self.core, i).cmp(&pad(&other.core, i)) {
                Ordering::Equal => {}
                other => return other,
            }
        }
        match (&self.pre, &other.pre) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(a), Some(b)) => a.cmp(b),
        }
    }
}

impl PartialEq for Semver {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Semver {}

impl PartialOrd for Semver {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A PEP 440 key, ordered as the standard orders releases: epoch, release (trailing zeros ignored), then the pre-release
/// (a development release sorts before any pre-release, a final release after them), the post-release and the
/// development release.
type Pep = (u64, Vec<u64>, (i32, u64), i64, i64);

fn pep440(text: &str) -> Pep {
    let text = text
        .trim()
        .trim_start_matches(['v', 'V'])
        .to_ascii_lowercase();
    let (epoch, rest) = match text.split_once('!') {
        Some((epoch, rest)) => (number(epoch), rest.to_string()),
        None => (0, text.clone()),
    };
    let split = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(rest.len());
    let mut release: Vec<u64> = rest[..split]
        .split('.')
        .filter(|p| !p.is_empty())
        .map(number)
        .collect();
    while release.len() > 1 && release.last() == Some(&0) {
        release.pop();
    }
    let mut pre = None;
    let mut post = None;
    let mut dev = None;
    let tail: Vec<char> = rest[split..].chars().collect();
    let mut i = 0;
    while i < tail.len() {
        if !tail[i].is_ascii_alphanumeric() {
            i += 1;
            continue;
        }
        let start = i;
        while i < tail.len() && tail[i].is_ascii_alphabetic() {
            i += 1;
        }
        let word: String = tail[start..i].iter().collect();
        let digits_start = i;
        while i < tail.len() && tail[i].is_ascii_digit() {
            i += 1;
        }
        let n: u64 = tail[digits_start..i]
            .iter()
            .collect::<String>()
            .parse()
            .unwrap_or(0);
        match word.as_str() {
            "a" | "alpha" => pre = Some((0, n)),
            "b" | "beta" => pre = Some((1, n)),
            "c" | "rc" | "pre" | "preview" => pre = Some((2, n)),
            "post" | "rev" | "r" => post = Some(n),
            "dev" => dev = Some(n),
            // A bare number after the release is an implicit post-release (`1.0-1`).
            "" if post.is_none() && pre.is_none() && digits_start > start => post = Some(n),
            _ => {}
        }
    }
    let pre_key = match pre {
        Some((rank, n)) => (rank, n),
        None if post.is_none() && dev.is_some() => (-1, 0),
        None => (3, 0),
    };
    (
        epoch,
        release,
        (pre_key.0, pre_key.1),
        post.map_or(-1, |n| n as i64),
        dev.map_or(i64::MAX, |n| n as i64),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_orders_cores_and_pre_releases() {
        let eco = Ecosystem::Cargo;
        assert_eq!(compare(eco, "0.3.9", "0.3.10"), Ordering::Less);
        assert_eq!(compare(eco, "1.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare(eco, "2.0.0-rc.1", "2.0.0"), Ordering::Less);
        assert_eq!(compare(eco, "2.0.0-rc.2", "2.0.0-rc.10"), Ordering::Less);
        assert_eq!(compare(Ecosystem::Go, "v1.2.3", "1.2.3"), Ordering::Equal);
        assert_eq!(
            compare(Ecosystem::Npm, "4.17.21+build", "4.17.21"),
            Ordering::Equal
        );
    }

    #[test]
    fn pep440_orders_releases_pre_and_post() {
        let eco = Ecosystem::Pip;
        assert_eq!(compare(eco, "2.0.dev1", "2.0a1"), Ordering::Less);
        assert_eq!(compare(eco, "2.0a1", "2.0b2"), Ordering::Less);
        assert_eq!(compare(eco, "2.0rc1", "2.0"), Ordering::Less);
        assert_eq!(compare(eco, "2.0", "2.0.post1"), Ordering::Less);
        assert_eq!(compare(eco, "1.10", "1.9"), Ordering::Greater);
        assert_eq!(compare(eco, "1!0.1", "9.9"), Ordering::Greater);
        assert_eq!(compare(eco, "3.0", "3.0.0"), Ordering::Equal);
    }
}
