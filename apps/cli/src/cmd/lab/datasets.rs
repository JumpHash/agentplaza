//! `agentplaza lab datasets`: the datasets the lab trains on. An export is cut from the plaza's
//! records (cascade, tasks, usage or all) over an optional range; an upload is a JSONL file.

use clap::Subcommand;
use serde_json::{Value, json};

use super::{refusal, short};
use crate::cmd::models::items;
use crate::ctx::{self, Ctx};

const KINDS: [&str; 4] = ["cascade", "tasks", "usage", "all"];

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The datasets, newest first: their kind, their rows and when they were made.
    List,
    /// Export a dataset from the plaza's records. --since is inclusive and --until exclusive: epoch
    /// milliseconds, or a UTC date or date and time (2026-10-01, 2026-10-01T12:30).
    Export {
        /// The records: cascade, tasks, usage or all.
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(KINDS))]
        kind: String,
        /// The dataset's name.
        #[arg(long)]
        name: String,
        /// The first moment to include.
        #[arg(long)]
        since: Option<String>,
        /// The moment to stop before.
        #[arg(long)]
        until: Option<String>,
    },
    /// Upload a JSONL file as a dataset. Each row needs `ts` (unix milliseconds), `label` and the recipe's feature fields.
    Upload {
        /// The JSONL file.
        file: String,
        /// The dataset's name.
        #[arg(long)]
        name: String,
    },
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List => {
            let list = ctx.client.get("/v1/foundry/datasets")?;
            ctx.out(&list, |v| {
                let rows: Vec<Vec<String>> = items(v)
                    .map(|d| {
                        vec![
                            ctx::s(d, "id").to_string(),
                            ctx::s(d, "name").to_string(),
                            ctx::s(d, "kind").to_string(),
                            ctx::text(d, "rows"),
                            ctx::text(d, "frozen_rows"),
                            ctx::when(&d["created_ms"]),
                            short(ctx::s(d, "sha256")),
                        ]
                    })
                    .collect();
                ctx::table(
                    &["id", "name", "kind", "rows", "frozen", "created", "sha256"],
                    &rows,
                );
            });
            Ok(())
        }
        Cmd::Export {
            kind,
            name,
            since,
            until,
        } => {
            let since = since.as_deref().map(parse_when).transpose()?;
            let until = until.as_deref().map(parse_when).transpose()?;
            let spec = export_body(&kind, &name, since, until)?;
            let ds = ctx.client.post("/v1/foundry/datasets", &spec)?;
            ctx.out(&ds, |d| {
                println!(
                    "exported dataset {} ({} rows)",
                    ctx::s(d, "id"),
                    ctx::text(d, "rows")
                )
            });
            Ok(())
        }
        Cmd::Upload { file, name } => {
            let bytes = std::fs::read(&file).map_err(|e| format!("{file}: {e}"))?;
            let ds = raw_post(
                ctx,
                &format!("/v1/foundry/datasets/upload?name={}", ctx::seg(&name)),
                &bytes,
            )?;
            ctx.out(&ds, |d| {
                println!(
                    "uploaded dataset {} ({} rows)",
                    ctx::s(d, "id"),
                    ctx::text(d, "rows")
                )
            });
            Ok(())
        }
    }
}

/// A POST with a raw body (the JSONL file), not JSON. The shared client labels every body as JSON; the
/// hub reads the upload's bytes as they are, so the label does not matter to it.
fn raw_post(ctx: &Ctx, path: &str, body: &[u8]) -> Result<Value, String> {
    let reply = apz_http::client::request(
        &ctx.client.target,
        "POST",
        path,
        Some(body),
        ctx.client.timeout,
    )
    .map_err(|e| format!("{} {path}: {e}", ctx.client.base()))?;
    let value = if reply.body.is_empty() {
        Value::Null
    } else {
        reply.json().unwrap_or(Value::Null)
    };
    if (200..300).contains(&reply.status) {
        Ok(value)
    } else {
        Err(refusal(&value, path, reply.status))
    }
}

/// The export request: the kind, the name, and each bound that is given. A range that is empty is refused.
pub fn export_body(
    kind: &str,
    name: &str,
    since: Option<u64>,
    until: Option<u64>,
) -> Result<Value, String> {
    if since.zip(until).is_some_and(|(from, to)| from >= to) {
        return Err("--until must be after --since".into());
    }
    let mut body = json!({ "kind": kind, "name": name });
    if let Some(from) = since {
        body["since_ms"] = json!(from);
    }
    if let Some(to) = until {
        body["until_ms"] = json!(to);
    }
    Ok(body)
}

/// A time for --since and --until: epoch milliseconds, or a UTC date (`2026-10-01`) or date and time
/// (`2026-10-01T12:30`, seconds optional, a trailing `Z` allowed).
pub fn parse_when(text: &str) -> Result<u64, String> {
    let trimmed = text.trim();
    if let Ok(ms) = trimmed.parse::<u64>() {
        return Ok(ms);
    }
    let bad = || {
        format!(
            "{text}: a time is epoch milliseconds, or a UTC date or date and time (2026-10-01, 2026-10-01T12:30)"
        )
    };
    let body = trimmed.strip_suffix('Z').unwrap_or(trimmed);
    let (date, clock) = match body.split_once('T') {
        Some((date, clock)) => (date, Some(clock)),
        None => (body, None),
    };
    let fields: Vec<i64> = date
        .split('-')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .map_err(|_| bad())?;
    let [year, month, day] = fields[..] else {
        return Err(bad());
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(bad());
    }
    let (hour, minute, second) = match clock {
        None => (0, 0, 0),
        Some(clock) => {
            let parts: Vec<i64> = clock
                .split(':')
                .map(str::parse)
                .collect::<Result<_, _>>()
                .map_err(|_| bad())?;
            match parts[..] {
                [h, m] => (h, m, 0),
                [h, m, s] => (h, m, s),
                _ => return Err(bad()),
            }
        }
    };
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) || !(0..=59).contains(&second) {
        return Err(bad());
    }
    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;
    u64::try_from(seconds * 1000).map_err(|_| bad())
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's civil-day count).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_is_epoch_milliseconds_or_a_utc_date_or_moment() {
        assert_eq!(parse_when("86400000").unwrap(), 86_400_000);
        assert_eq!(parse_when("1970-01-02").unwrap(), 86_400_000);
        assert_eq!(parse_when("1970-01-01T00:00:30Z").unwrap(), 30_000);
        assert_eq!(parse_when("1970-01-01T01:02").unwrap(), 3_720_000);
        assert_eq!(parse_when("2000-03-01").unwrap(), 11_017 * 86_400_000);
        for bad in [
            "2026-13-01",
            "2026-10-32",
            "2026-10-01T25:00",
            "2026-10",
            "soon",
            "2026-10-01T10:00:00:00",
        ] {
            assert!(parse_when(bad).is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn an_export_names_its_kind_and_only_the_bounds_given() {
        assert_eq!(
            export_body("cascade", "may", Some(1), Some(5)).unwrap(),
            json!({ "kind": "cascade", "name": "may", "since_ms": 1, "until_ms": 5 })
        );
        assert_eq!(
            export_body("all", "x", None, None).unwrap(),
            json!({ "kind": "all", "name": "x" })
        );
        assert_eq!(
            export_body("tasks", "t", Some(9), None).unwrap(),
            json!({ "kind": "tasks", "name": "t", "since_ms": 9 })
        );
        assert!(
            export_body("all", "x", Some(5), Some(5)).is_err(),
            "an empty range is refused"
        );
    }
}
