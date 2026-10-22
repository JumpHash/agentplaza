//! The rules and words of the work engine's budgets and seats, as the web's `apps/web/src/views/budget-model.ts` has them:
//! a budget's money as dollars (micro-dollars on the wire) or a count of tokens, the set-budget form read into the body
//! the hub takes, the ledger's and the seats' labels, and the test that a live frame is for this owner or this task. No
//! drawing here, so the budget panel and the tests below say the same things as the web.

use serde_json::{Value, json};

use crate::console::work_api::{Budget, BudgetUnit, ModelPolicy, OwnerKind, Seat, SeatState};

/// Micro-dollars in a dollar: a `usd` amount on the wire is dollars times this.
pub const MICROS: i64 = 1_000_000;

/// The share of a cap at which the owner is told, when the budget names none.
pub const DEFAULT_WARN_PCT: u32 = 80;

/// The largest whole number a JavaScript number holds exactly: an amount past it is refused, as the web refuses it.
const MAX_SAFE: i64 = 9_007_199_254_740_991;

/// The unit names a set-budget form offers, in its order.
pub const UNIT_OPTIONS: [(BudgetUnit, &str); 2] =
    [(BudgetUnit::Usd, "Dollars"), (BudgetUnit::Tokens, "Tokens")];

/// The set-budget form's fields, as typed: the unit, the cap (blank for none), the warn share and the monthly cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    pub unit: BudgetUnit,
    pub cap: String,
    pub warn_pct: String,
    pub monthly_cap: String,
}

// ---- Money and counts -------------------------------------------------------

