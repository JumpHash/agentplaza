//! The terminal UI's first start: a setup wizard (four screens) and a skippable tour of the tabs.
//! The steps and their words are the same in the web and desktop apps. The state machine is pure
//! (`advance`), so it is tested without a terminal; `persist` saves what finished in `cli.json`
//! (`setup_done`, `tour_done`), so the wizard shows once.

use std::path::Path;

use apz_setup::{Check, Need, State};
use crossterm::event::KeyCode;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use serde_json::Value;

use super::app::{Palette, Tab};
use crate::ctx::{self, Client, Config};

/// The wizard's screens: welcome, hub, dependencies, model keys, summary.
pub const SETUP_SCREENS: usize = 5;
/// The screen that checks this machine's dependencies, and fixes the missing ones on `f`.
pub const DEPS_SCREEN: usize = 2;
/// The most lines of a running fix's output the screen keeps.
const LOG_LINES: usize = 200;

/// One tour panel: the tab it switches to (none for the screens without one) and its lines.
pub struct TourStep {
    pub title: &'static str,
    pub tab: Option<Tab>,
    pub lines: &'static [&'static str],
}

pub const TOUR: [TourStep; 8] = [
    TourStep {
        title: "Overview",
        tab: Some(Tab::Overview),
        lines: &[
            "The plaza at a glance: the hub, its nodes and agents, the work and the spend.",
            "Command: `agentplaza overview`.",
        ],
    },
    TourStep {
        title: "Floor",
        tab: Some(Tab::Floor),
        lines: &[
            "The plaza drawn live: agents move between rooms as their work and messages flow.",
            "The machines are the nodes (press 8); `agentplaza machines` lists them too.",
        ],
    },
    TourStep {
        title: "Agents",
        tab: Some(Tab::Agents),
        lines: &[
            "Every agent and its state. Enter opens its details; t sends it a message.",
            "Command: `agentplaza agents`.",
        ],
    },
    TourStep {
        title: "Projects & Tasks",
        tab: Some(Tab::Tasks),
        lines: &[
            "Tasks are the work queue; `agentplaza tasks` lists it.",
            "Projects and their repositories: `agentplaza projects`.",
        ],
    },
    TourStep {
        title: "Approvals",
        tab: Some(Tab::Approvals),
        lines: &[
            "Decisions the agents wait on for you: a approves the selected one, d denies it.",
            "Command: `agentplaza approvals`.",
        ],
    },
    TourStep {
        title: "Conversations",
        tab: Some(Tab::Conversations),
        lines: &[
            "Talk between agents, and with you.",
            "Command: `agentplaza conversations`.",
        ],
    },
    TourStep {
        title: "Models & Usage",
        tab: None,
        lines: &[
            "Model providers, their keys and models: `agentplaza models providers`.",
            "The budget, the spend and the balances: `agentplaza treasury balances`.",
        ],
    },
    TourStep {
        title: "Setup",
        tab: None,
        lines: &[
            "The hub you talk to: `agentplaza hub status` checks it; `agentplaza hub login <url>` adds one.",
            "Run this setup again with `agentplaza tui --setup`; the tour with `--tour`, or ? here.",
        ],
    },
];

/// Where the flow is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Wizard screen `n` (0-based, below `SETUP_SCREENS`).
    Setup(usize),
    /// "Take a 1-minute tour? [y/N]".
    Ask,
    /// Tour step `n` (0-based, below `TOUR.len()`).
    Tour(usize),
}

/// A key, in the flow's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// Enter or →.
    Next,
    /// ←.
    Back,
    /// Esc or `s`.
    Skip,
    Yes,
    No,
    /// `f`: fix the missing dependencies (on the Dependencies screen, after a confirmation).
    Fix,
}

/// What finishing (or skipping) a part saves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Setup,
    Tour,
}

/// Where one input leads: the next stage (`None`: onboarding is over) and what to save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub next: Option<Stage>,
    pub mark: Option<Mark>,
}

/// The key that means an onboarding input; other keys do nothing during onboarding.
pub fn input_for(code: KeyCode) -> Option<Input> {
    match code {
        KeyCode::Enter | KeyCode::Right => Some(Input::Next),
        KeyCode::Left => Some(Input::Back),
        KeyCode::Esc | KeyCode::Char('s') => Some(Input::Skip),
        KeyCode::Char('y' | 'Y') => Some(Input::Yes),
        KeyCode::Char('n' | 'N') => Some(Input::No),
        KeyCode::Char('f' | 'F') => Some(Input::Fix),
        _ => None,
    }
}

