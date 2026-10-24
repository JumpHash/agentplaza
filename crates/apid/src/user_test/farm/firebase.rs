//! Firebase Test Lab (PLAN 19.3, 8.5): an instrumentation suite as a test matrix.
//!
//! The credential is a service account (`FIREBASE_SERVICE_ACCOUNT`, its key file; `FIREBASE_PROJECT_ID`; and the Cloud Storage
//! bucket the packages are uploaded to, `FIREBASE_STORAGE_BUCKET`). The hub signs an RS256 JWT with the account's key and
//! exchanges it for an access token (the bearer grant the Play store uses, `releases::stores::jwt`). The app and the test
//! package are uploaded to the bucket (`storage.googleapis.com`, `uploadType=media`), and the matrix is created with
//! `POST https://testing.googleapis.com/v1/projects/{project}/testMatrices`. The matrix is read until it is finished.
//!
//! ASSUMED from Google's public reference (not checked against the live API): the token request and the Cloud Storage upload
//! are as the store code uses them; the matrix's `state` is `FINISHED`, `ERROR` or `INVALID` when it has ended; each entry of
//! `testExecutions` names its `id`, its `environment.androidDevice` (`androidModelId`, `androidVersionId`) and its
//! `outcome.outcomeSummary` (`SUCCESS` for a pass), and its `testDetails` may name a `videoUri` and a `logUri`, which are
//! downloaded into the run. Any of these that differs from the live API is a change to this file only.

use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    Api, Artifact, Deps, Report, Session, answered, collect, credential, manifest_of, secrets,
    unix_nanos, unix_seconds,
};
use crate::recipes::model::FarmTest;
use crate::releases::stores::jwt;

const CONNECTOR: &str = "firebase-test-lab";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
pub const API: &str = "https://testing.googleapis.com/v1";
const STORAGE: &str = "https://storage.googleapis.com/upload/storage/v1";

/// The parts of a service account's key file that the token request needs.
#[derive(Deserialize)]
struct ServiceAccount {
    client_email: String,
    private_key: String,
}

/// Runs the suite as a test matrix: the packages up, the matrix created, read until finished, and its executions as sessions.
pub fn run(deps: &Deps, config: &FarmTest, app: &[u8], tests: &[u8]) -> Result<Report, String> {
    let values = secrets(
        deps,
        CONNECTOR,
        &[
            "FIREBASE_SERVICE_ACCOUNT",
            "FIREBASE_PROJECT_ID",
            "FIREBASE_STORAGE_BUCKET",
        ],
    )?;
    let account: ServiceAccount = serde_json::from_str(&values[0])
        .map_err(|e| format!("FIREBASE_SERVICE_ACCOUNT is not a service account key: {e}"))?;
    let (project, bucket) = (values[1].as_str(), values[2].as_str());
    let approved = manifest_of(CONNECTOR)?;
    let now = unix_seconds();
    let assertion = jwt::rs256(
        &account.private_key,
        &json!({
            "iss": account.client_email,
            "scope": SCOPE,
            "aud": TOKEN_URL,
            "iat": now,
            "exp": now + 3600
        }),
    )?;
    let form = format!(
        "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={assertion}"
    );
    let exchange = Api {
        outbound: deps.outbound,
        egress: approved.egress.clone(),
        secret: None,
    };
    let reply = exchange.send(
        "POST",
        TOKEN_URL,
        &[("Content-Type", "application/x-www-form-urlencoded")],
        form.as_bytes(),
    )?;
    answered("POST", TOKEN_URL, &reply)?;
    let token: Value = serde_json::from_str(&reply.body)
        .map_err(|e| format!("the token endpoint answered text that is not JSON: {e}"))?;
    let token = token["access_token"]
        .as_str()
        .ok_or("the token endpoint gave no access token")?;
    let api = Api {
        outbound: deps.outbound,
        egress: approved.egress.clone(),
        secret: Some(credential(&approved, token)),
    };

    let stamp = unix_nanos();
    let app_gcs = upload(&api, bucket, &format!("apz-farm-{stamp}-app.apk"), app)?;
    let test_gcs = upload(&api, bucket, &format!("apz-farm-{stamp}-tests.apk"), tests)?;
    let devices = devices_of(&config.devices)?;
    let url = format!("{API}/projects/{project}/testMatrices");
    let created = api.json(
        "POST",
        &url,
        Some(&matrix_request(&app_gcs, &test_gcs, &devices)),
    )?;
    let id = created["testMatrixId"]
        .as_str()
        .ok_or("the test matrix names no testMatrixId")?
        .to_string();
    let read_url = format!("{url}/{id}");
    for poll in 0..deps.pace.polls {
        let doc = api.json("GET", &read_url, None)?;
        if finished(&doc) {
            return Ok(collect(&api, report(&id, &doc)));
        }
        deps.pace.wait(poll);
    }
    Err(format!(
        "test matrix {id} is not finished after {} checks",
        deps.pace.polls
    ))
}

