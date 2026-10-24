//! Store submissions against fake store APIs (PLAN 22.3, 9.6). Each store's API is one fake HTTP server on 127.0.0.1 that
//! checks what it is sent, verifies the credential's signature with the test's own key, and records the request; the
//! transport sends each store host to it (`registries::tests::Redirect`), and every request goes through the glue
//! mediator. The keys are generated here: an EC P-256 key (ring) for App Store Connect and an RSA key (`openssl genpkey`)
//! for Google's service account. The real accounts are not run.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use base64::Engine;
use ring::rand::SystemRandom;
use ring::signature::{
    ECDSA_P256_SHA256_FIXED, ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair,
    RSA_PKCS1_2048_8192_SHA256, RsaKeyPair, UnparsedPublicKey, VerificationAlgorithm,
};
use serde_json::{Value, json};

use super::{Options, Pace, Run, Step, Track, appstore, play, promote};
use crate::releases::entity::{self, Release, ReleaseArtifact, ReleaseState, ReleaseStoreStep};
use crate::releases::flow::{self, Builder, Built, Outcome};
use crate::releases::registries::tests::{Log, Redirect, Seen, add, fake, test_vault};
use crate::releases::tests::{World, cut, lab_with};
use crate::vault::Store as Vault;

const NOW_MS: u64 = 1_750_000_000_000;
const ASC_HOSTS: [&str; 2] = [
    "api.appstoreconnect.apple.com",
    "upload.appstoreconnect.apple.com",
];
const PLAY_HOSTS: [&str; 2] = ["oauth2.googleapis.com", "androidpublisher.googleapis.com"];
const PACKAGE: &str = "com.acme.app";
const IOS_TARGET: &str = "xcode:app@ios-arm64";
const ANDROID_TARGET: &str = "gradle:app@android-aarch64";

/// The bytes a target's artifact holds: a zip (the Android bundle's stand-in) or the iOS build's bytes.
fn bytes_of(target: &str) -> Vec<u8> {
    if target.starts_with("gradle:") {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file("base/root.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut writer, format!("built for {target}").as_bytes()).unwrap();
        writer.finish().unwrap().into_inner()
    } else {
        format!("the iOS build of {target}").into_bytes()
    }
}

/// Builds every target as its bytes.
struct Bundles;

impl Builder for Bundles {
    fn build(&self, _release: &Release, target: &str) -> Result<Outcome, String> {
        Ok(Outcome::Built(Built {
            bytes: bytes_of(target),
            pool: "fake-pool".into(),
            base: "fake-base".into(),
            builder: "fake-builder".into(),
            command: "package".into(),
        }))
    }
}

/// A published release by hand, with one artifact per target (digests `d0`, `d1`, …).
fn published(version: &str, number: Option<u32>, targets: &[&str]) -> Release {
    let artifacts = targets
        .iter()
        .enumerate()
        .map(|(i, target)| ReleaseArtifact {
            target: target.to_string(),
            artifact: format!("a{i}"),
            digest: format!("d{i}"),
            size: 1,
            signatures: String::new(),
            sbom: String::new(),
            provenance: String::new(),
            pool: String::new(),
            base: String::new(),
            run: String::new(),
        })
        .collect();
    Release {
        id: "rel-1".into(),
        project: "app".into(),
        repo: "app".into(),
        version: version.into(),
        channel: "stable".into(),
        base: None,
        lane: "main".into(),
        from: "rev".into(),
        state: ReleaseState::Published,
        number,
        notes: String::new(),
        run: None,
        changes: vec![],
        artifacts,
        not_run: vec![],
        verdicts: vec![],
        promotions: vec![],
        sync: vec![],
        store_steps: vec![],
        compatibility: vec![],
        created_ms: 1,
        updated_ms: 1,
    }
}

/// The PEM block of a DER body.
fn pem(label: &str, der: &[u8]) -> String {
    let body = base64::engine::general_purpose::STANDARD.encode(der);
    let lines: Vec<&str> = body
        .as_bytes()
        .chunks(64)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect();
    format!(
        "-----BEGIN {label}-----\n{}\n-----END {label}-----\n",
        lines.join("\n")
    )
}

/// The DER inside a PEM block.
fn der_of(pem: &str) -> Vec<u8> {
    let body: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
    base64::engine::general_purpose::STANDARD
        .decode(body)
        .unwrap()
}

/// An EC P-256 key for App Store Connect: its PEM (PKCS#8, as a `.p8` is) and its public point.
fn ec_key() -> (String, Vec<u8>) {
    let rng = SystemRandom::new();
    let doc = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
    let pair =
        EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, doc.as_ref(), &rng).unwrap();
    (
        pem("PRIVATE KEY", doc.as_ref()),
        pair.public_key().as_ref().to_vec(),
    )
}

