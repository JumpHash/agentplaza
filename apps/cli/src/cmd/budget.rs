//! `agentplaza budget`: the work engine's budgets, a project's and a task's: the cap, what is spent, reserved, held and
//! left, and the ledger of what each one was charged for. A budget is in dollars (`usd`) or in whole tokens (`tokens`);
//! no cap is no limit. The ledger lists what the hub reserved, held, charged, released or raised, newest first.

use clap::Subcommand;
use serde_json::{Value, json};

use crate::ctx::{self, Ctx};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// A project's budget: its cap, what is spent, reserved for its tasks, held and left, and its tasks' budgets.
    /// `--set` sets the cap; the warn share and the monthly cap keep their values unless given.
    Project {
        /// The project's id.
        id: String,
        /// The unit and the cap: `usd 50` (dollars), `tokens 2000000`; `usd` alone for no cap.
        #[arg(long, num_args = 1..=2, value_names = ["UNIT", "CAP"])]
        set: Option<Vec<String>>,
        /// The share of the cap, in percent, at which the owner is told (80 when the budget has none).
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=100), requires = "set")]
        warn: Option<u8>,
        /// The cap per month for the planner's long sessions, in the budget's unit (none by default).
        #[arg(long, requires = "set")]
        monthly: Option<String>,
    },
    /// A task's budget: its cap, and what is spent, reserved from its project, and left.
    Task {
        /// The task's id.
        id: String,
        /// The unit and the cap, as for `budget project --set`.
        #[arg(long, num_args = 1..=2, value_names = ["UNIT", "CAP"])]
        set: Option<Vec<String>>,
    },
    /// A budget's ledger, newest first: what was reserved, held, charged, released or raised, and by which session.
    Ledger {
        /// The project's id (the task's, with `--task`).
        id: String,
        /// The id is a task's.
        #[arg(long)]
        task: bool,
        /// How many entries (at most 1000).
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=1000))]
        limit: u32,
    },
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::Project {
            id,
            set,
            warn,
            monthly,
        } => project(ctx, &id, set, warn, monthly.as_deref()),
        Cmd::Task { id, set } => task(ctx, &id, set),
        Cmd::Ledger { id, task, limit } => ledger(ctx, &id, task, limit),
    }
}

/// The path of a budget's read or write: a project's or a task's.
fn budget_path(task: bool, id: &str) -> String {
    let owner = if task { "tasks" } else { "projects" };
    format!("/v1/{owner}/{}/budget", ctx::seg(id))
}

fn project(
    ctx: &Ctx,
    id: &str,
    set: Option<Vec<String>>,
    warn: Option<u8>,
    monthly: Option<&str>,
) -> Result<(), String> {
    let path = budget_path(false, id);
    let budget = match set {
        None => ctx.client.get(&path)?,
        Some(values) => {
            let (unit, cap) = parse_set(&values)?;
            let current = ctx.client.get(&path)?;
            let monthly = monthly.map(|text| amount_of(&unit, text)).transpose()?;
            let body = project_body(&unit, cap, warn, monthly, &current);
            ctx.client.put(&path, &body)?
        }
    };
    ctx.out(&budget, |b| {
        println!("project {id}");
        ctx::fields(&budget_lines(b));
        let tasks = b
            .get("tasks")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if !tasks.is_empty() {
            println!();
            let rows: Vec<Vec<String>> = tasks
                .iter()
                .map(|t| {
                    let unit = t.get("unit").and_then(Value::as_str);
                    vec![
                        ctx::text(t, "owner"),
                        ctx::s(t, "unit").to_string(),
                        cap_text(unit, t.get("cap")),
                        amount_text(unit, count(t, "spent")),
                        left_text(unit, t.get("left")),
                    ]
                })
                .collect();
            ctx::table(&["task", "unit", "cap", "spent", "left"], &rows);
        }
    });
    Ok(())
}

