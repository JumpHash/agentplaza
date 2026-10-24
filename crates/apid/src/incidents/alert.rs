//! The alert connectors' payloads (PLAN 24.1): each source's webhook, read into typed [`Alert`]s. A delivery names its
//! source by the connector it arrives on (`/v1/hooks/{connector}/{token}`); the formats are the sources' documented
//! webhook shapes (Sentry's issue webhooks, Alertmanager's and Grafana's, CloudWatch's alarm notifications through SNS).
//! Cloudflare's notifications and the uptime checks are read from the fields they document or, for uptime, from the
//! hub's own JSON (`uptime` takes `{monitor, id, service, state, url, at}`: no vendor shape is assumed).
//!
//! Signatures: Sentry signs its deliveries (`Sentry-Hook-Signature`, the HMAC-SHA256 of the body in hex, under the
//! webhook's secret). The others carry the connector's secret as `Authorization: Bearer`. SNS cannot send headers, so
//! CloudWatch's delivery is authenticated by the webhook token in its path alone.

use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;

use crate::triggers::hooks::same;

/// The connectors that carry alerts, in catalog order.
pub const SOURCES: [&str; 6] = [
    "sentry",
    "alertmanager",
    "grafana",
    "cloudwatch",
    "cloudflare",
    "uptime",
];

/// One alert, as a source said it: firing or resolved, about one service, with a fingerprint that names the same
/// alert in every delivery of it (the dedupe key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    /// The connector it came in on (`sentry`, `alertmanager`, ...).
    pub source: String,
    /// `<source>:<the source's fingerprint>`: the same alert, in every delivery, has the same one.
    pub fingerprint: String,
    /// The service the alert is about: a project's environment names it (`ALERT_SERVICE`). Empty when the source says none.
    pub service: String,
    /// `critical`, `high`, `medium` or `low` (Sentry's level, Alertmanager's and Grafana's severity label, or the
    /// source's own default).
    pub severity: String,
    pub title: String,
    /// The source's own description, in its words (its summary, culprit or reason).
    pub detail: String,
    /// Whether the alert is firing (false: resolved).
    pub firing: bool,
    /// When the source says it started (ms), when it says.
    pub started_ms: Option<u64>,
    /// When the source says it resolved (ms), when it says.
    pub resolved_ms: Option<u64>,
    /// Links the source gives (its dashboards, the runbook, the issue).
    pub links: Vec<String>,
}

/// What a delivery says: its alerts, or why none of them needs a record (a subscription's confirmation, a state with no
/// alarm in it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Alerts(Vec<Alert>),
    Ignored(String),
}

/// Whether `connector` is an alert source.
pub fn is_source(connector: &str) -> bool {
    SOURCES.contains(&connector)
}

