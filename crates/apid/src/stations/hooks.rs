//! The hooks the workstation actions and the leases' ends pass through.
//!
//! - `after_agent_op`, after every operation an agent or the owner sends to a station (`act::agent_op`): the action
//!   goes into its lease's recording (recordings.rs), refused or not.
//! - `lease_ended`, when a lease ends (released, its session ended, its station revoked): the lease's wall time is
//!   charged to its task. The charge is the station's `rate_per_hour` (plaza credits, 1 credit is $0.001) times the
//!   time the lease was active, rounded up to a whole credit, and it is capped at what the task's escrow still holds.
//!   It is posted to the treasury as real spend, counts toward the task's metered spend (the chain's margin cap), and
//!   goes to the usage ledger as `workstation/<station>` (provider `workstation`), so the Usage view shows it.
//!
//! Only lease wall time is charged. GPU time on a workstation is not metered by the hub yet, so it is not charged.
//! A rate of 0 (the default) makes a workstation free.

use apz_plaza::treasury::Treasury;
use apz_station::Op;
use serde_json::{Value, json};

use super::act;
use super::leases::Lease;
use super::ops::{self, Failure};
use super::recordings;
use crate::prices::{Tokens, USD_PER_CREDIT};
use crate::state::App;
use crate::usage;

/// One hour, in milliseconds.
const HOUR_MS: u64 = 3_600_000;

/// Called after every operation an agent or the owner sends to a station, with its station, its lease (if any), the
/// operation and its outcome, whether it ran or was refused.
pub fn after_agent_op(app: &App, station: &str, lease: Option<&str>, op: &Op, result: &Result<Value, Failure>) {
    recordings::on_action(app, station, lease, &action_summary(op), result.is_ok());
}

/// The operation as its lease's recording keeps it: the audit's summary, with the workstation tool's name as its
/// `kind` (`click`, `screenshot`, `shell`, ...; `action` for an operation no tool names).
pub(super) fn action_summary(op: &Op) -> Value {
    let mut summary = act::summary(op);
    let kind = act::law_name(op).map_or("action", |name| name.trim_start_matches("station_"));
    if let Value::Object(map) = &mut summary {
        map.insert("kind".into(), json!(kind));
    }
    summary
}

/// Called when a lease ends on `station`, with the lease as it was. `now` is when it ends (ms).
pub fn lease_ended(app: &App, station: &str, lease: &Lease, now: u64) {
    let Some(rate) = app.stations.station(station).map(|s| s.settings.rate_per_hour) else {
        return;
    };
    let (Some(task), Some(active_from)) = (lease.task.as_deref(), lease.active_ms) else {
        return;
    };
    let wanted = credits_for(rate, active_from, now);
    if wanted <= 0 {
        return;
    }
    let memo = format!("workstation {station} lease {}", lease.id);
    let charge = charge_task(&app.plaza.treasury, task, wanted, &memo);
    if let Some(error) = &charge.error {
        eprintln!("apid: workstation {station}: the lease {} was not charged to task {task}: {error}", lease.id);
    }
    if charge.charged > 0 {
        app.work.add_spend(task, charge.charged);
        let cost = charge.charged as f64 * USD_PER_CREDIT;
        usage::meter(app, lease.agent, &format!("workstation/{station}"), Tokens::default(), Some(cost), Some(task.to_string()), "station");
    }
    let row = json!({
        "lease": lease.id,
        "task": task,
        "rate_per_hour": rate,
        "active_ms": now.saturating_sub(active_from),
        "wanted": charge.wanted,
        "charged": charge.charged,
        "error": charge.error,
    });
    if let Err(reason) = ops::audit(app, Some(station), Some(lease.agent), "lease_charge", "station", row) {
        eprintln!("apid: workstation {station}: the charge of lease {} is not in the audit: {reason}", lease.id);
    }
}

/// The credits a lease's wall time costs at `rate_per_hour`: from `active_from` to `now` (ms), rounded up to a whole
/// credit. Zero at a rate of 0.
pub fn credits_for(rate_per_hour: u64, active_from: u64, now: u64) -> i64 {
    let millis = u128::from(now.saturating_sub(active_from));
    let credits = (millis * u128::from(rate_per_hour)).div_ceil(u128::from(HOUR_MS));
    i64::try_from(credits).unwrap_or(i64::MAX)
}

/// What one lease's charge came to: the credits wanted, those the task's escrow paid, and the treasury's refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Charge {
    pub wanted: i64,
    pub charged: i64,
    pub error: Option<String>,
}