/// The flow: one input from `stage`. Skipping the wizard goes on to the tour question; skipping
/// the tour, or declining it, ends the onboarding.
pub fn advance(stage: Stage, input: Input) -> Step {
    let stay = Step {
        next: Some(stage),
        mark: None,
    };
    match (stage, input) {
        (Stage::Setup(i), Input::Next) if i + 1 < SETUP_SCREENS => Step {
            next: Some(Stage::Setup(i + 1)),
            mark: None,
        },
        (Stage::Setup(_), Input::Next) => Step {
            next: Some(Stage::Ask),
            mark: Some(Mark::Setup),
        },
        (Stage::Setup(i), Input::Back) => Step {
            next: Some(Stage::Setup(i.saturating_sub(1))),
            mark: None,
        },
        (Stage::Setup(_), Input::Skip) => Step {
            next: Some(Stage::Ask),
            mark: Some(Mark::Setup),
        },
        (Stage::Ask, Input::Yes) => Step {
            next: Some(Stage::Tour(0)),
            mark: None,
        },
        (Stage::Ask, Input::No | Input::Next | Input::Skip) => Step {
            next: None,
            mark: Some(Mark::Tour),
        },
        (Stage::Ask, Input::Back) => Step {
            next: Some(Stage::Setup(SETUP_SCREENS - 1)),
            mark: None,
        },
        (Stage::Tour(i), Input::Next) if i + 1 < TOUR.len() => Step {
            next: Some(Stage::Tour(i + 1)),
            mark: None,
        },
        (Stage::Tour(_), Input::Next | Input::Skip) => Step {
            next: None,
            mark: Some(Mark::Tour),
        },
        (Stage::Tour(i), Input::Back) => Step {
            next: Some(Stage::Tour(i.saturating_sub(1))),
            mark: None,
        },
        _ => stay,
    }
}

/// Saves a completed part in the config at `path`, keeping the rest of it.
pub fn persist(path: &Path, mark: Mark) -> Result<(), String> {
    let mut config = Config::load_from(path);
    match mark {
        Mark::Setup => config.setup_done = true,
        Mark::Tour => config.tour_done = true,
    }
    config.save_to(path)
}

/// The model providers, as the wizard counts them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keys {
    pub total: usize,
    /// The ids of the providers whose key is missing (never the keys).
    pub missing: Vec<String>,
}

impl Keys {
    /// From `GET /v1/providers`: a provider is missing its key when its credential is `missing`.
    pub fn from_providers(list: &Value) -> Keys {
        let providers = list.as_array().map(Vec::as_slice).unwrap_or(&[]);
        let missing = providers
            .iter()
            .filter(|p| ctx::s(p, "credential") == "missing")
            .map(|p| ctx::s(p, "id").to_string())
            .collect();
        Keys {
            total: providers.len(),
            missing,
        }
    }
}

/// What the wizard reads from the hub: the owner token's check and the hub's name, and the keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// `Ok` with the hub's name and version ("" if it did not say), `Err` with why it does not answer.
    pub hub: Result<String, String>,
    pub keys: Result<Keys, String>,
}

/// Reads the facts from the hub (blocking; the UI calls it from a thread).
pub fn gather(client: &Client) -> Facts {
    let hub = match client.get("/v1/auth/check") {
        Err(e) => Err(e),
        Ok(_) => Ok(match client.get("/v1/hub/identity") {
            Ok(id) => [ctx::s(&id, "name"), ctx::s(&id, "version")]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(" "),
            Err(_) => String::new(),
        }),
    };
    let keys = client
        .get("/v1/providers")
        .map(|list| Keys::from_providers(&list));
    Facts { hub, keys }
}

/// The onboarding in progress: where it is, and the facts once the hub has answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Onboarding {
    pub stage: Stage,
    pub facts: Option<Facts>,
    pub deps: Deps,
}

/// The Dependencies screen: this machine's checks (once read), a fix waiting for `y`, and the
/// messages and output of the fixes that ran.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Deps {
    /// The checks; `None` until they are read.
    pub checks: Option<Vec<Check>>,
    /// The fixes are on screen for a `y` (`n`, Esc or any other key cancels them).
    pub confirming: bool,
    /// The fixes are running: the screen takes no keys until they end.
    pub running: bool,
    /// The last messages and lines of output, oldest first.
    pub log: Vec<String>,
}