/// An RSA key for Google's service account, made by OpenSSL here: its PEM (PKCS#8) and its public key (PKCS#1, as ring reads it).
fn rsa_key() -> (String, Vec<u8>) {
    let out = std::process::Command::new("openssl")
        .args([
            "genpkey",
            "-algorithm",
            "RSA",
            "-pkeyopt",
            "rsa_keygen_bits:2048",
        ])
        .output()
        .expect("openssl makes an RSA key");
    assert!(out.status.success(), "openssl genpkey failed");
    let text = String::from_utf8(out.stdout).unwrap();
    let pair = RsaKeyPair::from_pkcs8(&der_of(&text)).unwrap();
    (text, pair.public().as_ref().to_vec())
}

/// Verifies a JWT's signature with `key`, and returns its header and claims.
fn verify(jwt: &str, alg: &'static dyn VerificationAlgorithm, key: &[u8]) -> (Value, Value) {
    let parts: Vec<&str> = jwt.split('.').collect();
    assert_eq!(parts.len(), 3, "a JWT has three parts: {jwt}");
    let input = format!("{}.{}", parts[0], parts[1]);
    let sig = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[2])
        .unwrap();
    UnparsedPublicKey::new(alg, key)
        .verify(input.as_bytes(), &sig)
        .expect("the JWT's signature verifies with the test key");
    let decode = |part: &str| -> Value {
        serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(part)
                .unwrap(),
        )
        .unwrap()
    };
    (decode(parts[0]), decode(parts[1]))
}

/// Whether a JWT's signature verifies with `key` (the fakes' check).
fn signed_by(jwt: &str, alg: &'static dyn VerificationAlgorithm, key: &[u8]) -> bool {
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() != 3 {
        return false;
    }
    let input = format!("{}.{}", parts[0], parts[1]);
    let Ok(sig) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[2]) else {
        return false;
    };
    UnparsedPublicKey::new(alg, key)
        .verify(input.as_bytes(), &sig)
        .is_ok()
}

/// The path of a request, without its query.
fn path_of(seen: &Seen) -> &str {
    seen.path.split('?').next().unwrap_or("")
}

/// `METHOD path` for each request a fake received, in order.
fn calls(log: &Log) -> Vec<String> {
    log.lock()
        .unwrap()
        .iter()
        .map(|s| format!("{} {}", s.method, path_of(s)))
        .collect()
}

/// The vault of an App Store Connect submission: the key, its issuer, the app, each bound to the store's connector.
fn asc_vault(name: &str, key_pem: &str) -> Vault {
    test_vault(
        name,
        &[
            (
                "APP_STORE_CONNECT_KEY_ID",
                "connector:app-store-connect",
                "KEY123",
            ),
            (
                "APP_STORE_CONNECT_ISSUER_ID",
                "connector:app-store-connect",
                "issuer-uuid",
            ),
            (
                "APP_STORE_CONNECT_PRIVATE_KEY",
                "connector:app-store-connect",
                key_pem,
            ),
            (
                "APP_STORE_CONNECT_APP_ID",
                "connector:app-store-connect",
                "app-1",
            ),
        ],
    )
}

/// The vault of a Google Play submission: the service account's key file, and the package.
fn play_vault(name: &str, account: &str) -> Vault {
    test_vault(
        name,
        &[
            (
                "GOOGLE_PLAY_SERVICE_ACCOUNT",
                "connector:google-play",
                account,
            ),
            ("GOOGLE_PLAY_PACKAGE_NAME", "connector:google-play", PACKAGE),
        ],
    )
}

fn account_json(rsa_pem: &str) -> String {
    json!({
        "type": "service_account",
        "client_email": "publisher@acme-project.iam.gserviceaccount.com",
        "private_key": rsa_pem,
    })
    .to_string()
}

