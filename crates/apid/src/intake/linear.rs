//! Linear intake (PLAN 14.5, Engine 5.3): issue webhooks, and the GraphQL API's reads and writes. The webhook says
//! which issue changed; the issue's text is read from the API, so the body of the webhook is never trusted as text.

use serde_json::{Value, json};

use super::{Api, Incoming, Item, Phase, comment_of, has};
use crate::state::App;

const ISSUE: &str = "query PlazaIssue($id: String!) { issue(id: $id) { id identifier title description url \
    creator { name } team { key states { nodes { id name type } } } labels { nodes { name } } } }";
const SET_STATE: &str = "mutation PlazaState($id: String!, $state: String!) { \
    issueUpdate(id: $id, input: { stateId: $state }) { success } }";
const COMMENT: &str = "mutation PlazaComment($id: String!, $body: String!) { \
    commentCreate(input: { issueId: $id, body: $body }) { success } }";

/// One GraphQL call. A Linear error in the answer is an error here, whatever the status.
fn graphql(api: &Api<'_>, text: &str, variables: Value) -> Result<Value, String> {
    let answer = api.call(
        "POST",
        &api.base,
        Some(&json!({ "query": text, "variables": variables })),
    )?;
    if let Some(first) = answer["errors"]
        .as_array()
        .and_then(|errors| errors.first())
    {
        return Err(format!(
            "Linear: {}",
            first["message"].as_str().unwrap_or("an error")
        ));
    }
    Ok(answer["data"].clone())
}

/// The issue `id` (its UUID or its identifier), as Linear's API has it now.
pub fn fetch_issue(api: &Api<'_>, id: &str) -> Result<Value, String> {
    let data = graphql(api, ISSUE, json!({ "id": id }))?;
    if data["issue"].is_null() {
        return Err(format!("Linear has no issue {id}"));
    }
    Ok(data["issue"].clone())
}

/// An issue delivery as an item: a created or updated issue, its text read from Linear's API (the delivery's body is
/// not trusted as text). A delivery that is not an issue's is `None`. The delivery's timestamp must be fresh.
pub fn delivery(app: &App, body: &[u8], now: u64) -> Result<Option<Item>, String> {
    let payload: Value = serde_json::from_slice(body).map_err(|e| format!("bad body: {e}"))?;
    if payload["type"] != "Issue" {
        return Ok(None);
    }
    if !matches!(payload["action"].as_str(), Some("create" | "update")) {
        return Ok(None);
    }
    if !super::fresh(payload["webhookTimestamp"].as_u64(), now) {
        return Err("the webhook is stale".into());
    }
    let id = payload["data"]["id"]
        .as_str()
        .ok_or("the issue has no id")?;
    let api = super::api(app, super::LINEAR)?;
    let issue = fetch_issue(&api, id)?;
    Ok(Some(super::ingest(app, incoming(&issue), now)))
}

/// A fetched issue as an item: its key is its identifier (`ENG-12`), its scope the team's key.
pub fn incoming(issue: &Value) -> Incoming {
    let text = |v: &Value| v.as_str().unwrap_or("").to_string();
    Incoming {
        source: super::LINEAR.into(),
        external_id: text(&issue["id"]),
        key: text(&issue["identifier"]),
        scope: text(&issue["team"]["key"]),
        title: text(&issue["title"]),
        body: text(&issue["description"]),
        labels: issue["labels"]["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|label| label["name"].as_str().map(str::to_string))
            .collect(),
        links: issue["url"]
            .as_str()
            .filter(|url| !url.is_empty())
            .map(|url| vec![url.to_string()])
            .unwrap_or_default(),
        author: text(&issue["creator"]["name"]),
        public: false,
    }
}

/// The write-back to Linear: a comment naming the task or the change, then the issue's state moves to the team's
/// `started` state, or its `completed` one (which closes it). Each push the item has not made is made.
pub fn push(
    app: &App,
    item: &Item,
    done: &[String],
    phase: Phase,
) -> Result<Vec<(String, Result<String, String>)>, String> {
    let api = super::api(app, super::LINEAR)?;
    let issue = graphql(&api, ISSUE, json!({ "id": item.external_id }))?["issue"].clone();
    let state_type = match phase {
        Phase::Started => "started",
        Phase::Landed => "completed",
    };
    let state = issue["team"]["states"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|s| s["type"] == state_type)
        .and_then(|s| s["id"].as_str())
        .map(str::to_string);
    let word = phase.word();
    let mut out = Vec::new();
    let comment_kind = format!("{word}:comment");
    if !has(done, &comment_kind) {
        let text = comment_of(item, phase);
        let result = graphql(
            &api,
            COMMENT,
            json!({ "id": item.external_id, "body": text }),
        )
        .and_then(|data| {
            if data["commentCreate"]["success"] == true {
                Ok("commented".to_string())
            } else {
                Err("Linear did not confirm the comment".to_string())
            }
        });
        out.push((comment_kind, result));
    }
    let status_kind = format!("{word}:status");
    if !has(done, &status_kind) {
        let result = match state {
            None => Err(format!("the team has no {state_type} state")),
            Some(state) => graphql(
                &api,
                SET_STATE,
                json!({ "id": item.external_id, "state": state }),
            )
            .and_then(|data| {
                if data["issueUpdate"]["success"] == true {
                    Ok(format!("state {state_type}"))
                } else {
                    Err("Linear did not confirm the state".to_string())
                }
            }),
        };
        out.push((status_kind, result));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fetched_issue_becomes_an_item_keyed_by_its_identifier_and_team() {
        let issue = json!({
            "id": "c0ffee00-0000-4000-8000-000000000001", "identifier": "ENG-12",
            "title": "Export fails", "description": "It fails on export.",
            "url": "https://linear.app/acme/issue/ENG-12", "creator": { "name": "Ada" },
            "team": { "key": "ENG", "states": { "nodes": [] } },
            "labels": { "nodes": [{ "name": "plaza" }] }
        });
        let item = incoming(&issue);
        assert_eq!(item.key, "ENG-12");
        assert_eq!(item.scope, "ENG");
        assert_eq!(item.labels, vec!["plaza".to_string()]);
        assert_eq!(item.author, "Ada");
        assert!(!item.public);
    }
}