/// The HMAC-SHA256 of `body` under `secret`, in lowercase hex (Sentry's `Sentry-Hook-Signature`).
pub fn hmac_hex(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("an HMAC takes a key of any length");
    mac.update(body);
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Whether a delivery carries the right secret for its source: Sentry's signature, a bearer secret for the others, and
/// nothing for CloudWatch (its token in the path is its secret).
pub fn check(
    connector: &str,
    secret: &str,
    header: &dyn Fn(&str) -> Option<String>,
    body: &[u8],
) -> bool {
    match connector {
        "sentry" => same(
            &hmac_hex(secret, body),
            &header("sentry-hook-signature").unwrap_or_default(),
        ),
        "cloudwatch" => true,
        _ => same(
            &format!("Bearer {secret}"),
            &header("authorization").unwrap_or_default(),
        ),
    }
}

/// Reads a delivery on `connector` into its alerts. A body that is not JSON, or one missing what identifies an alert, is
/// an error (the delivery is refused); a delivery that says nothing to record is `Ignored`.
pub fn parse(
    connector: &str,
    header: &dyn Fn(&str) -> Option<String>,
    body: &[u8],
) -> Result<Parsed, String> {
    if connector == "cloudwatch" {
        return cloudwatch(header, body);
    }
    let json: Value = serde_json::from_slice(body).map_err(|e| format!("bad body: {e}"))?;
    match connector {
        "sentry" => sentry(&json, header),
        "alertmanager" | "grafana" => grouped(connector, &json),
        "cloudflare" => cloudflare(&json),
        "uptime" => uptime(&json),
        other => Err(format!("{other} sends no alerts")),
    }
}

/// Sentry's issue webhook (`action` created, triggered, unresolved or resolved; `data.issue`). The issue's id is the
/// fingerprint: every event of an issue is the same alert.
fn sentry(json: &Value, header: &dyn Fn(&str) -> Option<String>) -> Result<Parsed, String> {
    let issue = &json["data"]["issue"];
    if !issue.is_object() {
        let resource = header("sentry-hook-resource").unwrap_or_else(|| "a delivery".into());
        return Ok(Parsed::Ignored(format!("{resource} names no issue")));
    }
    let firing = match json["action"].as_str().unwrap_or("") {
        "created" | "triggered" | "unresolved" => true,
        "resolved" => false,
        other => return Ok(Parsed::Ignored(format!("issue {other}"))),
    };
    let id = scalar(&issue["id"]).ok_or("an issue names its id")?;
    let severity = match text_of(&issue["level"]).as_deref() {
        Some("fatal") => "critical",
        Some("warning") => "medium",
        Some("info" | "debug" | "sample") => "low",
        _ => "high",
    };
    Ok(Parsed::Alerts(vec![Alert {
        source: "sentry".into(),
        fingerprint: format!("sentry:{id}"),
        service: first_of(&[&issue["project"]["slug"]]).unwrap_or_default(),
        severity: severity.into(),
        title: text_of(&issue["title"]).unwrap_or_else(|| format!("Sentry issue {id}")),
        detail: text_of(&issue["culprit"]).unwrap_or_default(),
        firing,
        started_ms: if firing {
            time_of(&issue["firstSeen"])
        } else {
            None
        },
        resolved_ms: None,
        links: first_of(&[&issue["permalink"]]).into_iter().collect(),
    }]))
}

/// Alertmanager's and Grafana's webhooks: `alerts`, each with its `status`, `labels`, `annotations`, `startsAt`,
/// `endsAt`, `fingerprint` and links. Grafana's adds its dashboard and panel links.
fn grouped(connector: &str, json: &Value) -> Result<Parsed, String> {
    let list = json["alerts"]
        .as_array()
        .ok_or("a delivery names its alerts")?;
    let mut out = Vec::with_capacity(list.len());
    for alert in list {
        let firing = match alert["status"].as_str() {
            Some("firing") => true,
            Some("resolved") => false,
            _ => return Err("an alert names its status: firing or resolved".into()),
        };
        let labels = &alert["labels"];
        let annotations = &alert["annotations"];
        let fingerprint = scalar(&alert["fingerprint"]).ok_or("an alert names its fingerprint")?;
        let name = text_of(&labels["alertname"]).unwrap_or_else(|| "alert".into());
        let links = [
            &alert["generatorURL"],
            &alert["dashboardURL"],
            &alert["panelURL"],
            &annotations["runbook_url"],
        ]
        .into_iter()
        .filter_map(text_of)
        .filter(|link| link.starts_with("http://") || link.starts_with("https://"))
        .collect();
        out.push(Alert {
            source: connector.into(),
            fingerprint: format!("{connector}:{fingerprint}"),
            service: first_of(&[&labels["service"], &labels["job"], &labels["namespace"]])
                .unwrap_or_default(),
            severity: alertmanager_severity(&labels["severity"]).into(),
            title: text_of(&annotations["summary"]).unwrap_or_else(|| name.clone()),
            detail: text_of(&annotations["description"]).unwrap_or_default(),
            firing,
            started_ms: time_of(&alert["startsAt"]),
            resolved_ms: if firing {
                None
            } else {
                time_of(&alert["endsAt"])
            },
            links,
        });
    }
    Ok(Parsed::Alerts(out))
}

/// The severity label of Alertmanager and Grafana: `critical`, `error`, `warning`, `info`; none is medium.
fn alertmanager_severity(label: &Value) -> &'static str {
    match text_of(label).as_deref() {
        Some("critical") => "critical",
        Some("error" | "high" | "page") => "high",
        Some("info" | "none" | "notice") => "low",
        _ => "medium",
    }
}

