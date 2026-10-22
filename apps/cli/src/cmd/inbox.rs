//! `agentplaza inbox`: the cards waiting for the owner, the decisions the plaza needs before it goes on: a project
//! started, a budget raised, an allowance, a seat, a release. Each card has its buttons (its choices). A choice that
//! takes an amount takes `--amount`: dollars for a usd budget, whole tokens for a tokens one, in the unit of the card's
//! task's budget (else its project's), as the web console counts it.

use clap::{Args as ClapArgs, Subcommand};
use serde_json::{Value, json};

use crate::cmd::budget::amount_of;
use crate::ctx::{self, Client, Ctx};

#[derive(ClapArgs, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub action: Option<Action>,
}

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Answer a card with one of its choices (its ids are in brackets in `inbox`).
    Answer {
        /// The card's id, `c<n>`.
        card: String,
        /// The choice's id, as the card lists it.
        choice: String,
        /// The amount, for a choice that takes one: dollars (`12.50`), or whole tokens for a tokens budget.
        #[arg(long)]
        amount: Option<String>,
    },
}

pub fn run(ctx: &Ctx, args: Args) -> Result<(), String> {
    match args.action {
        None => list(ctx),
        Some(Action::Answer {
            card,
            choice,
            amount,
        }) => answer(ctx, &card, &choice, amount.as_deref()),
    }
}

/// The open cards, newest first, each with its title, body, what it is about and its buttons.
fn list(ctx: &Ctx) -> Result<(), String> {
    let cards = ctx.client.get("/v1/cards?open=true")?;
    ctx.out(&cards, |cards| {
        let items = cards.as_array().map(Vec::as_slice).unwrap_or_default();
        if items.is_empty() {
            println!("nothing waits for you");
            return;
        }
        for (i, card) in items.iter().enumerate() {
            if i > 0 {
                println!();
            }
            print_card(card);
        }
    });
    Ok(())
}

fn print_card(card: &Value) {
    let about = about_of(card);
    println!(
        "{}  {}  {}",
        ctx::s(card, "id"),
        ctx::s(card, "kind"),
        ctx::s(card, "title")
    );
    if !about.is_empty() {
        println!("  about {}", about.join(" · "));
    }
    for line in ctx::s(card, "body").lines() {
        println!("  {line}");
    }
    for choice in card
        .get("choices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        println!("  {}", choice_line(choice));
    }
}

/// What a card is about, as the names of its project, task and chat that it has.
fn about_of(card: &Value) -> Vec<&str> {
    ["project", "task", "chat"]
        .iter()
        .map(|key| ctx::s(card, key))
        .filter(|s| !s.is_empty())
        .collect()
}

/// One button as `inbox` shows it: its id in brackets, its label, the detail after a dash, and a note when it takes an
/// amount.
fn choice_line(choice: &Value) -> String {
    let mut line = format!("[{}] {}", ctx::s(choice, "id"), ctx::s(choice, "label"));
    if choice.get("takes_amount").and_then(Value::as_bool) == Some(true) {
        line.push_str(" (takes --amount)");
    }
    let detail = ctx::s(choice, "detail");
    if !detail.is_empty() {
        line.push_str(" — ");
        line.push_str(detail);
    }
    line
}

/// Answers a card with one of its choices, as `answer_card` does, and prints what the answer decided done.
fn answer(ctx: &Ctx, card_id: &str, choice: &str, amount: Option<&str>) -> Result<(), String> {
    let answered = answer_card(&ctx.client, card_id, choice, amount)?;
    ctx.out(&answered, |a| {
        let done = ctx::s(a, "done");
        if done.is_empty() {
            println!("answered {card_id} with `{choice}`");
        } else {
            println!("{done}");
        }
    });
    Ok(())
}

/// Answers a card with one of its choices. The choice is checked against the card first: an amount is asked for when
/// the choice takes one, and refused otherwise; the amount is in the unit of the card's budget. The hub's answer comes
/// back as its card and what the answer decided done.
pub fn answer_card(
    client: &Client,
    card_id: &str,
    choice: &str,
    amount: Option<&str>,
) -> Result<Value, String> {
    let card = client.get(&format!("/v1/cards/{}", ctx::seg(card_id)))?;
    let body = match checked(&card, choice, amount)? {
        Ask::Plain => json!({ "choice": choice }),
        Ask::Amount(text) => {
            let unit = unit_of(client, &card)?;
            json!({ "choice": choice, "amount": wire_amount(unit.as_deref(), text)? })
        }
    };
    client.post(&format!("/v1/cards/{}", ctx::seg(card_id)), &body)
}

/// What a card's choice asks for, once checked against the card: nothing more, or the amount as it was typed.
#[derive(Debug, PartialEq, Eq)]
enum Ask<'a> {
    Plain,
    Amount(&'a str),
}

/// Checks a choice of a card and the amount given with it: the choice must be one of the card's, a choice that takes an
/// amount must be given one, and one that takes none must not be given one.
fn checked<'a>(card: &Value, choice: &str, amount: Option<&'a str>) -> Result<Ask<'a>, String> {
    let picked = choice_of(card, choice).ok_or_else(|| {
        format!(
            "card {} has no choice `{choice}`: its choices are {}",
            ctx::s(card, "id"),
            choice_ids(card)
        )
    })?;
    let takes_amount = picked.get("takes_amount").and_then(Value::as_bool) == Some(true);
    match (takes_amount, amount) {
        (true, None) => Err(format!(
            "choice `{choice}` takes an amount: pass --amount (dollars, or whole tokens for a tokens budget)"
        )),
        (false, Some(_)) => Err(format!("choice `{choice}` takes no amount")),
        (true, Some(text)) => Ok(Ask::Amount(text)),
        (false, None) => Ok(Ask::Plain),
    }
}