/// The fake App Store Connect API: every `/v1` request must carry a JWT the team's key signed. The build's upload part is
/// checked against `build`, and `fail_once` (a path suffix) fails its first request with a 500.
fn fake_asc(public: Vec<u8>, build: Vec<u8>, fail_once: Option<&'static str>) -> (u16, Log) {
    let failed = Mutex::new(false);
    let builds_seen = AtomicUsize::new(0);
    let part_len = build.len();
    fake(move |seen| {
        let path = path_of(seen);
        if seen.method == "PUT" && path == "/ops/part-1" {
            return if seen.body == build {
                (200, String::new())
            } else {
                (400, "the part is not the build".into())
            };
        }
        let signed = seen
            .headers
            .get("authorization")
            .and_then(|a| a.strip_prefix("Bearer "))
            .is_some_and(|token| signed_by(token, &ECDSA_P256_SHA256_FIXED, &public));
        if !signed {
            return (401, "{\"errors\":[{\"status\":\"401\"}]}".into());
        }
        if let Some(suffix) = fail_once {
            let mut done = failed.lock().unwrap();
            if path.ends_with(suffix) && !*done {
                *done = true;
                return (500, "{\"errors\":[{\"status\":\"500\"}]}".into());
            }
        }
        let answer = match (seen.method.as_str(), path) {
            ("GET", "/v1/builds") => {
                let n = builds_seen.fetch_add(1, Ordering::SeqCst);
                let data = match n {
                    0 => json!([]),
                    1 => {
                        json!([{ "id": "build-1", "attributes": { "processingState": "PROCESSING" } }])
                    }
                    _ => json!([{ "id": "build-1", "attributes": { "processingState": "VALID" } }]),
                };
                json!({ "data": data })
            }
            ("POST", "/v1/buildUploads") => {
                json!({ "data": { "type": "buildUploads", "id": "up-1" } })
            }
            ("POST", "/v1/buildUploadFiles") => json!({
                "data": {
                    "type": "buildUploadFiles",
                    "id": "uf-1",
                    "attributes": {
                        "uploadOperations": [{
                            "method": "PUT",
                            "url": "https://upload.appstoreconnect.apple.com/ops/part-1",
                            "offset": 0,
                            "length": part_len,
                            "requestHeaders": [{ "name": "Content-Type", "value": "application/octet-stream" }]
                        }]
                    }
                }
            }),
            ("PATCH", "/v1/buildUploadFiles/uf-1") => json!({}),
            ("GET", "/v1/betaGroups") => json!({
                "data": [
                    { "id": "g-int", "attributes": { "name": "Team", "isInternalGroup": true } },
                    { "id": "g-ext", "attributes": { "name": "Public", "isInternalGroup": false } }
                ]
            }),
            ("POST", "/v1/betaGroups/g-int/relationships/builds")
            | ("POST", "/v1/betaGroups/g-ext/relationships/builds") => json!({}),
            ("POST", "/v1/betaAppReviewSubmissions") => json!({ "data": { "id": "bar-1" } }),
            ("GET", "/v1/apps/app-1/appStoreVersions") => json!({ "data": [] }),
            ("POST", "/v1/appStoreVersions") => json!({ "data": { "id": "ver-1" } }),
            ("PATCH", "/v1/appStoreVersions/ver-1/relationships/build") => json!({}),
            ("POST", "/v1/appStoreVersionPhasedReleases") => json!({ "data": { "id": "ph-1" } }),
            ("POST", "/v1/reviewSubmissions") => json!({ "data": { "id": "rs-1" } }),
            ("POST", "/v1/reviewSubmissionItems") => json!({ "data": { "id": "rsi-1" } }),
            ("PATCH", "/v1/reviewSubmissions/rs-1") => json!({}),
            _ => return (404, "{}".into()),
        };
        (200, answer.to_string())
    })
}

/// The fake Google Play: the token endpoint checks the service account's RS256 assertion; the API checks the access token
/// it issued. The bundle is checked against `bundle`; `fail_once` (a path suffix) fails its first request with a 500.
fn fake_play(rsa_public: Vec<u8>, bundle: Vec<u8>, fail_once: Option<&'static str>) -> (u16, Log) {
    let failed = Mutex::new(false);
    fake(move |seen| {
        let path = path_of(seen);
        if seen.method == "POST" && path == "/token" {
            let form = String::from_utf8_lossy(&seen.body).into_owned();
            let assertion = form.split("assertion=").nth(1).unwrap_or("");
            let granted = form
                .starts_with("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer");
            if !granted || !signed_by(assertion, &RSA_PKCS1_2048_8192_SHA256, &rsa_public) {
                return (400, "{\"error\":\"invalid_grant\"}".into());
            }
            return (
                200,
                json!({ "access_token": "ya29.test-token", "expires_in": 3600, "token_type": "Bearer" })
                    .to_string(),
            );
        }
        if seen.headers.get("authorization").map(String::as_str) != Some("Bearer ya29.test-token") {
            return (401, "{\"error\":{\"code\":401}}".into());
        }
        if let Some(suffix) = fail_once {
            let mut done = failed.lock().unwrap();
            if path.ends_with(suffix) && !*done {
                *done = true;
                return (500, "{\"error\":{\"code\":500}}".into());
            }
        }
        let app = format!("/androidpublisher/v3/applications/{PACKAGE}");
        let answer = match seen.method.as_str() {
            "POST" if path == format!("{app}/edits") => json!({ "id": "edit-1" }),
            "POST"
                if path
                    == format!(
                        "/upload/androidpublisher/v3/applications/{PACKAGE}/edits/edit-1/bundles"
                    ) =>
            {
                if seen.body != bundle {
                    return (400, "the bundle is not the release's".into());
                }
                json!({ "versionCode": 42 })
            }
            "PUT" if path.starts_with(&format!("{app}/edits/edit-1/tracks/")) => json!({}),
            "POST" if path == format!("{app}/edits/edit-1:commit") => json!({ "id": "edit-1" }),
            _ => return (404, "{}".into()),
        };
        (200, answer.to_string())
    })
}

