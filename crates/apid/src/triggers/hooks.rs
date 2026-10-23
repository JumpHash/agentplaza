//! Connector webhooks (PLAN 14.6, 5.2): one URL per connector, `POST /v1/hooks/{connector}/{token}`, generalising the
//! maintenance ingest address. The token is the connector's, kept in the vault (`HOOK_<CONNECTOR>_TOKEN`, bound to
//! `hub:triggers`); the owner makes it with `POST /v1/connectors/{id}/webhook`, which shows it once. GitHub's deliveries
//! are also checked against the webhook's signing secret (`HOOK_GITHUB_SECRET`): `X-Hub-Signature-256` is the HMAC-SHA256
//! of the body under it. A delivery is read into an [`Event`] for the triggers; a delivery the triggers do not need (a
//! ping, another action) is acknowledged and ignored.

use apz_http::{Params, Request, Response, RouteDoc, Router};
use hmac::{Hmac, Mac};
use secrecy::SecretString;
use serde_json::{Value, json};
use sha2::Sha256;

use super::{BINDING, Event};
use crate::state::App;
use crate::vault::OnBehalf;

/// The vault name of a connector's webhook token.
pub fn token_name(connector: &str) -> String {
    format!("HOOK_{}_TOKEN", upper(connector))
}

/// The vault name of a connector's webhook signing secret.
pub fn secret_name(connector: &str) -> String {
    format!("HOOK_{}_SECRET", upper(connector))
}

fn upper(connector: &str) -> String {
    connector
        .to_ascii_uppercase()
        .replace(|c: char| !c.is_ascii_alphanumeric(), "_")
}

/// A connector id: lowercase letters, digits and dashes, starting with a letter or digit, at most 64 characters.
pub fn valid_connector(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// What a delivery says: an event for the triggers, or nothing they need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Event(Event),
    Ignored(String),
}

/// GitHub's signature of a body: `sha256=` and the HMAC-SHA256 of the body under the secret, in hex.
pub fn github_signature(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("an HMAC takes a key of any length");
    mac.update(body);
    let hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("sha256={hex}")
}

/// Compares two strings in time that depends on their length only.
pub fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// Reads a delivery. GitHub's `issues` deliveries with the `labeled` action are the `issue.labelled` event, for the
/// issue `owner/repo#number` and the label added. Another connector's delivery names its event and item in its JSON:
/// `{"event": "...", "item": "...", "label": "..."}`.
pub fn parse(
    connector: &str,
    header: &dyn Fn(&str) -> Option<String>,
    body: &[u8],
) -> Result<Parsed, String> {
    let json: Value = serde_json::from_slice(body).map_err(|e| format!("bad body: {e}"))?;
    if connector == "github" {
        let kind = header("x-github-event").unwrap_or_default();
        if kind != "issues" {
            return Ok(Parsed::Ignored(kind));
        }
        let action = json["action"].as_str().unwrap_or("");
        if action != "labeled" {
            return Ok(Parsed::Ignored(format!("issues {action}")));
        }
        let repo = json["repository"]["full_name"]
            .as_str()
            .ok_or("an issue delivery names its repository")?;
        let number = json["issue"]["number"]
            .as_u64()
            .ok_or("an issue delivery names its issue")?;
        return Ok(Parsed::Event(Event {
            source: "connector:github".into(),
            event: "issue.labelled".into(),
            item: format!("{repo}#{number}"),
            label: json["label"]["name"].as_str().unwrap_or("").into(),
            dedup: None,
        }));
    }
    // Linear's own issue deliveries: `{"type": "Issue", "action": "create" | "update", "data": {"identifier": ...}}`.
    if connector == "linear" && json["type"] == "Issue" {
        let action = json["action"].as_str().unwrap_or("");
        if !matches!(action, "create" | "update") {
            return Ok(Parsed::Ignored(format!("issue {action}")));
        }
        let item = json["data"]["identifier"]
            .as_str()
            .ok_or("an issue delivery names its identifier")?;
        let event = if action == "create" {
            "issue.created"
        } else {
            "issue.updated"
        };
        return Ok(Parsed::Event(Event {
            source: "connector:linear".into(),
            event: event.into(),
            item: item.into(),
            label: String::new(),
            dedup: None,
        }));
    }
    let event = json["event"].as_str().ok_or("a delivery names its event")?;
    let item = json["item"].as_str().ok_or("a delivery names its item")?;
    Ok(Parsed::Event(Event {
        source: format!("connector:{connector}"),
        event: event.into(),
        item: item.into(),
        label: json["label"].as_str().unwrap_or("").into(),
        dedup: None,
    }))
}

