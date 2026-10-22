use serde_json::json;

use super::*;
use crate::console::client::Client;

fn ctx() -> Ctx {
    Ctx {
        client: Client::new("http://127.0.0.1:1", None, None),
        project: None,
    }
}

fn event(kind: &str, data: serde_json::Value) -> events::Event {
    events::Event {
        kind: kind.into(),
        data,
    }
}

fn budget(unit: BudgetUnit, cap: i64, spent: i64) -> Budget {
    Budget {
        owner: "pr1".into(),
        unit: Some(unit),
        cap: Some(cap),
        spent,
        ..Default::default()
    }
}

fn read(budget: Budget) -> Read {
    Read {
        budget,
        ledger: Vec::new(),
    }
}

#[test]
fn a_panel_shows_the_budget_of_its_owner_only() {
    let panel = Panel::new(OwnerKind::Project, "pr1");
    assert!(panel.owned_by(OwnerKind::Project, "pr1"));
    assert!(!panel.owned_by(OwnerKind::Task, "pr1"));
    assert!(!panel.owned_by(OwnerKind::Project, "pr2"));
}

#[test]
fn a_budget_frame_of_its_owner_reloads_the_panel_and_another_does_not() {
    let ctx = ctx();
    let mut panel = Panel::new(OwnerKind::Project, "pr1");
    let _ = update(
        &mut panel,
        Msg::Event(event(
            "budget",
            json!({ "owner": "pr1", "owner_kind": "task" }),
        )),
        &ctx,
    );
    assert!(!panel.loading, "a task's budget is not the project's");
    let _ = update(
        &mut panel,
        Msg::Event(event(
            "budget",
            json!({ "owner": "pr1", "owner_kind": "project" }),
        )),
        &ctx,
    );
    assert!(panel.loading);
}

#[test]
fn a_session_or_a_card_of_the_owner_reloads_the_panel() {
    let ctx = ctx();
    let mut panel = Panel::new(OwnerKind::Task, "t3");
    let _ = update(
        &mut panel,
        Msg::Event(event("session", json!({ "task": "t9" }))),
        &ctx,
    );
    assert!(!panel.loading);
    let _ = update(
        &mut panel,
        Msg::Event(event("session", json!({ "task": "t3" }))),
        &ctx,
    );
    assert!(panel.loading);
    let mut again = Panel::new(OwnerKind::Project, "pr1");
    let _ = update(
        &mut again,
        Msg::Event(event("card", json!({ "project": "pr1" }))),
        &ctx,
    );
    assert!(again.loading);
}

#[test]
fn a_read_sets_the_budget_and_a_failed_one_keeps_the_figures() {
    let ctx = ctx();
    let mut panel = Panel::new(OwnerKind::Task, "t3");
    let _ = update(
        &mut panel,
        Msg::Loaded(Ok(read(budget(BudgetUnit::Usd, 25_000_000, 1_500_000)))),
        &ctx,
    );
    let _ = update(&mut panel, Msg::Loaded(Err("hub down".into())), &ctx);
    assert_eq!(panel.budget.as_ref().map(|b| b.spent), Some(1_500_000));
    assert_eq!(panel.failure.as_deref(), Some("hub down"));
}

#[test]
fn opening_the_form_starts_from_the_budget_and_a_unit_change_clears_the_caps() {
    let ctx = ctx();
    let mut panel = Panel::new(OwnerKind::Task, "t3");
    let _ = update(
        &mut panel,
        Msg::Loaded(Ok(read(budget(BudgetUnit::Tokens, 500_000, 0)))),
        &ctx,
    );
    let _ = update(&mut panel, Msg::OpenForm, &ctx);
    let form = panel.form.clone().expect("the form is open");
    assert_eq!(
        (form.unit, form.cap.as_str()),
        (BudgetUnit::Tokens, "500000")
    );
    let _ = update(&mut panel, Msg::FormUnit(UnitChoice(BudgetUnit::Usd)), &ctx);
    let form = panel.form.clone().unwrap();
    assert_eq!(form.unit, BudgetUnit::Usd);
    assert!(
        form.cap.is_empty(),
        "the caps were in the old unit, so they are cleared"
    );
}

#[test]
fn a_save_that_blanks_a_cap_by_changing_its_unit_is_refused() {
    let ctx = ctx();
    let mut panel = Panel::new(OwnerKind::Task, "t3");
    let _ = update(
        &mut panel,
        Msg::Loaded(Ok(read(budget(BudgetUnit::Usd, 25_000_000, 0)))),
        &ctx,
    );
    panel.form = Some(Form {
        unit: BudgetUnit::Tokens,
        cap: String::new(),
        warn_pct: "80".into(),
        monthly_cap: String::new(),
    });
    let _ = update(&mut panel, Msg::Save, &ctx);
    assert_eq!(
        panel.note.as_deref(),
        Some("Type the cap again in the new unit, or keep the unit.")
    );
    assert!(!panel.saving);
}

#[test]
fn a_save_with_an_amount_that_is_not_one_says_why() {
    let ctx = ctx();
    let mut panel = Panel::new(OwnerKind::Task, "t3");
    panel.form = Some(Form {
        unit: BudgetUnit::Usd,
        cap: "lots".into(),
        warn_pct: "80".into(),
        monthly_cap: String::new(),
    });
    let _ = update(&mut panel, Msg::Save, &ctx);
    assert_eq!(
        panel.note.as_deref(),
        Some("Enter the amount in dollars, like 25 or 2.50.")
    );
}

#[test]
fn a_saved_budget_closes_the_form_and_says_so() {
    let ctx = ctx();
    let mut panel = Panel::new(OwnerKind::Task, "t3");
    panel.form = Some(Form {
        unit: BudgetUnit::Usd,
        cap: "25".into(),
        warn_pct: "80".into(),
        monthly_cap: String::new(),
    });
    panel.saving = true;
    let _ = update(&mut panel, Msg::Saved(Ok(())), &ctx);
    assert!(panel.form.is_none());
    assert!(!panel.saving);
    assert_eq!(panel.note.as_deref(), Some("Budget saved"));
}