fn task(ctx: &Ctx, id: &str, set: Option<Vec<String>>) -> Result<(), String> {
    let path = budget_path(true, id);
    let budget = match set {
        None => ctx.client.get(&path)?,
        Some(values) => {
            let (unit, cap) = parse_set(&values)?;
            ctx.client
                .put(&path, &json!({ "unit": unit, "cap": cap }))?
        }
    };
    ctx.out(&budget, |b| {
        println!("task {id}");
        ctx::fields(&budget_lines(b));
    });
    Ok(())
}

fn ledger(ctx: &Ctx, id: &str, task: bool, limit: u32) -> Result<(), String> {
    let owner = if task { "tasks" } else { "projects" };
    let budget = ctx.client.get(&budget_path(task, id))?;
    let unit = budget
        .get("unit")
        .and_then(Value::as_str)
        .map(str::to_string);
    let path = format!("/v1/{owner}/{}/budget/ledger?limit={limit}", ctx::seg(id));
    let entries = ctx.client.get(&path)?;
    ctx.out(&entries, |entries| {
        let rows: Vec<Vec<String>> = entries
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|e| {
                vec![
                    ctx::when(&e["at_ms"]),
                    ctx::s(e, "kind").to_string(),
                    amount_text(unit.as_deref(), count(e, "amount")),
                    ctx::text(e, "session"),
                    ctx::text(e, "model"),
                ]
            })
            .collect();
        ctx::table(&["at", "kind", "amount", "session", "model"], &rows);
    });
    Ok(())
}

/// `--set UNIT [CAP]`: the unit, `usd` or `tokens`, and the cap in it (None for no cap).
pub fn parse_set(values: &[String]) -> Result<(String, Option<u64>), String> {
    let unit = values.first().map(String::as_str).unwrap_or_default();
    if unit != "usd" && unit != "tokens" {
        return Err(format!("the unit is usd (dollars) or tokens, not `{unit}`"));
    }
    let cap = values
        .get(1)
        .map(|text| amount_of(unit, text))
        .transpose()?;
    Ok((unit.to_string(), cap))
}

/// An amount in a budget's unit, more than zero: dollars for `usd` (`12`, `12.50`, at most six decimals), whole tokens
/// for `tokens`.
pub fn amount_of(unit: &str, text: &str) -> Result<u64, String> {
    let amount = if unit == "tokens" {
        text.trim()
            .parse::<u64>()
            .map_err(|_| format!("`{text}` is not a whole number of tokens"))?
    } else {
        micros_of_dollars(text)?
    };
    if amount == 0 {
        return Err("an amount is more than zero: pass no cap for none".into());
    }
    Ok(amount)
}

/// Dollars as micro-dollars: `12` is 12000000, `12.5` is 12500000 (no sign, no grouping, at most six decimals).
pub fn micros_of_dollars(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let bad = || format!("`{text}` is not an amount of dollars, like 12 or 12.50");
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    let digits = |s: &str| s.chars().all(|c| c.is_ascii_digit());
    if (whole.is_empty() && fraction.is_empty())
        || !digits(whole)
        || !digits(fraction)
        || fraction.len() > 6
    {
        return Err(bad());
    }
    let whole: u64 = if whole.is_empty() {
        0
    } else {
        whole.parse().map_err(|_| bad())?
    };
    let fraction: u64 = format!("{fraction:0<6}").parse().map_err(|_| bad())?;
    whole
        .checked_mul(1_000_000)
        .and_then(|micros| micros.checked_add(fraction))
        .ok_or_else(bad)
}

/// Micro-dollars as dollars with at least two decimals: `420000` is `$0.42`, `1500000` is `$1.50`.
pub fn dollars(micros: u64) -> String {
    let mut fraction = format!("{:06}", micros % 1_000_000);
    while fraction.len() > 2 && fraction.ends_with('0') {
        fraction.pop();
    }
    format!("${}.{fraction}", micros / 1_000_000)
}

