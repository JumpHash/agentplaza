//! `plaza_toolbox`'s answer: the toolbox as it stands for one agent, from live state.

use std::collections::HashSet;

use crate::agents::Agent;
use crate::connectors::{self, Connector};
use crate::state::App;
use crate::toolbox::Discovery;

/// The most discoveries listed.
const MAX_FOUND: usize = 30;
/// The most characters of a first sentence.
const SENTENCE_MAX: usize = 160;
/// The most characters of a discovery's how or verdict.
const FIELD_MAX: usize = 300;
/// The line that closes every survey.
const CLOSING: &str = "Nothing here fits? Search the web for what professionals use, install it in your VM, try it, and record it with plaza_toolbox_share.";

/// A plain-text survey for `agent`, filtered by the words of `query` when it is not empty.
pub fn survey(app: &App, agent: &Agent, query: &str) -> String {
    let query = query.trim();
    let words = words_of(query);
    let filtered = !words.is_empty();
    let who = agent.label();

    let mut out: Vec<String> = vec![if filtered {
        format!("Toolbox for {who}, matching \"{query}\"")
    } else {
        format!("Toolbox for {who}")
    }];

    // Classify every tool this agent is offered. A connector counts as enabled when one of its
    // tools is among those offered here.
    let mut vm: Vec<String> = Vec::new();
    let mut plaza: Vec<String> = Vec::new();
    let mut built: Vec<String> = Vec::new();
    let mut services: Vec<(&'static Connector, Vec<(String, String)>)> = Vec::new();
    let mut enabled: HashSet<&'static str> = HashSet::new();
    for spec in crate::tools::specs_for_agent(app, agent) {
        let (name, description) = (spec.name, spec.description);
        let text = format!("{name} {description}");
        if name.starts_with(crate::library::PREFIX) {
            if wanted(&words, &text) {
                built.push(format!("- {name}: {}", first_sentence(&description)));
            }
        } else if let Some((connector, action)) = connectors::by_tool(&name) {
            enabled.insert(connector.id);
            let index = match services.iter().position(|(c, _)| c.id == connector.id) {
                Some(index) => index,
                None => {
                    services.push((connector, Vec::new()));
                    services.len() - 1
                }
            };
            services[index]
                .1
                .push((name, action.description.to_string()));
        } else {
            match crate::tools::runs(&name) {
                Some(crate::tools::Runs::Guest) if wanted(&words, &text) => {
                    vm.push(format!("- {name}: {}", first_sentence(&description)));
                }
                Some(crate::tools::Runs::Host) if wanted(&words, &text) => {
                    plaza.push(format!("- {name}: {}", first_sentence(&description)));
                }
                _ => {}
            }
        }
    }

    let mut service_lines: Vec<String> = Vec::new();
    for (connector, actions) in &services {
        let connector_text = format!(
            "{} {} {}",
            connector.label, connector.category, connector.description
        );
        let connector_hit = wanted(&words, &connector_text);
        let hits: Vec<String> = actions
            .iter()
            .filter(|(n, d)| connector_hit || wanted(&words, &format!("{n} {d}")))
            .map(|(n, d)| format!("  - {n}: {}", first_sentence(d)))
            .collect();
        if hits.is_empty() {
            continue;
        }
        service_lines.push(format!(
            "- {} ({}): {}",
            connector.label,
            connector.category,
            first_sentence(connector.description)
        ));
        service_lines.extend(hits);
    }

    let mut not_enabled: Vec<String> = Vec::new();
    for connector in connectors::catalog() {
        if enabled.contains(&connector.id) {
            continue;
        }
        let connector_text = format!(
            "{} {} {}",
            connector.label, connector.category, connector.description
        );
        if wanted(&words, &connector_text) {
            not_enabled.push(format!(
                "- {} ({}): {}",
                connector.label,
                connector.category,
                first_sentence(connector.description)
            ));
        }
    }

    let mut found: Vec<Discovery> = app.toolbox.search(query);
    if found.is_empty() && filtered {
        found = app
            .toolbox
            .search("")
            .into_iter()
            .filter(|d| matches_any(&words, &discovery_text(d)))
            .collect();
    }
    // Flagged discoveries go after the unflagged ones; the sort is stable, so each group keeps its order.
    found.sort_by_key(|d| d.flagged.is_some());
    let found_lines: Vec<String> = found.iter().take(MAX_FOUND).flat_map(found_lines).collect();

    section(&mut out, "In your VM", vm);
    section(&mut out, "On the plaza", plaza);
    section(
        &mut out,
        "Outside services (keys held by the plaza)",
        service_lines,
    );
    if !not_enabled.is_empty() {
        let mut lines = vec![
            "The owner can enable these; propose it on the board or with plaza_connector_propose if you need one."
                .to_string(),
        ];
        lines.extend(not_enabled);
        section(&mut out, "Not enabled yet", lines);
    }
    section(&mut out, "Built by agents", built);
    section(&mut out, "Found by agents", found_lines);

    out.push(String::new());
    out.push(CLOSING.to_string());
    out.join("\n")
}

/// Adds a titled block of `lines` to `out`, unless there are none.
fn section(out: &mut Vec<String>, title: &str, lines: Vec<String>) {
    if lines.is_empty() {
        return;
    }
    out.push(String::new());
    out.push(format!("## {title}"));
    out.extend(lines);
}

/// The listing of one discovery: its header line, then how, verdict and url when known.
fn found_lines(d: &Discovery) -> Vec<String> {
    let mut header = format!(
        "- {} [{}] — {} (by {}, {} endorsements)",
        d.name,
        d.kind,
        first_sentence(&d.what_for),
        d.by,
        d.endorsed_by.len()
    );
    if let Some(reason) = d.flagged.as_deref() {
        header.push_str(&format!(" ⚠ owner flagged: {reason}"));
    }
    let mut lines = vec![header];
    lines.push(format!("  how: {}", clip(&d.how, FIELD_MAX)));
    lines.push(format!("  verdict: {}", clip(&d.verdict, FIELD_MAX)));
    if let Some(url) = d.url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
        lines.push(format!("  url: {url}"));
    }
    lines
}

/// The text a discovery is searched by, when the store's own search finds nothing.
fn discovery_text(d: &Discovery) -> String {
    format!(
        "{} {} {} {} {} {}",
        d.name,
        d.kind,
        d.what_for,
        d.how,
        d.verdict,
        d.tags.join(" ")
    )
}

/// The lowercase words of a query.
fn words_of(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// True when there is no filter, or when `text` holds one of `words`.
fn wanted(words: &[String], text: &str) -> bool {
    words.is_empty() || matches_any(words, text)
}

/// True when `text` contains any of `words`, ignoring case.
fn matches_any(words: &[String], text: &str) -> bool {
    let text = text.to_lowercase();
    words.iter().any(|w| text.contains(w.as_str()))
}

/// `text` with runs of whitespace collapsed to single spaces.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first `max` characters of `text`, whitespace collapsed.
fn clip(text: &str, max: usize) -> String {
    collapse(text).chars().take(max).collect()
}

/// The text up to and including the first ". ", or the whole text; at most `SENTENCE_MAX` chars.
fn first_sentence(text: &str) -> String {
    let text = collapse(text);
    let head = match text.find(". ") {
        Some(i) => &text[..=i],
        None => text.as_str(),
    };
    head.chars().take(SENTENCE_MAX).collect()
}
