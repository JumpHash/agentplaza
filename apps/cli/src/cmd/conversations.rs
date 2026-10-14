//! `agentplaza conversations`: Conversations.

use std::collections::BTreeMap;

use clap::Subcommand;
use serde_json::{json, Value};

use crate::ctx::{self, Ctx};
use crate::rich;

/// The most exchanges one page holds (the hub's `CONVERSATIONS_LIMIT`).
const PAGE_MAX: u64 = 256;
/// The longest search the hub takes, in bytes (its `MAX_QUERY`).
const QUERY_BYTES: usize = 200;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// List the conversations, one line per pair of agents, newest first.
    List {
        /// Exchanges in the page, 1 to 256 (the hub's default is 50).
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=256))]
        limit: Option<u64>,
        /// Only the exchanges older than this one (`c<n>`, as the hint under a list names it).
        #[arg(long)]
        before: Option<String>,
        /// Search the exchanges' text and roles, and the agents whose name or role matches (200 bytes at most).
        #[arg(long)]
        q: Option<String>,
    },
    /// Show the messages between two agents, oldest first: the newest page, or all with `--all`.
    Show {
        /// The two agent ids, as `3-7`.
        pair: String,
        /// Every page back to the oldest message, not only the newest 256.
        #[arg(long)]
        all: bool,
    },
    /// The conversation retention: how many exchanges are kept and for how long. `set` changes it.
    Retention {
        #[command(subcommand)]
        action: Option<RetentionCmd>,
    },
}

#[derive(Subcommand, Debug)]
pub enum RetentionCmd {
    /// Set the retention. Give `--keep`, `--days`, or both; the other settings are kept as they are.
    Set {
        /// The most exchanges kept, 100 to 100000.
        #[arg(long, value_parser = clap::value_parser!(u64).range(100..=100_000))]
        keep: Option<u64>,
        /// Days before an exchange is deleted, 0 to 3650 (0 keeps them forever).
        #[arg(long, value_parser = clap::value_parser!(u64).range(0..=3650))]
        days: Option<u64>,
    },
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List { limit, before, q } => list(ctx, limit, before, q),
        Cmd::Show { pair, all } => show(ctx, &pair, all),
        Cmd::Retention { action: None } => retention(ctx),
        Cmd::Retention { action: Some(RetentionCmd::Set { keep, days }) } => retention_set(ctx, keep, days),
    }
}

fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}

/// The array at `key` of an object (empty when missing).
fn items<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).map(array).unwrap_or_default()
}

/// The ordered pair `low-high` of the two agents a message is between.
fn pair_key(m: &Value) -> Option<String> {
    let a = m.get("from_cid")?.as_i64()?;
    let b = m.get("to_cid")?.as_i64()?;
    Some(if a <= b { format!("{a}-{b}") } else { format!("{b}-{a}") })
}

fn at(m: &Value) -> f64 {
    m.get("created_at").and_then(Value::as_f64).unwrap_or(0.0)
}

/// The two agent ids of a pair `3-7`, lower first.
fn parse_pair(pair: &str) -> Result<(u64, u64), String> {
    let refused = || format!("`{pair}` is two agent ids, like 3-7");
    let ids: Vec<u64> = pair.split('-').map(|id| id.trim().parse::<u64>().map_err(|_| refused())).collect::<Result<_, _>>()?;
    match ids.as_slice() {
        [a, b] => Ok(if a <= b { (*a, *b) } else { (*b, *a) }),
        _ => Err(refused()),
    }
}

/// An exchange id, `c<n>` or `<n>`, as the number the hub pages by.
fn parse_exchange(text: &str) -> Result<u64, String> {
    let trimmed = text.trim();
    trimmed
        .strip_prefix('c')
        .unwrap_or(trimmed)
        .parse::<u64>()
        .map_err(|_| format!("`{text}` is an exchange id, like c12"))
}

