//! `agentplaza unfurl <url-or-ref>`: the hub's card for one reference, the hover card of the web and the desktop,
//! as a few lines. The hub answers `GET /v1/unfurl?url=` for a link and `POST /v1/unfurl {refs}` for a reference
//! (docs/design/workspace.md, section 3.3). `--json` prints the hub's answer.

use apz_rich::{Ctx as RichCtx, Ref, Span, classify};
use serde_json::{Value, json};

use crate::ctx::{self, Ctx};
use crate::rich::{self, Mode};

/// `agentplaza unfurl` options.
#[derive(clap::Args, Debug)]
pub struct Args {
    /// A link (GitHub, the plaza's git, a hub file or page), `#N` or `task #N` for a task, or `agent:<cid>` for an agent.
    pub target: String,
}

pub fn run(ctx: &Ctx, args: Args) -> Result<(), String> {
    let target = args.target.trim();
    // The hub's host for the excerpt's links; a link in JSON needs none of it.
    let rich_ctx = if ctx.json && is_url(target) {
        RichCtx::default()
    } else {
        rich::context(&ctx.client)
    };
    let answer = if is_url(target) {
        ctx.client
            .get(&format!("/v1/unfurl?url={}", ctx::seg(target)))?
    } else {
        let found = reference(target, &rich_ctx).ok_or_else(|| {
            format!("not a link or a reference: {target} (give a link, #N for a task, or agent:<cid> for an agent)")
        })?;
        let batch = ctx.client.post("/v1/unfurl", &json!({ "refs": [found] }))?;
        batch["cards"].get(0).cloned().unwrap_or(Value::Null)
    };
    let shown_ok = answer["ok"].as_bool() == Some(true);
    ctx.out(&answer, |a| {
        print!("{}", format_answer(a, &rich_ctx, &ctx.rich))
    });
    if shown_ok {
        Ok(())
    } else {
        Err(error_of(&answer))
    }
}

fn is_url(target: &str) -> bool {
    target.starts_with("http://") || target.starts_with("https://")
}

/// The reference a target names: a task (`#N`, `task #N`, `task N`), else the first reference in the text.
fn reference(target: &str, ctx: &RichCtx) -> Option<Ref> {
    if let Some(number) = task_number(target) {
        return Some(Ref::Task { number });
    }
    classify(target, ctx)
        .into_iter()
        .find_map(|span| match span {
            Span::Ref { r, .. } => Some(r),
            _ => None,
        })
}

fn task_number(target: &str) -> Option<u64> {
    let t = target.trim();
    let digits = match t.strip_prefix("task") {
        Some(rest) => rest.trim().trim_start_matches('#'),
        None => t.strip_prefix('#')?,
    };
    digits.parse().ok()
}

/// The card as lines: the kind and title, the author and state, the excerpt (references in it drawn as in the
/// other printers), and the link. A failed answer says why the card is missing.
pub fn format_answer(answer: &Value, rich_ctx: &RichCtx, mode: &Mode) -> String {
    if answer["ok"].as_bool() != Some(true) {
        return format!("preview unavailable: {}\n", error_of(answer));
    }
    let card = &answer["card"];
    let kind = match ctx::s(card, "kind") {
        "" => "reference".to_string(),
        k => k.replace('_', " "),
    };
    let title = first_text(card, &["title", "name"]);
    let mut lines = vec![if title.is_empty() {
        kind
    } else {
        format!("{kind}: {title}")
    }];
    let meta: Vec<String> = [author_of(card), first_text(card, &["state", "status"])]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
    if !meta.is_empty() {
        lines.push(format!("  {}", meta.join(" · ")));
    }
    let excerpt = first_text(card, &["body_excerpt", "excerpt", "summary"]);
    if !excerpt.is_empty() {
        lines.push(format!("  {}", rich::render(&excerpt, rich_ctx, mode)));
    }
    if let Some(url) = card["url"].as_str().filter(|u| !u.is_empty()) {
        lines.push(format!("  {url}"));
    }
    lines.join("\n") + "\n"
}

/// The first of the keys that holds text (strings as they are, numbers as digits).
fn first_text(card: &Value, keys: &[&str]) -> String {
    keys.iter()
        .map(|k| ctx::text(card, k))
        .find(|s| !s.is_empty())
        .unwrap_or_default()
}

/// The author's login: a card's author is an object (`{login}`) or a name.
fn author_of(card: &Value) -> String {
    match &card["author"] {
        Value::String(name) => name.clone(),
        Value::Object(_) => ctx::text(&card["author"], "login"),
        _ => String::new(),
    }
}

fn error_of(answer: &Value) -> String {
    match ctx::s(answer, "error") {
        "" => "the hub gave no card".to_string(),
        error => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/unfurl_pull.json");

    fn fixture() -> Value {
        serde_json::from_str(FIXTURE).unwrap()
    }

    fn mode(colour: bool, hyperlinks: bool) -> Mode {
        Mode {
            plain: false,
            colour,
            hyperlinks,
            hub: Some("https://hub.test".into()),
        }
    }

    #[test]
    fn the_excerpt_draws_its_references_as_labels_and_links_in_rich_mode() {
        let text = format_answer(&fixture(), &RichCtx::default(), &mode(false, true));
        assert!(
            text.contains("\x1b]8;;https://hub.test/#/agents/3\x1b\\agent:3\x1b]8;;\x1b\\"),
            "{text:?}"
        );
        assert!(
            text.contains("See #528"),
            "a task needs the project in context: {text:?}"
        );
    }

    #[test]
    fn a_target_is_a_task_an_agent_or_the_first_reference_in_the_text() {
        assert_eq!(
            reference("#528", &RichCtx::default()),
            Some(Ref::Task { number: 528 })
        );
        assert_eq!(
            reference("task 528", &RichCtx::default()),
            Some(Ref::Task { number: 528 })
        );
        assert_eq!(
            reference("task #528", &RichCtx::default()),
            Some(Ref::Task { number: 528 })
        );
        assert_eq!(
            reference("agent:3", &RichCtx::default()),
            Some(Ref::Agent {
                name: None,
                cid: Some(3)
            })
        );
        assert_eq!(reference("nothing here", &RichCtx::default()), None);
        assert!(is_url("https://github.com/o/r/pull/12"));
        assert!(!is_url("#528"));
    }
}