/// An SNS delivery of a CloudWatch alarm: the envelope's `Type` (only a `Notification` carries an alarm), and the
/// alarm's state in its `Message` (a JSON text): `ALARM` fires, `OK` resolves, `INSUFFICIENT_DATA` records nothing.
fn cloudwatch(header: &dyn Fn(&str) -> Option<String>, body: &[u8]) -> Result<Parsed, String> {
    let envelope: Value = serde_json::from_slice(body).map_err(|e| format!("bad body: {e}"))?;
    let kind = header("x-amz-sns-message-type")
        .or_else(|| text_of(&envelope["Type"]))
        .unwrap_or_default();
    if kind != "Notification" {
        return Ok(Parsed::Ignored(format!(
            "{kind}: the hub does not follow SNS's subscription links; confirm the subscription by hand"
        )));
    }
    let message = envelope["Message"]
        .as_str()
        .ok_or("a notification names its message")?;
    let alarm: Value =
        serde_json::from_str(message).map_err(|e| format!("bad alarm message: {e}"))?;
    let firing = match alarm["NewStateValue"].as_str() {
        Some("ALARM") => true,
        Some("OK") => false,
        other => {
            return Ok(Parsed::Ignored(format!(
                "alarm state {}",
                other.unwrap_or("none")
            )));
        }
    };
    let name = text_of(&alarm["AlarmName"]).ok_or("an alarm names itself")?;
    let arn = text_of(&alarm["AlarmArn"]).unwrap_or_else(|| name.clone());
    let at = time_of(&alarm["StateChangeTime"]);
    let detail = [
        text_of(&alarm["AlarmDescription"]),
        text_of(&alarm["NewStateReason"]),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ");
    Ok(Parsed::Alerts(vec![Alert {
        source: "cloudwatch".into(),
        fingerprint: format!("cloudwatch:{arn}"),
        service: name.clone(),
        severity: "high".into(),
        title: name,
        detail,
        firing,
        started_ms: if firing { at } else { None },
        resolved_ms: if firing { None } else { at },
        links: Vec::new(),
    }]))
}

/// A Cloudflare notification: its `name`, `text`, `policy_id` and `ts` (Unix seconds). The Worker or Pages project it is
/// about is in `data` when it says; a status of `resolved`, `ok`, `healthy` or `up` resolves it.
fn cloudflare(json: &Value) -> Result<Parsed, String> {
    let name = text_of(&json["name"]).ok_or("a notification names itself")?;
    let policy = text_of(&json["policy_id"]).unwrap_or_default();
    let status = text_of(&json["data"]["status"])
        .unwrap_or_default()
        .to_lowercase();
    let firing = !matches!(status.as_str(), "resolved" | "ok" | "healthy" | "up");
    let at = time_of(&json["ts"]);
    Ok(Parsed::Alerts(vec![Alert {
        source: "cloudflare".into(),
        fingerprint: format!("cloudflare:{policy}:{name}"),
        service: first_of(&[
            &json["data"]["worker"],
            &json["data"]["pages"],
            &json["data"]["service"],
        ])
        .unwrap_or_else(|| name.clone()),
        severity: "high".into(),
        title: name,
        detail: text_of(&json["text"]).unwrap_or_default(),
        firing,
        started_ms: if firing { at } else { None },
        resolved_ms: if firing { None } else { at },
        links: Vec::new(),
    }]))
}

/// An uptime check's delivery in the hub's own shape: `{monitor, id, service, state: "down" | "up", url, at}`. A check
/// that is down fires; one that is up resolves.
fn uptime(json: &Value) -> Result<Parsed, String> {
    let monitor = text_of(&json["monitor"]).ok_or("a check names its monitor")?;
    let id = scalar(&json["id"]).unwrap_or_else(|| monitor.clone());
    let firing = match text_of(&json["state"]).as_deref() {
        Some("down") => true,
        Some("up") => false,
        other => {
            return Ok(Parsed::Ignored(format!(
                "check state {}",
                other.unwrap_or("none")
            )));
        }
    };
    let at = time_of(&json["at"]);
    Ok(Parsed::Alerts(vec![Alert {
        source: "uptime".into(),
        fingerprint: format!("uptime:{id}"),
        service: first_of(&[&json["service"]]).unwrap_or_else(|| monitor.clone()),
        severity: "high".into(),
        title: format!("{monitor} is down"),
        detail: String::new(),
        firing,
        started_ms: if firing { at } else { None },
        resolved_ms: if firing { None } else { at },
        links: first_of(&[&json["url"]]).into_iter().collect(),
    }]))
}

/// A non-empty string, trimmed.
fn text_of(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// A string, or a number written as its digits (ids), when non-empty.
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::Number(n) => Some(n.to_string()),
        _ => text_of(value),
    }
}

fn first_of(values: &[&Value]) -> Option<String> {
    values.iter().find_map(|v| text_of(v))
}

