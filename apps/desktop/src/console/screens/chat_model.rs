//! The inbox's and the chats' logic as plain functions: the labels of cards, chats and authors, the tone a card kind has,
//! the amount an answer takes, the text a settled card shows, the order and the tail of a chat's messages, and the lines
//! of the expert view. No drawing and no I/O here, so the screens and the tests below say the same things as the web
//! (`apps/web/src/views/chat-model.ts`).

use serde_json::Value;

use super::budget_model::{parse_dollars, parse_tokens};
use crate::console::ui::Tone;
use crate::console::work_api::{Card, CardChoice, ChatMessage};

/// Sentence case: the words of `text` with underscores as spaces, the first letter capital.
fn sentence_case(text: &str) -> String {
    let plain = text.replace('_', " ");
    let plain = plain.trim().to_lowercase();
    let mut chars = plain.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// A card kind's label: `Budget`, `Tool call`, `Start a project`; a kind not listed reads in sentence case.
pub fn card_kind_label(kind: &str) -> String {
    if kind == "start" {
        "Start a project".into()
    } else {
        sentence_case(kind)
    }
}

/// A card kind's tone: an incident is danger; the kinds that spend, grant or release something are warnings.
pub fn kind_tone(kind: &str) -> Tone {
    match kind {
        "incident" => Tone::Danger,
        "budget" | "allowance" | "disk" | "tool_call" | "deploy" | "release" => Tone::Warn,
        _ => Tone::Muted,
    }
}

/// The choices that stop, refuse or fail something: their buttons are danger.
pub fn is_danger_choice(id: &str) -> bool {
    matches!(id, "stop" | "deny" | "reject" | "fail")
}

/// The unit an answer's amount is counted in, as the card's owner's budget has it: dollars (also for no budget at all),
/// tokens, or unknown when the budget did not read, so the amount is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmountUnit {
    Unknown,
    Dollars,
    Tokens,
}

/// The unit an amount is typed in, as a card names it.
pub fn amount_unit_name(unit: AmountUnit) -> &'static str {
    match unit {
        AmountUnit::Unknown => "unit not known",
        AmountUnit::Dollars => "dollars",
        AmountUnit::Tokens => "tokens",
    }
}

/// The integer an answer's amount is sent as, in the card's unit: micro-dollars for dollars (the dollars typed, as the
/// budget form reads them), whole tokens for tokens. The amount must be more than zero; a refusal says why.
pub fn amount_wire(unit: AmountUnit, text: &str) -> Result<i64, String> {
    let value = match unit {
        AmountUnit::Unknown => {
            return Err(
                "The budget could not be read, so the unit of the amount is not known.".into(),
            );
        }
        AmountUnit::Dollars => parse_dollars(text)?,
        AmountUnit::Tokens => parse_tokens(text)?,
    };
    if value > 0 {
        Ok(value)
    } else {
        Err("The amount is more than zero.".into())
    }
}

/// What a card that is no longer open reads as: `Answered: <the chosen label>`, or `Withdrawn`. An open card has no such
/// text; its buttons are its answer.
pub fn settled_text(state: &str, answer: Option<&str>, choices: &[CardChoice]) -> String {
    match state {
        "withdrawn" => "Withdrawn".into(),
        "open" => String::new(),
        _ => match answer.and_then(|a| choices.iter().find(|c| c.id == a)) {
            Some(chosen) => format!("Answered: {}", chosen.label),
            None => "Answered".into(),
        },
    }
}

/// Who a message is from: `You` for the owner, the seat's name for a session (`Session` without one), `Plaza` for the
/// hub.
pub fn author_label(author: &str, seat: Option<&str>) -> String {
    match author {
        "owner" => "You".into(),
        "system" => "Plaza".into(),
        _ => match seat {
            Some(seat) => sentence_case(seat),
            None => "Session".into(),
        },
    }
}

/// A chat's kind, as the list names it: the orchestrator's, a project's planner, or a task's.
pub fn chat_kind_label(kind: &str) -> &'static str {
    match kind {
        "main" => "Orchestrator",
        "task" => "Task",
        _ => "Project",
    }
}

