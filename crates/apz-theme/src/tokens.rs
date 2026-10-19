//! The token list: every colour both consoles read, written once. The web reads them as CSS custom properties
//! (`--paper`, `--ink-3` …, generated into `apps/web/src/styles/themes.gen.css`); the desktop reads them through
//! `theme::palette()`, whose fields are these names with `_` for `-`. A test on each side fails when it uses a
//! name this list lacks, and one here fails when the list has a name neither side reads.

/// How a token is used, which decides what it must contrast with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Ground,
    Lines,
    Text,
    Accent,
    Action,
    States,
    Diff,
    Code,
    Terminal,
    Data,
    Overlay,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Ground => "Ground",
            Group::Lines => "Lines",
            Group::Text => "Text",
            Group::Accent => "Accent",
            Group::Action => "Action",
            Group::States => "States",
            Group::Diff => "Diff",
            Group::Code => "Code",
            Group::Terminal => "Terminal",
            Group::Data => "Data",
            Group::Overlay => "Overlay",
        }
    }
}

/// One colour token.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct Token {
    pub name: &'static str,
    pub group: Group,
    /// One line for the theme editor.
    pub about: &'static str,
}

const fn t(name: &'static str, group: Group, about: &'static str) -> Token {
    Token { name, group, about }
}

use Group::*;

/// Every colour token, in the editor's order.
pub const TOKENS: &[Token] = &[
    t("paper", Ground, "The page behind everything"),
    t("paper-side", Ground, "The sidebar and side panels"),
    t(
        "panel",
        Ground,
        "The main column's ground under the ambient layer",
    ),
    t(
        "sheet",
        Ground,
        "Cards, panels and dialogs laid on the page",
    ),
    t(
        "raised",
        Ground,
        "Raised surfaces: popovers, the composer, inputs",
    ),
    t(
        "sheet-2",
        Ground,
        "A recessed area: code blocks, wells, table heads",
    ),
    t("hover", Ground, "A row under the pointer"),
    t("press", Ground, "A pressed or selected row"),
    t("pattern", Ground, "The background pattern's dots and lines"),
    t("amb-1", Ground, "The background's first colour field"),
    t("amb-2", Ground, "The background's second colour field"),
    t("amb-3", Ground, "The background's third colour field"),
    t("rule", Lines, "Hairlines between rows and around cards"),
    t(
        "rule-strong",
        Lines,
        "Stronger borders: inputs, focused cards",
    ),
    t("rule-faint", Lines, "The faintest separators"),
    t("ink", Text, "Body text and titles"),
    t("ink-2", Text, "Secondary text"),
    t("ink-3", Text, "Meta text: labels, times, hints"),
    t(
        "signal",
        Accent,
        "You: the owner's colour, for dots and marks",
    ),
    t(
        "signal-fill",
        Accent,
        "Filled controls in the owner's colour",
    ),
    t("signal-ink", Accent, "Text and links in the owner's colour"),
    t("on-signal", Accent, "Text on a signal fill"),
    t(
        "signal-soft",
        Accent,
        "A soft signal tint behind chips and selections",
    ),
    t("action", Action, "Primary buttons"),
    t("on-action", Action, "Text on a primary button"),
    t("ok", States, "Finished well"),
    t("ok-soft", States, "A soft ok tint"),
    t("warn", States, "Needs watching"),
    t("warn-soft", States, "A soft warn tint"),
    t("crit", States, "Failed, refused or killed"),
    t("crit-soft", States, "A soft crit tint"),
    t("info", States, "Working, a passive note"),
    t("info-soft", States, "A soft info tint"),
    t("review", States, "In review"),
    t("review-soft", States, "A soft review tint"),
    t("add-bg", Diff, "An added line's ground"),
    t("add-fg", Diff, "An added line's text"),
    t("add-bar", Diff, "The added share of a change bar"),
    t("del-bg", Diff, "A removed line's ground"),
    t("del-fg", Diff, "A removed line's text"),
    t("del-bar", Diff, "The removed share of a change bar"),
    t("syn-keyword", Code, "Keywords"),
    t("syn-string", Code, "Strings"),
    t("syn-type", Code, "Types"),
    t("syn-number", Code, "Numbers and constants"),
    t("syn-comment", Code, "Comments"),
    t("syn-function", Code, "Function names"),
    t("syn-variable", Code, "Variables"),
    t("term-bg", Terminal, "The terminal's ground"),
    t("term-fg", Terminal, "The terminal's text"),
    t("ansi-0", Terminal, "Black"),
    t("ansi-1", Terminal, "Red"),
    t("ansi-2", Terminal, "Green"),
    t("ansi-3", Terminal, "Yellow"),
    t("ansi-4", Terminal, "Blue"),
    t("ansi-5", Terminal, "Magenta"),
    t("ansi-6", Terminal, "Cyan"),
    t("ansi-7", Terminal, "White"),
    t("ansi-8", Terminal, "Bright black"),
    t("ansi-9", Terminal, "Bright red"),
    t("ansi-10", Terminal, "Bright green"),
    t("ansi-11", Terminal, "Bright yellow"),
    t("ansi-12", Terminal, "Bright blue"),
    t("ansi-13", Terminal, "Bright magenta"),
    t("ansi-14", Terminal, "Bright cyan"),
    t("ansi-15", Terminal, "Bright white"),
    t("chart-1", Data, "Series 1"),
    t("chart-2", Data, "Series 2"),
    t("chart-3", Data, "Series 3"),
    t("chart-4", Data, "Series 4"),
    t("chart-5", Data, "Series 5"),
    t("chart-6", Data, "Series 6"),
    t("chart-7", Data, "Series 7"),
    t("chart-8", Data, "Series 8"),
    t("chart-other", Data, "The rest of a breakdown"),
    t("chart-grid", Data, "Chart grid lines"),
    t("chart-axis", Data, "Chart axis labels"),
    t("hue-0", Data, "Project and agent hue 0"),
    t("hue-1", Data, "Project and agent hue 1"),
    t("hue-2", Data, "Project and agent hue 2"),
    t("hue-3", Data, "Project and agent hue 3"),
    t("hue-4", Data, "Project and agent hue 4"),
    t("hue-5", Data, "Project and agent hue 5"),
    t("on-hue", Data, "Text on a hue"),
    t("toast-bg", Overlay, "A toast's ground"),
    t("toast-fg", Overlay, "A toast's text"),
    t("scrim", Overlay, "The veil behind a dialog"),
    t("amb-lit", Overlay, "The pointer light's weave lines"),
    t("amb-line", Overlay, "Faint background ornament lines"),
];