/// A time: an RFC 3339 text, or a number of Unix seconds.
fn time_of(value: &Value) -> Option<u64> {
    match value {
        Value::String(text) => parse_time(text),
        Value::Number(n) => n.as_u64().map(|secs| secs.saturating_mul(1000)),
        _ => None,
    }
}

/// An RFC 3339 time as ms since the Unix epoch: `2026-10-24T10:00:00Z`, with fractions, and `+hh:mm`, `+hhmm` or `-hh:mm`
/// offsets (CloudWatch writes `+0000`). Times before the epoch are none.
pub fn parse_time(text: &str) -> Option<u64> {
    let (date, rest) = text.trim().split_once(['T', 't', ' '])?;
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let zone_at = rest.find(['Z', 'z', '+', '-'])?;
    let (clock, zone) = rest.split_at(zone_at);
    let mut hms = clock.split(':');
    let hour: i64 = hms.next()?.parse().ok()?;
    let minute: i64 = hms.next()?.parse().ok()?;
    let seconds_text = hms.next()?;
    if hms.next().is_some() {
        return None;
    }
    let (seconds, fraction) = seconds_text.split_once('.').unwrap_or((seconds_text, ""));
    let seconds: i64 = seconds.parse().ok()?;
    if !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let millis: i64 = format!("{:0<3}", &fraction[..fraction.len().min(3)])
        .parse()
        .ok()?;
    let offset_minutes: i64 = match zone {
        "Z" | "z" => 0,
        signed => {
            let sign = if signed.starts_with('-') { -1 } else { 1 };
            let digits: String = signed[1..].chars().filter(|c| c.is_ascii_digit()).collect();
            if digits.len() != 4 {
                return None;
            }
            let h: i64 = digits[..2].parse().ok()?;
            let m: i64 = digits[2..].parse().ok()?;
            sign * (h * 60 + m)
        }
    };
    let days = days_from_civil(year, month, day);
    let ms = (((days * 24 + hour) * 60 + minute) * 60 + seconds) * 1000 + millis
        - offset_minutes * 60_000;
    u64::try_from(ms).ok()
}