/// Whether a delivery's signature is right for its connector: GitHub's `X-Hub-Signature-256`, and Linear's
/// `Linear-Signature` (hex), each the HMAC-SHA256 of the body under the connector's signing secret.
pub fn signed_right(
    connector: &str,
    secret: &str,
    header: &dyn Fn(&str) -> Option<String>,
    body: &[u8],
) -> bool {
    match connector {
        "github" => same(
            &github_signature(secret, body),
            &header("x-hub-signature-256").unwrap_or_default(),
        ),
        "linear" => crate::intake::hmac_ok(
            secret,
            body,
            &header("linear-signature").unwrap_or_default(),
        ),
        _ => true,
    }
}

fn put(app: &App, name: &str, value: &str, label: &str, now: u64) -> Result<(), String> {
    app.vault
        .put_plaza(
            name,
            BINDING,
            label,
            &SecretString::from(value.to_string()),
            "owner",
            now,
        )
        .map(|_| ())
        .map_err(|e| format!("the vault: {}", e.message()))
}

/// `POST /v1/hooks/{connector}/{token}`: public. A wrong token, an unknown connector and a sealed vault are all a 404.
fn hook_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let connector = p.get("connector");
    let token = p.get("token");
    if !valid_connector(connector) {
        return Response::error(404, "no such webhook");
    }
    let stored = app
        .vault
        .value_for(&token_name(connector), BINDING, &OnBehalf::Hub);
    if !stored.is_some_and(|s| same(&s, token)) {
        return Response::error(404, "no such webhook");
    }
    if matches!(connector, "github" | "linear") {
        let Some(secret) = app
            .vault
            .value_for(&secret_name(connector), BINDING, &OnBehalf::Hub)
        else {
            return Response::error(401, "the webhook has no signing secret");
        };
        if !signed_right(
            connector,
            &secret,
            &|name| req.header(name).map(str::to_string),
            &req.body,
        ) {
            return Response::error(401, "the signature does not match the body");
        }
    }
    let parsed = match parse(
        connector,
        &|name| req.header(name).map(str::to_string),
        &req.body,
    ) {
        Ok(parsed) => parsed,
        Err(why) => return Response::error(400, &why),
    };
    // An intake source's delivery also keeps its item (intake's own routes went into this one).
    let now = apz_plaza::now_ms();
    let intake = match crate::intake::delivery(
        app,
        connector,
        &|name| req.header(name).map(str::to_string),
        &req.body,
        now,
    ) {
        Ok(intake) => intake,
        Err(why) => return Response::error(502, &why),
    };
    match parsed {
        Parsed::Event(event) => {
            let matched = app.triggers.deliver(&event, now);
            Response::json(
                202,
                &json!({"event": event.event, "item": event.item, "triggers": matched, "intake": intake}),
            )
        }
        Parsed::Ignored(why) => Response::json(
            202,
            &json!({"ignored": why, "triggers": 0, "intake": intake}),
        ),
    }
}

