//! The command palette: Ctrl+K opens a list of the terminal UI's actions (go to a tab, the next
//! project, refresh, stop, the tour, quit), narrowed by what is typed. The palette keeps only the
//! list, the query and the selection. The chosen action comes back as a `Step::Run`, and `app` runs
//! it with the same methods its key uses.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

use super::app::{Palette, Tab};
use super::style;

/// The palette's widest and tallest size, border included.
const WIDTH: u16 = 72;
const HEIGHT: u16 = 14;
/// The opening: for this long after it opens the palette sits one row lower, then settles.
const OPEN_SECS: f32 = 0.15;
/// The cursor blinks: on for half a second, off for half a second.
const BLINK_SECS: f32 = 0.5;

/// What a command does. `App::run_command` maps each one to the existing method its key calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    GoTo(Tab),
    NewConversation,
    MessageAgent,
    NextProject,
    PreviousProject,
    Refresh,
    StopAll,
    TakeTour,
    Quit,
}

/// What a key did to the palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The palette stays open.
    Stay,
    /// The palette closes and nothing runs.
    Close,
    /// The palette closes and the action runs.
    Run(Action),
}

/// One command: its label, the key that does the same (shown on the right), and its action.
struct Command {
    label: String,
    key: Option<char>,
    action: Action,
}

/// The palette while it is open: the query, the selected row and when it opened (on the clock).
pub struct CommandPalette {
    query: String,
    selected: usize,
    opened: f32,
    commands: Vec<Command>,
}

impl CommandPalette {
    /// A palette opened at `clock` (seconds), with an empty query.
    pub fn new(clock: f32) -> CommandPalette {
        CommandPalette {
            query: String::new(),
            selected: 0,
            opened: clock,
            commands: commands(),
        }
    }

