//! `agentplaza treasury`: The treasury: the plaza budget and its accounts, the wallets, the journal,
//! the treasury over time, minting and fines.

use clap::Subcommand;
use serde_json::{Value, json};

use crate::ctx::{self, Ctx};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The plaza budget, what has been spent, and the accounts that hold the money.
    Balances,
    /// Every agent's wallet.
    Wallets,
    /// The journal: every money movement, newest first.
    Journal {
        /// How many entries to fetch (1 to 256).
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=256))]
        limit: Option<u32>,
        /// Only the entries older than this entry id (the previous page's next id).
        #[arg(long)]
        before: Option<u64>,
        /// Only the entries of this kind (mint, spend, fine, model, ...).
        #[arg(long)]
        kind: Option<String>,
        /// Only the entries posting to this agent's wallet (its cid).
        #[arg(long)]
        wallet: Option<u64>,
        /// Only the entries of this task.
        #[arg(long)]
        task: Option<String>,
    },
    /// The treasury over time: what flowed in and out in each bucket.
    Series {
        /// Start, in milliseconds since the Unix epoch (default: 30 days before --to).
        #[arg(long)]
        from: Option<u64>,
        /// End, in milliseconds since the Unix epoch (default: now).
        #[arg(long)]
        to: Option<u64>,
        /// Bucket size, in UTC: hour or day (default: day).
        #[arg(long, value_parser = ["hour", "day"])]
        bucket: Option<String>,
    },
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
        /// What the fine is for (required).
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
        .map(|p| format!("{} {}", ctx::text(p, "account"), ctx::text(p, "amount")))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `?key=value&…` for the parameters that are set (values percent-encoded); "" when none are.
/// A parameter with an empty value names no filter, so it is left out.
fn query(params: &[(&str, Option<String>)]) -> String {
    let parts: Vec<String> = params
        .iter()
        .filter_map(|(key, value)| {
            value
                .as_ref()
                .filter(|v| !v.is_empty())
                .map(|v| format!("{key}={}", ctx::seg(v)))
        })
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!("?{}", parts.join("&"))
    }
}

/// The journal's path with its paging and filters.
fn journal_path(
    limit: Option<u32>,
    before: Option<u64>,
    kind: Option<&str>,
    wallet: Option<u64>,
    task: Option<&str>,
) -> String {
    format!(
        "/v1/treasury/journal{}",
        query(&[
            ("limit", limit.map(|n| n.to_string())),
            ("before", before.map(|id| id.to_string())),
            ("kind", kind.map(str::to_string)),
            ("wallet", wallet.map(|cid| cid.to_string())),
            ("task", task.map(str::to_string)),
        ])
    )
}

/// The series' path: the range and bucket the request names (the hub defaults the rest).
fn series_path(from: Option<u64>, to: Option<u64>, bucket: Option<&str>) -> String {
    format!(
        "/v1/treasury/series{}",
        query(&[
            ("from", from.map(|ms| ms.to_string())),
            ("to", to.map(|ms| ms.to_string())),
            ("bucket", bucket.map(str::to_string)),
        ])
    )
}

/// A fine's memo, trimmed. The web refuses a blank memo, so the CLI refuses it before the POST.
fn fine_memo(memo: &str) -> Result<String, String> {
    let text = memo.trim();
    if text.is_empty() {
        return Err("A fine needs a memo.".into());
    }
    Ok(text.to_string())
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::Balances => {
            let v = ctx.client.get("/v1/treasury")?;
            ctx.out(&v, |v| {
                let balance = v
                    .get("totals")
                    .map(|t| ctx::text(t, "balance"))
                    .unwrap_or_default();
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
                    .map(|w| {
                        vec![
                            ctx::text(w, "cid"),
                            ctx::s(w, "name").to_string(),
                            ctx::text(w, "balance"),
                        ]
                    })
                    .collect();
                ctx::table(&["cid", "name", "balance"], &rows);
            });
        }
        Cmd::Journal {
            limit,
            before,
            kind,
            wallet,
            task,
        } => {
            let v = ctx.client.get(&journal_path(
                limit,
                before,
                kind.as_deref(),
                wallet,
                task.as_deref(),
            ))?;
            ctx.out(&v, |v| {
                // The hub answers with the entries in a page: {entries, total, next_before, ...}.
                let entries = v.get("entries").unwrap_or(v);
                let rows: Vec<Vec<String>> = array(entries)
                    .iter()
                    .map(|e| {
                        vec![
                            ctx::text(e, "id"),
                            ctx::s(e, "kind").to_string(),
                            ctx::when(&e["created_at"]),
                            postings(e),
                            ctx::s(e, "memo").to_string(),
                            ctx::text(e, "task_id"),
                        ]
                    })
                    .collect();
                ctx::table(&["id", "kind", "at", "postings", "memo", "task"], &rows);
                if let Some(next) = v.get("next_before").and_then(Value::as_u64) {
                    let total = ctx::text(v, "total");
                    println!();
                    println!("next page: --before {next} ({total} entries match)");
                }
            });
        }
        Cmd::Series { from, to, bucket } => {
            let v = ctx.client.get(&series_path(from, to, bucket.as_deref()))?;
            ctx.out(&v, |v| {
                let rows: Vec<Vec<String>> = array(v.get("points").unwrap_or(&Value::Null))
                    .iter()
                    .map(|p| {
                        vec![
                            ctx::when(&p["at_ms"]),
                            ctx::text(p, "entries"),
                            ctx::text(p, "minted"),
                            ctx::text(p, "escrowed"),
                            ctx::text(p, "paid"),
                            ctx::text(p, "refunded"),
                            ctx::text(p, "fined"),
                            ctx::text(p, "spent"),
                            ctx::text(p, "treasury_end"),
                        ]
                    })
                    .collect();
                ctx::table(
                    &[
                        "at",
                        "entries",
                        "minted",
                        "escrowed",
                        "paid",
                        "refunded",
                        "fined",
                        "spent",
                        "treasury end",
                    ],
                    &rows,
                );
            });
        }
        Cmd::Mint { amount } => {
            let v = ctx
                .client
                .post("/v1/treasury/mint", &json!({ "amount": amount }))?;
            ctx.out(&v, |_| println!("minted {amount} into the plaza budget"));
        }
        Cmd::Fine { cid, amount, memo } => {
            let memo = fine_memo(&memo)?;
            let v = ctx.client.post(
                "/v1/treasury/fine",
                &json!({ "cid": cid, "amount": amount, "memo": memo }),
            )?;
            ctx.out(&v, |_| println!("fined agent {cid} {amount}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_paths_carry_only_the_query_they_are_given() {
        // The journal path carries only the filters it is given.
        {
            assert_eq!(
                journal_path(None, None, None, None, None),
                "/v1/treasury/journal"
            );
            assert_eq!(
                journal_path(Some(10), None, Some(""), None, Some("")),
                "/v1/treasury/journal?limit=10"
            );
            assert_eq!(
                journal_path(Some(25), Some(42), Some("fine"), Some(7), Some("t/1 x")),
                "/v1/treasury/journal?limit=25&before=42&kind=fine&wallet=7&task=t%2F1%20x"
            );
        }
        // The series path names the range and bucket given.
        {
            assert_eq!(series_path(None, None, None), "/v1/treasury/series");
            assert_eq!(
                series_path(Some(1000), Some(5000), Some("hour")),
                "/v1/treasury/series?from=1000&to=5000&bucket=hour"
            );
        }
    }
}
