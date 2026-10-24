use serde_json::json;

use super::*;
use crate::console::client::Client;

fn ctx() -> Ctx {
    Ctx {
        client: Client::new("http://127.0.0.1:1", None, None),
        project: None,
    }
}

/// An open card with a choice that takes an amount ("raise") and one that does not ("stop").
fn card(id: &str) -> Card {
    Card {
        id: id.into(),
        kind: "budget".into(),
        title: "Raise the cap?".into(),
        state: "open".into(),
        choices: vec![
            CardChoice {
                id: "raise".into(),
                label: "Raise".into(),
                takes_amount: true,
                ..Default::default()
            },
            CardChoice {
                id: "stop".into(),
                label: "Stop".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

fn with_card(id: &str, unit: Option<AmountUnit>) -> State {
    let mut state = State {
        cards: vec![card(id)],
        loaded: true,
        ..Default::default()
    };
    if let Some(unit) = unit {
        state
            .answers
            .set_units(HashMap::from([(id.to_string(), unit)]));
    }
    state
}

#[test]
fn an_amount_is_kept_until_it_is_cleared() {
    let ctx = ctx();
    let mut state = with_card("c1", Some(AmountUnit::Dollars));
    let _ = update(&mut state, Msg::Amount("c1".into(), "2.50".into()), &ctx);
    assert_eq!(
        state.answers.amounts.get("c1").map(String::as_str),
        Some("2.50")
    );
    let _ = update(&mut state, Msg::Amount("c1".into(), String::new()), &ctx);
    assert!(!state.answers.amounts.contains_key("c1"));
}

#[test]
fn a_choice_that_takes_an_amount_is_refused_without_one_and_nothing_is_sent() {
    let ctx = ctx();
    let mut state = with_card("c1", Some(AmountUnit::Dollars));
    let _ = update(&mut state, Msg::Choose("c1".into(), "raise".into()), &ctx);
    assert_eq!(
        state.note.as_deref(),
        Some("Enter the amount in dollars, like 25 or 2.50.")
    );
    assert!(!state.answers.busy("c1"), "a refused answer is not sent");
}

#[test]
fn a_choice_with_an_amount_is_sent_and_its_buttons_are_off_meanwhile() {
    let ctx = ctx();
    let mut state = with_card("c1", Some(AmountUnit::Tokens));
    let _ = update(&mut state, Msg::Amount("c1".into(), "500,000".into()), &ctx);
    let _ = update(&mut state, Msg::Choose("c1".into(), "raise".into()), &ctx);
    assert!(state.answers.busy("c1"));
    assert!(state.note.is_none());
}

#[test]
fn a_card_whose_budget_did_not_read_refuses_its_amount() {
    let ctx = ctx();
    let mut state = with_card("c1", None);
    let _ = update(&mut state, Msg::Amount("c1".into(), "5".into()), &ctx);
    let _ = update(&mut state, Msg::Choose("c1".into(), "raise".into()), &ctx);
    assert_eq!(
        state.note.as_deref(),
        Some("The budget could not be read, so the unit of the amount is not known.")
    );
    assert!(!state.answers.busy("c1"));
}

#[test]
fn a_choice_that_takes_no_amount_is_sent_at_once() {
    let ctx = ctx();
    let mut state = with_card("c1", None);
    let _ = update(&mut state, Msg::Choose("c1".into(), "stop".into()), &ctx);
    assert!(state.answers.busy("c1"));
    assert!(state.note.is_none());
}

#[test]
fn an_answer_takes_the_card_off_the_list_and_says_what_was_done() {
    let ctx = ctx();
    let mut state = with_card("c1", None);
    state.answers.set_amount("c1", "9".into());
    let answer = CardAnswer {
        card: Card {
            state: "answered".into(),
            ..card("c1")
        },
        done: "Budget raised".into(),
    };
    let _ = update(&mut state, Msg::Answered("c1".into(), Ok(answer)), &ctx);
    assert!(state.cards.is_empty());
    assert_eq!(state.note.as_deref(), Some("Budget raised"));
    assert!(!state.answers.busy("c1"));
    assert!(!state.answers.amounts.contains_key("c1"));
}

#[test]
fn a_refused_answer_frees_the_card_and_says_why() {
    let ctx = ctx();
    let mut state = with_card("c1", None);
    state.answers.set_units(HashMap::new());
    let _ = update(&mut state, Msg::Choose("c1".into(), "stop".into()), &ctx);
    let _ = update(
        &mut state,
        Msg::Answered("c1".into(), Err("That card is already answered.".into())),
        &ctx,
    );
    assert!(!state.answers.busy("c1"));
    assert_eq!(
        state.note.as_deref(),
        Some("That card is already answered.")
    );
    assert_eq!(
        state.cards.len(),
        1,
        "the card stays until the reload settles it"
    );
}

#[test]
fn a_card_frame_reloads_the_inbox_and_other_frames_do_not() {
    let ctx = ctx();
    let mut state = State::default();
    let _ = update(
        &mut state,
        Msg::Event(events::Event {
            kind: "session".into(),
            data: json!({}),
        }),
        &ctx,
    );
    assert!(!state.loading);
    let _ = update(
        &mut state,
        Msg::Event(events::Event {
            kind: "card".into(),
            data: json!({}),
        }),
        &ctx,
    );
    assert!(state.loading);
}

#[test]
fn a_failed_read_is_marked_and_a_good_one_clears_it() {
    let ctx = ctx();
    let mut state = State::default();
    let _ = update(&mut state, Msg::Loaded(Err("hub down".into())), &ctx);
    assert!(state.loaded && state.failed);
    let _ = update(
        &mut state,
        Msg::Loaded(Ok(Loaded {
            cards: vec![card("c1")],
            units: HashMap::from([("c1".into(), AmountUnit::Dollars)]),
        })),
        &ctx,
    );
    assert!(!state.failed);
    assert_eq!(state.cards.len(), 1);
    assert!(state.answers.units.contains_key("c1"));
}

#[test]
fn a_card_s_link_goes_to_its_chat_or_session() {
    assert_eq!(
        go(&Msg::Go(Id::Chat("main".into()))),
        Some(Id::Chat("main".into()))
    );
    assert_eq!(go(&Msg::Expert(true)), None);
}

#[test]
fn an_answer_on_a_card_of_the_chat_page_settles_it_in_place() {
    let mut answers = Answers::default();
    answers.set_amount("c1", "3".into());
    answers.settled("c1", true);
    assert!(!answers.amounts.contains_key("c1"));
    assert!(!answers.busy("c1"));
}