    /// Handles one key press. Ctrl+K (toggle), Ctrl+C and Esc close; Enter runs the selected command;
    /// Up and Down move (and wrap); Backspace edits the query; other characters are typed into it.
    pub fn on_key(&mut self, key: KeyEvent) -> Step {
        if key.kind != KeyEventKind::Press {
            return Step::Stay;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('k' | 'c') if ctrl => return Step::Close,
            KeyCode::Esc => return Step::Close,
            KeyCode::Enter => {
                let found = self.matches();
                return match found.get(self.selected) {
                    Some((index, _)) => Step::Run(self.commands[*index].action),
                    None => Step::Stay,
                };
            }
            KeyCode::Up => self.move_by(-1),
            KeyCode::Down => self.move_by(1),
            KeyCode::Backspace => {
                self.query.pop();
                self.selected = 0;
            }
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                self.query.push(c);
                self.selected = 0;
            }
            _ => {}
        }
        Step::Stay
    }

    /// The commands the query matches, best first; the list's own order where the scores tie. Each
    /// comes with the positions of its matched characters.
    fn matches(&self) -> Vec<(usize, Vec<usize>)> {
        let mut found: Vec<(i32, usize, Vec<usize>)> = self
            .commands
            .iter()
            .enumerate()
            .filter_map(|(i, command)| {
                fuzzy(&self.query, &command.label).map(|(score, hits)| (score, i, hits))
            })
            .collect();
        found.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        found.into_iter().map(|(_, i, hits)| (i, hits)).collect()
    }

    /// Moves the selection by `delta` rows, wrapping at both ends.
    fn move_by(&mut self, delta: isize) {
        let count = self.matches().len();
        if count == 0 {
            return;
        }
        let next = (self.selected as isize + delta).rem_euclid(count as isize);
        self.selected = next as usize;
    }

    /// Draws the palette centred over `body`, one row lower while it is opening.
    pub fn draw(&self, frame: &mut Frame, body: Rect, palette: Palette, clock: f32) {
        let width = WIDTH.min(body.width.saturating_sub(4));
        let height = HEIGHT.min(body.height);
        if width < 24 || height < 6 {
            return;
        }
        let lift = u16::from(clock - self.opened < OPEN_SECS);
        let x = body.x + (body.width - width) / 2;
        let y = (body.y + (body.height - height) / 2 + lift).min(body.bottom() - height);
        let area = Rect::new(x, y, width, height);

        frame.render_widget(Clear, area);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(palette.accent))
            .title(Span::styled(
                " Commands ",
                Style::new().fg(palette.ink()).add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let [prompt, rule, list, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(inner);
        frame.render_widget(Paragraph::new(self.prompt_line(palette, clock)), prompt);
        let rule_text = "─".repeat(usize::from(rule.width));
        let rule_line = Line::from(Span::styled(rule_text, Style::new().fg(palette.rule())));
        frame.render_widget(Paragraph::new(rule_line), rule);
        frame.render_widget(Paragraph::new(self.list_lines(palette, list)), list);
        let keys = [("↑↓", "move"), ("Enter", "run"), ("Esc", "close")];
        frame.render_widget(
            Paragraph::new(style::hints(&keys, footer.width, &palette)),
            footer,
        );
    }

    /// The prompt: the chevron, the query and the cursor, or the placeholder while the query is empty.
    fn prompt_line(&self, palette: Palette, clock: f32) -> Line<'static> {
        let blink_on = ((clock / BLINK_SECS) as u64).is_multiple_of(2);
        let cursor = Span::styled(
            if blink_on { "▌" } else { " " },
            Style::new().fg(palette.accent),
        );
        let mut spans = vec![
            Span::styled(
                "› ",
                Style::new().fg(palette.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(self.query.clone(), Style::new().fg(palette.ink())),
            cursor,
        ];
        if self.query.is_empty() {
            spans.push(Span::styled(
                " Type a command…",
                Style::new().fg(palette.muted),
            ));
        }
        Line::from(spans)
    }

    /// The matching commands, from the top of the list and scrolled to keep the selection in view.
    fn list_lines(&self, palette: Palette, area: Rect) -> Vec<Line<'static>> {
        let found = self.matches();
        if found.is_empty() {
            let text = format!(" No command matches \"{}\".", self.query);
            return vec![Line::from(Span::styled(
                text,
                Style::new().fg(palette.muted),
            ))];
        }
        let rows = usize::from(area.height);
        let first = if self.selected >= rows {
            self.selected + 1 - rows
        } else {
            0
        };
        found
            .iter()
            .enumerate()
            .skip(first)
            .take(rows)
            .map(|(row, hit)| self.row(hit, row == self.selected, area.width, palette))
            .collect()
    }

    /// One command's row: the accent marker when selected, the label with its matched characters
    /// in accent, the key on the right. The selected row sits on the sheet colour, across its width.
    fn row(
        &self,
        hit: &(usize, Vec<usize>),
        selected: bool,
        width: u16,
        palette: Palette,
    ) -> Line<'static> {
        let (index, hits) = hit;
        let command = &self.commands[*index];
        let label_colour = if selected {
            palette.ink()
        } else {
            palette.ink2()
        };
        let marker = if selected {
            Span::styled("▌ ", Style::new().fg(palette.accent))
        } else {
            Span::raw("  ")
        };
        let mut spans = vec![marker];
        let mut used = 2;
        for (j, c) in command.label.chars().enumerate() {
            let style = if hits.contains(&j) {
                Style::new().fg(palette.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(label_colour)
            };
            spans.push(Span::styled(c.to_string(), style));
            used += 1;
        }
        let key = command.key.map(String::from).unwrap_or_default();
        let gap = usize::from(width).saturating_sub(used + key.chars().count() + 1);
        spans.push(Span::raw(" ".repeat(gap)));
        spans.push(Span::styled(key, Style::new().fg(palette.muted)));
        spans.push(Span::raw(" "));
        let base = if selected {
            Style::new().bg(palette.sheet())
        } else {
            Style::new()
        };
        Line::from(spans).style(base)
    }
}

/// The palette's commands, in order: one "Go to" per tab, then the actions that have a key.
fn commands() -> Vec<Command> {
    let mut list: Vec<Command> = Tab::ALL
        .iter()
        .map(|&tab| Command {
            label: format!("Go to {}", tab.label()),
            key: Some(tab.key()),
            action: Action::GoTo(tab),
        })
        .collect();
    let actions = [
        ("New conversation", 'n', Action::NewConversation),
        ("Message the selected agent", 't', Action::MessageAgent),
        ("Next project", 'p', Action::NextProject),
        ("Previous project", 'P', Action::PreviousProject),
        ("Refresh", 'r', Action::Refresh),
        ("Stop all agents", 's', Action::StopAll),
        ("Take the tour", '?', Action::TakeTour),
        ("Quit", 'q', Action::Quit),
    ];
    list.extend(actions.into_iter().map(|(label, key, action)| Command {
        label: label.to_string(),
        key: Some(key),
        action,
    }));
    list
}

/// Case folding for matching: the lower-case form of a character.
fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// How well `query` matches `text` as a subsequence, ignoring case: the score (higher is better)
/// and the positions of the matched characters in `text`. Each match scores; a match that starts a
/// word, or follows the one before it, scores more; a gap costs a point per character. An empty
/// query matches everything with score 0. None when the query's characters do not all appear in
/// order.
pub fn fuzzy(query: &str, text: &str) -> Option<(i32, Vec<usize>)> {
    let text: Vec<char> = text.chars().collect();
    let mut hits: Vec<usize> = Vec::new();
    let mut score = 0;
    let mut from = 0;
    for q in query.trim().chars() {
        let q = fold(q);
        let at = (from..text.len()).find(|&i| fold(text[i]) == q)?;
        score += 10;
        match hits.last() {
            Some(&prev) if prev + 1 == at => score += 5,
            Some(&prev) => score -= (at - prev - 1) as i32,
            None => score -= at as i32,
        }
        if at == 0 || !text[at - 1].is_alphanumeric() {
            score += 8;
        }
        hits.push(at);
        from = at + 1;
    }
    Some((score, hits))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn type_text(palette: &mut CommandPalette, text: &str) {
        for c in text.chars() {
            assert_eq!(palette.on_key(key(KeyCode::Char(c))), Step::Stay);
        }
    }

    /// The label of the command at the top of the matches.
    fn top(palette: &CommandPalette) -> String {
        let found = palette.matches();
        let (index, _) = found.first().expect("a match");
        palette.commands[*index].label.clone()
    }

    #[test]
    fn fuzzy_matches_a_subsequence_ignoring_case() {
        assert!(fuzzy("mach", "Go to Machines").is_some());
        assert!(fuzzy("GTM", "Go to Machines").is_some());
        assert!(fuzzy("", "anything").is_some_and(|(score, hits)| score == 0 && hits.is_empty()));
        assert!(fuzzy("xyz", "Go to Machines").is_none());
        assert!(
            fuzzy("sehcam", "Go to Machines").is_none(),
            "the order counts"
        );
        let (_, hits) = fuzzy("mach", "Go to Machines").unwrap();
        assert_eq!(hits, vec![6, 7, 8, 9]);
    }

    #[test]
    fn fuzzy_ranks_word_starts_and_runs_first() {
        let (word, _) = fuzzy("ov", "Overview").unwrap();
        let (inner, _) = fuzzy("ov", "Approvals").unwrap();
        assert!(word > inner, "a word start beats a letter inside a word");
        let (tight, _) = fuzzy("mac", "Machines").unwrap();
        let (loose, _) = fuzzy("mac", "Map Agent Chat").unwrap();
        assert!(tight > loose, "a run of characters beats spread-out ones");
    }

    #[test]
    fn the_query_narrows_and_ranks_the_commands() {
        let mut palette = CommandPalette::new(0.0);
        assert_eq!(palette.matches().len(), palette.commands.len());
        type_text(&mut palette, "mach");
        assert_eq!(top(&palette), "Go to Machines");
        assert_eq!(palette.matches().len(), 1, "only the Machines tab matches");
        let mut palette = CommandPalette::new(0.0);
        type_text(&mut palette, "ov");
        assert_eq!(top(&palette), "Go to Overview");
    }

    #[test]
    fn typing_opens_and_enter_runs_the_selected_command() {
        let mut palette = CommandPalette::new(0.0);
        type_text(&mut palette, "mach");
        assert_eq!(
            palette.on_key(key(KeyCode::Enter)),
            Step::Run(Action::GoTo(Tab::Machines))
        );
    }

    #[test]
    fn backspace_widens_the_query_again() {
        let mut palette = CommandPalette::new(0.0);
        type_text(&mut palette, "mach");
        palette.on_key(key(KeyCode::Backspace));
        palette.on_key(key(KeyCode::Backspace));
        palette.on_key(key(KeyCode::Backspace));
        palette.on_key(key(KeyCode::Backspace));
        palette.on_key(key(KeyCode::Backspace));
        assert_eq!(palette.query, "");
        assert_eq!(palette.matches().len(), palette.commands.len());
    }

    #[test]
    fn enter_with_no_match_keeps_the_palette_open() {
        let mut palette = CommandPalette::new(0.0);
        type_text(&mut palette, "zzz");
        assert_eq!(palette.on_key(key(KeyCode::Enter)), Step::Stay);
    }

    #[test]
    fn up_and_down_move_and_wrap() {
        let mut palette = CommandPalette::new(0.0);
        let count = palette.commands.len();
        assert_eq!(palette.selected, 0);
        palette.on_key(key(KeyCode::Up));
        assert_eq!(
            palette.selected,
            count - 1,
            "up from the top wraps to the bottom"
        );
        palette.on_key(key(KeyCode::Down));
        assert_eq!(palette.selected, 0, "down from the bottom wraps to the top");
        palette.on_key(key(KeyCode::Down));
        palette.on_key(key(KeyCode::Down));
        assert_eq!(
            palette.on_key(key(KeyCode::Enter)),
            Step::Run(Action::GoTo(Tab::Tasks))
        );
    }

    #[test]
    fn typing_resets_the_selection() {
        let mut palette = CommandPalette::new(0.0);
        palette.on_key(key(KeyCode::Down));
        palette.on_key(key(KeyCode::Down));
        type_text(&mut palette, "q");
        assert_eq!(palette.selected, 0);
    }

    #[test]
    fn esc_and_ctrl_c_close_and_ctrl_k_toggles() {
        let mut palette = CommandPalette::new(0.0);
        assert_eq!(palette.on_key(key(KeyCode::Esc)), Step::Close);
        assert_eq!(CommandPalette::new(0.0).on_key(ctrl('c')), Step::Close);
        assert_eq!(CommandPalette::new(0.0).on_key(ctrl('k')), Step::Close);
    }

    #[test]
    fn other_control_keys_do_not_type() {
        let mut palette = CommandPalette::new(0.0);
        assert_eq!(palette.on_key(ctrl('x')), Step::Stay);
        assert_eq!(palette.query, "");
    }

    #[test]
    fn the_actions_are_the_listed_commands() {
        let labels: Vec<String> = commands().into_iter().map(|c| c.label).collect();
        assert_eq!(labels.len(), Tab::ALL.len() + 8);
        assert!(labels.contains(&"Go to Providers".to_string()));
        for wanted in [
            "New conversation",
            "Message the selected agent",
            "Next project",
            "Previous project",
            "Refresh",
            "Stop all agents",
            "Take the tour",
            "Quit",
        ] {
            assert!(labels.iter().any(|l| l == wanted), "{wanted}");
        }
    }

    #[test]
    fn the_palette_draws_its_prompt_and_commands() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let palette = CommandPalette::new(0.0);
        terminal
            .draw(|frame| palette.draw(frame, frame.area(), Palette::detect(), 1.0))
            .unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("Type a command"));
        assert!(screen.contains("Go to Overview"));
        assert!(screen.contains("Esc"));
    }
}
