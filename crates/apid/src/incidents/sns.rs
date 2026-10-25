//! SNS message signatures (PLAN 24.1, 12.1): a CloudWatch alarm arrives as an SNS notification, and the hub checks its
//! signature before it reads one. The canonical string is the documented one: the fields of the message's type, sorted by
//! name, each as `Name\nValue\n`. SignatureVersion 1 is SHA1withRSA and 2 is SHA256withRSA. The signing certificate is
//! fetched only from an https URL on `sns.<region>.amazonaws.com`, the region of the topic, and is cached by its URL. A
//! subscription confirmation is confirmed by a GET of its SubscribeURL, on the same host rule, and only once its
//! signature checks out.

use std::collections::HashMap;
use std::sync::Mutex;

use base64::Engine as _;
use serde_json::Value;

use crate::platforms::http::Http;

/// The SNS client: the HTTP path the certificate and the confirmation go through (a test's fake answers them), and the
/// certificates already read, by URL.
pub struct Sns<'a> {
    pub http: &'a dyn Http,
    pub certs: &'a Mutex<HashMap<String, Vec<u8>>>,
}

/// The canonical string a message's signature covers, or None when the message lacks a field it covers.
pub fn canonical(envelope: &Value) -> Option<String> {
    let kind = envelope["Type"].as_str()?;
    let text = |key: &str| envelope[key].as_str().map(str::to_string);
    let mut fields: Vec<(&str, String)> = vec![
        ("Message", text("Message")?),
        ("MessageId", text("MessageId")?),
        ("Timestamp", text("Timestamp")?),
        ("TopicArn", text("TopicArn")?),
        ("Type", kind.to_string()),
    ];
    match kind {
        "Notification" => {
            if let Some(subject) = text("Subject") {
                fields.push(("Subject", subject));
            }
        }
        "SubscriptionConfirmation" => {
            fields.push(("SubscribeURL", text("SubscribeURL")?));
            fields.push(("Token", text("Token")?));
        }
        "UnsubscribeConfirmation" => {
            fields.push(("Token", text("Token")?));
            fields.push(("UnsubscribeURL", text("UnsubscribeURL")?));
        }
        _ => return None,
    }
    fields.sort_by(|a, b| a.0.cmp(b.0));
    Some(fields.iter().map(|(k, v)| format!("{k}\n{v}\n")).collect())
}