/// Digits in groups of three: `1234567` is `1,234,567`; a negative keeps its sign.
pub fn grouped(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// Dollars from micro-dollars: `1500000` is `$1.50`. Cents round half up; a negative amount reads `-$0.50`.
pub fn dollars_text(micros: i64) -> String {
    let cents = (micros.unsigned_abs() + 5_000) / 10_000;
    let sign = if micros < 0 && cents > 0 { "-" } else { "" };
    format!(
        "{sign}${}.{:02}",
        grouped((cents / 100) as i64),
        cents % 100
    )
}

/// An amount in its budget's unit: dollars for `usd`, a grouped count of tokens otherwise.
pub fn amount_text(unit: BudgetUnit, amount: i64) -> String {
    match unit {
        BudgetUnit::Usd => dollars_text(amount),
        BudgetUnit::Tokens => format!("{} tokens", grouped(amount)),
    }
}

/// A limit's text: its amount, or "No cap" when it has none.
pub fn cap_text(unit: BudgetUnit, cap: Option<i64>) -> String {
    cap.map_or_else(|| "No cap".to_string(), |cap| amount_text(unit, cap))
}

/// The unit's name as a budget shows it.
pub fn unit_name(unit: BudgetUnit) -> &'static str {
    match unit {
        BudgetUnit::Usd => "Dollars",
        BudgetUnit::Tokens => "Tokens",
    }
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// The month a `yyyymm` number names: `202610` is "October 2026"; a number that is no month reads as itself.
pub fn month_text(yyyymm: u32) -> String {
    let month = (yyyymm % 100) as usize;
    match MONTHS.get(month.wrapping_sub(1)) {
        Some(name) => format!("{name} {}", yyyymm / 100),
        None => yyyymm.to_string(),
    }
}

/// The share of the cap spent, with the share the owner is told at: "21% of the cap · warns at 80%". A cap of zero
/// reads "The cap is zero"; an uncapped budget reads "No cap".
pub fn share_text(cap: Option<i64>, spent_pct: Option<f64>, warn_at: Option<u32>) -> String {
    if cap == Some(0) {
        return "The cap is zero".into();
    }
    match spent_pct {
        Some(pct) => format!(
            "{:.0}% of the cap · warns at {}%",
            pct.round(),
            warn_at.unwrap_or(DEFAULT_WARN_PCT)
        ),
        None => "No cap".into(),
    }
}

// ---- The set-budget form ----------------------------------------------------

/// A cap as the form shows it: dollars with the decimals it has, or a plain count of tokens; blank for none.
pub fn cap_input(unit: BudgetUnit, value: Option<i64>) -> String {
    match (unit, value) {
        (_, None) => String::new(),
        (BudgetUnit::Tokens, Some(value)) => value.to_string(),
        (BudgetUnit::Usd, Some(value)) => {
            let (whole, rest) = (value / MICROS, value % MICROS);
            if rest == 0 {
                whole.to_string()
            } else {
                let fraction = format!("{rest:06}");
                format!("{whole}.{}", fraction.trim_end_matches('0'))
            }
        }
    }
}

/// The form's start: the budget's own values, or the defaults (dollars, no cap, the default warn share) when it has
/// none.
pub fn form_of(budget: Option<&Budget>) -> Form {
    let unit = budget.and_then(|b| b.unit).unwrap_or(BudgetUnit::Usd);
    Form {
        unit,
        cap: cap_input(unit, budget.and_then(|b| b.cap)),
        warn_pct: budget
            .and_then(|b| b.warn_at_pct)
            .unwrap_or(DEFAULT_WARN_PCT)
            .to_string(),
        monthly_cap: cap_input(unit, budget.and_then(|b| b.monthly_cap)),
    }
}

/// Whether a budget's unit is fixed: spend, a call in flight or a reservation is counted in the unit it was set in.
pub fn unit_locked(budget: &Budget) -> bool {
    budget.spent > 0 || budget.held > 0 || budget.reserved > 0
}

/// Whether saving a form drops a cap the budget has: a unit change that leaves the cap blank. The cap was in the old unit
/// and is not carried over, so it is typed again in the new one. A blank cap in the same unit is a deliberate "no cap".
pub fn drops_cap(
    stored_unit: Option<BudgetUnit>,
    stored_cap: Option<i64>,
    unit: BudgetUnit,
    cap_text: &str,
) -> bool {
    stored_cap.is_some() && stored_unit != Some(unit) && cap_text.trim().is_empty()
}

/// A typed amount with its commas taken out. Commas may only group thousands (`1,000`, `1,000.50`); a comma anywhere
/// else (`2,5`, `5,00`, `1,2,3`) gives none.
fn ungrouped(text: &str) -> Option<String> {
    if !text.contains(',') {
        return Some(text.to_string());
    }
    let (whole, fraction) = match text.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (text, None),
    };
    let groups: Vec<&str> = whole.split(',').collect();
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let first_ok = (1..=3).contains(&groups[0].len()) && digits(groups[0]);
    let rest_ok = groups.len() >= 2 && groups[1..].iter().all(|&g| g.len() == 3 && digits(g));
    let fraction_ok = fraction.is_none_or(digits);
    (first_ok && rest_ok && fraction_ok).then(|| text.replace(',', ""))
}

/// Dollars typed in a form (`2.5`, `$1,200`) as micro-dollars: at most six decimals, and never negative. The whole part
/// and the fraction are read as integers, so no rounding moves an amount.
pub fn parse_dollars(text: &str) -> Result<i64, String> {
    let typed = text.trim();
    let typed = typed.strip_prefix('$').unwrap_or(typed);
    if typed.starts_with('-') {
        return Err("The amount is 0 or more dollars.".into());
    }
    let s = ungrouped(typed).ok_or("Commas only group thousands, like 1,000.")?;
    let (whole, fraction) = s.split_once('.').unwrap_or((s.as_str(), ""));
    let digits = |t: &str| t.bytes().all(|b| b.is_ascii_digit());
    let shape = "Enter the amount in dollars, like 25 or 2.50.";
    if (whole.is_empty() && fraction.is_empty()) || !digits(whole) || !digits(fraction) {
        return Err(shape.into());
    }
    if fraction.len() > 6 {
        return Err("Dollars take at most six decimals.".into());
    }
    let whole: i64 = if whole.is_empty() {
        0
    } else {
        whole.parse().map_err(|_| "That amount is too large.")?
    };
    let fraction: i64 = format!("{fraction:0<6}").parse().unwrap_or(0);
    whole
        .checked_mul(MICROS)
        .and_then(|micros| micros.checked_add(fraction))
        .filter(|micros| *micros <= MAX_SAFE)
        .ok_or_else(|| "That amount is too large.".into())
}