/// The owner's search: trimmed, and cut to the hub's byte cap on a character boundary (as the web does).
fn clamp_query(text: &str) -> String {
    let mut out = String::new();
    for ch in text.trim().chars() {
        if out.len() + ch.len_utf8() > QUERY_BYTES {
            break;
        }
        out.push(ch);
    }
    out
}

/// The cursor for the next older page. The hub's `next_before` is only followed when it is older than the
/// cursor this page was asked with, so a cursor that does not move back can never loop.
fn older_cursor(next: Option<u64>, asked: Option<u64>) -> Option<u64> {
    next.filter(|seq| asked.is_none_or(|prev| *seq < prev))
}

/// The pages in the order they were fetched (newest first), each oldest first, as one oldest-first list.
fn join_pages(pages: Vec<Vec<Value>>) -> Vec<Value> {
    pages.into_iter().rev().flatten().collect()
}

/// The path of a conversations listing with the paging and search it asks for.
fn listing_path(limit: Option<u64>, before: Option<u64>, q: Option<&str>) -> String {
    let mut params: Vec<String> = Vec::new();
    if let Some(limit) = limit {
        params.push(format!("limit={limit}"));
    }
    if let Some(seq) = before {
        params.push(format!("before={seq}"));
    }
    if let Some(q) = q {
        params.push(format!("q={}", ctx::seg(q)));
    }
    if params.is_empty() {
        "/v1/conversations".to_string()
    } else {
        format!("/v1/conversations?{}", params.join("&"))
    }
}

/// The command that lists the next older page, with the same limit and search.
fn more_hint(next: u64, limit: Option<u64>, q: Option<&str>) -> String {
    let mut parts = vec![format!("agentplaza conversations list --before c{next}")];
    if let Some(limit) = limit {
        parts.push(format!("--limit {limit}"));
    }
    if let Some(q) = q {
        parts.push(format!("--q {q:?}"));
    }
    parts.join(" ")
}

/// The retention in words: how many exchanges are kept, and when they go.
fn retention_line(r: &Value) -> String {
    let keep = ctx::text(r, "conversations_max");
    let days = r.get("conversations_days").and_then(Value::as_u64).unwrap_or(0);
    let age = if days > 0 { format!("deleted after {days} days") } else { "kept forever".to_string() };
    format!("keeps the newest {keep} exchanges, {age}")
}

/// The settings document with the conversation retention changed and everything else kept. `dev_models` is the
/// hub's own field, so it is dropped before a save (as the web does).
fn set_retention(settings: &mut Value, keep: Option<u64>, days: Option<u64>) -> Result<(), String> {
    let root = settings.as_object_mut().ok_or("the settings are not a JSON object")?;
    root.remove("dev_models");
    let block = root.entry("retention").or_insert_with(|| json!({}));
    let block = block.as_object_mut().ok_or("the retention settings are not a JSON object")?;
    if let Some(keep) = keep {
        block.insert("conversations_max".into(), json!(keep));
    }
    if let Some(days) = days {
        block.insert("conversations_days".into(), json!(days));
    }
    Ok(())
}

fn list(ctx: &Ctx, limit: Option<u64>, before: Option<String>, q: Option<String>) -> Result<(), String> {
    let before = before.as_deref().map(parse_exchange).transpose()?;
    let q = q.map(|text| clamp_query(&text)).filter(|text| !text.is_empty());
    let value = ctx.client.get(&listing_path(limit, before, q.as_deref()))?;
    ctx.out(&value, |v| {
        // Per pair: how many messages, and the newest one.
        let mut pairs: BTreeMap<String, (usize, &Value)> = BTreeMap::new();
        for m in items(v, "conversations") {
            let Some(key) = pair_key(m) else { continue };
            let entry = pairs.entry(key).or_insert((0, m));
            entry.0 += 1;
            if at(m) >= at(entry.1) {
                entry.1 = m;
            }
        }
        let rows: Vec<Vec<String>> = pairs
            .iter()
            .map(|(key, (count, last))| {
                vec![
                    key.clone(),
                    count.to_string(),
                    format!("{} {}", ctx::s(last, "from_role"), ctx::text(last, "from_cid")),
                    format!("{} {}", ctx::s(last, "to_role"), ctx::text(last, "to_cid")),
                    ctx::when(&last["created_at"]),
                    ctx::text(last, "text"),
                ]
            })
            .collect();
        ctx::table(&["pair", "messages", "from", "to", "last at", "last message"], &rows);
        if let Some(next) = v.get("next_before").and_then(Value::as_u64) {
            println!("\nolder: {}", more_hint(next, limit, q.as_deref()));
        }
    });
    Ok(())
}

