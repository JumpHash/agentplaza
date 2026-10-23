//! GitHub intake (PLAN 14.5, Engine 5.3): a repository's issue webhook, and the write-back through the platform's own
//! client (`platforms::github`), with the token of the authorization the connection names (`platforms::access`), which
//! the hub keeps.

use std::sync::Arc;

use serde_json::Value;

use super::{Incoming, IntakeRule, Item, Phase, comment_of, has, hmac_ok};
use crate::platforms::github::GitHub;
use crate::platforms::http::Ureq;
use crate::platforms::{self};
use crate::state::App;

/// The label a started item gets when its rule names none.
const DEFAULT_LABEL: &str = "plaza:in-progress";

/// Whether `header` (`sha256=<hex>`) is the HMAC-SHA256 of `body` under the webhook `secret`.
pub fn signature_ok(secret: &str, body: &[u8], header: &str) -> bool {
    match header.strip_prefix("sha256=") {
        Some(hex) => hmac_ok(secret, body, hex),
        None => false,
    }
}

/// An issue event's payload as an item: the repository's `owner/name`, the issue's number, title, body, labels and
/// author. An item from a repository that is not private (or whose privacy the payload does not say) is public.
pub fn incoming(payload: &Value) -> Result<Incoming, String> {
    let repo = payload["repository"]["full_name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .ok_or("the payload names no repository")?;
    let issue = &payload["issue"];
    let number = issue["number"]
        .as_u64()
        .ok_or("the payload names no issue number")?;
    let key = format!("{repo}#{number}");
    Ok(Incoming {
        source: super::GITHUB.into(),
        external_id: key.clone(),
        key,
        scope: repo.to_string(),
        title: issue["title"].as_str().unwrap_or("").to_string(),
        body: issue["body"].as_str().unwrap_or("").to_string(),
        labels: issue["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|label| label["name"].as_str().map(str::to_string))
            .collect(),
        links: issue["html_url"]
            .as_str()
            .filter(|url| !url.is_empty())
            .map(|url| vec![url.to_string()])
            .unwrap_or_default(),
        author: issue["user"]["login"].as_str().unwrap_or("").to_string(),
        public: payload["repository"]["private"].as_bool() != Some(true),
    })
}

/// `owner/name#n` as its repository and number.
fn split_key(external_id: &str) -> Result<(&str, u64), String> {
    let (repo, number) = external_id
        .rsplit_once('#')
        .ok_or_else(|| format!("{external_id} is not owner/name#number"))?;
    let number = number
        .parse::<u64>()
        .map_err(|_| format!("{external_id} is not owner/name#number"))?;
    Ok((repo, number))
}

/// The write-back to GitHub: a started item gets the rule's label (`plaza:in-progress` by default) and a comment; a
/// landed item gets a comment and is closed. Each push the item has not made is made; the outcomes come back.
pub fn push(
    app: &App,
    item: &Item,
    rule: &IntakeRule,
    done: &[String],
    phase: Phase,
) -> Result<Vec<(String, Result<String, String>)>, String> {
    let conn = app.intake.connection(super::GITHUB);
    if conn.integration.is_empty() {
        return Err("no GitHub authorization is set for intake".into());
    }
    let integration = app
        .platforms
        .integration(&conn.integration)
        .ok_or_else(|| format!("no authorization {}", conn.integration))?;
    let token = platforms::access(app, &integration)?.token;
    let (repo, number) = split_key(&item.external_id)?;
    let gh = GitHub::new(&integration.host, Arc::new(Ureq));
    let comment = comment_of(item, phase);
    let mut out = Vec::new();
    match phase {
        Phase::Started => {
            let label = match rule.started.trim() {
                "" => DEFAULT_LABEL,
                named => named,
            };
            if !has(done, "started:status") {
                out.push((
                    "started:status".to_string(),
                    gh.add_label(&token, repo, number, label)
                        .map(|()| format!("label {label}")),
                ));
            }
            if !has(done, "started:comment") {
                out.push((
                    "started:comment".to_string(),
                    gh.comment(&token, repo, number, &comment)
                        .map(|()| "commented".to_string()),
                ));
            }
        }
        Phase::Landed => {
            if !has(done, "landed:comment") {
                out.push((
                    "landed:comment".to_string(),
                    gh.comment(&token, repo, number, &comment)
                        .map(|()| "commented".to_string()),
                ));
            }
            if !has(done, "landed:close") {
                out.push((
                    "landed:close".to_string(),
                    gh.close_issue(&token, repo, number)
                        .map(|()| "closed".to_string()),
                ));
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_webhook_is_signed_by_the_secret_and_a_forged_one_is_refused() {
        let body = br#"{"action":"opened"}"#;
        let good = {
            use hmac::{Hmac, Mac};
            let mut mac = Hmac::<sha2::Sha256>::new_from_slice(b"s3cret").unwrap();
            mac.update(body);
            let bytes = mac.finalize().into_bytes();
            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
        };
        assert!(signature_ok("s3cret", body, &format!("sha256={good}")));
        assert!(!signature_ok("other", body, &format!("sha256={good}")));
        assert!(!signature_ok("s3cret", b"{}", &format!("sha256={good}")));
        assert!(
            !signature_ok("s3cret", body, &good),
            "the prefix is required"
        );
    }

    #[test]
    fn an_issue_payload_becomes_an_item_keyed_by_repository_and_number() {
        let payload = serde_json::json!({
            "action": "opened",
            "repository": { "full_name": "acme/widgets", "private": false },
            "issue": {
                "number": 12, "title": "Crash on start", "body": "It crashes.",
                "html_url": "https://github.com/acme/widgets/issues/12",
                "labels": [{ "name": "plaza" }], "user": { "login": "reporter" }
            }
        });
        let item = incoming(&payload).unwrap();
        assert_eq!(item.external_id, "acme/widgets#12");
        assert_eq!(item.scope, "acme/widgets");
        assert_eq!(item.labels, vec!["plaza".to_string()]);
        assert!(item.public);
        assert_eq!(split_key(&item.external_id).unwrap(), ("acme/widgets", 12));
        assert!(split_key("acme/widgets").is_err());
    }
}
