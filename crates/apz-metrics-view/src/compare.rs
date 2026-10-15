//! What a view is compared with, and the change percentages tiles show.

/// The comparison of a view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Compare {
    /// The window just before.
    Prev,
    /// The same window a week earlier.
    Week,
    /// The same window of another scope.
    Scope(String),
    None,
}

impl Compare {
    /// Parses `prev`, `week`, `none`, `scope:<id>` (anything else is `Prev`).
    pub fn parse(s: &str) -> Compare {
        match s {
            "week" => Compare::Week,
            "none" => Compare::None,
            _ => match s.strip_prefix("scope:") {
                Some(id) if !id.is_empty() => Compare::Scope(id.to_string()),
                _ => Compare::Prev,
            },
        }
    }

    /// The window to read for the comparison of `[from, to)`: the previous window for `Prev`, a week back for
    /// `Week`, the same window for `Scope`, none for `None`.
    pub fn window(&self, from_ms: u64, to_ms: u64) -> Option<(u64, u64)> {
        const WEEK_MS: u64 = 7 * 24 * 60 * 60 * 1000;
        match self {
            Compare::Prev => {
                let width = to_ms.saturating_sub(from_ms);
                Some((from_ms.saturating_sub(width), from_ms))
            }
            Compare::Week => Some((
                from_ms.saturating_sub(WEEK_MS),
                to_ms.saturating_sub(WEEK_MS),
            )),
            Compare::Scope(_) => Some((from_ms, to_ms)),
            Compare::None => None,
        }
    }

    /// "previous", "last week", the other scope's label (passed in), or "".
    pub fn label(&self, scope_label: &str) -> String {
        match self {
            Compare::Prev => "previous".to_string(),
            Compare::Week => "last week".to_string(),
            Compare::Scope(_) => scope_label.to_string(),
            Compare::None => String::new(),
        }
    }
}

/// Change of `now` against `before` in percent; `None` when `before` is 0 or either is not finite.
pub fn change_pct(now: f64, before: f64) -> Option<f64> {
    if before == 0.0 || !now.is_finite() || !before.is_finite() {
        return None;
    }
    Some((now - before) / before.abs() * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEEK_MS: u64 = 7 * 24 * 60 * 60 * 1000;

    #[test]
    fn the_comparison_names_parse_and_pick_the_window_to_read() {
        assert_eq!(Compare::parse("week"), Compare::Week);
        assert_eq!(Compare::parse("none"), Compare::None);
        assert_eq!(Compare::parse("scope:eu"), Compare::Scope("eu".to_string()));
        assert_eq!(Compare::parse("scope:"), Compare::Prev);
        assert_eq!(Compare::parse("prev"), Compare::Prev);
        assert_eq!(Compare::parse("bogus"), Compare::Prev);

        assert_eq!(Compare::Prev.window(20_000, 30_000), Some((10_000, 20_000)));
        assert_eq!(
            Compare::Week.window(WEEK_MS + 1000, WEEK_MS + 2000),
            Some((1000, 2000))
        );
        assert_eq!(Compare::Week.window(0, 100), Some((0, 0)));
        assert_eq!(Compare::Scope("eu".into()).window(5, 9), Some((5, 9)));
        assert_eq!(Compare::None.window(5, 9), None);

        assert_eq!(Compare::Prev.label(""), "previous");
        assert_eq!(Compare::Week.label(""), "last week");
        assert_eq!(Compare::Scope("eu".into()).label("EU"), "EU");
        assert_eq!(Compare::None.label(""), "");
    }

    #[test]
    fn change_pct_is_relative_to_the_earlier_value_and_absent_when_it_cannot_be_computed() {
        let close = |got: Option<f64>, want: f64| {
            let got = got.expect("a change");
            assert!((got - want).abs() < 1e-9, "got {got}, want {want}");
        };
        close(change_pct(120.0, 100.0), 20.0);
        close(change_pct(80.0, 100.0), -20.0);
        close(change_pct(-50.0, -100.0), 50.0);

        assert_eq!(change_pct(5.0, 0.0), None);
        assert_eq!(change_pct(f64::NAN, 1.0), None);
        assert_eq!(change_pct(1.0, f64::INFINITY), None);
    }
}
