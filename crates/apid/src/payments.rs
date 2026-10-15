//! Payments the owner must approve before they move, and the one charge every model call makes.
//!
//! A payment over `settings.payments.approve_over` is queued as a `payment` approval by
//! `plaza_pay`. Approving it executes the transfer; denying it cancels it. A held payment is
//! not a law violation.
//!
//! [`charge_model_call`] is the charge the agent loop (`internal::meter`) and the gateway
//! (`llm_proxy::bill_call`) both make for a model call: it honours `payments.charge_models`,
//! posts one `model` entry, and records the task's metered spend for the chain's margin.

use serde_json::Value;

use crate::approvals::Approval;
use crate::state::App;
use apz_plaza::treasury::Treasury;

/// The payer of an agent's model call: the task's escrow while the task still has money in it,
/// else the agent's wallet (no task: the wallet). Returns the task to charge (`None` for the
/// wallet) and the balance there.
pub(crate) fn paying_from(task: Option<&str>, escrow: i128, wallet: i128) -> (Option<&str>, i128) {
    match task {
        Some(task) if escrow > 0 => (Some(task), escrow),
        _ => (None, wallet),
    }
}

/// What one model call's charge came to: the credits wanted, those charged, the balance of the
/// payer, whether a task's escrow paid, and the error when the treasury refused the charge.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Settled {
    pub wanted: i64,
    pub charged: i64,
    pub available: i128,
    pub task_paid: bool,
    pub error: Option<String>,
}

/// Charges `credits` for agent `cid`'s call to its payer (see [`paying_from`]), capped at that
/// payer's balance so no balance goes negative.
pub(crate) fn settle(
    treasury: &Treasury,
    cid: u64,
    task: Option<&str>,
    credits: i64,
    memo: &str,
) -> Settled {
    let escrow = task.map_or(0, |task| treasury.escrow_balance(task));
    let (charge_task, available) = paying_from(task, escrow, treasury.wallet(cid));
    let amount = i64::try_from(available.max(0))
        .unwrap_or(i64::MAX)
        .min(credits);
    let (charged, error) = if amount > 0 {
        match treasury.charge_model(cid, charge_task, amount, memo) {
            Ok(_) => (amount, None),
            Err(e) => (0, Some(e.to_string())),
        }
    } else {
        (0, None)
    };
    Settled {
        wanted: credits,
        charged,
        available,
        task_paid: charge_task.is_some() && charged > 0,
        error,
    }
}

/// The charge every model call makes, whichever path it came by. With `charging` off (the
/// `payments.charge_models` switch) nothing is charged and this returns `None`. Otherwise one
/// `model` entry for the charge goes to the payer, and when the task's escrow paid, `spend`
/// records that amount as the task's metered spend (it feeds the chain's margin cap). A
/// shortfall (`charged < wanted`) is for the caller to act on: the agent loop cancels the
/// task's work, the gateway notes the call in the transcript.
pub(crate) fn charge_model_call(
    treasury: &Treasury,
    charging: bool,
    spend: impl FnOnce(&str, i64),
    cid: u64,
    task: Option<&str>,
    credits: i64,
    memo: &str,
) -> Option<Settled> {
    if !charging || credits <= 0 {
        return None;
    }
    let settled = settle(treasury, cid, task, credits, memo);
    if let (true, Some(task)) = (settled.task_paid, task) {
        spend(task, settled.charged);
    }
    Some(settled)
}

/// Whether a payment of `amount` is held for the owner: it is over `approve_over`. With
/// `approve_over` at 0 every payment asks. A payment of zero or less is not held; the transfer
/// refuses it.
pub fn held(amount: i64, approve_over: u64) -> bool {
    u64::try_from(amount).is_ok_and(|amount| amount > approve_over)
}

/// The payment a `payment` approval holds, read from its detail.
#[derive(Debug, Clone, PartialEq)]
struct Payment {
    to: u64,
    amount: i64,
    memo: String,
}

impl Payment {
    fn from_detail(detail: &Value) -> Result<Payment, String> {
        let to = detail
            .get("to")
            .and_then(Value::as_u64)
            .ok_or("the payment names no `to`")?;
        let amount = detail
            .get("amount")
            .and_then(Value::as_i64)
            .ok_or("the payment names no `amount`")?;
        let memo = detail
            .get("memo")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok(Payment { to, amount, memo })
    }