/// The steps a track's run recorded on the release, as (step, state) in the order they were recorded.
fn steps_of(store: &entity::Store, track: Track) -> Vec<(String, String)> {
    store
        .get("rel-1")
        .unwrap()
        .store_steps
        .into_iter()
        .filter(|s| s.track == track.name())
        .map(|s| (s.step, s.state))
        .collect()
}

fn step_row<'a>(rows: &'a [ReleaseStoreStep], step: &str) -> &'a ReleaseStoreStep {
    rows.iter()
        .find(|s| s.step == step)
        .expect("the step is recorded")
}

fn pace() -> Pace {
    Pace {
        polls: 3,
        interval: Duration::ZERO,
    }
}

/// The run of a track over one release held in `store`.
fn run_in(store: &entity::Store, track: Track) -> Run<'_> {
    Run {
        releases: store,
        id: "rel-1",
        track,
        at_ms: NOW_MS,
    }
}

/// Checks that every `/v1` request of an App Store Connect fake carried a JWT for the team: the key id in the header, the
/// issuer in the claims, the audience, and at most twenty minutes of life.
fn assert_asc_jwts(log: &Log, public: &[u8]) {
    for seen in log.lock().unwrap().iter() {
        let Some(token) = seen
            .headers
            .get("authorization")
            .and_then(|a| a.strip_prefix("Bearer "))
        else {
            continue;
        };
        let (header, claims) = verify(token, &ECDSA_P256_SHA256_FIXED, public);
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["kid"], "KEY123");
        assert_eq!(claims["iss"], "issuer-uuid");
        assert_eq!(claims["aud"], "appstoreconnect-v1");
        assert!(claims["exp"].as_u64().unwrap() - claims["iat"].as_u64().unwrap() <= 1200);
    }
}

#[test]
fn the_jwts_are_signed_with_the_keys_they_name_and_a_tampered_one_does_not_verify() {
    let (ec_pem, ec_public) = ec_key();
    let token = super::jwt::es256(
        &ec_pem,
        "KEY123",
        &json!({ "iss": "issuer-uuid", "iat": 1, "exp": 1201, "aud": "appstoreconnect-v1" }),
    )
    .unwrap();
    let (header, claims) = verify(&token, &ECDSA_P256_SHA256_FIXED, &ec_public);
    assert_eq!(
        header,
        json!({ "alg": "ES256", "kid": "KEY123", "typ": "JWT" })
    );
    assert_eq!(claims["iss"], "issuer-uuid");
    let (head, rest) = token.split_once('.').unwrap();
    let forged = format!(
        "{head}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"iss":"someone-else"}"#)
    );
    let (_, sig) = rest.split_once('.').unwrap();
    assert!(!signed_by(
        &format!("{forged}.{sig}"),
        &ECDSA_P256_SHA256_FIXED,
        &ec_public
    ));

    let (rsa_pem, rsa_public) = rsa_key();
    let assertion = super::jwt::rs256(
        &rsa_pem,
        &json!({ "iss": "publisher@acme", "scope": "s", "aud": "https://oauth2.googleapis.com/token", "iat": 1, "exp": 3601 }),
    )
    .unwrap();
    let (header, claims) = verify(&assertion, &RSA_PKCS1_2048_8192_SHA256, &rsa_public);
    assert_eq!(header["alg"], "RS256");
    assert_eq!(claims["exp"], 3601);
}

#[test]
fn a_build_reaches_testflight_internal_in_the_order_apple_takes_it() {
    let (ec_pem, ec_public) = ec_key();
    let build = bytes_of(IOS_TARGET);
    let (asc, log) = fake_asc(ec_public.clone(), build.clone(), None);
    let redirect = Redirect::new(ASC_HOSTS.iter().map(|h| (*h, asc)).collect());
    let vault = asc_vault("testflight_internal", &ec_pem);
    let store = entity::Store::default();
    let rel = published("1.2.0", Some(3), &[IOS_TARGET]);
    store.insert(rel.clone());
    let s = Step {
        outbound: &redirect,
        vault: &vault,
        at_ms: NOW_MS,
        pace: pace(),
    };
    appstore::submit(
        &run_in(&store, Track::TestFlightInternal),
        &rel,
        Track::TestFlightInternal,
        &Options::of(&json!({}), Track::TestFlightInternal).unwrap(),
        &s,
        &build,
    )
    .unwrap();

    assert_eq!(
        calls(&log),
        [
            "GET /v1/builds",
            "POST /v1/buildUploads",
            "POST /v1/buildUploadFiles",
            "PUT /ops/part-1",
            "PATCH /v1/buildUploadFiles/uf-1",
            "GET /v1/builds",
            "GET /v1/builds",
            "GET /v1/betaGroups",
            "POST /v1/betaGroups/g-int/relationships/builds",
        ]
    );
    assert_asc_jwts(&log, &ec_public);
    assert_eq!(
        steps_of(&store, Track::TestFlightInternal),
        [
            ("auth", "done"),
            ("upload", "done"),
            ("processing", "done"),
            ("groups", "done")
        ]
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .to_vec()
    );
    let rows = store.get("rel-1").unwrap().store_steps;
    assert_eq!(step_row(&rows, "processing").reference, "build-1");
    assert_eq!(step_row(&rows, "groups").reference, "g-int");
}