/// A whole count of tokens typed in a form; commas may only group thousands.
pub fn parse_tokens(text: &str) -> Result<i64, String> {
    let s = ungrouped(text.trim())
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .ok_or("Enter a whole number of tokens, like 500000.")?;
    s.parse::<i64>()
        .ok()
        .filter(|n| *n <= MAX_SAFE)
        .ok_or_else(|| "That many tokens is too large.".into())
}

/// A cap typed for `unit`: blank is no cap (none); otherwise dollars for `usd` and whole tokens for `tokens`.
pub fn parse_cap(unit: BudgetUnit, text: &str) -> Result<Option<i64>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    match unit {
        BudgetUnit::Usd => parse_dollars(text).map(Some),
        BudgetUnit::Tokens => parse_tokens(text).map(Some),
    }
}

/// The warn share typed in a form: blank is the default; otherwise a whole percent from 1 to 100.
pub fn parse_warn_pct(text: &str) -> Result<u32, String> {
    let s = text.trim();
    if s.is_empty() {
        return Ok(DEFAULT_WARN_PCT);
    }
    match s.parse::<u32>() {
        Ok(n) if (1..=100).contains(&n) && s.bytes().all(|b| b.is_ascii_digit()) => Ok(n),
        _ => Err("Warn at a whole percent from 1 to 100.".into()),
    }
}

/// A monthly cap typed for `unit`: blank is none; otherwise more than zero.
fn parse_monthly_cap(unit: BudgetUnit, text: &str) -> Result<Option<i64>, String> {
    match parse_cap(unit, text)? {
        Some(0) => Err("The monthly cap is more than zero, or blank for none.".into()),
        cap => Ok(cap),
    }
}

/// The write's body from the form. A task's takes only the unit and the cap; a project's also takes the warn share and
/// the monthly cap (blank: none), both in the project's unit.
pub fn budget_body(kind: OwnerKind, form: &Form) -> Result<Value, String> {
    let cap = parse_cap(form.unit, &form.cap)?;
    match kind {
        OwnerKind::Task => Ok(json!({ "unit": form.unit.key(), "cap": cap })),
        OwnerKind::Project => {
            let warn = parse_warn_pct(&form.warn_pct)?;
            let monthly = parse_monthly_cap(form.unit, &form.monthly_cap)?;
            Ok(json!({
                "unit": form.unit.key(),
                "cap": cap,
                "warn_at_pct": warn,
                "monthly_cap": monthly,
            }))
        }
    }
}

// ---- Words ------------------------------------------------------------------

/// What a ledger entry records, in words; a kind the console does not know shows as it came.
pub fn ledger_label(kind: &str) -> String {
    match kind {
        "set" => "Set",
        "reserve" => "Reserved",
        "hold" => "Held for a call",
        "charge" => "Charged",
        "release" => "Released",
        "raise" => "Raised",
        other => other,
    }
    .to_string()
}

/// A ledger entry's amount in its budget's unit; a `set` of zero took the cap away, so it reads No cap.
pub fn ledger_amount(unit: BudgetUnit, kind: &str, amount: i64) -> String {
    if kind == "set" && amount == 0 {
        "No cap".into()
    } else {
        amount_text(unit, amount)
    }
}

/// A seat's state in words: a seat with a session in it is working.
pub fn seat_label(state: SeatState) -> &'static str {
    match state {
        SeatState::Waiting => "Waiting",
        SeatState::Ready => "Ready",
        SeatState::Filled => "Working",
        SeatState::Done => "Done",
        SeatState::Failed => "Failed",
    }
}

/// A seat's tries and rounds, when they are past the first: "Attempt 2", "Round 3".
pub fn seat_notes(seat: &Seat) -> Vec<String> {
    let mut notes = Vec::new();
    if seat.attempts > 1 {
        notes.push(format!("Attempt {}", seat.attempts));
    }
    if seat.round > 1 {
        notes.push(format!("Round {}", seat.round));
    }
    notes
}

/// Which models may fill a seat, in words.
pub fn policy_text(policy: ModelPolicy) -> &'static str {
    match policy {
        ModelPolicy::Any => "Any model the project allows",
        ModelPolicy::Differ => "A model no other seat used",
    }
}

