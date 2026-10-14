//! `agentplaza usage`: what the plaza's model calls consumed and what they cost: the summary over a
//! window, the series over time, the call log, and the model prices behind the estimates.

use std::time::{SystemTime, UNIX_EPOCH};

use clap::Subcommand;
use serde_json::Value;

use crate::ctx::{self, Ctx};

const DAY_MS: u64 = 86_400_000;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// What the model calls cost over a window: the totals, and the models ranked by spend.
    Summary {
        /// The window ending now: 24h, 7d, 30d or 90d.
        #[arg(long, default_value = "7d", value_parser = ["24h", "7d", "30d", "90d"])]
        window: String,
    },
    /// The calls over a window, bucket by bucket, grouped.
    Series {
        /// The window ending now: 24h, 7d, 30d or 90d.
        #[arg(long, default_value = "7d", value_parser = ["24h", "7d", "30d", "90d"])]
        window: String,
        /// Bucket size, in UTC: hour or day (default: hour for 24h and 7d, day for 30d and 90d).
        #[arg(long, value_parser = ["hour", "day"])]
        bucket: Option<String>,
        /// The grouping: model, agent, role or provider.
        #[arg(long, default_value = "model", value_parser = ["model", "agent", "role", "provider"])]
        by: String,
    },
    /// The newest model calls, a page at a time.
    Calls {
        /// How many calls to fetch (1 to 200).
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=200))]
        limit: Option<u32>,
        /// Only the calls older than this call id (the previous page's next id).
        #[arg(long)]
        before: Option<u64>,
        /// Only the calls of this model, e.g. anthropic/claude-x.
        #[arg(long)]
        model: Option<String>,
        /// Only the calls of this agent (its cid).
        #[arg(long)]
        cid: Option<u64>,
    },
    /// The model prices behind the estimates: all of them, or one model's (USD per million tokens).
    #[command(args_conflicts_with_subcommands = true)]
    Prices {
        /// One model's price, e.g. anthropic/claude-x (all of them when omitted).
        model: Option<String>,
        #[command(subcommand)]
        action: Option<PricesAction>,
    },
}

#[derive(Subcommand, Debug)]
pub enum PricesAction {
    /// Fetches the price sources again now, then lists the prices.
    Refresh,
}

/// The elements of a JSON array (empty when it is not one).
fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

/// A number field (0 when missing).
fn num(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

/// Dollars: "$0.00", four places under a cent, else two.
fn usd(v: f64) -> String {
    if v == 0.0 {
        "$0.00".to_string()
    } else if v < 0.01 {
        format!("${v:.4}")
    } else {
        format!("${v:.2}")
    }
}

/// A fraction as a percentage with one decimal.
fn pct(fraction: f64) -> String {
    format!("{:.1}%", fraction * 100.0)
}

/// What a call cost: the provider's reported cost where it gave one, else the estimate.
fn call_usd(call: &Value) -> f64 {
    call.get("cost_usd")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| num(call, "est_usd"))
}

/// A price rate, or "—" when the source does not price it.
fn rate(price: &Value, key: &str) -> String {
    match ctx::text(price, key) {
        text if text.is_empty() => "—".to_string(),
        text => text,
    }
}

/// The window's length and the bucket its chart uses (the web's ranges).
fn window_of(window: &str) -> Result<(u64, &'static str), String> {
    match window {
        "24h" => Ok((DAY_MS, "hour")),
        "7d" => Ok((7 * DAY_MS, "hour")),
        "30d" => Ok((30 * DAY_MS, "day")),
        "90d" => Ok((90 * DAY_MS, "day")),
        other => Err(format!("window must be 24h, 7d, 30d or 90d, not {other:?}")),
    }
}