#[test]
fn an_external_testflight_group_takes_a_beta_review_and_the_app_store_takes_a_phased_review() {
    let (ec_pem, ec_public) = ec_key();
    let build = bytes_of(IOS_TARGET);

    let (asc, log) = fake_asc(ec_public.clone(), build.clone(), None);
    let redirect = Redirect::new(ASC_HOSTS.iter().map(|h| (*h, asc)).collect());
    let vault = asc_vault("external", &ec_pem);
    let store = entity::Store::default();
    let rel = published("1.2.0", Some(3), &[IOS_TARGET]);
    store.insert(rel.clone());
    let s = Step {
        outbound: &redirect,
        vault: &vault,
        at_ms: NOW_MS,
        pace: pace(),
    };
    appstore::submit(
        &run_in(&store, Track::TestFlightExternal),
        &rel,
        Track::TestFlightExternal,
        &Options::of(&json!({}), Track::TestFlightExternal).unwrap(),
        &s,
        &build,
    )
    .unwrap();
    let external = calls(&log);
    assert_eq!(
        external[external.len() - 3..].to_vec(),
        [
            "GET /v1/betaGroups",
            "POST /v1/betaGroups/g-ext/relationships/builds",
            "POST /v1/betaAppReviewSubmissions"
        ]
        .map(String::from)
        .to_vec()
    );
    assert!(
        !external.iter().any(|c| c.contains("g-int")),
        "the external run leaves the internal group alone"
    );

    let (asc, log) = fake_asc(ec_public.clone(), build.clone(), None);
    let redirect = Redirect::new(ASC_HOSTS.iter().map(|h| (*h, asc)).collect());
    let store = entity::Store::default();
    store.insert(rel.clone());
    let s = Step {
        outbound: &redirect,
        vault: &vault,
        at_ms: NOW_MS,
        pace: pace(),
    };
    let options = Options::of(&json!({ "phased": true }), Track::AppStore).unwrap();
    appstore::submit(
        &run_in(&store, Track::AppStore),
        &rel,
        Track::AppStore,
        &options,
        &s,
        &build,
    )
    .unwrap();
    let appstore_calls = calls(&log);
    // The upload and the processing wait (seven requests) come first.
    assert_eq!(
        appstore_calls[7..].to_vec(),
        [
            "GET /v1/apps/app-1/appStoreVersions",
            "POST /v1/appStoreVersions",
            "PATCH /v1/appStoreVersions/ver-1/relationships/build",
            "POST /v1/appStoreVersionPhasedReleases",
            "POST /v1/reviewSubmissions",
            "POST /v1/reviewSubmissionItems",
            "PATCH /v1/reviewSubmissions/rs-1",
        ]
        .map(String::from)
        .to_vec()
    );
    let phased = log
        .lock()
        .unwrap()
        .iter()
        .find(|s| path_of(s) == "/v1/appStoreVersionPhasedReleases")
        .cloned()
        .unwrap();
    let body: Value = serde_json::from_slice(&phased.body).unwrap();
    assert_eq!(body["data"]["attributes"]["phasedReleaseState"], "ACTIVE");
    assert_eq!(
        body["data"]["relationships"]["appStoreVersion"]["data"]["id"],
        "ver-1"
    );
    let submitted = log
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|s| s.method == "PATCH" && path_of(s) == "/v1/reviewSubmissions/rs-1")
        .cloned()
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&submitted.body).unwrap()["data"]["attributes"]["submitted"].as_bool(),
        Some(true)
    );
    assert_asc_jwts(&log, &ec_public);

    // Phased release off: the version is released to everyone at once, and no phased release is made.
    let (asc, log) = fake_asc(ec_public.clone(), build.clone(), None);
    let redirect = Redirect::new(ASC_HOSTS.iter().map(|h| (*h, asc)).collect());
    let store = entity::Store::default();
    store.insert(rel.clone());
    let s = Step {
        outbound: &redirect,
        vault: &vault,
        at_ms: NOW_MS,
        pace: pace(),
    };
    let full = Options::of(&json!({ "phased": false }), Track::AppStore).unwrap();
    appstore::submit(
        &run_in(&store, Track::AppStore),
        &rel,
        Track::AppStore,
        &full,
        &s,
        &build,
    )
    .unwrap();
    assert!(!calls(&log).iter().any(|c| c.contains("PhasedReleases")));
    assert_eq!(
        calls(&log).last().unwrap(),
        "PATCH /v1/reviewSubmissions/rs-1"
    );
}