    /// The payment in words, as the agent is told it: `900 to 9`.
    fn describe(&self) -> String {
        format!("{} to {}", self.amount, self.to)
    }
}

/// Moves the credits of an approved payment from `from`; returns the journal entry.
fn execute(
    treasury: &apz_plaza::treasury::Treasury,
    from: u64,
    payment: &Payment,
) -> Result<u64, String> {
    treasury
        .transfer(from, payment.to, payment.amount, &payment.memo)
        .map_err(|e| format!("{e:?}"))
}

/// Executes or cancels a decided `payment` approval. Called by `approvals::decide` after the
/// decision is recorded, so this only moves the money (approve) or tells the agent (deny).
/// An approved payment that fails is told to the agent and returned as an error for the owner.
pub fn decided(app: &App, item: &Approval, approve: bool) -> Result<(), String> {
    let payment = Payment::from_detail(&item.detail)?;
    if !approve {
        let why = item
            .reason
            .as_ref()
            .map(|r| format!(": {r}"))
            .unwrap_or_default();
        tell(
            app,
            item.cid,
            format!(
                "The owner cancelled your payment of {}{why}.",
                payment.describe()
            ),
        );
        return Ok(());
    }
    match execute(&app.plaza.treasury, item.cid, &payment) {
        Ok(entry) => {
            crate::society::interact(
                app,
                item.cid,
                payment.to,
                apz_plaza::bonds::Interaction::Credit,
            );
            tell(
                app,
                item.cid,
                format!(
                    "The owner approved your payment of {} (entry {entry}).",
                    payment.describe()
                ),
            );
            Ok(())
        }
        Err(why) => {
            tell(
                app,
                item.cid,
                format!(
                    "The owner approved your payment of {}, but it failed: {why}. No credits moved.",
                    payment.describe()
                ),
            );
            Err(why)
        }
    }
}