/// The last `limit` messages, oldest first. The list is read, not reordered; the rows are references into it.
pub fn recent_messages(messages: &[ChatMessage], limit: usize) -> Vec<&ChatMessage> {
    let mut ordered: Vec<&ChatMessage> = messages.iter().collect();
    ordered.sort_by_key(|m| m.seq);
    let start = ordered.len().saturating_sub(limit);
    ordered.split_off(start)
}

/// A message's time of day in UTC, `HH:MM`, as the timeline draws its clock.
pub fn clock_text(ms: u64) -> String {
    let s = (ms / 1000) % 86_400;
    format!("{:02}:{:02}", s / 3600, (s % 3600) / 60)
}

/// A card's dense line for the expert view: its id, raw kind, subject and the raw ids of its choices.
pub fn card_expert_line(card: &Card) -> String {
    let choices: Vec<&str> = card.choices.iter().map(|c| c.id.as_str()).collect();
    format!(
        "{} · {} · {} · choices {}",
        card.id,
        card.kind,
        card.subject,
        choices.join(", ")
    )
}

/// A message's dense line for the expert view: its number, raw kind and author, its session and its card.
pub fn message_expert_line(message: &ChatMessage) -> String {
    let mut parts = vec![
        format!("#{}", message.seq),
        message.kind.clone(),
        message.author.clone(),
    ];
    parts.extend(message.session.clone());
    if let Some(card) = &message.card {
        parts.push(format!("card {card}"));
    }
    parts.join(" · ")
}