/// `POST /v1/connectors/{id}/webhook` `{secret?}`: the owner's. Makes the connector's webhook token (a new one each time,
/// the old one stops) and, when given, stores the webhook's signing secret. The token is shown once, in the URL.
fn webhook_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let connector = p.get("id");
    if !valid_connector(connector) {
        return Response::error(
            400,
            "a connector id is lowercase letters, digits and dashes",
        );
    }
    let body: Value = if req.body.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice(&req.body) {
            Ok(body) => body,
            Err(e) => return Response::error(400, &format!("bad body: {e}")),
        }
    };
    let token = match crate::hub_mcp::auth::random_hex(24) {
        Ok(token) => token,
        Err(why) => return Response::error(500, &why),
    };
    let now = apz_plaza::now_ms();
    if let Err(why) = put(app, &token_name(connector), &token, "a webhook token", now) {
        return Response::error(409, &why);
    }
    let secret = body["secret"].as_str();
    if let Some(secret) = secret
        && let Err(why) = put(
            app,
            &secret_name(connector),
            secret,
            "a webhook signing secret",
            now,
        )
    {
        return Response::error(409, &why);
    }
    Response::json(
        201,
        &json!({
            "connector": connector,
            "path": format!("/v1/hooks/{connector}/{token}"),
            "signing_secret": secret.is_some(),
        }),
    )
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .post("/v1/hooks/{connector}/{token}", hook_route)
        .doc(
            RouteDoc::new(
                "A connector's webhook delivery (public: the token in the path is the connector's, kept in the vault; GitHub's are signed)",
            )
            .status(202)
            .errors(&[400, 401, 404]),
        )
        .post("/v1/connectors/{id}/webhook", webhook_route)
        .doc(
            RouteDoc::new(
                "Make a connector's webhook token, shown once in the path; a new one stops the old. A signing secret is stored with it",
            )
            .status(201)
            .errors(&[400, 409]),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "It's a Secret to Everybody";
    const BODY: &[u8] = br#"{"action":"labeled","label":{"name":"agent"},"issue":{"number":7},"repository":{"full_name":"acme/app"}}"#;

    /// The example from GitHub's documentation of the signature.
    #[test]
    fn github_signs_a_body_with_the_hmac_sha256_of_the_secret() {
        assert_eq!(
            github_signature(SECRET, b"Hello, World!"),
            "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17"
        );
        assert!(!same(
            &github_signature("another", BODY),
            &github_signature(SECRET, BODY)
        ));
        assert!(same(
            &github_signature(SECRET, BODY),
            &github_signature(SECRET, BODY)
        ));
    }

    fn header(name: &'static str) -> impl Fn(&str) -> Option<String> {
        move |asked: &str| (asked == name).then(|| "issues".to_string())
    }

    #[test]
    fn a_labelled_issue_is_the_issue_labelled_event_of_its_repository_and_number() {
        let parsed = parse("github", &header("x-github-event"), BODY).unwrap();
        assert_eq!(
            parsed,
            Parsed::Event(Event {
                source: "connector:github".into(),
                event: "issue.labelled".into(),
                item: "acme/app#7".into(),
                label: "agent".into(),
                dedup: None,
            })
        );
    }

    #[test]
    fn other_actions_and_events_are_ignored_and_a_bad_body_is_refused() {
        let opened =
            br#"{"action":"opened","issue":{"number":7},"repository":{"full_name":"acme/app"}}"#;
        assert_eq!(
            parse("github", &header("x-github-event"), opened).unwrap(),
            Parsed::Ignored("issues opened".into())
        );
        let ping = |_: &str| Some("ping".to_string());
        assert_eq!(
            parse("github", &ping, b"{}").unwrap(),
            Parsed::Ignored("ping".into())
        );
        assert!(parse("github", &header("x-github-event"), b"not json").is_err());
    }

    #[test]
    fn another_connector_names_its_event_and_item_in_its_json() {
        let body = br#"{"event":"issue.labelled","item":"ENG-12","label":"agent"}"#;
        let parsed = parse("linear", &|_| None, body).unwrap();
        let Parsed::Event(event) = parsed else {
            panic!("an event");
        };
        assert_eq!(event.source, "connector:linear");
        assert_eq!(event.item, "ENG-12");
        assert!(parse("linear", &|_| None, b"{\"item\":\"x\"}").is_err());
    }

    #[test]
    fn each_connector_checks_its_own_signature_and_an_unsigned_delivery_is_refused() {
        let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
        mac.update(BODY);
        let hex: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let linear = |name: &str| (name == "linear-signature").then(|| hex.clone());
        assert!(signed_right("linear", SECRET, &linear, BODY));
        assert!(!signed_right("linear", "another", &linear, BODY));
        let github =
            |name: &str| (name == "x-hub-signature-256").then(|| github_signature(SECRET, BODY));
        assert!(signed_right("github", SECRET, &github, BODY));
        assert!(
            !signed_right("github", SECRET, &linear, BODY),
            "the wrong header"
        );
        assert!(!signed_right("github", SECRET, &|_| None, BODY));
    }

    #[test]
    fn connector_ids_and_vault_names_are_plain() {
        assert!(valid_connector("github"));
        assert!(valid_connector("my-tracker-2"));
        assert!(!valid_connector("My Tracker"));
        assert!(!valid_connector("../x"));
        assert!(!valid_connector(""));
        assert_eq!(token_name("my-tracker"), "HOOK_MY_TRACKER_TOKEN");
        assert_eq!(secret_name("github"), "HOOK_GITHUB_SECRET");
    }
}