#[test]
fn a_failed_step_stops_the_run_and_a_retry_resumes_from_it() {
    let (ec_pem, ec_public) = ec_key();
    let build = bytes_of(IOS_TARGET);
    // The beta groups' listing fails the first time.
    let (asc, log) = fake_asc(ec_public.clone(), build.clone(), Some("/v1/betaGroups"));
    let redirect = Redirect::new(ASC_HOSTS.iter().map(|h| (*h, asc)).collect());
    let vault = asc_vault("resume", &ec_pem);
    let store = entity::Store::default();
    let rel = published("1.2.0", Some(3), &[IOS_TARGET]);
    store.insert(rel.clone());
    let s = Step {
        outbound: &redirect,
        vault: &vault,
        at_ms: NOW_MS,
        pace: pace(),
    };
    let options = Options::of(&json!({}), Track::TestFlightInternal).unwrap();
    let err = appstore::submit(
        &run_in(&store, Track::TestFlightInternal),
        &rel,
        Track::TestFlightInternal,
        &options,
        &s,
        &build,
    )
    .unwrap_err();
    assert!(err.contains("groups failed"), "{err}");
    assert_eq!(
        steps_of(&store, Track::TestFlightInternal),
        [
            ("auth", "done"),
            ("upload", "done"),
            ("processing", "done"),
            ("groups", "failed")
        ]
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .to_vec()
    );
    let failed = store.get("rel-1").unwrap().store_steps;
    assert!(step_row(&failed, "groups").reason.contains("500"));

    let before = log.lock().unwrap().len();
    appstore::submit(
        &run_in(&store, Track::TestFlightInternal),
        &rel,
        Track::TestFlightInternal,
        &options,
        &s,
        &build,
    )
    .unwrap();
    let retried: Vec<String> = calls(&log)[before..].to_vec();
    assert_eq!(
        retried,
        [
            "GET /v1/betaGroups",
            "POST /v1/betaGroups/g-int/relationships/builds"
        ]
        .map(String::from)
        .to_vec(),
        "the retry sends only what had not been done"
    );
    assert_eq!(
        steps_of(&store, Track::TestFlightInternal),
        [
            ("auth", "done"),
            ("upload", "done"),
            ("processing", "done"),
            ("groups", "done")
        ]
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .to_vec()
    );
}

#[test]
fn a_bundle_reaches_the_play_internal_track_in_one_committed_edit() {
    let (rsa_pem, rsa_public) = rsa_key();
    let bundle = bytes_of(ANDROID_TARGET);
    let (play_server, log) = fake_play(rsa_public.clone(), bundle.clone(), None);
    let redirect = Redirect::new(PLAY_HOSTS.iter().map(|h| (*h, play_server)).collect());
    let vault = play_vault("play_internal", &account_json(&rsa_pem));
    let store = entity::Store::default();
    let rel = published("1.2.0", Some(3), &[ANDROID_TARGET]);
    store.insert(rel.clone());
    let s = Step {
        outbound: &redirect,
        vault: &vault,
        at_ms: NOW_MS,
        pace: pace(),
    };
    let options = Options::of(&json!({}), Track::PlayInternal).unwrap();
    play::submit(
        &run_in(&store, Track::PlayInternal),
        Track::PlayInternal,
        &options,
        &s,
        &bundle,
    )
    .unwrap();

    assert_eq!(
        calls(&log),
        [
            "POST /token",
            "POST /androidpublisher/v3/applications/com.acme.app/edits",
            "POST /upload/androidpublisher/v3/applications/com.acme.app/edits/edit-1/bundles",
            "PUT /androidpublisher/v3/applications/com.acme.app/edits/edit-1/tracks/internal",
            "POST /androidpublisher/v3/applications/com.acme.app/edits/edit-1:commit",
        ]
    );
    let seen = log.lock().unwrap().clone();
    let track_body: Value = serde_json::from_slice(&seen[3].body).unwrap();
    assert_eq!(
        track_body,
        json!({ "track": "internal", "releases": [{ "versionCodes": [42], "status": "completed" }] })
    );
    let (header, claims) = verify(
        std::str::from_utf8(&seen[0].body)
            .unwrap()
            .split("assertion=")
            .nth(1)
            .unwrap(),
        &RSA_PKCS1_2048_8192_SHA256,
        &rsa_public,
    );
    assert_eq!(header["alg"], "RS256");
    assert_eq!(
        claims["iss"],
        "publisher@acme-project.iam.gserviceaccount.com"
    );
    assert_eq!(claims["aud"], "https://oauth2.googleapis.com/token");
    assert_eq!(
        claims["scope"],
        "https://www.googleapis.com/auth/androidpublisher"
    );
    assert_eq!(
        claims["exp"].as_u64().unwrap() - claims["iat"].as_u64().unwrap(),
        3600
    );
    assert_eq!(
        steps_of(&store, Track::PlayInternal),
        [
            ("auth", "done"),
            ("edit", "done"),
            ("upload", "done"),
            ("track", "done"),
            ("commit", "done")
        ]
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .to_vec()
    );
}