/// The region of an SNS topic's ARN (`arn:aws:sns:<region>:<account>:<name>`).
pub fn region_of(topic_arn: &str) -> Option<&str> {
    topic_arn.split(':').nth(3).filter(|r| {
        !r.is_empty()
            && r.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

/// Whether `url` is an https address on exactly `sns.<region>.amazonaws.com`: no port, user or other host.
pub fn host_allowed(url: &str, region: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split('/').next().unwrap_or("");
    !region.is_empty() && host == format!("sns.{region}.amazonaws.com")
}

impl Sns<'_> {
    /// Checks a message's signature against its signing certificate. The certificate's host must be SNS's for the topic's
    /// region; the certificate is read once per URL.
    pub fn verify(&self, envelope: &Value) -> Result<(), String> {
        let topic = envelope["TopicArn"]
            .as_str()
            .ok_or("the notification names its topic")?;
        let region = region_of(topic).ok_or("the topic's ARN names no region")?;
        let url = envelope["SigningCertURL"]
            .as_str()
            .ok_or("the notification names its signing certificate")?;
        if !host_allowed(url, region) {
            return Err(format!(
                "the signing certificate is not on sns.{region}.amazonaws.com"
            ));
        }
        let canon = canonical(envelope).ok_or("the notification names the fields it signs")?;
        let signature = envelope["Signature"]
            .as_str()
            .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
            .ok_or("the signature is not base64")?;
        let version = envelope["SignatureVersion"].as_str().unwrap_or("");
        let key = self.key_for(url)?;
        if verify_rsa(version, &key, canon.as_bytes(), &signature) {
            Ok(())
        } else {
            Err("the signature does not match the message".into())
        }
    }

    /// Confirms a subscription: its signature is checked, then its SubscribeURL is fetched on the same host rule.
    pub fn confirm(&self, envelope: &Value) -> Result<(), String> {
        self.verify(envelope)?;
        let topic = envelope["TopicArn"].as_str().unwrap_or("");
        let region = region_of(topic).ok_or("the topic's ARN names no region")?;
        let url = envelope["SubscribeURL"]
            .as_str()
            .ok_or("the subscription names its SubscribeURL")?;
        if !host_allowed(url, region) {
            return Err(format!(
                "the SubscribeURL is not on sns.{region}.amazonaws.com"
            ));
        }
        let reply = self.http.call("GET", url, &[], None)?;
        if (200..300).contains(&reply.status) {
            Ok(())
        } else {
            Err(format!("SNS answered {} to the confirmation", reply.status))
        }
    }

    fn key_for(&self, url: &str) -> Result<Vec<u8>, String> {
        if let Some(key) = lock(self.certs).get(url).cloned() {
            return Ok(key);
        }
        let reply = self.http.call("GET", url, &[], None)?;
        if !(200..300).contains(&reply.status) {
            return Err(format!("the signing certificate answered {}", reply.status));
        }
        let pem = reply
            .body
            .as_str()
            .ok_or("the signing certificate is not a PEM text")?;
        let key = rsa_key_of_pem(pem).ok_or("the signing certificate holds no RSA key")?;
        lock(self.certs).insert(url.to_string(), key.clone());
        Ok(key)
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// RSA verification of `message` under a PKCS#1 public key, with the algorithm of the signature version.
fn verify_rsa(version: &str, key: &[u8], message: &[u8], signature: &[u8]) -> bool {
    let algorithm: &dyn ring::signature::VerificationAlgorithm = match version {
        "1" => &ring::signature::RSA_PKCS1_2048_8192_SHA1_FOR_LEGACY_USE_ONLY,
        "2" => &ring::signature::RSA_PKCS1_2048_8192_SHA256,
        _ => return false,
    };
    ring::signature::UnparsedPublicKey::new(algorithm, key)
        .verify(message, signature)
        .is_ok()
}

/// The PKCS#1 RSA public key of a PEM certificate.
fn rsa_key_of_pem(pem: &str) -> Option<Vec<u8>> {
    let body: String = pem
        .lines()
        .skip_while(|l| !l.starts_with("-----BEGIN CERTIFICATE-----"))
        .skip(1)
        .take_while(|l| !l.starts_with("-----END"))
        .collect();
    let der = base64::engine::general_purpose::STANDARD
        .decode(body)
        .ok()?;
    rsa_key(spki(&der)?)
}

/// One DER element: its tag, its content, and what follows it.
fn tlv(input: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let tag = *input.first()?;
    let first = *input.get(1)?;
    let (len, start) = if first < 0x80 {
        (usize::from(first), 2)
    } else {
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 4 {
            return None;
        }
        let len = input
            .get(2..2 + count)?
            .iter()
            .fold(0usize, |acc, b| (acc << 8) | usize::from(*b));
        (len, 2 + count)
    };
    let end = start.checked_add(len)?;
    Some((tag, input.get(start..end)?, input.get(end..)?))
}

/// The SubjectPublicKeyInfo element of a certificate: the sixth field of its TBSCertificate, after the optional version.
fn spki(cert: &[u8]) -> Option<&[u8]> {
    let (tag, body, _) = tlv(cert)?;
    if tag != 0x30 {
        return None;
    }
    let (tag, mut tbs, _) = tlv(body)?;
    if tag != 0x30 {
        return None;
    }
    if tbs.first() == Some(&0xa0) {
        tbs = tlv(tbs)?.2;
    }
    // Serial, signature algorithm, issuer, validity, subject, then the key.
    let mut rest = tbs;
    for _ in 0..5 {
        rest = tlv(rest)?.2;
    }
    let after = tlv(rest)?;
    if after.0 != 0x30 {
        return None;
    }
    Some(&rest[..rest.len() - after.2.len()])
}

/// The PKCS#1 key inside an rsaEncryption SubjectPublicKeyInfo.
fn rsa_key(spki: &[u8]) -> Option<Vec<u8>> {
    const RSA_ENCRYPTION: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
    let (_, body, _) = tlv(spki)?;
    let (tag, algorithm, rest) = tlv(body)?;
    if tag != 0x30 || tlv(algorithm)?.1 != RSA_ENCRYPTION {
        return None;
    }
    let (tag, bits, _) = tlv(rest)?;
    if tag != 0x03 || bits.first() != Some(&0) {
        return None;
    }
    Some(bits[1..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const CERT: &str = include_str!("fixtures/sns/test_cert.pem");
    const V1: &str = include_str!("fixtures/sns/notification_v1.json");
    const V2: &str = include_str!("fixtures/sns/notification_v2.json");
    const SUBSCRIPTION: &str = include_str!("fixtures/sns/subscription_v2.json");
    const CERT_URL: &str = "https://sns.eu-west-1.amazonaws.com/SimpleNotificationService-test.pem";

    /// The network the hub would reach: the certificate and the confirmation, counted.
    struct Fake {
        calls: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new() -> Fake {
            Fake {
                calls: Mutex::new(Vec::new()),
            }
        }
        fn fetches(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    impl Http for Fake {
        fn call(
            &self,
            method: &str,
            url: &str,
            _headers: &[(&str, String)],
            _body: Option<crate::platforms::http::Body>,
        ) -> Result<crate::platforms::http::Reply, String> {
            self.calls.lock().unwrap().push(format!("{method} {url}"));
            let body = if url == CERT_URL {
                Value::String(CERT.to_string())
            } else {
                Value::Null
            };
            Ok(crate::platforms::http::Reply { status: 200, body })
        }
    }

    fn parsed(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    fn sns<'a>(http: &'a Fake, certs: &'a Mutex<HashMap<String, Vec<u8>>>) -> Sns<'a> {
        Sns { http, certs }
    }

    #[test]
    fn a_version_1_and_a_version_2_notification_verify_under_the_generated_certificate() {
        let http = Fake::new();
        let certs = Mutex::new(HashMap::new());
        let client = sns(&http, &certs);
        client
            .verify(&parsed(V1))
            .expect("version 1 (SHA1) verifies");
        client
            .verify(&parsed(V2))
            .expect("version 2 (SHA256) verifies");
        assert_eq!(http.fetches(), 1, "the certificate is read once and cached");
    }

    #[test]
    fn a_changed_message_or_signature_is_refused() {
        let http = Fake::new();
        let certs = Mutex::new(HashMap::new());
        let client = sns(&http, &certs);
        let mut tampered = parsed(V2);
        tampered["Message"] = Value::String(
            tampered["Message"]
                .as_str()
                .unwrap()
                .replace("site-5xx", "site-4xx"),
        );
        assert!(
            client
                .verify(&tampered)
                .unwrap_err()
                .contains("does not match")
        );
        let mut wrong_version = parsed(V1);
        wrong_version["SignatureVersion"] = Value::String("2".into());
        assert!(
            client.verify(&wrong_version).is_err(),
            "SHA1 under the SHA256 rule is refused"
        );
        let mut garbled = parsed(V2);
        garbled["Signature"] = Value::String("!!not base64!!".into());
        assert!(client.verify(&garbled).unwrap_err().contains("base64"));
    }

    #[test]
    fn a_certificate_on_a_foreign_host_is_refused_before_anything_is_fetched() {
        let http = Fake::new();
        let certs = Mutex::new(HashMap::new());
        let client = sns(&http, &certs);
        let mut foreign = parsed(V2);
        foreign["SigningCertURL"] =
            Value::String("https://evil.example.com/SimpleNotificationService.pem".into());
        assert!(
            client
                .verify(&foreign)
                .unwrap_err()
                .contains("not on sns.eu-west-1")
        );
        let mut port = parsed(V2);
        port["SigningCertURL"] =
            Value::String("https://sns.eu-west-1.amazonaws.com:8443/cert.pem".into());
        assert!(client.verify(&port).is_err());
        let mut plain = parsed(V2);
        plain["SigningCertURL"] =
            Value::String("http://sns.eu-west-1.amazonaws.com/cert.pem".into());
        assert!(client.verify(&plain).is_err());
        assert_eq!(
            http.fetches(),
            0,
            "no certificate is fetched from a foreign host"
        );
    }

    #[test]
    fn the_host_rule_is_the_topic_regions_sns_host_and_nothing_else() {
        assert!(host_allowed(
            "https://sns.eu-west-1.amazonaws.com/x.pem",
            "eu-west-1"
        ));
        assert!(!host_allowed(
            "https://sns.us-east-1.amazonaws.com/x.pem",
            "eu-west-1"
        ));
        assert!(!host_allowed(
            "https://sns.eu-west-1.amazonaws.com.evil.example/x",
            "eu-west-1"
        ));
        assert!(!host_allowed(
            "https://user@sns.eu-west-1.amazonaws.com/x",
            "eu-west-1"
        ));
        assert!(!host_allowed("https://sns.eu-west-1.amazonaws.com", ""));
        assert_eq!(
            region_of("arn:aws:sns:eu-west-1:123456789012:plaza-alarms"),
            Some("eu-west-1")
        );
        assert_eq!(
            region_of("arn:aws:sns:Eu-West-1:1:t"),
            None,
            "a region is lower case"
        );
    }

    #[test]
    fn a_subscription_is_confirmed_only_once_its_signature_checks_out() {
        let http = Fake::new();
        let certs = Mutex::new(HashMap::new());
        let client = sns(&http, &certs);
        client
            .confirm(&parsed(SUBSCRIPTION))
            .expect("the subscription confirms");
        assert!(
            http.calls.lock().unwrap().iter().any(|c| c.starts_with(
                "GET https://sns.eu-west-1.amazonaws.com/?Action=ConfirmSubscription"
            )),
            "the SubscribeURL is fetched"
        );
        let before = http.fetches();
        let mut tampered = parsed(SUBSCRIPTION);
        tampered["SubscribeURL"] = Value::String(
            "https://sns.eu-west-1.amazonaws.com/?Action=ConfirmSubscription&Token=other".into(),
        );
        assert!(client.confirm(&tampered).is_err());
        assert_eq!(
            http.fetches(),
            before,
            "a confirmation whose signature fails is not fetched"
        );
    }

    #[test]
    fn the_canonical_string_is_the_documented_one_sorted_by_field() {
        let notification = canonical(&parsed(V1)).unwrap();
        assert!(notification.starts_with("Message\n"));
        assert!(
            notification.contains("\nSubject\nALARM: \"site-5xx\" in EU (Ireland)\nTimestamp\n")
        );
        assert!(notification.ends_with("\nType\nNotification\n"));
        let mut no_subject = parsed(V1);
        no_subject.as_object_mut().unwrap().remove("Subject");
        assert!(!canonical(&no_subject).unwrap().contains("Subject"));
        let confirmation = canonical(&parsed(SUBSCRIPTION)).unwrap();
        assert!(confirmation.contains("\nSubscribeURL\n"));
        assert!(confirmation.ends_with("\nType\nSubscriptionConfirmation\n"));
    }

    #[test]
    fn the_public_key_is_read_out_of_the_certificate() {
        let key = rsa_key_of_pem(CERT).expect("an RSA key");
        assert!(key.len() > 200, "a 2048-bit PKCS#1 key is about 270 bytes");
        assert_eq!(key[0], 0x30, "a PKCS#1 RSAPublicKey is a SEQUENCE");
        assert!(
            rsa_key_of_pem("-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----")
                .is_none()
        );
    }
}
