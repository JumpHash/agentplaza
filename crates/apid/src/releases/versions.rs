//! Versions (PLAN 22.3): a release's version is the next one after its base, bumped by the changes it holds (a breaking
//! change is a major, a feature a minor, anything else a patch), or numbered by the project's own scheme: calendar
//! versions (`2026.10.2`) or build numbers for the stores. A maintenance release (a release line) takes patches only:
//! a feature or a breaking change ships on main.

/// What a change does to the version, in the order the bumps apply: a patch is the least.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Bump {
    Patch,
    Minor,
    Major,
}

/// One intent of a change as the version reads it: its kind (`feature`, `fix`, ...), its title and its summary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Note {
    pub kind: String,
    pub title: String,
    pub summary: String,
}

/// How a project numbers its releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    /// `MAJOR.MINOR.PATCH`, bumped by the changes (the default).
    Semver,
    /// `YEAR.MONTH.N`: N counts the releases of the month.
    Calendar,
    /// A plain build number, one more than the last (the stores' build numbers).
    Build,
}

impl Scheme {
    pub fn parse(name: Option<&str>) -> Result<Scheme, String> {
        match name.map(str::trim).filter(|n| !n.is_empty()) {
            None | Some("semver") => Ok(Scheme::Semver),
            Some("calendar") => Ok(Scheme::Calendar),
            Some("build") => Ok(Scheme::Build),
            Some(other) => Err(format!(
                "no version scheme {other}: semver, calendar or build"
            )),
        }
    }
}

/// The bump one change asks for: the largest bump of its intents (a change with no intents is read by its title).
pub fn bump_of(notes: &[Note]) -> Bump {
    notes.iter().map(bump_of_note).max().unwrap_or(Bump::Patch)
}

fn bump_of_note(note: &Note) -> Bump {
    let header = conventional(&note.title);
    let breaking = note.kind == "breaking"
        || header.as_ref().is_some_and(|h| h.breaking)
        || note.title.contains("BREAKING CHANGE")
        || note.summary.contains("BREAKING CHANGE");
    if breaking {
        Bump::Major
    } else if note.kind == "feature" || header.is_some_and(|h| h.kind == "feat") {
        Bump::Minor
    } else {
        Bump::Patch
    }
}

/// A conventional-commit header, `type(scope)!: title`.
struct Header {
    kind: String,
    breaking: bool,
}

fn conventional(title: &str) -> Option<Header> {
    let (left, _) = title.split_once(':')?;
    let breaking = left.ends_with('!');
    let left = left.trim_end_matches('!');
    let kind = left.split('(').next()?.trim();
    if kind.is_empty() || !kind.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some(Header {
        kind: kind.to_ascii_lowercase(),
        breaking,
    })
}

/// The next version after `base` (`0.0.0` when there is none), bumped by `bump`. A `v` prefix is kept.
pub fn semver_next(base: &str, bump: Bump) -> Result<String, String> {
    let prefix = if base.starts_with('v') { "v" } else { "" };
    let core = base
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()
        .unwrap_or("");
    let parts: Vec<u64> = core
        .split('.')
        .map(|p| p.parse::<u64>())
        .collect::<Result<_, _>>()
        .map_err(|_| format!("{base} is not a semantic version"))?;
    let &[major, minor, patch] = parts.as_slice() else {
        return Err(format!(
            "{base} is not a semantic version (MAJOR.MINOR.PATCH)"
        ));
    };
    let (major, minor, patch) = match bump {
        Bump::Major => (major + 1, 0, 0),
        Bump::Minor => (major, minor + 1, 0),
        Bump::Patch => (major, minor, patch + 1),
    };
    Ok(format!("{prefix}{major}.{minor}.{patch}"))
}

/// The calendar version of a release in `year`.`month`: the month's next count after `base`.
pub fn calendar_next(base: Option<&str>, year: u32, month: u32) -> String {
    let stem = format!("{year}.{month}.");
    let count = base
        .and_then(|b| b.strip_prefix(&stem))
        .and_then(|rest| rest.split(['-', '+']).next())
        .and_then(|n| n.parse::<u32>().ok())
        .unwrap_or(0);
    format!("{stem}{}", count + 1)
}

/// The build number after `base` (1 for the first).
pub fn build_next(base: Option<&str>) -> Result<String, String> {
    match base {
        None => Ok("1".into()),
        Some(b) => b
            .trim()
            .parse::<u64>()
            .map(|n| (n + 1).to_string())
            .map_err(|_| format!("{b} is not a build number")),
    }
}

/// The version of a release after `base`, from the bumps of its changes, in `scheme`. `maintenance` is a release line:
/// it takes patch releases only. `now` is the year and month the release is cut in (for calendar versions).
pub fn next(
    scheme: Scheme,
    base: Option<&str>,
    bumps: &[Bump],
    maintenance: bool,
    (year, month): (u32, u32),
) -> Result<String, String> {
    let bump = bumps.iter().copied().max().unwrap_or(Bump::Patch);
    match scheme {
        Scheme::Semver => {
            if maintenance && bump > Bump::Patch {
                return Err(
                    "a maintenance release takes patches only: a feature or a breaking change ships on main"
                        .into(),
                );
            }
            semver_next(base.unwrap_or("0.0.0"), bump)
        }
        Scheme::Calendar => Ok(calendar_next(base, year, month)),
        Scheme::Build => build_next(base),
    }
}