#[test]
fn a_staged_rollout_sends_its_fraction_on_the_production_track_and_only_there() {
    let (rsa_pem, rsa_public) = rsa_key();
    let bundle = bytes_of(ANDROID_TARGET);
    let (play_server, log) = fake_play(rsa_public, bundle.clone(), None);
    let redirect = Redirect::new(PLAY_HOSTS.iter().map(|h| (*h, play_server)).collect());
    let vault = play_vault("rollout", &account_json(&rsa_pem));
    let store = entity::Store::default();
    let rel = published("1.2.0", Some(3), &[ANDROID_TARGET]);
    store.insert(rel.clone());
    let s = Step {
        outbound: &redirect,
        vault: &vault,
        at_ms: NOW_MS,
        pace: pace(),
    };
    let options = Options::of(&json!({ "rollout": 0.25 }), Track::PlayProduction).unwrap();
    play::submit(
        &run_in(&store, Track::PlayProduction),
        Track::PlayProduction,
        &options,
        &s,
        &bundle,
    )
    .unwrap();
    let put = log
        .lock()
        .unwrap()
        .iter()
        .find(|s| s.method == "PUT")
        .cloned()
        .unwrap();
    assert!(path_of(&put).ends_with("/tracks/production"));
    let body: Value = serde_json::from_slice(&put.body).unwrap();
    assert_eq!(body["releases"][0]["status"], "inProgress");
    assert_eq!(body["releases"][0]["userFraction"], 0.25);

    // A fraction is refused where there is no staged rollout, and out of range.
    assert!(
        Options::of(&json!({ "rollout": 0.25 }), Track::PlayInternal)
            .unwrap_err()
            .contains("no staged rollout")
    );
    assert!(Options::of(&json!({ "rollout": 0 }), Track::PlayProduction).is_err());
    assert!(Options::of(&json!({ "rollout": 1.5 }), Track::PlayProduction).is_err());
    assert_eq!(
        Options::of(&json!({ "rollout": 1.0 }), Track::PlayProduction)
            .unwrap()
            .rollout,
        None
    );
    assert_eq!(
        Options::of(&json!({}), Track::AppStore).unwrap(),
        Options {
            phased: true,
            rollout: None
        }
    );
}

#[test]
fn a_failed_commit_is_retried_on_the_same_edit_without_sending_the_bundle_again() {
    let (rsa_pem, rsa_public) = rsa_key();
    let bundle = bytes_of(ANDROID_TARGET);
    let (play_server, log) = fake_play(rsa_public, bundle.clone(), Some(":commit"));
    let redirect = Redirect::new(PLAY_HOSTS.iter().map(|h| (*h, play_server)).collect());
    let vault = play_vault("commit", &account_json(&rsa_pem));
    let store = entity::Store::default();
    let rel = published("1.2.0", Some(3), &[ANDROID_TARGET]);
    store.insert(rel.clone());
    let s = Step {
        outbound: &redirect,
        vault: &vault,
        at_ms: NOW_MS,
        pace: pace(),
    };
    let options = Options::of(&json!({}), Track::PlayInternal).unwrap();
    let err = play::submit(
        &run_in(&store, Track::PlayInternal),
        Track::PlayInternal,
        &options,
        &s,
        &bundle,
    )
    .unwrap_err();
    assert!(err.contains("commit failed"), "{err}");
    let before = log.lock().unwrap().len();
    play::submit(
        &run_in(&store, Track::PlayInternal),
        Track::PlayInternal,
        &options,
        &s,
        &bundle,
    )
    .unwrap();
    assert_eq!(
        calls(&log)[before..].to_vec(),
        [
            "POST /token",
            "POST /androidpublisher/v3/applications/com.acme.app/edits/edit-1:commit"
        ]
        .map(String::from)
        .to_vec(),
        "the retry takes a new token and commits the edit it has"
    );
}