/// What a key does on the Dependencies screen, before the wizard's own flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepsKey {
    /// Not the Dependencies screen, or a key that means navigation there: the flow takes it.
    Pass,
    /// The screen took the key.
    Handled,
    /// The confirmed fixes start, for these requirements in this order.
    Run(Vec<Need>),
}

/// The requirements a fix run would change, in the checklist's order.
pub fn fixable_needs(checks: &[Check]) -> Vec<Need> {
    apz_setup::fixable(checks)
        .into_iter()
        .map(|check| check.need)
        .collect()
}

impl Onboarding {
    pub fn new(stage: Stage) -> Onboarding {
        Onboarding {
            stage,
            facts: None,
            deps: Deps::default(),
        }
    }

    /// Routes a key on the Dependencies screen: `f` asks for a confirmation, `y` runs the fixes,
    /// and while they run the screen takes no keys. Other keys go on to the flow.
    pub fn deps_key(&mut self, input: Input) -> DepsKey {
        if self.stage != Stage::Setup(DEPS_SCREEN) {
            return DepsKey::Pass;
        }
        let deps = &mut self.deps;
        if deps.running {
            return DepsKey::Handled;
        }
        if deps.confirming {
            deps.confirming = false;
            if input != Input::Yes {
                return DepsKey::Handled;
            }
            let needs = deps
                .checks
                .as_deref()
                .map(fixable_needs)
                .unwrap_or_default();
            if needs.is_empty() {
                return DepsKey::Handled;
            }
            deps.running = true;
            deps.log.clear();
            return DepsKey::Run(needs);
        }
        if input != Input::Fix {
            return DepsKey::Pass;
        }
        match deps.checks.as_deref() {
            None => deps.log = vec!["the checks are still being read".to_string()],
            Some(checks) if fixable_needs(checks).is_empty() => {
                deps.log = vec!["nothing here needs a fix".to_string()];
            }
            Some(_) => deps.confirming = true,
        }
        DepsKey::Handled
    }

    /// Keeps a line of a running fix's output, the last `LOG_LINES`.
    pub fn push_log(&mut self, line: String) {
        self.deps.log.push(line);
        if self.deps.log.len() > LOG_LINES {
            let extra = self.deps.log.len() - LOG_LINES;
            self.deps.log.drain(..extra);
        }
    }