/// The year and month (UTC) of a time in milliseconds since the epoch.
pub fn year_month(now_ms: u64) -> (u32, u32) {
    let z = (now_ms / 86_400_000) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year as u32, month as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(kind: &str, title: &str, summary: &str) -> Note {
        Note {
            kind: kind.into(),
            title: title.into(),
            summary: summary.into(),
        }
    }

    #[test]
    fn a_change_bumps_by_what_its_intents_say() {
        assert_eq!(bump_of(&[note("fix", "Stop the crash", "")]), Bump::Patch);
        assert_eq!(bump_of(&[note("feature", "Add export", "")]), Bump::Minor);
        assert_eq!(
            bump_of(&[
                note("docs", "Document it", ""),
                note("feature", "Add it", "")
            ]),
            Bump::Minor,
            "the largest of a change's intents"
        );
        assert_eq!(bump_of(&[note("breaking", "Drop v1", "")]), Bump::Major);
        // Conventional markers, when the kind is not given.
        assert_eq!(bump_of(&[note("", "feat: add export", "")]), Bump::Minor);
        assert_eq!(bump_of(&[note("", "feat(api)!: drop v1", "")]), Bump::Major);
        assert_eq!(bump_of(&[note("", "fix: a crash", "")]), Bump::Patch);
        assert_eq!(
            bump_of(&[note(
                "fix",
                "Rename",
                "BREAKING CHANGE: the old flag is gone"
            )]),
            Bump::Major
        );
        assert_eq!(bump_of(&[]), Bump::Patch, "no intents read as a patch");
    }

    #[test]
    fn semver_moves_by_the_bump_and_keeps_a_v_prefix() {
        assert_eq!(semver_next("1.4.0", Bump::Patch).unwrap(), "1.4.1");
        assert_eq!(semver_next("1.4.3", Bump::Minor).unwrap(), "1.5.0");
        assert_eq!(semver_next("1.4.3", Bump::Major).unwrap(), "2.0.0");
        assert_eq!(semver_next("v0.3.0", Bump::Minor).unwrap(), "v0.4.0");
        assert_eq!(semver_next("1.4.0-rc.2", Bump::Patch).unwrap(), "1.4.1");
        assert_eq!(semver_next("0.0.0", Bump::Minor).unwrap(), "0.1.0");
        assert!(
            semver_next("1.4", Bump::Patch)
                .unwrap_err()
                .contains("semantic version")
        );
    }

    #[test]
    fn a_maintenance_release_takes_patches_only() {
        let fix = next(
            Scheme::Semver,
            Some("1.4.0"),
            &[Bump::Patch],
            true,
            (2026, 10),
        );
        assert_eq!(fix.unwrap(), "1.4.1");
        let feature = next(
            Scheme::Semver,
            Some("1.4.0"),
            &[Bump::Minor],
            true,
            (2026, 10),
        );
        assert!(feature.unwrap_err().contains("patches only"));
        // On main a feature is a minor, and the largest bump of the release wins.
        let main = next(
            Scheme::Semver,
            Some("1.4.1"),
            &[Bump::Patch, Bump::Minor],
            false,
            (2026, 10),
        );
        assert_eq!(main.unwrap(), "1.5.0");
    }

    #[test]
    fn calendar_versions_count_the_releases_of_the_month() {
        assert_eq!(calendar_next(None, 2026, 10), "2026.10.1");
        assert_eq!(calendar_next(Some("2026.10.1"), 2026, 10), "2026.10.2");
        assert_eq!(calendar_next(Some("2026.9.4"), 2026, 10), "2026.10.1");
        assert_eq!(
            calendar_next(Some("2026.10.9-rc.1"), 2026, 10),
            "2026.10.10"
        );
        assert_eq!(
            next(
                Scheme::Calendar,
                Some("2026.10.2"),
                &[Bump::Minor],
                false,
                (2026, 10)
            )
            .unwrap(),
            "2026.10.3"
        );
    }

    #[test]
    fn build_numbers_count_up() {
        assert_eq!(build_next(None).unwrap(), "1");
        assert_eq!(build_next(Some("41")).unwrap(), "42");
        assert!(
            build_next(Some("1.4"))
                .unwrap_err()
                .contains("build number")
        );
        assert_eq!(
            next(Scheme::Build, Some("41"), &[], false, (2026, 10)).unwrap(),
            "42"
        );
    }

    #[test]
    fn schemes_parse_and_reject_unknown_names() {
        assert_eq!(Scheme::parse(None).unwrap(), Scheme::Semver);
        assert_eq!(Scheme::parse(Some(" ")).unwrap(), Scheme::Semver);
        assert_eq!(Scheme::parse(Some("calendar")).unwrap(), Scheme::Calendar);
        assert_eq!(Scheme::parse(Some("build")).unwrap(), Scheme::Build);
        assert!(
            Scheme::parse(Some("hex"))
                .unwrap_err()
                .contains("no version scheme")
        );
    }

    #[test]
    fn the_month_of_a_time_is_read_in_utc() {
        // 2026-10-23T00:00:00Z, and one millisecond before 2026-01-01 (the year before).
        assert_eq!(year_month(1_792_473_600_000), (2026, 10));
        assert_eq!(year_month(1_767_225_600_000 - 1), (2025, 12));
        assert_eq!(year_month(1_767_225_600_000), (2026, 1));
    }
}
