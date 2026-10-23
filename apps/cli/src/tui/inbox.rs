//! The Inbox tab: the cards waiting for you (`GET /v1/cards?open=true`), the decisions the plaza needs before it goes on.
//! The list is on top; under it the selected card: its title, its body, what it is about, and its buttons (its choices).
//! `←` and `→` move the focus between the buttons, and Enter answers the one in focus, after a confirm. A button that
//! takes an amount asks for it in the footer first (dollars, or whole tokens for a tokens budget). Declared from
//! `app.rs`, which holds the focus and the amount being typed.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph};
use serde_json::Value;

use super::App;
use super::style::{chip, hints, panel};
use crate::tui::screens::{Grid, bottom_hint, grid, keys, show, titled};

/// The hub's list of the open cards, which the tab polls while it shows.
pub const PATH: &str = "/v1/cards?open=true";
/// The height of the card under the table.
const DETAIL_HEIGHT: u16 = 11;
/// The most lines of a card's body the pane shows.
const BODY_LINES: usize = 4;

/// An amount waiting for its typed text: the card and the choice it is for, and the text so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Amount {
    pub card: String,
    pub choice: String,
    pub text: String,
}

/// The choices of a card, in the order the hub lists them.
pub fn choices_of(card: &Value) -> Vec<&Value> {
    card.get("choices")
        .and_then(Value::as_array)
        .map(|c| c.iter().collect())
        .unwrap_or_default()
}

/// What a card is about: its project, task and chat, the ones it has, joined by a dot.
fn about(card: &Value) -> String {
    ["project", "task", "chat"]
        .iter()
        .filter_map(|key| card.get(*key).and_then(Value::as_str))
        .collect::<Vec<&str>>()
        .join(" · ")
}

/// A button's text: its label, and `(amount)` after a choice that takes one.
pub fn button_label(choice: &Value) -> String {
    let label = show(choice.get("label"));
    if choice.get("takes_amount").and_then(Value::as_bool) == Some(true) {
        format!("{label} (amount)")
    } else {
        label
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let p = &app.palette;
    let cards = app.array(PATH);
    let (table_area, detail_area) = if cards.is_empty() {
        (area, None)
    } else {
        let [table, detail] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(DETAIL_HEIGHT)]).areas(area);
        (table, Some(detail))
    };
    let rows: Vec<Vec<Cell<'static>>> = cards
        .iter()
        .map(|c| {
            vec![
                Cell::from(show(c.get("id"))),
                Cell::from(show(c.get("kind"))),
                Cell::from(show(c.get("title"))),
                Cell::from(about(c)),
            ]
        })
        .collect();
    grid(
        frame,
        table_area,
        Grid {
            title: titled("inbox", cards.len(), app),
            empty: "Nothing waits for you",
            header: &["ID", "KIND", "TITLE", "ABOUT"],
            widths: &[
                Constraint::Length(7),
                Constraint::Length(12),
                Constraint::Fill(1),
                Constraint::Length(20),
            ],
            rows,
            hint: Some(keys(&[("Enter", "answer")], p)),
        },
        app,
    );
    if let (Some(detail), Some(card)) = (detail_area, cards.get(app.selected)) {
        card_detail(frame, detail, card, app);
    }
}

/// The selected card: its title, its body, what it is about, and its buttons, the one in focus lit. The focused button's
/// detail goes under them.
fn card_detail(frame: &mut Frame, area: Rect, card: &Value, app: &App) {
    let p = &app.palette;
    let ink = Style::new().fg(p.ink());
    let muted = Style::new().fg(p.muted);
    let choices = choices_of(card);
    let focus = app.button.min(choices.len().saturating_sub(1));
    let mut lines = vec![Line::from(Span::styled(
        show(card.get("title")),
        ink.add_modifier(Modifier::BOLD),
    ))];
    let about = about(card);
    if !about.is_empty() {
        lines.push(Line::from(Span::styled(format!("about {about}"), muted)));
    }
    for body in show(card.get("body")).lines().take(BODY_LINES) {
        lines.push(Line::from(Span::styled(
            body.to_string(),
            Style::new().fg(p.ink2()),
        )));
    }
    lines.push(Line::raw(""));
    let mut buttons: Vec<Span<'static>> = Vec::new();
    for (i, choice) in choices.iter().enumerate() {
        let text = format!(" {} ", button_label(choice));
        if i == focus {
            buttons.push(chip(&text, p.on_accent(), p.accent));
        } else {
            buttons.push(Span::styled(text, Style::new().fg(p.ink2())));
        }
        buttons.push(Span::raw("  "));
    }
    lines.push(Line::from(buttons));
    if let Some(choice) = choices.get(focus) {
        let detail = show(choice.get("detail"));
        if detail != "-" {
            lines.push(Line::from(Span::styled(detail, muted)));
        }
    }
    let block = panel(
        &format!(" {} · {} ", show(card.get("id")), show(card.get("kind"))),
        p,
        true,
    )
    .title_bottom(bottom_hint(hints(
        &[("←→", "button"), ("Enter", "answer")],
        area.width,
        p,
    )));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use serde_json::json;

    use super::*;
    use crate::ctx::Client;

    #[test]
    fn a_button_names_the_choice_and_says_when_it_takes_an_amount() {
        assert_eq!(
            button_label(&json!({ "id": "start", "label": "Start", "takes_amount": false })),
            "Start"
        );
        assert_eq!(
            button_label(&json!({ "id": "raise", "label": "Raise the cap", "takes_amount": true })),
            "Raise the cap (amount)"
        );
    }

    #[test]
    fn a_card_is_about_its_project_task_and_chat_where_it_has_them() {
        assert_eq!(
            about(&json!({ "project": "pr3", "task": null, "chat": "main" })),
            "pr3 · main"
        );
        assert_eq!(
            about(&json!({ "project": null, "task": null, "chat": null })),
            ""
        );
    }

    /// The Inbox tab draws the open cards, the selected one's title and body, and its buttons.
    #[test]
    fn the_tab_draws_the_selected_card_and_its_buttons() {
        let mut app = App::new(Client::new("http://127.0.0.1:9", None, None));
        app.data.insert(
            PATH,
            json!([{
                "id": "c7", "kind": "start", "title": "Start pr3?", "body": "Its tasks are ready.",
                "project": "pr3", "task": null, "chat": "main",
                "choices": [
                    { "id": "start", "label": "Start the project", "detail": "Agents begin", "takes_amount": false },
                    { "id": "later", "label": "Later", "detail": null, "takes_amount": false }
                ]
            }]),
        );
        app.select_tab(super::super::Tab::Inbox);
        let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &app))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Start pr3?"), "the card's title");
        assert!(text.contains("Its tasks are ready."), "the card's body");
        assert!(text.contains("Start the project"), "the first button");
        assert!(text.contains("Agents begin"), "the focused button's detail");
    }
}