    /// The footer's hint for the stage.
    pub fn hint(&self) -> &'static str {
        match self.stage {
            Stage::Setup(DEPS_SCREEN) if self.deps.running => {
                " the fixes are running · Ctrl-C quits"
            }
            Stage::Setup(DEPS_SCREEN) if self.deps.confirming => {
                " y runs the fixes · any other key cancels · Ctrl-C quits"
            }
            Stage::Setup(DEPS_SCREEN) => {
                " f fixes missing · Enter/→ next · ← back · Esc skips setup · Ctrl-C quits"
            }
            Stage::Setup(i) if i + 1 == SETUP_SCREENS => {
                " Enter finishes setup · ← back · Esc skips it · Ctrl-C quits"
            }
            Stage::Setup(_) => " Enter/→ next · ← back · Esc skips setup · Ctrl-C quits",
            Stage::Ask => " y starts the tour · n or Enter skips it · Ctrl-C quits",
            Stage::Tour(i) if i + 1 == TOUR.len() => {
                " Enter/→ finishes the tour · ← back · Esc skips it · Ctrl-C quits"
            }
            Stage::Tour(_) => " Enter/→ next · ← back · Esc skips the tour · Ctrl-C quits",
        }
    }

    /// Draws the panel for the stage: the wizard centred over the body, the tour along its bottom.
    pub fn draw(&self, frame: &mut Frame, body: Rect, palette: Palette, base: &str) {
        match self.stage {
            Stage::Setup(i) => {
                let lines = self.setup_lines(i, palette, base);
                let height = if i == DEPS_SCREEN { 21 } else { 17 };
                let area = centred(body, 74, height);
                panel(frame, area, palette, " AgentPlaza setup ", lines);
            }
            Stage::Ask => {
                let lines = vec![
                    line("Setup is done.", palette.paper, true),
                    Line::from(""),
                    line("Take a 1-minute tour? [y/N]", palette.accent, true),
                    Line::from(""),
                    line(
                        "The tour shows each screen of this terminal UI in turn.",
                        palette.muted,
                        false,
                    ),
                ];
                let area = centred(body, 74, 9);
                panel(frame, area, palette, " AgentPlaza ", lines);
            }
            Stage::Tour(i) => {
                let step = &TOUR[i];
                let mut lines = vec![line(
                    &format!("Step {} of {} · {}", i + 1, TOUR.len(), step.title),
                    palette.accent,
                    true,
                )];
                lines.extend(
                    step.lines
                        .iter()
                        .map(|text| line(text, palette.paper, false)),
                );
                lines.push(Line::from(""));
                lines.push(line(self.hint().trim_start(), palette.muted, false));
                let height = (lines.len() as u16 + 2).min(body.height);
                let width = body.width.min(78);
                let area = Rect::new(
                    body.x + (body.width - width) / 2,
                    body.y + body.height - height,
                    width,
                    height,
                );
                panel(frame, area, palette, " Tour ", lines);
            }
        }
    }

    fn setup_lines(&self, i: usize, palette: Palette, base: &str) -> Vec<Line<'static>> {
        let mut lines = vec![
            line(
                &format!("Step {} of {SETUP_SCREENS}", i + 1),
                palette.accent,
                true,
            ),
            Line::from(""),
        ];
        match i {
            0 => {
                lines.push(line("Welcome to AgentPlaza.", palette.paper, true));
                lines.push(line(
                    "AgentPlaza is a town for your AI agents, from the terminal.",
                    palette.paper,
                    false,
                ));
                lines.push(line(
                    "This short setup checks the hub and the model keys. Esc skips it.",
                    palette.muted,
                    false,
                ));
            }
            1 => {
                lines.push(line(
                    &format!("This CLI talks to the hub at {base}."),
                    palette.paper,
                    false,
                ));
                lines.push(line(&self.hub_text(), self.hub_color(palette), true));
                lines.push(Line::from(""));
                lines.push(line(
                    "Change it: `agentplaza hub login <url>` saves another hub,",
                    palette.muted,
                    false,
                ));
                lines.push(line(
                    "`agentplaza hub use <name>` makes one the default, and",
                    palette.muted,
                    false,
                ));
                lines.push(line(
                    "`--hub <name>` picks one for a single command.",
                    palette.muted,
                    false,
                ));
            }
            DEPS_SCREEN => lines.extend(self.deps_lines(palette)),
            3 => {
                lines.push(line(&self.keys_text(), palette.paper, true));
                if let Some(Ok(keys)) = self.facts.as_ref().map(|f| &f.keys) {
                    if !keys.missing.is_empty() {
                        lines.push(line(
                            &format!("Without a key: {}", keys.missing.join(", ")),
                            palette.accent,
                            false,
                        ));
                    }
                }
                lines.push(Line::from(""));
                lines.push(line(
                    "Add a key with `agentplaza models key <id> --value -`, then paste",
                    palette.muted,
                    false,
                ));
                lines.push(line(
                    "it on stdin. The key is never shown.",
                    palette.muted,
                    false,
                ));
            }
            _ => {
                lines.push(line(&self.hub_summary(), self.hub_color(palette), true));
                lines.push(line(&self.keys_summary(), palette.paper, false));
                lines.push(Line::from(""));
                lines.push(line(&self.deps_summary(), self.deps_color(palette), false));
                lines.push(line(
                    "`agentplaza setup` lists the same checks and fixes them from the shell.",
                    palette.muted,
                    false,
                ));
            }
        }
        lines.push(Line::from(""));
        lines.push(line(self.hint().trim_start(), palette.muted, false));
        lines
    }

    fn hub_text(&self) -> String {
        match self.facts.as_ref().map(|f| &f.hub) {
            None => "Checking the hub...".to_string(),
            Some(Ok(who)) if who.is_empty() => "The hub answers.".to_string(),
            Some(Ok(who)) => format!("The hub answers ({who})."),
            Some(Err(e)) => format!("The hub does not answer: {e}"),
        }
    }

    fn hub_summary(&self) -> String {
        match self.facts.as_ref().map(|f| &f.hub) {
            None => "Hub: checking...".to_string(),
            Some(Ok(_)) => "Hub: answers.".to_string(),
            Some(Err(e)) => format!("Hub: does not answer ({e})"),
        }
    }

    fn hub_color(&self, palette: Palette) -> ratatui::style::Color {
        match self.facts.as_ref().map(|f| &f.hub) {
            Some(Ok(_)) => palette.ok,
            Some(Err(_)) => palette.accent,
            None => palette.muted,
        }
    }

    /// The Dependencies screen: each check with its state, the confirmation of a fix or the way to
    /// ask for one, and the last messages and output.
    fn deps_lines(&self, palette: Palette) -> Vec<Line<'static>> {
        let deps = &self.deps;
        let mut lines = vec![line(
            "This machine: what a local plaza needs.",
            palette.paper,
            true,
        )];
        let fixable: Vec<Check> = deps
            .checks
            .as_deref()
            .map(apz_setup::fixable)
            .unwrap_or_default();
        match &deps.checks {
            None => lines.push(line("Checking this machine...", palette.muted, false)),
            Some(checks) => {
                for check in checks {
                    let (word, color) = match check.state {
                        State::Ok => ("OK", palette.ok),
                        State::Missing => ("MISSING", palette.accent),
                        State::Blocked => ("BLOCKED", palette.accent),
                        State::Warning => ("WARNING", palette.muted),
                    };
                    let optional = if check.required { "" } else { " (optional)" };
                    lines.push(line(
                        &format!(
                            "{:<11} {:<8} {}{optional}",
                            check.label,
                            word,
                            clip(&check.detail, 36)
                        ),
                        color,
                        false,
                    ));
                }
            }
        }
        lines.push(Line::from(""));
        if deps.confirming {
            let labels: Vec<&str> = fixable.iter().map(|check| check.label).collect();
            lines.push(line(
                &format!("Run the fixes for {}? [y/N]", labels.join(", ")),
                palette.accent,
                true,
            ));
            lines.push(line(
                "The steps are the ones `agentplaza setup --fix` shows.",
                palette.muted,
                false,
            ));
        } else if deps.running {
            lines.push(line("Fixing: this takes a while.", palette.accent, true));
        } else if !fixable.is_empty() {
            lines.push(line(
                "Press f to fix the missing ones: you confirm first.",
                palette.muted,
                false,
            ));
        }
        let start = deps.log.len().saturating_sub(3);
        for text in &deps.log[start..] {
            lines.push(line(&clip(text, 70), palette.muted, false));
        }
        lines
    }

    fn deps_summary(&self) -> String {
        match self.deps.checks.as_deref() {
            None => "Dependencies: checking...".to_string(),
            Some(checks) => {
                let open = checks
                    .iter()
                    .filter(|c| matches!(c.state, State::Missing | State::Blocked))
                    .count();
                if open == 0 {
                    "Dependencies: all set.".to_string()
                } else {
                    format!("Dependencies: {open} to do (the Dependencies step).")
                }
            }
        }
    }

    fn deps_color(&self, palette: Palette) -> ratatui::style::Color {
        match self.deps.checks.as_deref() {
            None => palette.muted,
            Some(checks)
                if checks
                    .iter()
                    .any(|c| matches!(c.state, State::Missing | State::Blocked)) =>
            {
                palette.accent
            }
            Some(_) => palette.ok,
        }
    }

    fn keys_text(&self) -> String {
        match self.facts.as_ref().map(|f| &f.keys) {
            None => "Checking the model providers...".to_string(),
            Some(Err(e)) => format!("Could not read the model providers: {e}"),
            Some(Ok(keys)) if keys.total == 0 => "No model provider is configured yet.".to_string(),
            Some(Ok(keys)) => format!(
                "Model providers: {} configured, {} without a key.",
                keys.total,
                keys.missing.len()
            ),
        }
    }

    fn keys_summary(&self) -> String {
        match self.facts.as_ref().map(|f| &f.keys) {
            None => "Model keys: checking...".to_string(),
            Some(Err(e)) => format!("Model keys: could not read them ({e})"),
            Some(Ok(keys)) if keys.missing.is_empty() => "Model keys: all set.".to_string(),
            Some(Ok(keys)) => format!(
                "Model keys: {} missing ({}).",
                keys.missing.len(),
                keys.missing.join(", ")
            ),
        }
    }
}