#[test]
fn a_store_secret_the_vault_does_not_bind_to_its_store_is_not_read() {
    let (rsa_pem, rsa_public) = rsa_key();
    let bundle = bytes_of(ANDROID_TARGET);
    let (play_server, log) = fake_play(rsa_public, bundle.clone(), None);
    let redirect = Redirect::new(PLAY_HOSTS.iter().map(|h| (*h, play_server)).collect());
    // The service account is bound to npm's connector, not to Google Play's.
    let vault = test_vault(
        "unbound",
        &[("GOOGLE_PLAY_PACKAGE_NAME", "connector:google-play", PACKAGE)],
    );
    add(
        &vault,
        "GOOGLE_PLAY_SERVICE_ACCOUNT",
        "connector:npm",
        &account_json(&rsa_pem),
    );
    let store = entity::Store::default();
    let rel = published("1.2.0", Some(3), &[ANDROID_TARGET]);
    store.insert(rel.clone());
    let s = Step {
        outbound: &redirect,
        vault: &vault,
        at_ms: NOW_MS,
        pace: pace(),
    };
    let options = Options::of(&json!({}), Track::PlayInternal).unwrap();
    let err = play::submit(
        &run_in(&store, Track::PlayInternal),
        Track::PlayInternal,
        &options,
        &s,
        &bundle,
    )
    .unwrap_err();
    assert!(
        err.contains("GOOGLE_PLAY_SERVICE_ACCOUNT is not in the vault"),
        "{err}"
    );
    assert!(
        log.lock().unwrap().is_empty(),
        "nothing is sent without the credential"
    );
    let rows = store.get("rel-1").unwrap().store_steps;
    assert_eq!(step_row(&rows, "auth").state, "failed");
}

#[test]
fn an_upload_part_is_only_sent_to_apples_own_addresses() {
    let redirect = Redirect::new(vec![]);
    let operation = json!({ "method": "PUT", "url": "https://evil.example.com/part", "offset": 0, "length": 3 });
    let err = appstore::send_part(&redirect, &operation, b"abc").unwrap_err();
    assert!(err.contains("not Apple's"), "{err}");
}

#[test]
fn a_release_is_promoted_to_play_internal_through_weft_once_its_track_has_the_bundle() {
    let (rsa_pem, rsa_public) = rsa_key();
    let (play_server, log) = fake_play(rsa_public, bytes_of(ANDROID_TARGET), None);
    let redirect = Redirect::new(PLAY_HOSTS.iter().map(|h| (*h, play_server)).collect());
    let vault = play_vault("promote", &account_json(&rsa_pem));

    let (client, _handle, _dir, _change, _agent) = lab_with(
        "store_promote",
        &[("build.gradle", "plugins { id 'com.android.application' }\n")],
    );
    let world = World::new(client, "store_promote");
    let d = world.deps();
    let drafted = flow::draft(&d, "app", "app", &cut("0.1.0")).unwrap();
    let s = Step {
        outbound: &redirect,
        vault: &vault,
        at_ms: NOW_MS,
        pace: pace(),
    };
    let options = Options::of(&json!({}), Track::PlayInternal).unwrap();
    let early = promote(&d, &drafted.id, Track::PlayInternal, &options, &s).unwrap_err();
    assert!(
        early.contains("only a published release is promoted"),
        "{early}"
    );
    assert!(log.lock().unwrap().is_empty());

    flow::candidate(&d, &drafted.id, &Bundles, Some(1)).unwrap();
    flow::checks(
        &d,
        &drafted.id,
        true,
        "fake CI: every check passed on the release commit",
    )
    .unwrap();
    flow::publish(&d, &drafted.id).unwrap();
    let promoted = promote(&d, &drafted.id, Track::PlayInternal, &options, &s).unwrap();
    assert_eq!(promoted.state, ReleaseState::Promoted);
    assert_eq!(promoted.promotions[0].environment, "play/internal");
    assert_eq!(
        steps_of(&world.releases, Track::PlayInternal).len(),
        5,
        "each step of the track is recorded on the release"
    );
    let sent = log.lock().unwrap().len();
    promote(&d, &drafted.id, Track::PlayInternal, &options, &s).unwrap();
    assert_eq!(
        log.lock().unwrap().len(),
        sent,
        "a promotion already made is not sent again"
    );
}

#[test]
fn the_store_manifests_pass_the_checks_and_name_the_hosts_the_calls_reach() {
    let manifests = crate::connectors::manifest::stores();
    assert_eq!(manifests.len(), 2);
    for m in &manifests {
        assert_eq!(
            crate::connectors::manifest::check(m),
            Vec::<String>::new(),
            "{}",
            m.id
        );
        assert_eq!(m.auth_header, "Authorization");
        assert_eq!(m.auth_format, "Bearer ${SECRET}");
    }
    let asc = manifests
        .iter()
        .find(|m| m.id == "app-store-connect")
        .unwrap();
    assert_eq!(asc.egress, ["api.appstoreconnect.apple.com"]);
    let play = manifests.iter().find(|m| m.id == "google-play").unwrap();
    assert_eq!(
        play.egress,
        ["oauth2.googleapis.com", "androidpublisher.googleapis.com"]
    );
    assert_eq!(Track::of("play/production"), Some(Track::PlayProduction));
    assert_eq!(
        Track::of("production"),
        None,
        "a deploy environment is not a store track"
    );
    assert_eq!(super::TRACKS.len(), 7);
}