/// The messages between two agents, oldest first: the newest page, or every page back when `all`. The flag says
/// whether older messages remain beyond what was read.
fn thread(ctx: &Ctx, a: u64, b: u64, all: bool) -> Result<(Vec<Value>, bool), String> {
    let mut pages: Vec<Vec<Value>> = Vec::new();
    let mut asked: Option<u64> = None;
    loop {
        let before = asked.map(|seq| format!("&before={seq}")).unwrap_or_default();
        let page = ctx.client.get(&format!("/v1/conversations/{a}/{b}?limit={PAGE_MAX}{before}"))?;
        pages.push(items(&page, "messages").to_vec());
        let next = older_cursor(page.get("next_before").and_then(Value::as_u64), asked);
        match next {
            Some(seq) if all => asked = Some(seq),
            _ => return Ok((join_pages(pages), next.is_some())),
        }
    }
}

fn show(ctx: &Ctx, pair: &str, all: bool) -> Result<(), String> {
    let (a, b) = parse_pair(pair)?;
    let (messages, older) = thread(ctx, a, b, all)?;
    let value = Value::Array(messages);
    let rich_ctx = rich::context_for(ctx);
    ctx.out(&value, |v| {
        if array(v).is_empty() {
            println!("(no messages between {a}-{b})");
        }
        for m in array(v) {
            let delivered = if m.get("delivered").and_then(Value::as_bool) == Some(false) { " (not delivered yet)" } else { "" };
            println!(
                "[{}] {} {} -> {} {}{delivered}\n{}\n",
                ctx::when(&m["created_at"]),
                ctx::s(m, "from_role"),
                ctx::text(m, "from_cid"),
                ctx::s(m, "to_role"),
                ctx::text(m, "to_cid"),
                rich::render(ctx::s(m, "text"), &rich_ctx, &ctx.rich),
            );
        }
        if older {
            println!("(older messages: pass --all)");
        }
    });
    Ok(())
}

fn retention(ctx: &Ctx) -> Result<(), String> {
    let settings = ctx.client.get("/v1/settings")?;
    let block = settings.get("retention").cloned().unwrap_or(Value::Null);
    ctx.out(&block, |r| println!("{}", retention_line(r)));
    Ok(())
}