/// `text`, cut to `max` characters (ending in "..." when it was longer).
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max.saturating_sub(3)).collect();
    format!("{head}...")
}

/// A one-line paragraph in a colour, optionally bold.
fn line(text: &str, color: ratatui::style::Color, bold: bool) -> Line<'static> {
    let mut style = Style::new().fg(color);
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    Line::from(Span::styled(text.to_string(), style))
}

/// A `width` × `height` rectangle at the centre of `body` (never larger than `body`).
fn centred(body: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(body.width);
    let height = height.min(body.height);
    Rect::new(
        body.x + (body.width - width) / 2,
        body.y + (body.height - height) / 2,
        width,
        height,
    )
}

/// Clears `area` and draws the lines in a bordered box.
fn panel(frame: &mut Frame, area: Rect, palette: Palette, title: &str, lines: Vec<Line<'static>>) {
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .title(title.to_string())
        .border_style(Style::new().fg(palette.accent));
    frame.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_file(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "apz-cli-onboarding-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("cli.json")
    }

    #[test]
    fn wizard_walks_forward_and_back_and_finishes_with_the_tour_question() {
        let mut stage = Stage::Setup(0);
        let mut marks = Vec::new();
        for expected in [
            Stage::Setup(1),
            Stage::Setup(2),
            Stage::Setup(3),
            Stage::Setup(4),
            Stage::Ask,
        ] {
            let step = advance(stage, Input::Next);
            assert_eq!(step.next, Some(expected));
            marks.push(step.mark);
            stage = expected;
        }
        assert_eq!(
            marks,
            [None, None, None, None, Some(Mark::Setup)],
            "the wizard is saved on its last screen"
        );

        assert_eq!(
            advance(Stage::Setup(0), Input::Back).next,
            Some(Stage::Setup(0)),
            "back on the first screen stays"
        );
        assert_eq!(
            advance(Stage::Setup(2), Input::Back).next,
            Some(Stage::Setup(1))
        );
        assert_eq!(
            advance(Stage::Ask, Input::Back).next,
            Some(Stage::Setup(SETUP_SCREENS - 1))
        );
    }

    #[test]
    fn skipping_the_wizard_saves_it_and_goes_on_to_the_question() {
        for i in 0..SETUP_SCREENS {
            let step = advance(Stage::Setup(i), Input::Skip);
            assert_eq!(
                step,
                Step {
                    next: Some(Stage::Ask),
                    mark: Some(Mark::Setup)
                }
            );
        }
    }

    #[test]
    fn the_tour_question_defaults_to_no() {
        assert_eq!(advance(Stage::Ask, Input::Yes).next, Some(Stage::Tour(0)));
        assert_eq!(
            advance(Stage::Ask, Input::No),
            Step {
                next: None,
                mark: Some(Mark::Tour)
            }
        );
        assert_eq!(
            advance(Stage::Ask, Input::Next),
            Step {
                next: None,
                mark: Some(Mark::Tour)
            },
            "Enter is no"
        );
        assert_eq!(
            advance(Stage::Ask, Input::Skip),
            Step {
                next: None,
                mark: Some(Mark::Tour)
            }
        );
    }

    #[test]
    fn the_tour_walks_its_steps_and_can_be_skipped_at_any_of_them() {
        assert!((6..=8).contains(&TOUR.len()));
        let mut stage = Stage::Tour(0);
        for i in 1..TOUR.len() {
            stage = advance(stage, Input::Next).next.expect("still touring");
            assert_eq!(stage, Stage::Tour(i));
        }
        assert_eq!(
            advance(stage, Input::Next),
            Step {
                next: None,
                mark: Some(Mark::Tour)
            },
            "the last Next finishes"
        );
        for i in 0..TOUR.len() {
            assert_eq!(
                advance(Stage::Tour(i), Input::Skip),
                Step {
                    next: None,
                    mark: Some(Mark::Tour)
                }
            );
        }
        assert_eq!(
            advance(Stage::Tour(0), Input::Back).next,
            Some(Stage::Tour(0))
        );
        assert_eq!(
            advance(Stage::Tour(3), Input::Back).next,
            Some(Stage::Tour(2))
        );
        assert_eq!(
            advance(Stage::Tour(0), Input::Yes).next,
            Some(Stage::Tour(0)),
            "yes/no mean nothing inside the tour"
        );
    }

    #[test]
    fn keys_map_to_inputs() {
        assert_eq!(input_for(KeyCode::Enter), Some(Input::Next));
        assert_eq!(input_for(KeyCode::Right), Some(Input::Next));
        assert_eq!(input_for(KeyCode::Left), Some(Input::Back));
        assert_eq!(input_for(KeyCode::Esc), Some(Input::Skip));
        assert_eq!(input_for(KeyCode::Char('s')), Some(Input::Skip));
        assert_eq!(input_for(KeyCode::Char('y')), Some(Input::Yes));
        assert_eq!(input_for(KeyCode::Char('N')), Some(Input::No));
        assert_eq!(input_for(KeyCode::Char('f')), Some(Input::Fix));
        assert_eq!(input_for(KeyCode::Char('x')), None);
    }

    fn check(need: Need, state: State, fix: Option<&str>) -> Check {
        Check {
            need,
            label: need.name(),
            state,
            detail: format!("{} detail", need.name()),
            fix: fix.map(str::to_string),
            required: true,
        }
    }

    /// The Dependencies screen, with `checks` read.
    fn on_deps(checks: Vec<Check>) -> Onboarding {
        let mut ob = Onboarding::new(Stage::Setup(DEPS_SCREEN));
        ob.deps.checks = Some(checks);
        ob
    }

    #[test]
    fn f_asks_for_a_confirmation_and_no_cancels_it_without_running_anything() {
        let mut ob = on_deps(vec![check(
            Need::Postgres,
            State::Missing,
            Some("Create it"),
        )]);
        assert_eq!(ob.deps_key(Input::Fix), DepsKey::Handled);
        assert!(ob.deps.confirming);
        assert!(!ob.deps.running);
        assert_eq!(ob.deps_key(Input::No), DepsKey::Handled);
        assert!(!ob.deps.confirming && !ob.deps.running);
    }

    #[test]
    fn y_after_the_question_runs_the_missing_fixable_needs_in_order_and_nothing_else() {
        let mut ob = on_deps(vec![
            check(Need::Docker, State::Ok, None),
            check(Need::Services, State::Blocked, None),
            check(Need::Postgres, State::Missing, Some("Create it")),
            check(Need::Garage, State::Missing, Some("Create Garage")),
        ]);
        ob.deps.log.push("left over".into());
        assert_eq!(ob.deps_key(Input::Fix), DepsKey::Handled);
        assert_eq!(
            ob.deps_key(Input::Yes),
            DepsKey::Run(vec![Need::Postgres, Need::Garage])
        );
        assert!(ob.deps.running && !ob.deps.confirming);
        assert!(ob.deps.log.is_empty(), "the log starts clean for the run");
    }

    #[test]
    fn a_running_fix_takes_no_keys_until_it_ends() {
        let mut ob = on_deps(vec![check(
            Need::Postgres,
            State::Missing,
            Some("Create it"),
        )]);
        ob.deps.running = true;
        for input in [
            Input::Next,
            Input::Skip,
            Input::Back,
            Input::Fix,
            Input::Yes,
        ] {
            assert_eq!(ob.deps_key(input), DepsKey::Handled, "{input:?}");
        }
        assert_eq!(
            ob.stage,
            Stage::Setup(DEPS_SCREEN),
            "Esc does not leave it either"
        );
    }

    #[test]
    fn other_keys_and_other_screens_go_on_to_the_flow() {
        let mut ob = on_deps(vec![]);
        assert_eq!(ob.deps_key(Input::Next), DepsKey::Pass);
        assert_eq!(ob.deps_key(Input::Skip), DepsKey::Pass);
        let mut elsewhere = Onboarding::new(Stage::Setup(0));
        assert_eq!(elsewhere.deps_key(Input::Fix), DepsKey::Pass);
    }

    #[test]
    fn f_with_nothing_to_fix_says_so_and_asks_for_nothing() {
        let mut ob = on_deps(vec![check(Need::Docker, State::Ok, None)]);
        assert_eq!(ob.deps_key(Input::Fix), DepsKey::Handled);
        assert!(!ob.deps.confirming);
        assert_eq!(ob.deps.log, vec!["nothing here needs a fix".to_string()]);

        let mut unread = Onboarding::new(Stage::Setup(DEPS_SCREEN));
        assert_eq!(unread.deps_key(Input::Fix), DepsKey::Handled);
        assert!(!unread.deps.confirming);
    }

    #[test]
    fn yes_with_nothing_fixable_runs_nothing() {
        let mut ob = on_deps(vec![check(Need::Docker, State::Ok, None)]);
        ob.deps.confirming = true;
        assert_eq!(ob.deps_key(Input::Yes), DepsKey::Handled);
        assert!(!ob.deps.running);
    }

    #[test]
    fn the_output_of_a_fix_keeps_only_its_last_lines() {
        let mut ob = Onboarding::new(Stage::Setup(DEPS_SCREEN));
        for i in 0..LOG_LINES + 5 {
            ob.push_log(format!("line {i}"));
        }
        assert_eq!(ob.deps.log.len(), LOG_LINES);
        assert_eq!(ob.deps.log.first().map(String::as_str), Some("line 5"));
    }

    #[test]
    fn the_dependencies_step_lists_each_check_with_its_state_and_the_fix_key() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut ob = on_deps(vec![
            check(Need::Docker, State::Ok, None),
            check(Need::Postgres, State::Missing, Some("Create it")),
        ]);
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|frame| {
            ob.draw(
                frame,
                frame.area(),
                Palette::detect(),
                "http://127.0.0.1:7791",
            )
        })
        .unwrap();
        let text: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Step 3 of 5"), "{text}");
        assert!(text.contains("MISSING"), "{text}");
        assert!(text.contains("postgres"), "{text}");
        assert!(text.contains("Press f"), "{text}");

        ob.deps.confirming = true;
        term.draw(|frame| {
            ob.draw(
                frame,
                frame.area(),
                Palette::detect(),
                "http://127.0.0.1:7791",
            )
        })
        .unwrap();
        let text: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Run the fixes for postgres? [y/N]"), "{text}");
    }

    #[test]
    fn clip_cuts_long_text_with_dots() {
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("abcdefghij", 6), "abc...");
        assert_eq!(clip("abcdef", 6), "abcdef");
    }

    #[test]
    fn tour_steps_point_at_real_tabs_and_commands() {
        let tabs: Vec<Tab> = TOUR.iter().filter_map(|s| s.tab).collect();
        assert_eq!(
            tabs,
            [
                Tab::Overview,
                Tab::Floor,
                Tab::Agents,
                Tab::Tasks,
                Tab::Approvals,
                Tab::Conversations
            ]
        );
        for step in &TOUR {
            assert!(!step.lines.is_empty(), "{}", step.title);
        }
    }

    #[test]
    fn saving_a_part_keeps_the_hubs_and_the_other_flag() {
        let path = temp_file("persist");
        let mut config = Config::default();
        config.default = Some("home".to_string());
        config.hubs.insert(
            "home".to_string(),
            crate::ctx::SavedHub {
                url: "http://127.0.0.1:9".into(),
                token: None,
                pin: None,
                mcp_token: None,
            },
        );
        config.save_to(&path).unwrap();

        assert!(!Config::load_from(&path).setup_done);
        persist(&path, Mark::Setup).unwrap();
        let saved = Config::load_from(&path);
        assert!(saved.setup_done && !saved.tour_done);
        assert_eq!(saved.default.as_deref(), Some("home"));
        assert!(saved.hubs.contains_key("home"));

        persist(&path, Mark::Tour).unwrap();
        let saved = Config::load_from(&path);
        assert!(saved.setup_done && saved.tour_done);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_config_from_before_the_flags_reads_as_not_done() {
        let config: Config = serde_json::from_str(r#"{"hubs":{},"default":null}"#).unwrap();
        assert!(!config.setup_done && !config.tour_done);
    }

    #[test]
    fn keys_count_the_providers_missing_a_key_and_never_read_a_key() {
        let list = json!([
            { "id": "openai", "kind": "openai", "credential": "secret" },
            { "id": "local", "kind": "ollama", "credential": "none" },
            { "id": "anthropic", "kind": "anthropic", "credential": "missing" },
        ]);
        let keys = Keys::from_providers(&list);
        assert_eq!(keys.total, 3);
        assert_eq!(keys.missing, ["anthropic"]);
        assert_eq!(
            Keys::from_providers(&json!([])),
            Keys {
                total: 0,
                missing: vec![]
            }
        );
    }

    #[test]
    fn every_panel_draws_at_small_sizes_and_shows_its_step() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let palette = Palette::detect();
        let mut stages = vec![Stage::Ask];
        stages.extend((0..SETUP_SCREENS).map(Stage::Setup));
        stages.extend((0..TOUR.len()).map(Stage::Tour));
        let facts = Facts {
            hub: Ok("apid 0.0.8".to_string()),
            keys: Ok(Keys {
                total: 2,
                missing: vec!["anthropic".to_string()],
            }),
        };
        for (width, height) in [(80, 24), (40, 12), (20, 6)] {
            for stage in &stages {
                let mut ob = Onboarding::new(*stage);
                ob.facts = Some(facts.clone());
                let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
                term.draw(|frame| ob.draw(frame, frame.area(), palette, "http://127.0.0.1:7791"))
                    .unwrap();
            }
        }

        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut ob = Onboarding::new(Stage::Setup(3));
        ob.facts = Some(facts);
        term.draw(|frame| ob.draw(frame, frame.area(), palette, "http://127.0.0.1:7791"))
            .unwrap();
        let text: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Step 4 of 5"), "{text}");
        assert!(
            text.contains("anthropic"),
            "the missing key's provider id is named"
        );
        assert!(
            text.contains("agentplaza models key"),
            "the way to add a key is shown"
        );
    }
}