/// The choice of a card with this id.
fn choice_of<'a>(card: &'a Value, id: &str) -> Option<&'a Value> {
    card.get("choices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|c| ctx::s(c, "id") == id)
}

/// The ids of a card's choices, comma-separated, for a refusal.
fn choice_ids(card: &Value) -> String {
    card.get("choices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|c| ctx::s(c, "id"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// An amount for a card's choice in the unit of its budget: dollars when the budget has no unit, else tokens or dollars.
fn wire_amount(unit: Option<&str>, text: &str) -> Result<u64, String> {
    amount_of(unit.unwrap_or("usd"), text)
}

/// The budget an amount for a card is counted in: the card's task's, else its project's (the web console's rule).
fn unit_of(client: &Client, card: &Value) -> Result<Option<String>, String> {
    let Some(path) = budget_path(card) else {
        return Ok(None);
    };
    let budget = client.get(&path)?;
    Ok(budget
        .get("unit")
        .and_then(Value::as_str)
        .map(str::to_string))
}

/// The budget read for a card's amount: its task's, else its project's; None when it has neither.
fn budget_path(card: &Value) -> Option<String> {
    match (ctx::s(card, "task"), ctx::s(card, "project")) {
        ("", "") => None,
        ("", project) => Some(format!("/v1/projects/{}/budget", ctx::seg(project))),
        (task, _) => Some(format!("/v1/tasks/{}/budget", ctx::seg(task))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_button_says_its_id_label_detail_and_whether_it_takes_an_amount() {
        let raise = json!({ "id": "raise", "label": "Raise the cap", "detail": "more money", "takes_amount": true });
        assert_eq!(
            choice_line(&raise),
            "[raise] Raise the cap (takes --amount) — more money"
        );
        let start = json!({ "id": "start", "label": "Start the project", "detail": null, "takes_amount": false });
        assert_eq!(choice_line(&start), "[start] Start the project");
    }

    #[test]
    fn a_card_is_about_its_project_task_and_chat_where_it_has_them() {
        let card = json!({ "id": "c7", "project": "pr3", "task": null, "chat": "main" });
        assert_eq!(about_of(&card), ["pr3", "main"]);
        assert!(about_of(&json!({ "project": null, "task": null, "chat": null })).is_empty());
    }

    #[test]
    fn a_choice_is_checked_against_its_card_and_its_amount() {
        let card = json!({ "id": "c7", "choices": [
            { "id": "start", "label": "Start", "takes_amount": false },
            { "id": "raise", "label": "Raise", "takes_amount": true }
        ]});
        assert_eq!(checked(&card, "start", None), Ok(Ask::Plain));
        assert_eq!(
            checked(&card, "raise", Some("12.5")),
            Ok(Ask::Amount("12.5"))
        );
        assert!(
            checked(&card, "raise", None)
                .unwrap_err()
                .contains("takes an amount")
        );
        assert_eq!(
            checked(&card, "start", Some("3")),
            Err("choice `start` takes no amount".to_string())
        );
        assert_eq!(
            checked(&card, "stop", None),
            Err("card c7 has no choice `stop`: its choices are start, raise".to_string())
        );
    }

    #[test]
    fn a_choice_is_found_on_its_card_or_refused_with_the_ones_it_has() {
        let card = json!({ "id": "c7", "choices": [
            { "id": "start", "label": "Start", "takes_amount": false },
            { "id": "raise", "label": "Raise", "takes_amount": true }
        ]});
        assert_eq!(
            choice_of(&card, "raise").map(|c| ctx::s(c, "label")),
            Some("Raise")
        );
        assert!(choice_of(&card, "stop").is_none());
        assert_eq!(choice_ids(&card), "start, raise");
        assert_eq!(choice_ids(&json!({ "choices": [] })), "");
    }

    #[test]
    fn an_amount_is_dollars_unless_the_cards_budget_counts_tokens() {
        assert_eq!(
            wire_amount(None, "12.5"),
            Ok(12_500_000),
            "no budget: dollars"
        );
        assert_eq!(wire_amount(Some("usd"), "3"), Ok(3_000_000));
        assert_eq!(wire_amount(Some("tokens"), "4000"), Ok(4000));
        assert!(
            wire_amount(Some("tokens"), "4.5").is_err(),
            "a token amount is whole"
        );
        assert!(
            wire_amount(None, "0").is_err(),
            "an amount is more than zero"
        );
    }

    #[test]
    fn an_amount_is_read_in_the_unit_of_the_cards_budget() {
        let in_task = json!({ "id": "c1", "task": "t4", "project": "pr3" });
        assert_eq!(
            budget_path(&in_task).as_deref(),
            Some("/v1/tasks/t4/budget")
        );
        let in_project = json!({ "id": "c2", "task": null, "project": "pr3" });
        assert_eq!(
            budget_path(&in_project).as_deref(),
            Some("/v1/projects/pr3/budget")
        );
        let neither = json!({ "id": "c3", "task": null, "project": null });
        assert_eq!(
            budget_path(&neither),
            None,
            "dollars when the card has no budget"
        );
    }
}