/// Uploads one file to the bucket, and answers its `gs://` address (the matrix names the packages by it).
fn upload(api: &Api, bucket: &str, object: &str, bytes: &[u8]) -> Result<String, String> {
    let url = format!("{STORAGE}/b/{bucket}/o?uploadType=media&name={object}");
    let reply = api.send(
        "POST",
        &url,
        &[("Content-Type", "application/octet-stream")],
        bytes,
    )?;
    answered("POST", &url, &reply)?;
    Ok(format!("gs://{bucket}/{object}"))
}

/// The devices as the matrix takes them: `model:version` (for example `Pixel2:28`).
pub fn devices_of(names: &[String]) -> Result<Vec<(String, String)>, String> {
    names
        .iter()
        .map(|name| {
            name.split_once(':')
                .filter(|(model, version)| !model.is_empty() && !version.is_empty())
                .map(|(model, version)| (model.to_string(), version.to_string()))
                .ok_or_else(|| {
                    format!("a Firebase device is model:version (Pixel2:28), not {name}")
                })
        })
        .collect()
}

/// The report of a finished matrix: its executions, one session each.
pub fn report(matrix: &str, doc: &Value) -> Report {
    let sessions = doc["testExecutions"]
        .as_array()
        .map(|list| list.iter().map(session).collect())
        .unwrap_or_default();
    Report::of(matrix.to_string(), sessions)
}

fn session(e: &Value) -> Session {
    let device = &e["environment"]["androidDevice"];
    let mut artifacts = Vec::new();
    for (name, field) in [("video.mp4", "videoUri"), ("log.txt", "logUri")] {
        if let Some(url) = e["testDetails"][field].as_str() {
            artifacts.push(Artifact {
                name: name.to_string(),
                url: url.to_string(),
            });
        }
    }
    Session {
        id: e["id"].as_str().unwrap_or_default().to_string(),
        device: format!(
            "{} {}",
            device["androidModelId"].as_str().unwrap_or_default(),
            device["androidVersionId"].as_str().unwrap_or_default()
        ),
        passed: e["outcome"]["outcomeSummary"].as_str() == Some("SUCCESS"),
        links: Vec::new(),
        tests: Vec::new(),
        artifacts,
    }
}

/// The create request of a test matrix: the packages' locations and the devices by model and API level.
pub fn matrix_request(app_gcs: &str, test_gcs: &str, devices: &[(String, String)]) -> Value {
    let devices: Vec<Value> = devices
        .iter()
        .map(|(model, version)| {
            json!({
                "androidModelId": model,
                "androidVersionId": version,
                "locale": "en",
                "orientation": "portrait",
            })
        })
        .collect();
    json!({
        "testSpecification": {
            "androidInstrumentationTest": {
                "appApk": {"gcsPath": app_gcs},
                "testApk": {"gcsPath": test_gcs},
            }
        },
        "environmentMatrix": {
            "androidDeviceList": {"androidDevices": devices}
        },
    })
}

/// Whether a matrix has finished: its `state` is `FINISHED`, `ERROR` or `INVALID`.
pub fn finished(doc: &Value) -> bool {
    matches!(
        doc["state"].as_str(),
        Some("FINISHED") | Some("ERROR") | Some("INVALID")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_matrix_request_names_its_packages_and_its_devices() {
        let body = matrix_request(
            "gs://bucket/app.apk",
            "gs://bucket/tests.apk",
            &[("Pixel2".into(), "28".into())],
        );
        let spec = &body["testSpecification"]["androidInstrumentationTest"];
        assert_eq!(spec["appApk"]["gcsPath"], "gs://bucket/app.apk");
        assert_eq!(spec["testApk"]["gcsPath"], "gs://bucket/tests.apk");
        let device = &body["environmentMatrix"]["androidDeviceList"]["androidDevices"][0];
        assert_eq!(device["androidModelId"], "Pixel2");
        assert_eq!(device["androidVersionId"], "28");
        assert!(finished(&json!({"state": "FINISHED"})));
        assert!(!finished(&json!({"state": "RUNNING"})));
        assert_eq!(
            devices_of(&["Pixel2:28".into()]).unwrap(),
            vec![("Pixel2".to_string(), "28".to_string())]
        );
        assert!(devices_of(&["Pixel2".into()]).is_err());
    }
}