/// Charges `wanted` credits to `task`'s escrow as real spend. The charge is capped at what the escrow holds, so a
/// lease never takes a task below zero: the shortfall is `wanted` less `charged`, and the audit row keeps it.
pub fn charge_task(treasury: &Treasury, task: &str, wanted: i64, memo: &str) -> Charge {
    let escrow = treasury.escrow_balance(task).max(0);
    let charged = i64::try_from(escrow).unwrap_or(i64::MAX).min(wanted);
    if charged <= 0 {
        return Charge { wanted, charged: 0, error: None };
    }
    match treasury.charge_spend(Some(task), charged, memo) {
        Ok(_) => Charge { wanted, charged, error: None },
        Err(error) => Charge { wanted, charged: 0, error: Some(error.to_string()) },
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use apz_plaza::events::EventBus;
    use apz_station::{Button, ImageFormat, InputAction, StationSettings};

    use super::*;
    use crate::db::Db;
    use crate::stations::leases::{Kind, LeaseView, State};
    use crate::stations::recordings::Recordings;

    #[test]
    fn a_lease_costs_its_wall_time_at_the_rate_rounded_up_and_nothing_at_no_rate() {
        assert_eq!(credits_for(0, 0, HOUR_MS), 0, "a free station");
        assert_eq!(credits_for(120, 0, HOUR_MS), 120, "an hour at 120 credits");
        assert_eq!(credits_for(120, 1_000, 1_000), 0, "no time, no charge");
        assert_eq!(credits_for(120, 0, 90_000), 3, "90 seconds at 120 an hour is 3 credits");
        assert_eq!(credits_for(1, 0, 1), 1, "any time at all rounds up to a credit");
        assert_eq!(credits_for(u64::MAX, 0, u64::MAX), i64::MAX, "a huge rate saturates");
    }

    #[test]
    fn a_charge_comes_off_the_task_escrow_and_is_capped_at_what_it_holds() {
        let treasury = Treasury::new(EventBus::new());
        treasury.mint_plaza_budget(10_000).expect("the plaza budget");
        treasury.fund_task("t1", 300).expect("the task is funded");

        let paid = charge_task(&treasury, "t1", 100, "lease");
        assert_eq!(paid, Charge { wanted: 100, charged: 100, error: None });
        assert_eq!(treasury.escrow_balance("t1"), 200);

        let short = charge_task(&treasury, "t1", 500, "lease");
        assert_eq!((short.wanted, short.charged), (500, 200), "capped at the escrow");
        assert_eq!(treasury.escrow_balance("t1"), 0, "never below zero");

        let none = charge_task(&treasury, "t1", 50, "lease");
        assert_eq!((none.wanted, none.charged, none.error), (50, 0, None), "an empty escrow charges nothing and refuses nothing");
    }

    #[test]
    fn an_action_is_recorded_under_its_tool_name() {
        let click = Op::Input {
            session: 9,
            action: InputAction::Click { x: 1, y: 2, button: Button::Left, count: 1 },
        };
        assert_eq!(action_summary(&click)["kind"], "click");
        let shot = Op::Screenshot { session: 9, scale: 0.5, grid: false, format: ImageFormat::Png };
        assert_eq!(action_summary(&shot)["kind"], "screenshot");
        assert_eq!(action_summary(&Op::Facts)["kind"], "action", "an operation no tool names");
    }

    #[test]
    fn an_action_lands_in_its_leases_recording_with_its_outcome() {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir: PathBuf = std::env::temp_dir().join(format!("apid-hooks-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        let db = Db::files_only(&dir);
        let recordings = Recordings::default();
        let lease = LeaseView {
            lease: Lease {
                id: "l1".into(),
                station: "w1".into(),
                agent: 7,
                task: Some("t1".into()),
                kind: Kind::Pixel,
                session: Some(9),
                started_ms: 1000,
                state: State::Active,
                paused: false,
                active_ms: Some(1000),
            },
            position: None,
        };
        let id = recordings.begin(&db, &StationSettings::default(), "w1", &lease, 1000).unwrap().expect("the station records");
        let op = Op::Input { session: 9, action: InputAction::Click { x: 1, y: 2, button: Button::Left, count: 1 } };
        assert!(recordings.action(&db, "w1", "l1", &action_summary(&op), false, 1100).unwrap());
        let events = db.recording_events(&id, 10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!((events[0].kind.as_str(), events[0].ok), ("click", false), "a refused click is kept as refused");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
