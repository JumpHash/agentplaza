//! Payments the owner must approve before they move.
//!
//! A payment over `settings.payments.approve_over` is queued as a `payment` approval by
//! `plaza_pay`. Approving it executes the transfer; denying it cancels it. A held payment is
//! not a law violation.

use serde_json::Value;

use crate::approvals::Approval;
use crate::state::App;

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
        let to = detail.get("to").and_then(Value::as_u64).ok_or("the payment names no `to`")?;
        let amount = detail.get("amount").and_then(Value::as_i64).ok_or("the payment names no `amount`")?;
        let memo = detail.get("memo").and_then(Value::as_str).unwrap_or_default().to_string();
        Ok(Payment { to, amount, memo })
    }

    /// The payment in words, as the agent is told it: `900 to 9`.
    fn describe(&self) -> String {
        format!("{} to {}", self.amount, self.to)
    }
}

/// Moves the credits of an approved payment from `from`; returns the journal entry.
fn execute(treasury: &apz_plaza::treasury::Treasury, from: u64, payment: &Payment) -> Result<u64, String> {
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
        let why = item.reason.as_ref().map(|r| format!(": {r}")).unwrap_or_default();
        tell(app, item.cid, format!("The owner cancelled your payment of {}{why}.", payment.describe()));
        return Ok(());
    }
    match execute(&app.plaza.treasury, item.cid, &payment) {
        Ok(entry) => {
            crate::society::interact(app, item.cid, payment.to, apz_plaza::bonds::Interaction::Credit);
            tell(app, item.cid, format!("The owner approved your payment of {} (entry {entry}).", payment.describe()));
            Ok(())
        }
        Err(why) => {
            tell(
                app,
                item.cid,
                format!("The owner approved your payment of {}, but it failed: {why}. No credits moved.", payment.describe()),
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
        treasury.charge_spend(Some("t1"), 100, "real cost").expect("spend");
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
        assert!(!held(-5, 0), "a non-positive amount is not held; the transfer refuses it");
    }

    #[test]
    fn an_approved_payment_moves_the_credits() {
        let treasury = funded();
        let payment = Payment::from_detail(&json!({"cid": 7, "to": 8, "amount": 10, "memo": "thanks"})).expect("detail");
        assert_eq!(payment.describe(), "10 to 8");
        execute(&treasury, 7, &payment).expect("approved payment executes");
        assert_eq!(treasury.wallet(7), 10);
        assert_eq!(treasury.wallet(8), 10);
    }

    #[test]
    fn an_approved_payment_the_wallet_cannot_cover_fails_and_moves_nothing() {
        let treasury = funded();
        let payment = Payment::from_detail(&json!({"cid": 7, "to": 8, "amount": 25, "memo": ""})).expect("detail");
        assert!(execute(&treasury, 7, &payment).is_err());
        assert_eq!(treasury.wallet(7), 20);
        assert_eq!(treasury.wallet(8), 0);
    }

    #[test]
    fn a_payment_detail_without_a_recipient_or_amount_is_refused() {
        assert!(Payment::from_detail(&json!({"cid": 7, "amount": 20})).is_err());
        assert!(Payment::from_detail(&json!({"cid": 7, "to": 8})).is_err());
    }
}
