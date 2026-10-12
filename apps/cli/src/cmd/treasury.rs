//! `agentplaza treasury`: The treasury: the plaza budget and its accounts, the wallets, the journal,
//! minting and fines.

use clap::Subcommand;
use serde_json::{json, Value};

use crate::ctx::{self, Ctx};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The plaza budget, what has been spent, and the accounts that hold the money.
    Balances,
    /// Every agent's wallet.
    Wallets,
    /// The journal: every money movement.
    Journal,
    /// Add whole units to the plaza budget.
    Mint {
        /// Whole units to add.
        amount: u64,
    },
    /// Fine an agent: take whole units from its wallet, with a memo.
    Fine {
        /// The agent's cid.
        cid: u64,
        /// Whole units to take.
        amount: u64,
        /// What the fine is for.
        #[arg(long, default_value = "")]
        memo: String,
    },
}

/// The elements of a JSON array (empty when it is not one).
fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

/// A journal entry's postings as "account amount, …".
fn postings(e: &Value) -> String {
    array(e.get("postings").unwrap_or(&Value::Null))
        .iter()
        .map(|p| format!("{} {}", ctx::s(p, "account"), ctx::text(p, "amount")))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::Balances => {
            let v = ctx.client.get("/v1/treasury")?;
            ctx.out(&v, |v| {
                let balance = v.get("totals").map(|t| ctx::text(t, "balance")).unwrap_or_default();
                ctx::fields(&[
                    ("plaza budget", ctx::text(v, "plaza_budget")),
                    ("spent", ctx::text(v, "spent")),
                    ("balance", balance),
                ]);
                println!();
                let rows: Vec<Vec<String>> = array(v.get("accounts").unwrap_or(&Value::Null))
                    .iter()
                    .map(|a| {
                        vec![
                            ctx::text(a, "id"),
                            ctx::s(a, "kind").to_string(),
                            ctx::s(a, "owner_ref").to_string(),
                            ctx::text(a, "balance"),
                            ctx::text(a, "credit_limit"),
                        ]
                    })
                    .collect();
                ctx::table(&["id", "kind", "owner", "balance", "credit limit"], &rows);
            });
        }
        Cmd::Wallets => {
            let v = ctx.client.get("/v1/treasury/wallets")?;
            ctx.out(&v, |v| {
                let rows: Vec<Vec<String>> = array(v)
                    .iter()
                    .map(|w| vec![ctx::text(w, "cid"), ctx::s(w, "name").to_string(), ctx::text(w, "balance")])
                    .collect();
                ctx::table(&["cid", "name", "balance"], &rows);
            });
        }
        Cmd::Journal => {
            let v = ctx.client.get("/v1/treasury/journal")?;
            ctx.out(&v, |v| {
                // The hub answers with the entries as a bare array or as {entries}.
                let entries = v.get("entries").unwrap_or(v);
                let rows: Vec<Vec<String>> = array(entries)
                    .iter()
                    .map(|e| {
                        vec![
                            ctx::text(e, "id"),
                            ctx::s(e, "kind").to_string(),
                            ctx::text(e, "at"),
                            postings(e),
                            ctx::s(e, "memo").to_string(),
                            ctx::text(e, "task"),
                        ]
                    })
                    .collect();
                ctx::table(&["id", "kind", "at", "postings", "memo", "task"], &rows);
            });
        }
        Cmd::Mint { amount } => {
            let v = ctx.client.post("/v1/treasury/mint", &json!({ "amount": amount }))?;
            ctx.out(&v, |_| println!("minted {amount} into the plaza budget"));
        }
        Cmd::Fine { cid, amount, memo } => {
            let v = ctx.client.post("/v1/treasury/fine", &json!({ "cid": cid, "amount": amount, "memo": memo }))?;
            ctx.out(&v, |_| println!("fined agent {cid} {amount}"));
        }
    }
    Ok(())
}
