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
        let _ = s;
        todo!()
    }

    /// The window to read for the comparison of `[from, to)`: the previous window for `Prev`, a week back for
    /// `Week`, the same window for `Scope`, none for `None`.
    pub fn window(&self, from_ms: u64, to_ms: u64) -> Option<(u64, u64)> {
        let _ = (from_ms, to_ms);
        todo!()
    }

    /// "previous", "last week", the other scope's label (passed in), or "".
    pub fn label(&self, scope_label: &str) -> String {
        let _ = scope_label;
        todo!()
    }
}

/// Change of `now` against `before` in percent; `None` when `before` is 0 or either is not finite.
pub fn change_pct(now: f64, before: f64) -> Option<f64> {
    let _ = (now, before);
    todo!()
}