/// The window ending at `now`, as `(from, to)` in epoch milliseconds.
pub fn window_range(window: &str, now: u64) -> Result<(u64, u64), String> {
    let (span, _) = window_of(window)?;
    Ok((now.saturating_sub(span), now))
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

/// The path of one model's price: the model id is one path segment, so its `/` is encoded.
fn price_path(model: &str) -> String {
    format!("/v1/prices/{}", ctx::seg(model))
}

/// Epoch milliseconds now.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// One row of totals: the group's key, then its calls, tokens, cache hit rate and spend.
fn totals_row(key: &str, totals: &Value) -> Vec<String> {
    vec![
        key.to_string(),
        ctx::text(totals, "calls"),
        ctx::text(totals, "input"),
        ctx::text(totals, "output"),
        ctx::text(totals, "cache_read"),
        pct(num(totals, "cache_hit_rate")),
        usd(num(totals, "usd")),
    ]
}

/// The price list: when it was fetched, the sources it came from, then every model's rates.
fn print_listing(v: &Value) {
    println!("fetched {}", ctx::when(&v["fetched_at_ms"]));
    println!();
    let sources: Vec<Vec<String>> = array(v.get("sources").unwrap_or(&Value::Null))
        .iter()
        .map(|s| {
            vec![
                ctx::s(s, "name").to_string(),
                ctx::text(s, "models"),
                ctx::text(s, "ok"),
                ctx::s(s, "error").to_string(),
            ]
        })
        .collect();
    ctx::table(&["source", "models", "ok", "error"], &sources);
    println!();
    println!("USD per million tokens");
    let rows: Vec<Vec<String>> = array(v.get("models").unwrap_or(&Value::Null))
        .iter()
        .map(|m| {
            let price = m.get("price").unwrap_or(&Value::Null);
            vec![
                ctx::s(m, "model").to_string(),
                rate(price, "input"),
                rate(price, "output"),
                rate(price, "cache_read"),
                rate(price, "cache_write"),
                ctx::s(price, "source").to_string(),
            ]
        })
        .collect();
    ctx::table(
        &[
            "model",
            "input",
            "output",
            "cache read",
            "cache write",
            "source",
        ],
        &rows,
    );
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::Summary { window } => {
            let (from, to) = window_range(&window, now_ms())?;
            let v = ctx.client.get(&format!(
                "/v1/usage/summary{}",
                query(&[
                    ("from", Some(from.to_string())),
                    ("to", Some(to.to_string()))
                ])
            ))?;
            ctx.out(&v, |v| {
                let t = v.get("totals").unwrap_or(&Value::Null);
                ctx::fields(&[
                    ("window", window.clone()),
                    ("calls", ctx::text(t, "calls")),
                    ("input tokens", ctx::text(t, "input")),
                    ("output tokens", ctx::text(t, "output")),
                    ("cache read", ctx::text(t, "cache_read")),
                    ("cache write", ctx::text(t, "cache_write")),
                    ("cache hit rate", pct(num(t, "cache_hit_rate"))),
                    ("spend", usd(num(t, "usd"))),
                    ("reported cost", usd(num(t, "cost_usd"))),
                    ("estimated cost", usd(num(t, "est_usd"))),
                ]);
                println!();
                let rows: Vec<Vec<String>> = array(v.get("by_model").unwrap_or(&Value::Null))
                    .iter()
                    .map(|g| totals_row(ctx::s(g, "key"), g.get("totals").unwrap_or(&Value::Null)))
                    .collect();
                ctx::table(
                    &[
                        "model",
                        "calls",
                        "input",
                        "output",
                        "cache read",
                        "cache hit",
                        "spend",
                    ],
                    &rows,
                );
                let unpriced: Vec<&str> = array(v.get("unpriced").unwrap_or(&Value::Null))
                    .iter()
                    .filter_map(Value::as_str)
                    .collect();
                if !unpriced.is_empty() {
                    println!();
                    println!("no price for (estimated at $0): {}", unpriced.join(", "));
                }
            });
        }
        Cmd::Series { window, bucket, by } => {
            let (from, to) = window_range(&window, now_ms())?;
            let bucket = match bucket {
                Some(bucket) => bucket,
                None => window_of(&window)?.1.to_string(),
            };
            let path = format!(
                "/v1/usage/series{}",
                query(&[
                    ("from", Some(from.to_string())),
                    ("to", Some(to.to_string())),
                    ("bucket", Some(bucket)),
                    ("by", Some(by.clone())),
                ])
            );
            let v = ctx.client.get(&path)?;
            ctx.out(&v, |v| {
                let mut rows: Vec<Vec<String>> = Vec::new();
                for point in array(v.get("points").unwrap_or(&Value::Null)) {
                    let at = ctx::when(&point["at_ms"]);
                    // The groups of one bucket, biggest spend first.
                    let mut groups: Vec<(&String, &Value)> = point
                        .get("groups")
                        .and_then(Value::as_object)
                        .map(|m| m.iter().collect())
                        .unwrap_or_default();
                    groups.sort_by(|a, b| num(b.1, "usd").total_cmp(&num(a.1, "usd")));
                    for (key, totals) in groups {
                        rows.push(vec![
                            at.clone(),
                            key.clone(),
                            ctx::text(totals, "calls"),
                            ctx::text(totals, "input"),
                            ctx::text(totals, "output"),
                            usd(num(totals, "usd")),
                        ]);
                    }
                }
                ctx::table(
                    &["at", by.as_str(), "calls", "input", "output", "spend"],
                    &rows,
                );
            });
        }
        Cmd::Calls {
            limit,
            before,
            model,
            cid,
        } => {
            let path = format!(
                "/v1/usage/calls{}",
                query(&[
                    ("limit", limit.map(|n| n.to_string())),
                    ("before", before.map(|id| id.to_string())),
                    ("model", model),
                    ("cid", cid.map(|c| c.to_string())),
                ])
            );
            let v = ctx.client.get(&path)?;
            ctx.out(&v, |v| {
                let rows: Vec<Vec<String>> = array(v.get("calls").unwrap_or(&Value::Null))
                    .iter()
                    .map(|c| {
                        vec![
                            ctx::text(c, "id"),
                            ctx::when(&c["at_ms"]),
                            ctx::text(c, "cid"),
                            ctx::s(c, "role").to_string(),
                            ctx::s(c, "model").to_string(),
                            ctx::text(c, "input"),
                            ctx::text(c, "output"),
                            ctx::text(c, "cache_read"),
                            usd(call_usd(c)),
                            ctx::text(c, "task"),
                        ]
                    })
                    .collect();
                ctx::table(
                    &[
                        "id",
                        "at",
                        "agent",
                        "role",
                        "model",
                        "input",
                        "output",
                        "cache read",
                        "spend",
                        "task",
                    ],
                    &rows,
                );
                if let Some(next) = v.get("next_before").and_then(Value::as_u64) {
                    println!();
                    println!("next page: --before {next}");
                }
            });
        }
        Cmd::Prices { model, action } => match (action, model) {
            (Some(PricesAction::Refresh), _) => {
                let v = ctx.client.call("POST", "/v1/prices/refresh", None)?;
                ctx.out(&v, |v| {
                    println!("refreshed the price sources");
                    println!();
                    print_listing(v);
                });
            }
            (None, Some(model)) => {
                let v = ctx.client.get(&price_path(&model))?;
                ctx.out(&v, |v| {
                    let price = v.get("price").unwrap_or(&Value::Null);
                    ctx::fields(&[
                        ("model", ctx::text(v, "model")),
                        ("input", rate(price, "input")),
                        ("output", rate(price, "output")),
                        ("cache read", rate(price, "cache_read")),
                        ("cache write", rate(price, "cache_write")),
                        ("source", ctx::s(price, "source").to_string()),
                    ]);
                    println!();
                    println!("USD per million tokens");
                });
            }
            (None, None) => {
                let v = ctx.client.get("/v1/prices")?;
                ctx.out(&v, print_listing);
            }
        },
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_window_is_the_span_before_now() {
        let now = 100 * DAY_MS;
        assert_eq!(window_range("24h", now), Ok((99 * DAY_MS, now)));
        assert_eq!(window_range("7d", now), Ok((93 * DAY_MS, now)));
        assert_eq!(window_range("30d", now), Ok((70 * DAY_MS, now)));
        assert_eq!(window_range("90d", now), Ok((10 * DAY_MS, now)));
        assert!(window_range("2d", now).is_err());
        assert!(window_range("", now).is_err());
    }

    #[test]
    fn the_short_windows_bucket_by_hour_and_the_long_ones_by_day() {
        assert_eq!(window_of("24h").unwrap().1, "hour");
        assert_eq!(window_of("7d").unwrap().1, "hour");
        assert_eq!(window_of("30d").unwrap().1, "day");
        assert_eq!(window_of("90d").unwrap().1, "day");
    }

    #[test]
    fn a_window_never_starts_before_the_epoch() {
        assert_eq!(window_range("90d", DAY_MS), Ok((0, DAY_MS)));
    }

    #[test]
    fn a_model_id_is_one_encoded_path_segment() {
        assert_eq!(
            price_path("anthropic/claude-x"),
            "/v1/prices/anthropic%2Fclaude-x"
        );
        assert_eq!(price_path("glm-5.3"), "/v1/prices/glm-5.3");
    }

    #[test]
    fn query_strings_skip_the_unset_and_the_empty_and_encode_values() {
        assert_eq!(query(&[("from", None), ("to", None)]), "");
        assert_eq!(
            query(&[
                ("from", Some("1".into())),
                ("model", None),
                ("to", Some("2".into()))
            ]),
            "?from=1&to=2"
        );
        assert_eq!(query(&[("model", Some(String::new()))]), "");
        assert_eq!(
            query(&[("model", Some("anthropic/claude-x".into()))]),
            "?model=anthropic%2Fclaude-x"
        );
    }

    #[test]
    fn a_call_costs_what_the_provider_reported_else_the_estimate() {
        assert_eq!(call_usd(&json!({ "cost_usd": null, "est_usd": 0.5 })), 0.5);
        assert_eq!(call_usd(&json!({ "cost_usd": 0.2, "est_usd": 0.5 })), 0.2);
    }

    #[test]
    fn money_and_percentages_read_as_people_write_them() {
        assert_eq!(usd(0.0), "$0.00");
        assert_eq!(usd(0.005), "$0.0050");
        assert_eq!(usd(12.5), "$12.50");
        assert_eq!(pct(0.5), "50.0%");
    }

    #[test]
    fn prices_takes_a_model_or_the_refresh_action() {
        use clap::Parser;
        #[derive(Parser)]
        struct Wrap {
            #[command(subcommand)]
            cmd: Cmd,
        }
        let parse = |args: &[&str]| {
            Wrap::try_parse_from(std::iter::once("usage").chain(args.iter().copied()))
                .map(|w| w.cmd)
        };
        assert!(matches!(
            parse(&["prices", "refresh"]),
            Ok(Cmd::Prices {
                model: None,
                action: Some(PricesAction::Refresh)
            })
        ));
        assert!(
            matches!(parse(&["prices", "anthropic/claude-x"]), Ok(Cmd::Prices { model: Some(ref m), action: None }) if m == "anthropic/claude-x")
        );
        assert!(matches!(
            parse(&["prices"]),
            Ok(Cmd::Prices {
                model: None,
                action: None
            })
        ));
        assert!(parse(&["summary", "--window", "2d"]).is_err());
        assert!(parse(&["calls", "--limit", "0"]).is_err());
    }

    #[test]
    fn a_price_the_source_does_not_give_reads_as_a_dash() {
        let price = json!({ "input": 3.0, "cache_read": null });
        assert_eq!(rate(&price, "input"), "3.0");
        assert_eq!(rate(&price, "cache_read"), "—");
        assert_eq!(rate(&price, "missing"), "—");
    }
}
