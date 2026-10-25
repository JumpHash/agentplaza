//! Versions and what an update is (PLAN 24.2): a release's three numbers, and the bump from one release to another. A
//! change of the first non-zero number is a major bump; for `0.x` a change of the minor number is breaking (Cargo's and
//! npm's reading of a caret requirement), so it is major too. Pre-releases are not candidates for an update.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    /// A release's version. `None` for a pre-release (`1.0.0-rc.1`), a build of something else, or text that is not
    /// three numbers. A missing minor or patch number counts as zero (`1.2` is `1.2.0`).
    pub fn parse(text: &str) -> Option<Version> {
        let core = text.trim().trim_start_matches('v');
        let core = core.split('+').next()?;
        if core.contains('-') {
            return None;
        }
        let mut parts = core.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = match parts.next() {
            Some(n) => n.parse().ok()?,
            None => 0,
        };
        let patch = match parts.next() {
            Some(n) => n.parse().ok()?,
            None => 0,
        };
        if parts.next().is_some() {
            return None;
        }
        Some(Version {
            major,
            minor,
            patch,
        })
    }

    /// The compatibility class a release belongs to: releases of one class may replace each other in a requirement
    /// (`1.x`, or `0.2.x`); a release of another class is a major update.
    pub fn compat(&self) -> (u64, u64) {
        if self.major > 0 {
            (self.major, 0)
        } else {
            (0, self.minor)
        }
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// What an update moves: a patch, a minor, or a major (breaking) change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bump {
    Patch,
    Minor,
    Major,
}

impl Bump {
    /// The bump from `from` to `to` (`to` newer than `from`).
    pub fn between(from: &Version, to: &Version) -> Bump {
        if from.compat() != to.compat() {
            Bump::Major
        } else if from.minor != to.minor {
            Bump::Minor
        } else {
            Bump::Patch
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_pre_releases_are_not_candidates() {
        assert_eq!(
            Version::parse("1.2.3"),
            Some(Version {
                major: 1,
                minor: 2,
                patch: 3
            })
        );
        assert_eq!(Version::parse("v2.0").unwrap().to_string(), "2.0.0");
        assert_eq!(Version::parse("1.0.0-rc.1"), None);
        assert_eq!(
            Version::parse("1.2.3+build.7").unwrap().to_string(),
            "1.2.3"
        );
        assert_eq!(Version::parse("1.2.3.4"), None);
        assert_eq!(Version::parse("latest"), None);
    }

    #[test]
    fn a_bump_is_patch_minor_or_major_and_zero_point_minors_are_breaking() {
        let v = |s: &str| Version::parse(s).unwrap();
        assert_eq!(Bump::between(&v("1.2.3"), &v("1.2.9")), Bump::Patch);
        assert_eq!(Bump::between(&v("1.2.3"), &v("1.3.0")), Bump::Minor);
        assert_eq!(Bump::between(&v("1.9.0"), &v("2.0.0")), Bump::Major);
        assert_eq!(Bump::between(&v("0.2.1"), &v("0.3.0")), Bump::Major);
        assert_eq!(Bump::between(&v("0.2.1"), &v("0.2.4")), Bump::Patch);
    }
}
