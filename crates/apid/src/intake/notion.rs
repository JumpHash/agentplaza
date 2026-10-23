//! Notion intake (PLAN 14.5, Engine 5.3): a database's pages, polled from the last edit the poll read, and the write-back
//! to the page's status property and a comment on the page.

use serde_json::{Map, Value, json};

use super::{Api, Incoming, IntakeRule, Item, Phase, comment_of, has};
use crate::state::App;

/// The block kinds whose text an item's body keeps.
const TEXT_BLOCKS: [&str; 10] = [
    "paragraph",
    "heading_1",
    "heading_2",
    "heading_3",
    "bulleted_list_item",
    "numbered_list_item",
    "to_do",
    "quote",
    "callout",
    "code",
];

/// The pages of `database` edited after `after` (ISO 8601), oldest first, following the cursor to the end.
pub fn query(api: &Api<'_>, database: &str, after: &str) -> Result<Vec<Value>, String> {
    let url = format!("{}/v1/databases/{database}/query", api.base);
    let mut pages = Vec::new();
    let mut start: Option<String> = None;
    loop {
        let mut body = json!({
            "filter": { "timestamp": "last_edited_time", "last_edited_time": { "after": after } },
            "sorts": [{ "timestamp": "last_edited_time", "direction": "ascending" }],
            "page_size": 100
        });
        if let Some(cursor) = &start {
            body["start_cursor"] = json!(cursor);
        }
        let answer = api.call("POST", &url, Some(&body))?;
        pages.extend(answer["results"].as_array().into_iter().flatten().cloned());
        match answer["next_cursor"].as_str() {
            Some(next) if answer["has_more"] == true => start = Some(next.to_string()),
            _ => break,
        }
    }
    Ok(pages)
}

/// The text of a page's blocks (its body), one line per block.
pub fn page_text(api: &Api<'_>, page: &Value) -> Result<String, String> {
    let id = page["id"].as_str().unwrap_or("");
    let url = format!("{}/v1/blocks/{id}/children?page_size=100", api.base);
    let answer = api.call("GET", &url, None)?;
    let lines: Vec<String> = answer["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| {
            block["type"]
                .as_str()
                .is_some_and(|kind| TEXT_BLOCKS.contains(&kind))
        })
        .filter_map(|block| {
            let kind = block["type"].as_str()?;
            let text = plain(&block[kind]["rich_text"]);
            (!text.is_empty()).then_some(text)
        })
        .collect();
    Ok(lines.join("\n"))
}

/// The text of rich text (Notion's `plain_text` runs).
fn plain(rich: &Value) -> String {
    rich.as_array()
        .into_iter()
        .flatten()
        .filter_map(|run| run["plain_text"].as_str())
        .collect()
}

/// The page's title: its one `title` property's text.
fn title_of(page: &Value) -> String {
    page["properties"]
        .as_object()
        .and_then(|props| props.values().find(|p| p["type"] == "title"))
        .map(|p| plain(&p["title"]))
        .unwrap_or_default()
}

/// A page as an item: keyed by a short form of its id, scoped to its database.
pub fn incoming(page: &Value, database: &str, body: String) -> Incoming {
    let id = page["id"].as_str().unwrap_or("").to_string();
    let short: String = id.chars().filter(|c| *c != '-').take(8).collect();
    Incoming {
        source: super::NOTION.into(),
        external_id: id,
        key: format!("notion-{short}"),
        scope: database.to_string(),
        title: title_of(page),
        body,
        labels: Vec::new(),
        links: page["url"]
            .as_str()
            .filter(|url| !url.is_empty())
            .map(|url| vec![url.to_string()])
            .unwrap_or_default(),
        author: String::new(),
        public: false,
    }
}

/// The write-back to Notion: a comment on the page, then its status property moves to the rule's started or done
/// option (`In progress` and `Done` by default). A status or a select property; any other type is refused.
pub fn push(
    app: &App,
    item: &Item,
    rule: &IntakeRule,
    done: &[String],
    phase: Phase,
) -> Result<Vec<(String, Result<String, String>)>, String> {
    let api = super::api(app, super::NOTION)?;
    let word = phase.word();
    let property = match rule.status_property.trim() {
        "" => "Status",
        named => named,
    };
    let option = match (phase, rule.started.trim(), rule.done.trim()) {
        (Phase::Started, named, _) if !named.is_empty() => named,
        (Phase::Started, _, _) => "In progress",
        (Phase::Landed, _, named) if !named.is_empty() => named,
        (Phase::Landed, _, _) => "Done",
    };
    let mut out = Vec::new();
    let comment_kind = format!("{word}:comment");
    if !has(done, &comment_kind) {
        let body = json!({
            "parent": { "page_id": item.external_id },
            "rich_text": [{ "type": "text", "text": { "content": comment_of(item, phase) } }]
        });
        let result = api
            .call("POST", &format!("{}/v1/comments", api.base), Some(&body))
            .map(|_| "commented".to_string());
        out.push((comment_kind, result));
    }
    let status_kind = format!("{word}:status");
    if !has(done, &status_kind) {
        let result = set_status(&api, &item.external_id, property, option);
        out.push((status_kind, result));
    }
    Ok(out)
}

/// Sets the page's status property to `option`, as the property's own type says.
fn set_status(api: &Api<'_>, page: &str, property: &str, option: &str) -> Result<String, String> {
    let current = api.call("GET", &format!("{}/v1/pages/{page}", api.base), None)?;
    let kind = current["properties"][property]["type"]
        .as_str()
        .ok_or_else(|| format!("the page has no property {property}"))?;
    let value = match kind {
        "status" => json!({ "status": { "name": option } }),
        "select" => json!({ "select": { "name": option } }),
        other => {
            return Err(format!(
                "property {property} is {other}, not a status or select"
            ));
        }
    };
    let mut properties = Map::new();
    properties.insert(property.to_string(), value);
    api.call(
        "PATCH",
        &format!("{}/v1/pages/{page}", api.base),
        Some(&json!({ "properties": properties })),
    )?;
    Ok(format!("{property} {option}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_becomes_an_item_titled_by_its_title_property() {
        let page = json!({
            "id": "0f2a1b3c-4d5e-6f70-8192-a3b4c5d6e7f8",
            "url": "https://www.notion.so/page",
            "properties": {
                "Status": { "type": "status", "status": { "name": "Todo" } },
                "Name": { "type": "title", "title": [{ "plain_text": "Add " }, { "plain_text": "export" }] }
            }
        });
        let item = incoming(&page, "db", "Body".into());
        assert_eq!(item.title, "Add export");
        assert_eq!(item.key, "notion-0f2a1b3c");
        assert_eq!(item.links, vec!["https://www.notion.so/page".to_string()]);
    }
}