/// The token with this name.
pub fn token(name: &str) -> Option<&'static Token> {
    TOKENS.iter().find(|t| t.name == name)
}

/// Whether `name` is a colour token.
pub fn is_token(name: &str) -> bool {
    token(name).is_some()
}

/// A contrast rule: `fg` must reach `min` against every ground in `on`.
#[derive(Clone, Copy, Debug)]
pub struct Check {
    pub fg: &'static str,
    pub on: &'static [&'static str],
    pub min: f32,
}

/// Text needs 4.5:1, marks 3:1.
pub const TEXT: f32 = 4.5;
pub const MARK: f32 = 3.0;

const GROUNDS: &[&str] = &["paper", "paper-side", "sheet", "sheet-2", "hover"];

/// The contrast rules, in the order the pass applies them. A token adjusted by an earlier rule is checked again by
/// later rules that name it.
pub const CHECKS: &[Check] = &[
    Check {
        fg: "ink",
        on: &["paper", "paper-side", "sheet", "sheet-2", "hover", "press"],
        min: TEXT,
    },
    Check {
        fg: "ink-2",
        on: GROUNDS,
        min: TEXT,
    },
    Check {
        fg: "ink-3",
        on: GROUNDS,
        min: TEXT,
    },
    Check {
        fg: "signal-ink",
        on: &["paper", "sheet", "hover", "signal-soft"],
        min: TEXT,
    },
    Check {
        fg: "signal",
        on: &["paper", "sheet"],
        min: MARK,
    },
    Check {
        fg: "on-signal",
        on: &["signal-fill"],
        min: TEXT,
    },
    Check {
        fg: "on-action",
        on: &["action"],
        min: TEXT,
    },
    Check {
        fg: "ok",
        on: &["paper", "sheet", "ok-soft"],
        min: TEXT,
    },
    Check {
        fg: "warn",
        on: &["paper", "sheet", "warn-soft"],
        min: TEXT,
    },
    Check {
        fg: "crit",
        on: &["paper", "sheet", "crit-soft"],
        min: TEXT,
    },
    Check {
        fg: "info",
        on: &["paper", "sheet", "info-soft"],
        min: TEXT,
    },
    Check {
        fg: "review",
        on: &["paper", "sheet", "review-soft"],
        min: TEXT,
    },
    Check {
        fg: "add-fg",
        on: &["add-bg"],
        min: TEXT,
    },
    Check {
        fg: "del-fg",
        on: &["del-bg"],
        min: TEXT,
    },
    Check {
        fg: "syn-keyword",
        on: &["sheet", "sheet-2"],
        min: TEXT,
    },
    Check {
        fg: "syn-string",
        on: &["sheet", "sheet-2"],
        min: TEXT,
    },
    Check {
        fg: "syn-type",
        on: &["sheet", "sheet-2"],
        min: TEXT,
    },
    Check {
        fg: "syn-number",
        on: &["sheet", "sheet-2"],
        min: TEXT,
    },
    Check {
        fg: "syn-comment",
        on: &["sheet", "sheet-2"],
        min: TEXT,
    },
    Check {
        fg: "syn-function",
        on: &["sheet", "sheet-2"],
        min: TEXT,
    },
    Check {
        fg: "syn-variable",
        on: &["sheet", "sheet-2"],
        min: TEXT,
    },
    Check {
        fg: "term-fg",
        on: &["term-bg"],
        min: TEXT,
    },
    Check {
        fg: "ansi-1",
        on: &["term-bg"],
        min: MARK,
    },
    Check {
        fg: "ansi-2",
        on: &["term-bg"],
        min: MARK,
    },
    Check {
        fg: "ansi-3",
        on: &["term-bg"],
        min: MARK,
    },
    Check {
        fg: "ansi-4",
        on: &["term-bg"],
        min: MARK,
    },
    Check {
        fg: "ansi-5",
        on: &["term-bg"],
        min: MARK,
    },
    Check {
        fg: "ansi-6",
        on: &["term-bg"],
        min: MARK,
    },
    Check {
        fg: "ansi-9",
        on: &["term-bg"],
        min: MARK,
    },
    Check {
        fg: "ansi-10",
        on: &["term-bg"],
        min: MARK,
    },
    Check {
        fg: "ansi-11",
        on: &["term-bg"],
        min: MARK,
    },
    Check {
        fg: "ansi-12",
        on: &["term-bg"],
        min: MARK,
    },
    Check {
        fg: "ansi-13",
        on: &["term-bg"],
        min: MARK,
    },
    Check {
        fg: "ansi-14",
        on: &["term-bg"],
        min: MARK,
    },
    Check {
        fg: "chart-1",
        on: &["sheet"],
        min: MARK,
    },
    Check {
        fg: "chart-2",
        on: &["sheet"],
        min: MARK,
    },
    Check {
        fg: "chart-3",
        on: &["sheet"],
        min: MARK,
    },
    Check {
        fg: "chart-4",
        on: &["sheet"],
        min: MARK,
    },
    Check {
        fg: "chart-5",
        on: &["sheet"],
        min: MARK,
    },
    Check {
        fg: "chart-6",
        on: &["sheet"],
        min: MARK,
    },
    Check {
        fg: "chart-axis",
        on: &["sheet"],
        min: TEXT,
    },
    Check {
        fg: "hue-0",
        on: &["on-hue"],
        min: MARK,
    },
    Check {
        fg: "hue-1",
        on: &["on-hue"],
        min: MARK,
    },
    Check {
        fg: "hue-2",
        on: &["on-hue"],
        min: MARK,
    },
    Check {
        fg: "hue-3",
        on: &["on-hue"],
        min: MARK,
    },
    Check {
        fg: "hue-4",
        on: &["on-hue"],
        min: MARK,
    },
    Check {
        fg: "hue-5",
        on: &["on-hue"],
        min: MARK,
    },
    Check {
        fg: "toast-fg",
        on: &["toast-bg"],
        min: TEXT,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_css_safe() {
        let mut seen = std::collections::BTreeSet::new();
        for t in TOKENS {
            assert!(seen.insert(t.name), "duplicate {}", t.name);
            assert!(
                t.name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            );
        }
    }

    #[test]
    fn checks_name_real_tokens() {
        for c in CHECKS {
            assert!(is_token(c.fg), "{}", c.fg);
            for g in c.on {
                assert!(is_token(g), "{g}");
            }
        }
    }
}