fn retention_set(ctx: &Ctx, keep: Option<u64>, days: Option<u64>) -> Result<(), String> {
    if keep.is_none() && days.is_none() {
        return Err("set the retention with --keep N, --days N, or both".into());
    }
    let mut settings = ctx.client.get("/v1/settings")?;
    set_retention(&mut settings, keep, days)?;
    let saved = ctx.client.put("/v1/settings", &settings)?;
    let block = saved.get("retention").cloned().unwrap_or(Value::Null);
    ctx.out(&block, |r| println!("saved: {}", retention_line(r)));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pair_reads_lower_first_and_refuses_what_is_not_two_ids() {
        assert_eq!(parse_pair("3-7").unwrap(), (3, 7));
        assert_eq!(parse_pair("7-3").unwrap(), (3, 7));
        assert_eq!(parse_pair(" 4 - 9 ").unwrap(), (4, 9));
        for bad in ["3", "3-7-9", "3-x", "-3", "", "3-"] {
            assert!(parse_pair(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn an_exchange_id_is_c_then_a_number_or_a_number() {
        assert_eq!(parse_exchange("c12").unwrap(), 12);
        assert_eq!(parse_exchange("12").unwrap(), 12);
        assert!(parse_exchange("cx").is_err());
        assert!(parse_exchange("").is_err());
    }

    #[test]
    fn a_search_is_trimmed_and_cut_on_a_character_boundary() {
        assert_eq!(clamp_query("  hi  "), "hi");
        assert_eq!(clamp_query(&"é".repeat(150)).len(), 200);
        // "a" then 100 two-byte characters: the 100th would make 201 bytes, so it is dropped whole.
        let text = format!("a{}", "é".repeat(100));
        assert_eq!(clamp_query(&text).len(), 199);
        assert_eq!(clamp_query("   "), "");
    }

    #[test]
    fn an_older_page_is_only_asked_for_when_it_is_older() {
        assert_eq!(older_cursor(Some(40), None), Some(40));
        assert_eq!(older_cursor(Some(40), Some(90)), Some(40));
        assert_eq!(older_cursor(Some(90), Some(90)), None);
        assert_eq!(older_cursor(None, Some(90)), None);
    }

    #[test]
    fn pages_join_oldest_first_whichever_order_they_were_read_in() {
        let msg = |n: i64| json!({ "id": format!("c{n}") });
        // Read newest page first: c9..c7 then c6..c4.
        let pages = vec![vec![msg(7), msg(8), msg(9)], vec![msg(4), msg(5), msg(6)]];
        let joined = join_pages(pages);
        let ids: Vec<&str> = joined.iter().map(|m| m["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["c4", "c5", "c6", "c7", "c8", "c9"]);
    }

    #[test]
    fn a_listing_carries_only_the_paging_and_search_it_was_given() {
        assert_eq!(listing_path(None, None, None), "/v1/conversations");
        assert_eq!(listing_path(Some(20), Some(55), None), "/v1/conversations?limit=20&before=55");
        assert_eq!(listing_path(None, None, Some("a b")), "/v1/conversations?q=a%20b");
    }

    #[test]
    fn the_next_page_hint_repeats_the_limit_and_the_search() {
        assert_eq!(more_hint(12, None, None), "agentplaza conversations list --before c12");
        assert_eq!(more_hint(12, Some(20), Some("hi")), "agentplaza conversations list --before c12 --limit 20 --q \"hi\"");
    }

    #[test]
    fn retention_is_read_in_words() {
        assert_eq!(retention_line(&json!({ "conversations_max": 4096, "conversations_days": 30 })), "keeps the newest 4096 exchanges, deleted after 30 days");
        assert_eq!(retention_line(&json!({ "conversations_max": 100, "conversations_days": 0 })), "keeps the newest 100 exchanges, kept forever");
    }

    #[test]
    fn setting_retention_keeps_the_other_settings_and_drops_dev_models() {
        let mut doc = json!({
            "dev_models": true,
            "laws": { "strict_permits": true },
            "retention": { "conversations_max": 4096, "conversations_days": 30, "posts_max": 1024 }
        });
        set_retention(&mut doc, Some(500), None).unwrap();
        assert!(doc.get("dev_models").is_none());
        assert_eq!(doc["laws"]["strict_permits"], true);
        assert_eq!(doc["retention"]["conversations_max"], 500);
        assert_eq!(doc["retention"]["conversations_days"], 30);
        assert_eq!(doc["retention"]["posts_max"], 1024);
        set_retention(&mut doc, None, Some(0)).unwrap();
        assert_eq!(doc["retention"]["conversations_max"], 500);
        assert_eq!(doc["retention"]["conversations_days"], 0);
    }

    #[test]
    fn a_settings_document_without_a_retention_block_gets_one() {
        let mut doc = json!({ "laws": {} });
        set_retention(&mut doc, Some(100), Some(7)).unwrap();
        assert_eq!(doc["retention"], json!({ "conversations_max": 100, "conversations_days": 7 }));
    }
}