/// The days from 1970-01-01 to `year`-`month`-`day` in the proleptic Gregorian calendar.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    const SENTRY_CREATED: &[u8] = include_bytes!("fixtures/sentry_issue_created.json");
    const SENTRY_RESOLVED: &[u8] = include_bytes!("fixtures/sentry_issue_resolved.json");
    const ALERTMANAGER_FIRING: &[u8] = include_bytes!("fixtures/alertmanager_firing.json");
    const ALERTMANAGER_RESOLVED: &[u8] = include_bytes!("fixtures/alertmanager_resolved.json");
    const GRAFANA_FIRING: &[u8] = include_bytes!("fixtures/grafana_firing.json");
    const CLOUDWATCH_ALARM: &[u8] = include_bytes!("fixtures/cloudwatch_alarm.json");
    const CLOUDFLARE: &[u8] = include_bytes!("fixtures/cloudflare_notification.json");
    const UPTIME_DOWN: &[u8] = include_bytes!("fixtures/uptime_down.json");

    fn none(_: &str) -> Option<String> {
        None
    }

    fn headers(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name: &str| {
            owned
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        }
    }

    fn alerts(parsed: Result<Parsed, String>) -> Vec<Alert> {
        match parsed {
            Ok(Parsed::Alerts(list)) => list,
            other => panic!("alerts expected, got {other:?}"),
        }
    }

    #[test]
    fn a_sentry_issue_reads_its_issue_as_the_alert() {
        let sentry = headers(&[("sentry-hook-resource", "issue")]);
        let list = alerts(parse("sentry", &sentry, SENTRY_CREATED));
        assert_eq!(list.len(), 1);
        let a = &list[0];
        assert_eq!(a.fingerprint, "sentry:4521187");
        assert_eq!(a.service, "site-web");
        assert_eq!(a.severity, "high");
        assert!(a.title.starts_with("TypeError"), "{a:?}");
        assert_eq!(a.detail, "src/checkout.ts in placeOrder");
        assert!(a.firing);
        assert_eq!(a.started_ms, Some(1_792_835_892_000));
        assert_eq!(a.resolved_ms, None);
        assert_eq!(
            a.links,
            ["https://sentry.example.com/organizations/acme/issues/4521187/"]
        );
    }

    #[test]
    fn a_resolved_sentry_issue_resolves_the_same_fingerprint() {
        let list = alerts(parse("sentry", &none, SENTRY_RESOLVED));
        assert!(!list[0].firing);
        assert_eq!(list[0].fingerprint, "sentry:4521187");
    }

    #[test]
    fn a_sentry_issue_action_the_hub_does_not_need_is_ignored() {
        let body = br#"{"action":"assigned","data":{"issue":{"id":"1"}}}"#;
        assert!(matches!(
            parse("sentry", &none, body),
            Ok(Parsed::Ignored(why)) if why.contains("assigned")
        ));
    }

    #[test]
    fn sentry_signs_its_body_and_a_bad_signature_is_refused() {
        // The HMAC-SHA256 of the fixture under `sentry-test-secret`, computed with openssl.
        let good = "06ac4ebf91e51531f157adedbf202ec940fb6ddf45201d5a97bfa5d588a4cf4a";
        assert_eq!(hmac_hex("sentry-test-secret", SENTRY_CREATED), good);
        let signed = headers(&[("sentry-hook-signature", good)]);
        assert!(check(
            "sentry",
            "sentry-test-secret",
            &signed,
            SENTRY_CREATED
        ));
        assert!(!check("sentry", "another-secret", &signed, SENTRY_CREATED));
        let tampered = headers(&[("sentry-hook-signature", "00")]);
        assert!(!check(
            "sentry",
            "sentry-test-secret",
            &tampered,
            SENTRY_CREATED
        ));
        assert!(!check(
            "sentry",
            "sentry-test-secret",
            &none,
            SENTRY_CREATED
        ));
    }

    #[test]
    fn the_other_sources_carry_their_secret_as_a_bearer_header() {
        let bearer = headers(&[("authorization", "Bearer s3cret")]);
        for source in ["alertmanager", "grafana", "cloudflare", "uptime"] {
            assert!(check(source, "s3cret", &bearer, GRAFANA_FIRING), "{source}");
            assert!(!check(source, "other", &bearer, GRAFANA_FIRING), "{source}");
            assert!(!check(source, "s3cret", &none, GRAFANA_FIRING), "{source}");
        }
        // SNS cannot send headers: CloudWatch's delivery is checked by its path token alone.
        assert!(check("cloudwatch", "anything", &none, CLOUDWATCH_ALARM));
    }

    #[test]
    fn alertmanager_reads_each_alert_with_its_fingerprint_and_times() {
        let list = alerts(parse("alertmanager", &none, ALERTMANAGER_FIRING));
        assert_eq!(list.len(), 1);
        let a = &list[0];
        assert_eq!(a.fingerprint, "alertmanager:a1b2c3d4e5f60718");
        assert_eq!(a.service, "api");
        assert_eq!(a.severity, "critical");
        assert_eq!(a.title, "Error ratio above 5% on api");
        assert_eq!(a.detail, "5xx responses are 12% of requests over 5m");
        assert!(a.firing);
        assert_eq!(a.started_ms, Some(1_792_836_000_000));
        assert_eq!(
            a.resolved_ms, None,
            "the zero endsAt of a firing alert is no time"
        );
        assert_eq!(
            a.links.len(),
            2,
            "the generator and the runbook: {:?}",
            a.links
        );
    }

    #[test]
    fn alertmanager_resolves_with_its_end_time() {
        let list = alerts(parse("alertmanager", &none, ALERTMANAGER_RESOLVED));
        assert!(!list[0].firing);
        assert_eq!(list[0].resolved_ms, Some(1_792_836_720_000));
        assert_eq!(list[0].fingerprint, "alertmanager:a1b2c3d4e5f60718");
    }

    #[test]
    fn grafana_reads_its_alerts_with_their_dashboard_links() {
        let list = alerts(parse("grafana", &none, GRAFANA_FIRING));
        let a = &list[0];
        assert_eq!(a.source, "grafana");
        assert_eq!(a.fingerprint, "grafana:9f8e7d6c5b4a3921");
        assert_eq!(a.service, "site-web");
        assert_eq!(a.severity, "medium");
        assert_eq!(a.title, "p95 checkout latency above 2s");
        assert_eq!(a.links.len(), 3, "{:?}", a.links);
    }

    #[test]
    fn a_cloudwatch_alarm_arrives_in_an_sns_envelope() {
        let sns = headers(&[("x-amz-sns-message-type", "Notification")]);
        let list = alerts(parse("cloudwatch", &sns, CLOUDWATCH_ALARM));
        let a = &list[0];
        assert_eq!(
            a.fingerprint,
            "cloudwatch:arn:aws:cloudwatch:eu-west-1:123456789012:alarm:site-5xx"
        );
        assert_eq!(a.service, "site-5xx");
        assert!(a.firing);
        assert_eq!(a.started_ms, Some(1_792_836_271_512));
        assert!(a.detail.contains("5xx count above 10"), "{}", a.detail);
    }

    #[test]
    fn an_sns_subscription_confirmation_is_not_an_alert() {
        let confirm = headers(&[("x-amz-sns-message-type", "SubscriptionConfirmation")]);
        let body =
            br#"{"Type":"SubscriptionConfirmation","SubscribeURL":"https://sns.example/confirm"}"#;
        assert!(matches!(
            parse("cloudwatch", &confirm, body),
            Ok(Parsed::Ignored(why)) if why.contains("SubscriptionConfirmation")
        ));
    }

    #[test]
    fn a_cloudwatch_ok_state_resolves_and_insufficient_data_records_nothing() {
        let ok = String::from_utf8(CLOUDWATCH_ALARM.to_vec())
            .unwrap()
            .replace(
                "\\\"NewStateValue\\\":\\\"ALARM\\\"",
                "\\\"NewStateValue\\\":\\\"OK\\\"",
            );
        let list = alerts(parse("cloudwatch", &none, ok.as_bytes()));
        assert!(!list[0].firing);
        assert!(list[0].resolved_ms.is_some());
        let unknown = String::from_utf8(CLOUDWATCH_ALARM.to_vec())
            .unwrap()
            .replace(
                "\\\"NewStateValue\\\":\\\"ALARM\\\"",
                "\\\"NewStateValue\\\":\\\"INSUFFICIENT_DATA\\\"",
            );
        assert!(matches!(
            parse("cloudwatch", &none, unknown.as_bytes()),
            Ok(Parsed::Ignored(_))
        ));
    }

    #[test]
    fn a_cloudflare_notification_names_its_worker_and_its_policy() {
        let list = alerts(parse("cloudflare", &none, CLOUDFLARE));
        let a = &list[0];
        assert_eq!(
            a.fingerprint,
            "cloudflare:5b1e0c9a-0000-4000-8000-000000000003:Worker site: errors above threshold"
        );
        assert_eq!(a.service, "site");
        assert!(a.firing);
        assert_eq!(a.started_ms, Some(1_729_764_300_000));
    }

    #[test]
    fn an_uptime_check_that_is_down_fires_and_that_is_up_resolves() {
        let list = alerts(parse("uptime", &none, UPTIME_DOWN));
        let a = &list[0];
        assert_eq!(a.fingerprint, "uptime:monitor-17");
        assert_eq!(a.service, "site-web");
        assert_eq!(a.title, "Site home page is down");
        assert!(a.firing);
        // 10:06 at +02:00 is 08:06 UTC.
        assert_eq!(a.started_ms, Some(1_792_829_160_000));
        let up = br#"{"monitor":"Site home page","id":"monitor-17","state":"up"}"#;
        assert!(!alerts(parse("uptime", &none, up))[0].firing);
    }

    #[test]
    fn a_body_that_names_no_alert_is_refused() {
        assert!(parse("alertmanager", &none, b"not json").is_err());
        assert!(parse("alertmanager", &none, br#"{"status":"firing"}"#).is_err());
        assert!(
            parse(
                "sentry",
                &none,
                br#"{"action":"created","data":{"issue":{"title":"x"}}}"#
            )
            .is_err()
        );
        assert!(parse("uptime", &none, br#"{"id":"1","state":"down"}"#).is_err());
    }

    #[test]
    fn times_are_read_in_their_offsets_and_before_the_epoch_is_none() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("2000-01-01T00:00:00Z"), Some(946_684_800_000));
        assert_eq!(
            parse_time("2000-01-01T01:00:00+01:00"),
            Some(946_684_800_000)
        );
        assert_eq!(
            parse_time("2000-01-01T00:00:00.5-0000"),
            Some(946_684_800_500)
        );
        assert_eq!(
            parse_time("2000-01-01T00:00:00.123456Z"),
            Some(946_684_800_123)
        );
        assert_eq!(parse_time("0001-01-01T00:00:00Z"), None);
        assert_eq!(parse_time("yesterday"), None);
        assert_eq!(parse_time("2000-13-01T00:00:00Z"), None);
    }
}
