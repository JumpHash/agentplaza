//! AWS Signature Version 4 (header signing), enough for Bedrock.

use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::http::Req;

/// Static AWS credentials.
pub(crate) struct Credentials {
    pub access_key: String,
    pub secret_key: String,
    pub session_token: Option<String>,
}

/// Signs `req` in place: adds `x-amz-date`, `x-amz-security-token` (with a
/// session token) and `authorization`. Every header already on the request
/// is signed, plus `host`.
pub(crate) fn sign(
    req: &mut Req,
    creds: &Credentials,
    region: &str,
    service: &str,
    now: SystemTime,
) {
    let amz_date = amz_date(now);
    req.set_header("x-amz-date", &amz_date);
    if let Some(token) = &creds.session_token {
        req.set_header("x-amz-security-token", token);
    }
    let (host, path, query) = split_url(&req.url);
    let mut headers: Vec<(String, String)> = req
        .headers
        .iter()
        .map(|(n, v)| {
            (
                n.to_ascii_lowercase(),
                v.split_whitespace().collect::<Vec<_>>().join(" "),
            )
        })
        .collect();
    headers.push(("host".into(), host.to_string()));
    headers.sort();
    let signed: Vec<&str> = headers.iter().map(|(n, _)| n.as_str()).collect();
    let signed = signed.join(";");
    let canonical = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        req.method(),
        canonical_uri(path),
        canonical_query(query),
        headers
            .iter()
            .map(|(n, v)| format!("{n}:{v}\n"))
            .collect::<String>(),
        signed,
        hex(&Sha256::digest(&req.body)),
    );
    let date = &amz_date[..8];
    let scope = format!("{date}/{region}/{service}/aws4_request");
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex(&Sha256::digest(canonical.as_bytes()))
    );
    let signature = hex(&hmac(
        &signing_key(&creds.secret_key, date, region, service),
        to_sign.as_bytes(),
    ));
    let auth = format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed}, Signature={signature}",
        creds.access_key
    );
    req.set_header("authorization", &auth);
    req.set_secret(&creds.secret_key);
}

fn signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k_region = hmac(&k_date, region.as_bytes());
    let k_service = hmac(&k_region, service.as_bytes());
    hmac(&k_service, b"aws4_request")
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `scheme://host/path?query` → (host, path, query).
fn split_url(url: &str) -> (&str, &str, &str) {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let (host, path_query) = rest
        .find('/')
        .map_or((rest, "/"), |i| (&rest[..i], &rest[i..]));
    let (path, query) = path_query.split_once('?').unwrap_or((path_query, ""));
    (host, path, query)
}

/// Every path segment URI-encoded (again — non-S3 services sign the
/// encoded path encoded once more).
fn canonical_uri(path: &str) -> String {
    path.split('/')
        .map(uri_encode)
        .collect::<Vec<_>>()
        .join("/")
}

fn canonical_query(query: &str) -> String {
    let mut pairs: Vec<(String, String)> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (
                uri_encode(&percent_decode(k)),
                uri_encode(&percent_decode(v)),
            )
        })
        .collect();
    pairs.sort();
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// RFC 3986 encoding: everything but unreserved characters.
pub(crate) fn uri_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        match (
            bytes[i],
            bytes.get(i + 1).and_then(|c| hex(*c)),
            bytes.get(i + 2).and_then(|c| hex(*c)),
        ) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (b, _, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `YYYYMMDDTHHMMSSZ` in UTC.
fn amz_date(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn example_creds() -> Credentials {
        Credentials {
            access_key: "AKIDEXAMPLE".into(),
            secret_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
            session_token: None,
        }
    }

    /// 2015-08-30T12:36:00Z, the AWS examples' timestamp.
    fn example_time() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_440_938_160)
    }

    /// The AWS documentation's IAM `ListUsers` example and the SigV4 test
    /// suite's `get-vanilla` case.
    #[test]
    fn aws_published_vectors() {
        assert_eq!(amz_date(example_time()), "20150830T123600Z");
        assert_eq!(
            hex(&signing_key(
                "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
                "20150830",
                "us-east-1",
                "iam"
            )),
            "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9"
        );

        let mut iam = Req::get("https://iam.amazonaws.com/?Version=2010-05-08&Action=ListUsers")
            .header(
                "content-type",
                "application/x-www-form-urlencoded; charset=utf-8",
            );
        sign(
            &mut iam,
            &example_creds(),
            "us-east-1",
            "iam",
            example_time(),
        );
        assert_eq!(
            iam.header_value("authorization"),
            Some(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/iam/aws4_request, \
                 SignedHeaders=content-type;host;x-amz-date, \
                 Signature=5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7"
            )
        );

        let mut vanilla = Req::get("https://example.amazonaws.com/");
        sign(
            &mut vanilla,
            &example_creds(),
            "us-east-1",
            "service",
            example_time(),
        );
        assert_eq!(
            vanilla.header_value("authorization"),
            Some(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
                 SignedHeaders=host;x-amz-date, \
                 Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
            )
        );
        assert_eq!(
            canonical_uri("/model/us.anthropic.claude%3A0/converse"),
            "/model/us.anthropic.claude%253A0/converse"
        );
    }
}
