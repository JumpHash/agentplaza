//! The IDE's tab, panel and diff-base state, as a pure reducer. The desktop app and
//! the web console run the same transitions, so they behave the same.

use serde::{Deserialize, Serialize};

use crate::workspace::WorkspaceId;

/// The panel on the right (bottom on narrow widths).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Panel {
    #[default]
    Changes,
    Agent,
    Approvals,
    Run,
    Terminal,
}

/// What the Changes panel compares the workspace against.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Base {
    /// An agent checkpoint, by its number.
    Checkpoint(u64),
    /// The checkpoint taken when the current work began. The default.
    #[default]
    WorkStart,
    /// A branch of a repo workspace.
    Branch(String),
}

/// The IDE's state for one screen.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct IdeState {
    pub workspace: Option<WorkspaceId>,
    /// Open editor tabs, in tab order. Paths are workspace-relative.
    pub open: Vec<String>,
    /// The tab shown, always one of `open` (or `None` when no tab is open).
    pub active: Option<String>,
    pub panel: Panel,
    pub base: Base,
    /// Review mode: the centre shows the diff instead of the editor.
    pub review: bool,
    /// The file tree shows only changed files.
    pub filter_changed: bool,
}

/// A user action on the IDE.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Opens a tab (or shows it, if it is already open), and makes it active.
    Open(String),
    /// Closes a tab. Closing the active tab activates its right neighbour, else its left.
    Close(String),
    /// Shows an open tab. A path that is not open does nothing.
    Activate(String),
    SetPanel(Panel),
    SetBase(Base),
    ToggleReview,
    ToggleChangedFilter,
    /// Switches workspace. Clears the tabs, since they belong to the old one.
    SetWorkspace(WorkspaceId),
}

/// The next state after `action`. Pure: the same inputs always give the same state.
pub fn reduce(mut state: IdeState, action: Action) -> IdeState {
    match action {
        Action::Open(path) => {
            if !state.open.contains(&path) {
                state.open.push(path.clone());
            }
            state.active = Some(path);
        }
        Action::Close(path) => {
            if let Some(i) = state.open.iter().position(|p| *p == path) {
                state.open.remove(i);
                if state.active.as_deref() == Some(path.as_str()) {
                    // The tab that moved into slot `i` is the right neighbour; else the one before it.
                    state.active = state
                        .open
                        .get(i)
                        .or_else(|| i.checked_sub(1).and_then(|j| state.open.get(j)))
                        .cloned();
                }
            }
        }
        Action::Activate(path) => {
            if state.open.contains(&path) {
                state.active = Some(path);
            }
        }
        Action::SetPanel(panel) => state.panel = panel,
        Action::SetBase(base) => state.base = base,
        Action::ToggleReview => state.review = !state.review,
        Action::ToggleChangedFilter => state.filter_changed = !state.filter_changed,
        Action::SetWorkspace(id) => {
            state.workspace = Some(id);
            state.open.clear();
            state.active = None;
        }
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(names: &[&str], active: &str) -> IdeState {
        let mut s = IdeState::default();
        for n in names {
            s = reduce(s, Action::Open((*n).to_string()));
        }
        reduce(s, Action::Activate(active.to_string()))
    }

    #[test]
    fn open_adds_a_tab_and_activates_it_or_only_activates_an_open_one() {
        let s = reduce(IdeState::default(), Action::Open("a.txt".into()));
        assert_eq!(s.open, ["a.txt"]);
        assert_eq!(s.active.as_deref(), Some("a.txt"));
        let s = reduce(s, Action::Open("b.txt".into()));
        assert_eq!(s.open, ["a.txt", "b.txt"]);
        assert_eq!(s.active.as_deref(), Some("b.txt"));
        let s = reduce(s, Action::Open("a.txt".into()));
        assert_eq!(s.open, ["a.txt", "b.txt"], "an open tab is not added twice");
        assert_eq!(s.active.as_deref(), Some("a.txt"));
    }

    #[test]
    fn closing_a_tab_picks_the_neighbour_or_keeps_the_active_one() {
        // (case, tabs, active, closed, open after, active after)
        let cases: [(&str, &[&str], &str, &str, &[&str], Option<&str>); 5] = [
            (
                "active tab: right neighbour",
                &["a", "b", "c"],
                "b",
                "b",
                &["a", "c"],
                Some("c"),
            ),
            (
                "last tab: left neighbour",
                &["a", "b", "c"],
                "c",
                "c",
                &["a", "b"],
                Some("b"),
            ),
            ("only tab: no active tab", &["a"], "a", "a", &[], None),
            (
                "inactive tab: active kept",
                &["a", "b", "c"],
                "b",
                "a",
                &["b", "c"],
                Some("b"),
            ),
            (
                "unknown tab: nothing changes",
                &["a", "b"],
                "a",
                "zzz",
                &["a", "b"],
                Some("a"),
            ),
        ];
        for (case, names, active, closed, want_open, want_active) in cases {
            let s = reduce(tabs(names, active), Action::Close(closed.into()));
            assert_eq!(s.open, want_open, "{case}");
            assert_eq!(s.active.as_deref(), want_active, "{case}");
        }
    }

    #[test]
    fn activating_a_tab_that_is_not_open_does_nothing() {
        let before = tabs(&["a"], "a");
        let after = reduce(before.clone(), Action::Activate("zzz".into()));
        assert_eq!(after, before);
        let after = reduce(before, Action::Activate("a".into()));
        assert_eq!(after.active.as_deref(), Some("a"));
    }

    #[test]
    fn switching_workspace_clears_the_tabs() {
        let s = tabs(&["a", "b"], "b");
        let s = reduce(s, Action::SetWorkspace(WorkspaceId::Agent(7)));
        assert_eq!(s.workspace, Some(WorkspaceId::Agent(7)));
        assert!(s.open.is_empty());
        assert_eq!(s.active, None);
    }

    #[test]
    fn state_round_trips_through_json() {
        let s = reduce(
            tabs(&["src/main.rs"], "src/main.rs"),
            Action::SetWorkspace(WorkspaceId::Repo {
                name: "plaza-web".into(),
                git_ref: "main".into(),
            }),
        );
        let s = reduce(s, Action::SetBase(Base::Checkpoint(4)));
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["workspace"], "repo:plaza-web@main");
        assert_eq!(v["base"]["checkpoint"], 4);
        assert_eq!(v["panel"], "changes");
        let back: IdeState = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
    }
}