/// An amount in a budget's unit as text: dollars for `usd` (or no unit), tokens for `tokens`.
pub fn amount_text(unit: Option<&str>, value: u64) -> String {
    match unit {
        Some("tokens") => format!("{value} tokens"),
        _ => dollars(value),
    }
}

/// A cap in a budget's unit, or `no cap` when there is none.
fn cap_text(unit: Option<&str>, cap: Option<&Value>) -> String {
    match cap.and_then(Value::as_u64) {
        Some(cap) => amount_text(unit, cap),
        None => "no cap".to_string(),
    }
}

/// What is left in a budget's unit, or `-` when the budget has no cap.
fn left_text(unit: Option<&str>, left: Option<&Value>) -> String {
    match left.and_then(Value::as_u64) {
        Some(left) => amount_text(unit, left),
        None => "-".to_string(),
    }
}

/// A count field (0 when missing).
fn count(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// One budget as `budget` shows it: its unit and cap, what is spent (with the share of the cap), reserved, held and
/// left, the warn share and the monthly cap a project has, and whether it is paused.
fn budget_lines(b: &Value) -> Vec<(&'static str, String)> {
    let unit = b.get("unit").and_then(Value::as_str);
    let spent = match b.get("spent_pct").and_then(Value::as_f64) {
        Some(pct) => format!("{} ({pct:.0}%)", amount_text(unit, count(b, "spent"))),
        None => amount_text(unit, count(b, "spent")),
    };
    let mut lines = vec![
        ("unit", unit.unwrap_or("none").to_string()),
        ("cap", cap_text(unit, b.get("cap"))),
        ("spent", spent),
        ("reserved", amount_text(unit, count(b, "reserved"))),
        ("held", amount_text(unit, count(b, "held"))),
        ("left", left_text(unit, b.get("left"))),
    ];
    if let Some(warn) = b.get("warn_at_pct").and_then(Value::as_u64) {
        lines.push(("warns at", format!("{warn}% of the cap")));
    }
    if let Some(monthly) = b.get("monthly_cap").and_then(Value::as_u64) {
        lines.push(("monthly cap", amount_text(unit, monthly)));
    }
    if b.get("paused").and_then(Value::as_bool) == Some(true) {
        lines.push((
            "paused",
            "yes: it reached its cap or the owner paused it".to_string(),
        ));
    }
    lines
}

/// The body of a project budget's write. The hub replaces the whole budget, so the warn share and the monthly cap keep
/// the values it has (when its unit stays the same), unless the command gives others.
fn project_body(
    unit: &str,
    cap: Option<u64>,
    warn: Option<u8>,
    monthly: Option<u64>,
    current: &Value,
) -> Value {
    let same_unit = current.get("unit").and_then(Value::as_str) == Some(unit);
    let kept = |key: &str| {
        if same_unit {
            current.get(key).cloned().unwrap_or(Value::Null)
        } else {
            Value::Null
        }
    };
    let warn = warn.map_or_else(|| kept("warn_at_pct"), |pct| json!(pct));
    let monthly = monthly.map_or_else(|| kept("monthly_cap"), |amount| json!(amount));
    json!({ "unit": unit, "cap": cap, "warn_at_pct": warn, "monthly_cap": monthly })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dollars_are_read_exactly_and_shown_with_two_decimals() {
        assert_eq!(micros_of_dollars("12"), Ok(12_000_000));
        assert_eq!(micros_of_dollars("12.5"), Ok(12_500_000));
        assert_eq!(micros_of_dollars(" 0.000001 "), Ok(1));
        assert_eq!(micros_of_dollars("7."), Ok(7_000_000));
        assert!(
            micros_of_dollars("1.1234567").is_err(),
            "at most six decimals"
        );
        assert!(micros_of_dollars("-1").is_err());
        assert!(micros_of_dollars(".").is_err());
        assert!(micros_of_dollars("ten").is_err());
        assert_eq!(dollars(420_000), "$0.42");
        assert_eq!(dollars(1_500_000), "$1.50");
        assert_eq!(dollars(1), "$0.000001");
        assert_eq!(dollars(0), "$0.00");
    }

    #[test]
    fn an_amount_is_more_than_zero_in_its_unit() {
        assert_eq!(amount_of("tokens", "2000"), Ok(2000));
        assert!(amount_of("tokens", "2.5").is_err(), "tokens are whole");
        assert!(amount_of("usd", "0").is_err());
        assert!(amount_of("tokens", "0").is_err());
        assert_eq!(amount_of("usd", "3.25"), Ok(3_250_000));
    }

    #[test]
    fn set_takes_a_unit_and_a_cap_or_none() {
        let set =
            |values: &[&str]| -> Vec<String> { values.iter().map(|v| v.to_string()).collect() };
        assert_eq!(
            parse_set(&set(&["usd", "50"])),
            Ok(("usd".into(), Some(50_000_000)))
        );
        assert_eq!(
            parse_set(&set(&["tokens", "9"])),
            Ok(("tokens".into(), Some(9)))
        );
        assert_eq!(parse_set(&set(&["usd"])), Ok(("usd".into(), None)));
        assert!(
            parse_set(&set(&["credits", "5"])).is_err(),
            "only usd and tokens"
        );
    }

    #[test]
    fn a_project_write_keeps_the_warn_share_and_monthly_cap_of_the_same_unit() {
        let current =
            json!({ "unit": "usd", "cap": 1, "warn_at_pct": 70, "monthly_cap": 5_000_000 });
        let kept = project_body("usd", Some(9_000_000), None, None, &current);
        assert_eq!(kept["cap"], 9_000_000);
        assert_eq!(kept["warn_at_pct"], 70);
        assert_eq!(kept["monthly_cap"], 5_000_000);
        let given = project_body("usd", Some(9_000_000), Some(90), Some(1_000_000), &current);
        assert_eq!(given["warn_at_pct"], 90);
        assert_eq!(given["monthly_cap"], 1_000_000);
        let other_unit = project_body("tokens", Some(9), None, None, &current);
        assert!(
            other_unit["monthly_cap"].is_null(),
            "a monthly cap in dollars is not one in tokens"
        );
        assert!(other_unit["warn_at_pct"].is_null(), "the hub's 80 applies");
    }

    #[test]
    fn a_budget_shows_its_cap_its_spending_and_what_is_left() {
        let b = json!({
            "owner": "pr3", "unit": "usd", "cap": 50_000_000, "spent": 12_300_000, "spent_pct": 25,
            "reserved": 5_000_000, "held": 200_000, "left": 32_500_000, "warn_at_pct": 80,
            "monthly_cap": 20_000_000, "paused": false
        });
        let lines = budget_lines(&b);
        assert_eq!(lines[0], ("unit", "usd".to_string()));
        assert_eq!(lines[1], ("cap", "$50.00".to_string()));
        assert_eq!(lines[2], ("spent", "$12.30 (25%)".to_string()));
        assert_eq!(lines[5], ("left", "$32.50".to_string()));
        assert!(lines.contains(&("warns at", "80% of the cap".to_string())));
        assert!(lines.contains(&("monthly cap", "$20.00".to_string())));
        assert!(!lines.iter().any(|(k, _)| *k == "paused"));
        let unset = budget_lines(&json!({ "owner": "t1", "unit": null, "cap": null, "spent": 0 }));
        assert_eq!(unset[0], ("unit", "none".to_string()));
        assert_eq!(unset[1], ("cap", "no cap".to_string()));
        assert_eq!(unset[5], ("left", "-".to_string()));
    }

    #[test]
    fn a_budget_path_names_its_project_or_task() {
        assert_eq!(budget_path(false, "pr3"), "/v1/projects/pr3/budget");
        assert_eq!(budget_path(true, "t4"), "/v1/tasks/t4/budget");
    }
}