/// Tells the agent what became of its payment, as a chat message, the way tool approvals do.
fn tell(app: &App, cid: u64, text: String) {
    app.work.enqueue(
        app,
        cid,
        apz_proto::wire::WorkKind::Chat,
        format!("op-{cid}"),
        text,
        crate::scheduler::Origin::default(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_plaza::events::EventBus;
    use apz_plaza::treasury::Treasury;
    use serde_json::json;

    /// Agent 7 holds 20 credits.
    fn funded() -> Treasury {
        let treasury = Treasury::new(EventBus::new());
        treasury.fund_task("t1", 300).expect("fund");
        treasury
            .charge_spend(Some("t1"), 100, "real cost")
            .expect("spend");
        treasury.pay_agent("t1", 7, 20).expect("pay");
        treasury
    }

    #[test]
    fn a_payment_is_held_only_above_the_threshold() {
        assert!(held(501, 500));
        assert!(!held(500, 500));
        assert!(!held(5, 500));
        assert!(held(1, 0), "approve_over 0: every payment asks");
        assert!(!held(0, 0));
        assert!(
            !held(-5, 0),
            "a non-positive amount is not held; the transfer refuses it"
        );
    }

    #[test]
    fn an_approved_payment_moves_the_credits_and_an_uncovered_one_moves_nothing() {
        // An approved payment moves the credits from the payer to the payee.
        {
            let treasury = funded();
            let payment =
                Payment::from_detail(&json!({"cid": 7, "to": 8, "amount": 10, "memo": "thanks"}))
                    .expect("detail");
            assert_eq!(payment.describe(), "10 to 8");
            execute(&treasury, 7, &payment).expect("approved payment executes");
            assert_eq!(treasury.wallet(7), 10);
            assert_eq!(treasury.wallet(8), 10);
        }
        // An approved payment the wallet cannot cover fails and moves nothing.
        {
            let treasury = funded();
            let payment =
                Payment::from_detail(&json!({"cid": 7, "to": 8, "amount": 25, "memo": ""}))
                    .expect("detail");
            assert!(execute(&treasury, 7, &payment).is_err());
            assert_eq!(treasury.wallet(7), 20);
            assert_eq!(treasury.wallet(8), 0);
        }
    }

    #[test]
    fn a_payment_detail_without_a_recipient_or_amount_is_refused() {
        assert!(Payment::from_detail(&json!({"cid": 7, "amount": 20})).is_err());
        assert!(Payment::from_detail(&json!({"cid": 7, "to": 8})).is_err());
    }

    /// Task "t1" holds 300 credits of escrow; agent 7 has no wallet.
    fn task_funded() -> Treasury {
        let treasury = Treasury::new(EventBus::new());
        treasury.fund_task("t1", 300).expect("fund");
        treasury
    }

    #[test]
    fn model_call_charges_settle_against_escrow_wallet_and_spend() {
        // The agent loop charge comes off the escrow and is metered spend when charging is on.
        {
            let treasury = task_funded();
            let mut metered = Vec::new();
            let settled = charge_model_call(
                &treasury,
                true,
                |task, credits| metered.push((task.to_string(), credits)),
                7,
                Some("t1"),
                40,
                "model m",
            )
            .expect("charging is on, so the call is charged");
            assert_eq!(
                (settled.wanted, settled.charged, settled.task_paid),
                (40, 40, true)
            );
            assert_eq!(settled.error, None);
            assert_eq!(treasury.escrow_balance("t1"), 260);
            assert_eq!(
                metered,
                vec![("t1".to_string(), 40)],
                "the task's metered spend is updated"
            );
            assert_eq!(treasury.view().spent, 40, "the Spent total counts it");
            let kinds: Vec<_> = treasury
                .journal(10)
                .entries
                .iter()
                .map(|e| e.kind.to_string())
                .collect();
            assert_eq!(
                kinds,
                vec![
                    "mint".to_string(),
                    "subcontract".to_string(),
                    "model".to_string()
                ],
                "the funding entries, then one model entry and no spend entry"
            );
        }
        // With charging off, neither a task call nor a wallet call is charged.
        {
            let treasury = task_funded();
            assert_eq!(
                charge_model_call(&treasury, false, |_, _| {}, 7, Some("t1"), 40, "model m"),
                None,
                "the agent loop call"
            );
            assert_eq!(treasury.escrow_balance("t1"), 300);
            assert_eq!(treasury.view().spent, 0);

            treasury
                .charge_spend(Some("t1"), 100, "real cost")
                .expect("the cost is spent");
            treasury.pay_agent("t1", 7, 20).expect("the agent is paid");
            assert_eq!(
                charge_model_call(&treasury, false, |_, _| {}, 7, None, 15, "model m"),
                None,
                "the gateway call from a wallet"
            );
            assert_eq!(treasury.wallet(7), 20);
            assert_eq!(treasury.view().spent, 100);
        }
        // A gateway call from a wallet counts in Spent when charging is on.
        {
            let treasury = task_funded();
            treasury
                .charge_spend(Some("t1"), 100, "real cost")
                .expect("the cost is spent");
            treasury.pay_agent("t1", 7, 20).expect("the agent is paid");
            let mut metered = Vec::new();
            let settled = charge_model_call(
                &treasury,
                true,
                |task, credits| metered.push((task.to_string(), credits)),
                7,
                None,
                15,
                "model m",
            )
            .expect("charging is on");
            assert_eq!((settled.charged, settled.task_paid), (15, false));
            assert_eq!(treasury.wallet(7), 5, "the wallet pays");
            assert!(metered.is_empty(), "a wallet call is no task's spend");
            assert_eq!(
                treasury.view().spent,
                115,
                "the 100 spent plus the 15 from the wallet"
            );
        }
        // A charge larger than the escrow is capped and reported short.
        {
            let treasury = Treasury::new(EventBus::new());
            treasury.fund_task("t1", 30).expect("fund");
            let settled =
                charge_model_call(&treasury, true, |_, _| {}, 7, Some("t1"), 50, "model m")
                    .expect("charging is on");
            assert_eq!(
                (settled.wanted, settled.charged, settled.available),
                (50, 30, 30)
            );
            assert_eq!(treasury.escrow_balance("t1"), 0, "never negative");
        }
    }

    #[test]
    fn a_model_charge_counts_toward_the_chain_margin_cap() {
        let treasury = task_funded();
        charge_model_call(&treasury, true, |_, _| {}, 7, Some("t1"), 40, "model m")
            .expect("charged");
        // The cap is 20 % of the chain's real cost: 40 of model cost allows 8 to the agent.
        assert!(
            treasury.pay_agent("t1", 7, 9).is_err(),
            "a model charge is real cost for the margin"
        );
        assert!(treasury.pay_agent("t1", 7, 8).is_ok());
    }
}