// ---- Live frames ------------------------------------------------------------

/// Whether a `budget` frame is the budget of `owner` (`id`): the same id and the same kind.
pub fn is_budget_of(data: &Value, owner: OwnerKind, id: &str) -> bool {
    data["owner"].as_str() == Some(id) && data["owner_kind"].as_str() == Some(owner.key())
}

/// Whether a `task` frame (`{id}`) is about the task `id`.
pub fn is_task_frame(data: &Value, id: &str) -> bool {
    data["id"].as_str() == Some(id)
}

/// Whether a `session` frame works on the task `id`; a long-lived session has no task.
pub fn is_task_session(data: &Value, id: &str) -> bool {
    data["task"].as_str() == Some(id)
}

/// Whether a `session` or `card` frame names the owner: a project's by its project, a task's by its task.
pub fn names_owner(data: &Value, owner: OwnerKind, id: &str) -> bool {
    data[owner.key()].as_str() == Some(id)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn budget(unit: Option<BudgetUnit>, cap: Option<i64>, spent: i64) -> Budget {
        Budget {
            owner: "pr4".into(),
            unit,
            cap,
            spent,
            ..Default::default()
        }
    }

    #[test]
    fn a_count_of_money_or_tokens_is_grouped_in_threes() {
        assert_eq!(grouped(1_234_567), "1,234,567");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(-4_200), "-4,200");
        assert_eq!(grouped(0), "0");
    }

    #[test]
    fn micro_dollars_read_as_dollars_to_the_cent_and_round_half_up() {
        assert_eq!(dollars_text(1_500_000), "$1.50");
        assert_eq!(dollars_text(0), "$0.00");
        assert_eq!(dollars_text(1_234_567_000), "$1,234.57");
        assert_eq!(dollars_text(-500_000), "-$0.50");
        assert_eq!(
            dollars_text(-4_000),
            "$0.00",
            "under a cent a negative reads as zero"
        );
        assert_eq!(dollars_text(5_000), "$0.01");
        assert_eq!(dollars_text(4_999), "$0.00");
    }

    #[test]
    fn an_amount_is_in_its_budget_s_unit_and_a_missing_cap_reads_no_cap() {
        assert_eq!(amount_text(BudgetUnit::Usd, 2_000_000), "$2.00");
        assert_eq!(amount_text(BudgetUnit::Tokens, 500_000), "500,000 tokens");
        assert_eq!(cap_text(BudgetUnit::Usd, None), "No cap");
        assert_eq!(cap_text(BudgetUnit::Tokens, Some(10)), "10 tokens");
        assert_eq!(unit_name(BudgetUnit::Tokens), "Tokens");
    }

    #[test]
    fn a_month_names_itself_and_a_non_month_reads_as_its_number() {
        assert_eq!(month_text(202_610), "October 2026");
        assert_eq!(month_text(202_601), "January 2026");
        assert_eq!(month_text(202_613), "202613");
    }

    #[test]
    fn the_share_says_how_far_the_cap_is_spent_and_when_the_owner_is_told() {
        assert_eq!(
            share_text(Some(10_000_000), Some(21.4), Some(90)),
            "21% of the cap · warns at 90%"
        );
        assert_eq!(
            share_text(Some(10_000_000), Some(5.0), None),
            "5% of the cap · warns at 80%"
        );
        assert_eq!(share_text(Some(0), Some(0.0), None), "The cap is zero");
        assert_eq!(share_text(None, None, None), "No cap");
    }

    #[test]
    fn a_cap_is_shown_in_the_form_as_dollars_with_their_decimals_or_as_tokens() {
        assert_eq!(cap_input(BudgetUnit::Usd, Some(25_000_000)), "25");
        assert_eq!(cap_input(BudgetUnit::Usd, Some(2_500_000)), "2.5");
        assert_eq!(cap_input(BudgetUnit::Usd, Some(1)), "0.000001");
        assert_eq!(cap_input(BudgetUnit::Tokens, Some(500_000)), "500000");
        assert_eq!(cap_input(BudgetUnit::Usd, None), "");
    }

    #[test]
    fn the_form_starts_from_the_budget_or_from_the_defaults() {
        let set = budget(Some(BudgetUnit::Tokens), Some(500_000), 0);
        let form = form_of(Some(&set));
        assert_eq!(form.unit, BudgetUnit::Tokens);
        assert_eq!(form.cap, "500000");
        assert_eq!(form.warn_pct, "80");
        let fresh = form_of(None);
        assert_eq!(
            fresh,
            Form {
                unit: BudgetUnit::Usd,
                cap: String::new(),
                warn_pct: "80".into(),
                monthly_cap: String::new(),
            }
        );
    }

    #[test]
    fn the_unit_is_fixed_once_there_is_spend_or_a_reservation() {
        assert!(!unit_locked(&budget(Some(BudgetUnit::Usd), Some(1), 0)));
        assert!(unit_locked(&budget(Some(BudgetUnit::Usd), Some(1), 5)));
        let mut reserved = budget(Some(BudgetUnit::Usd), Some(1), 0);
        reserved.reserved = 3;
        assert!(unit_locked(&reserved));
    }

    #[test]
    fn a_unit_change_that_blanks_the_cap_drops_it() {
        assert!(drops_cap(
            Some(BudgetUnit::Usd),
            Some(25_000_000),
            BudgetUnit::Tokens,
            ""
        ));
        assert!(!drops_cap(
            Some(BudgetUnit::Usd),
            Some(25_000_000),
            BudgetUnit::Tokens,
            "500000"
        ));
        assert!(!drops_cap(
            Some(BudgetUnit::Usd),
            Some(25_000_000),
            BudgetUnit::Usd,
            ""
        ));
        assert!(!drops_cap(
            Some(BudgetUnit::Usd),
            None,
            BudgetUnit::Tokens,
            ""
        ));
    }

    #[test]
    fn dollars_typed_in_the_form_become_micro_dollars() {
        assert_eq!(parse_dollars("2.5"), Ok(2_500_000));
        assert_eq!(parse_dollars("$1,200"), Ok(1_200_000_000));
        assert_eq!(parse_dollars("1,000.50"), Ok(1_000_500_000));
        assert_eq!(parse_dollars("25."), Ok(25_000_000));
        assert_eq!(parse_dollars(".5"), Ok(500_000));
        assert_eq!(parse_dollars("0.000001"), Ok(1));
    }

    #[test]
    fn a_dollar_amount_that_is_not_one_is_refused_with_the_reason() {
        assert_eq!(
            parse_dollars("-1"),
            Err("The amount is 0 or more dollars.".into())
        );
        assert_eq!(
            parse_dollars("2,5"),
            Err("Commas only group thousands, like 1,000.".into())
        );
        assert_eq!(
            parse_dollars("1,2,3"),
            Err("Commas only group thousands, like 1,000.".into())
        );
        assert_eq!(
            parse_dollars("1.2.3"),
            Err("Enter the amount in dollars, like 25 or 2.50.".into())
        );
        assert_eq!(
            parse_dollars("."),
            Err("Enter the amount in dollars, like 25 or 2.50.".into())
        );
        assert_eq!(
            parse_dollars("abc"),
            Err("Enter the amount in dollars, like 25 or 2.50.".into())
        );
        assert_eq!(
            parse_dollars("1.0000001"),
            Err("Dollars take at most six decimals.".into())
        );
        assert_eq!(
            parse_dollars("99999999999999"),
            Err("That amount is too large.".into())
        );
    }

    #[test]
    fn tokens_typed_in_the_form_are_a_whole_count() {
        assert_eq!(parse_tokens("500000"), Ok(500_000));
        assert_eq!(parse_tokens("1,000,000"), Ok(1_000_000));
        assert_eq!(
            parse_tokens("1.5"),
            Err("Enter a whole number of tokens, like 500000.".into())
        );
        assert_eq!(
            parse_tokens("1,2"),
            Err("Enter a whole number of tokens, like 500000.".into())
        );
        assert_eq!(
            parse_tokens("99999999999999999"),
            Err("That many tokens is too large.".into())
        );
    }

    #[test]
    fn a_blank_cap_is_no_cap_and_any_other_is_read_in_the_unit() {
        assert_eq!(parse_cap(BudgetUnit::Usd, "  "), Ok(None));
        assert_eq!(parse_cap(BudgetUnit::Usd, "25"), Ok(Some(25_000_000)));
        assert_eq!(parse_cap(BudgetUnit::Tokens, "25"), Ok(Some(25)));
    }

    #[test]
    fn the_warn_share_is_a_whole_percent_from_one_to_a_hundred() {
        assert_eq!(parse_warn_pct(""), Ok(80));
        assert_eq!(parse_warn_pct(" 90 "), Ok(90));
        assert_eq!(parse_warn_pct("100"), Ok(100));
        assert_eq!(
            parse_warn_pct("0"),
            Err("Warn at a whole percent from 1 to 100.".into())
        );
        assert_eq!(
            parse_warn_pct("101"),
            Err("Warn at a whole percent from 1 to 100.".into())
        );
        assert_eq!(
            parse_warn_pct("+5"),
            Err("Warn at a whole percent from 1 to 100.".into())
        );
    }

    #[test]
    fn a_task_s_body_takes_only_the_unit_and_the_cap() {
        let form = Form {
            unit: BudgetUnit::Usd,
            cap: "25".into(),
            warn_pct: "90".into(),
            monthly_cap: "100".into(),
        };
        assert_eq!(
            budget_body(OwnerKind::Task, &form),
            Ok(json!({ "unit": "usd", "cap": 25_000_000 }))
        );
    }

    #[test]
    fn a_project_s_body_takes_the_warn_share_and_the_monthly_cap_too() {
        let form = Form {
            unit: BudgetUnit::Tokens,
            cap: "".into(),
            warn_pct: "75".into(),
            monthly_cap: "2000".into(),
        };
        assert_eq!(
            budget_body(OwnerKind::Project, &form),
            Ok(json!({ "unit": "tokens", "cap": null, "warn_at_pct": 75, "monthly_cap": 2000 }))
        );
        let zero = Form {
            monthly_cap: "0".into(),
            ..form
        };
        assert_eq!(
            budget_body(OwnerKind::Project, &zero),
            Err("The monthly cap is more than zero, or blank for none.".into())
        );
    }

    #[test]
    fn a_ledger_line_says_what_it_recorded_and_a_zero_set_is_no_cap() {
        assert_eq!(ledger_label("hold"), "Held for a call");
        assert_eq!(ledger_label("refund"), "refund");
        assert_eq!(ledger_amount(BudgetUnit::Usd, "set", 0), "No cap");
        assert_eq!(ledger_amount(BudgetUnit::Usd, "charge", 1_500_000), "$1.50");
    }

    #[test]
    fn a_seat_is_working_when_it_is_filled_and_notes_its_tries_and_rounds() {
        assert_eq!(seat_label(SeatState::Filled), "Working");
        assert_eq!(seat_label(SeatState::Waiting), "Waiting");
        let mut seat = Seat {
            attempts: 2,
            round: 3,
            ..Default::default()
        };
        assert_eq!(seat_notes(&seat), ["Attempt 2", "Round 3"]);
        seat.attempts = 1;
        seat.round = 1;
        assert!(seat_notes(&seat).is_empty());
        assert_eq!(
            policy_text(ModelPolicy::Differ),
            "A model no other seat used"
        );
    }

    #[test]
    fn a_live_frame_is_for_its_owner_or_its_task() {
        let budget_frame = json!({ "owner": "pr4", "owner_kind": "project" });
        assert!(is_budget_of(&budget_frame, OwnerKind::Project, "pr4"));
        assert!(!is_budget_of(&budget_frame, OwnerKind::Task, "pr4"));
        assert!(!is_budget_of(&budget_frame, OwnerKind::Project, "pr5"));
        let task_frame = json!({ "id": "t3" });
        assert!(is_task_frame(&task_frame, "t3"));
        let session_frame = json!({ "id": "s1", "task": "t3", "project": "pr4" });
        assert!(is_task_session(&session_frame, "t3"));
        assert!(names_owner(&session_frame, OwnerKind::Project, "pr4"));
        assert!(names_owner(&session_frame, OwnerKind::Task, "t3"));
        assert!(!names_owner(
            &json!({ "task": null }),
            OwnerKind::Task,
            "t3"
        ));
    }
}