/// The trimmed text, or none when nothing is left of it.
pub fn non_blank(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The toast an answer shows: the `done` text the hub wrote, or `Answered` when nothing else follows.
pub fn done_text(done: &str) -> String {
    non_blank(done).unwrap_or_else(|| "Answered".into())
}

/// The project a new chat opens with: the scope's, when it is one of the live projects; otherwise the first of them.
pub fn preselect_project(live: &[String], scope: Option<&str>) -> Option<String> {
    match scope {
        Some(scope) if live.iter().any(|id| id == scope) => Some(scope.to_string()),
        _ => live.first().cloned(),
    }
}

/// The chat a `chat` frame is about (`{chat, opened}` or `{chat, message}`), or none for any other data.
pub fn frame_chat(data: &Value) -> Option<String> {
    data["chat"].as_str().map(String::from)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn choice(id: &str, label: &str, takes_amount: bool) -> CardChoice {
        CardChoice {
            id: id.into(),
            label: label.into(),
            takes_amount,
            ..Default::default()
        }
    }

    #[test]
    fn a_card_kind_reads_in_words_and_start_reads_as_a_project() {
        assert_eq!(card_kind_label("start"), "Start a project");
        assert_eq!(card_kind_label("tool_call"), "Tool call");
        assert_eq!(card_kind_label("budget"), "Budget");
        assert_eq!(card_kind_label("seat_release"), "Seat release");
    }

    #[test]
    fn the_kinds_that_spend_or_stop_something_are_marked() {
        assert_eq!(kind_tone("incident"), Tone::Danger);
        assert_eq!(kind_tone("budget"), Tone::Warn);
        assert_eq!(kind_tone("start"), Tone::Muted);
        assert!(is_danger_choice("stop"));
        assert!(is_danger_choice("reject"));
        assert!(!is_danger_choice("approve"));
    }

    #[test]
    fn an_answer_s_amount_is_micro_dollars_for_dollars_and_tokens_for_tokens() {
        assert_eq!(amount_wire(AmountUnit::Dollars, "2.50"), Ok(2_500_000));
        assert_eq!(amount_wire(AmountUnit::Tokens, "500,000"), Ok(500_000));
        assert_eq!(
            amount_wire(AmountUnit::Dollars, "0"),
            Err("The amount is more than zero.".into())
        );
        assert_eq!(
            amount_wire(AmountUnit::Tokens, "0"),
            Err("The amount is more than zero.".into())
        );
        assert!(amount_wire(AmountUnit::Dollars, "two").is_err());
    }

    #[test]
    fn an_amount_is_refused_while_its_budget_is_not_read() {
        assert_eq!(
            amount_wire(AmountUnit::Unknown, "5"),
            Err("The budget could not be read, so the unit of the amount is not known.".into())
        );
        assert_eq!(amount_unit_name(AmountUnit::Unknown), "unit not known");
        assert_eq!(amount_unit_name(AmountUnit::Tokens), "tokens");
    }

    #[test]
    fn a_settled_card_says_what_it_was_answered_with() {
        let choices = vec![
            choice("approve", "Approve", false),
            choice("deny", "Deny", false),
        ];
        assert_eq!(
            settled_text("answered", Some("deny"), &choices),
            "Answered: Deny"
        );
        assert_eq!(settled_text("answered", Some("gone"), &choices), "Answered");
        assert_eq!(settled_text("withdrawn", None, &choices), "Withdrawn");
        assert_eq!(settled_text("open", None, &choices), "");
    }

    #[test]
    fn an_author_is_you_the_plaza_or_the_seat_s_name() {
        assert_eq!(author_label("owner", None), "You");
        assert_eq!(author_label("system", None), "Plaza");
        assert_eq!(author_label("session", Some("builder")), "Builder");
        assert_eq!(author_label("session", None), "Session");
    }

    #[test]
    fn a_chat_is_named_by_its_kind() {
        assert_eq!(chat_kind_label("main"), "Orchestrator");
        assert_eq!(chat_kind_label("task"), "Task");
        assert_eq!(chat_kind_label("project"), "Project");
    }

    #[test]
    fn the_last_messages_come_oldest_first() {
        let messages: Vec<ChatMessage> = [3u64, 1, 4, 2]
            .into_iter()
            .map(|seq| ChatMessage {
                seq,
                ..Default::default()
            })
            .collect();
        let seqs: Vec<u64> = recent_messages(&messages, 3)
            .iter()
            .map(|m| m.seq)
            .collect();
        assert_eq!(seqs, [2, 3, 4]);
        assert_eq!(recent_messages(&messages, 9).len(), 4);
        assert_eq!(messages[0].seq, 3, "the messages are not reordered");
    }

    #[test]
    fn a_message_time_reads_hours_and_minutes() {
        assert_eq!(clock_text(0), "00:00");
        assert_eq!(clock_text(3_723_000), "01:02");
    }

    #[test]
    fn the_expert_lines_name_the_raw_ids() {
        let card = Card {
            id: "c7".into(),
            kind: "budget".into(),
            subject: "task t3".into(),
            choices: vec![
                choice("raise", "Raise", true),
                choice("stop", "Stop", false),
            ],
            ..Default::default()
        };
        assert_eq!(
            card_expert_line(&card),
            "c7 · budget · task t3 · choices raise, stop"
        );
        let message = ChatMessage {
            seq: 12,
            kind: "question".into(),
            author: "session".into(),
            session: Some("s4".into()),
            card: Some("c7".into()),
            ..Default::default()
        };
        assert_eq!(
            message_expert_line(&message),
            "#12 · question · session · s4 · card c7"
        );
        let notice = ChatMessage {
            seq: 1,
            kind: "notice".into(),
            author: "system".into(),
            ..Default::default()
        };
        assert_eq!(message_expert_line(&notice), "#1 · notice · system");
    }

    #[test]
    fn blank_text_is_none_and_an_answer_without_words_says_answered() {
        assert_eq!(non_blank("   "), None);
        assert_eq!(non_blank(" hi "), Some("hi".into()));
        assert_eq!(done_text(""), "Answered");
        assert_eq!(done_text("Budget raised"), "Budget raised");
    }

    #[test]
    fn a_new_chat_opens_with_the_scope_s_project_when_it_is_live() {
        let live = vec!["pr1".to_string(), "pr2".to_string()];
        assert_eq!(preselect_project(&live, Some("pr2")), Some("pr2".into()));
        assert_eq!(preselect_project(&live, Some("pr9")), Some("pr1".into()));
        assert_eq!(preselect_project(&live, None), Some("pr1".into()));
        assert_eq!(preselect_project(&[], Some("pr1")), None);
    }

    #[test]
    fn a_chat_frame_names_its_chat() {
        assert_eq!(
            frame_chat(&json!({ "chat": "main", "opened": true })),
            Some("main".into())
        );
        assert_eq!(frame_chat(&json!({ "message": 3 })), None);
        assert_eq!(frame_chat(&Value::Null), None);
    }
}
