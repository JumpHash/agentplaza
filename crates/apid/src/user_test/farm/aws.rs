//! AWS Device Farm (PLAN 19.3, 8.5): an instrumentation run, each call signed with AWS SigV4.
//!
//! The credential is an access key pair (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`) and the project's ARN
//! (`AWS_DEVICEFARM_PROJECT_ARN`). Each call is a JSON `POST` to `https://devicefarm.us-west-2.amazonaws.com/` with
//! `Content-Type: application/x-amz-json-1.1` and an `X-Amz-Target` of `DeviceFarm_20150623.<Operation>`, signed by
//! apz-gateway's SigV4 signer (`signed_headers`): the signing is per request, so the manifest names no static header. The
//! flow: `CreateUpload` for the app and for the test package, the bytes `PUT` to each upload's presigned address, the upload
//! polled until it succeeded, `ScheduleRun` on the device pool named first in the run's devices, the run polled with `GetRun`,
//! then `ListJobs` (one session per job) and `ListArtifacts` (the run's videos, downloaded and attributed to the first session).
//!
//! ASSUMED from the Device Farm API reference (not checked against the live API): the upload's `arn`, `url` and `status`
//! (`SUCCEEDED` when it is ready), the run's `arn`, `status` (`COMPLETED` or `STOPPED` when it has ended) and `result`
//! (`PASSED` for a pass), the job's `arn`, `name`, `result` and `device.name`, and the artifact's `url` and `name`. The
//! presigned upload and result addresses are on S3 hosts named in the manifest's egress (assumed names). Any of these that
//! differs from the live API is a change to this file only.

use std::time::SystemTime;

use serde_json::Value;

use super::{Api, Artifact, Deps, Pace, Report, Session, answered, collect, manifest_of, secrets};
use crate::connectors::manifest::Manifest;
use crate::recipes::model::FarmTest;
use apz_gateway::signed_headers;

pub const ENDPOINT: &str = "https://devicefarm.us-west-2.amazonaws.com/";
pub const CONTENT_TYPE: &str = "application/x-amz-json-1.1";
const CONNECTOR: &str = "aws-device-farm";
/// The region and the service name SigV4 signs for.
const REGION: &str = "us-west-2";
const SERVICE: &str = "devicefarm";

/// One call: its `X-Amz-Target` and its JSON body.
pub struct Call {
    pub target: String,
    pub body: Value,
}

/// The project and the key pair the calls are signed with.
struct Keys {
    access: String,
    secret: String,
    project: String,
}

/// Runs the suite on Device Farm: the packages up, the run scheduled and read, its jobs as sessions and its videos kept.
pub fn run(deps: &Deps, config: &FarmTest, app: &[u8], tests: &[u8]) -> Result<Report, String> {
    let values = secrets(
        deps,
        CONNECTOR,
        &[
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_DEVICEFARM_PROJECT_ARN",
        ],
    )?;
    let keys = Keys {
        access: values[0].clone(),
        secret: values[1].clone(),
        project: values[2].clone(),
    };
    let approved: Manifest = manifest_of(CONNECTOR)?;
    let api = Api {
        outbound: deps.outbound,
        egress: approved.egress.clone(),
        secret: None,
    };
    let app_arn = upload(&api, &keys, deps.pace, "ANDROID_APP", "app.apk", app)?;
    let test_arn = upload(
        &api,
        &keys,
        deps.pace,
        "INSTRUMENTATION_TEST_PACKAGE",
        "tests.apk",
        tests,
    )?;
    let scheduled = call(
        &api,
        &keys,
        &schedule_run(&keys.project, &app_arn, &config.devices[0], &test_arn),
    )?;
    let run_arn = text(&scheduled["run"], "arn")?;
    for poll in 0..deps.pace.polls {
        let doc = call(&api, &keys, &get_run(&run_arn))?;
        if finished(&doc["run"]) {
            let jobs = call(&api, &keys, &list_jobs(&run_arn))?;
            let mut sessions: Vec<Session> = jobs["jobs"]
                .as_array()
                .map(|list| list.iter().map(session).collect())
                .unwrap_or_default();
            let videos = call(&api, &keys, &list_artifacts(&run_arn, "VIDEO"))?;
            let artifacts: Vec<Artifact> = videos["artifacts"]
                .as_array()
                .map(|list| {
                    list.iter()
                        .filter_map(|a| {
                            Some(Artifact {
                                name: a["name"].as_str().unwrap_or("video.mp4").to_string(),
                                url: a["url"].as_str()?.to_string(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            if let Some(first) = sessions.first_mut() {
                first.artifacts = artifacts;
            }
            return Ok(collect(&api, Report::of(run_arn, sessions)));
        }
        deps.pace.wait(poll);
    }
    Err(format!(
        "run {run_arn} is not finished after {} checks",
        deps.pace.polls
    ))
}

/// Uploads one file: `CreateUpload`, the bytes to the presigned address, and the upload polled until it succeeded. Answers the
/// upload's ARN.
fn upload(
    api: &Api,
    keys: &Keys,
    pace: Pace,
    kind: &str,
    name: &str,
    bytes: &[u8],
) -> Result<String, String> {
    let created = call(api, keys, &create_upload(&keys.project, name, kind))?;
    let arn = text(&created["upload"], "arn")?;
    let url = text(&created["upload"], "url")?;
    let reply = api.send(
        "PUT",
        &url,
        &[("Content-Type", "application/octet-stream")],
        bytes,
    )?;
    answered("PUT", &url, &reply)?;
    for poll in 0..pace.polls {
        let doc = call(api, keys, &get_upload(&arn))?;
        match doc["upload"]["status"].as_str() {
            Some("SUCCEEDED") => return Ok(arn),
            Some("FAILED") => return Err(format!("the upload {name} failed on the farm")),
            _ => pace.wait(poll),
        }
    }
    Err(format!(
        "the upload {name} is not ready after {} checks",
        pace.polls
    ))
}

/// One signed call: its body, the SigV4 headers, and the answer as JSON.
fn call(api: &Api, keys: &Keys, call: &Call) -> Result<Value, String> {
    let body = call.body.to_string().into_bytes();
    let own = vec![
        ("Content-Type".to_string(), CONTENT_TYPE.to_string()),
        ("X-Amz-Target".to_string(), call.target.clone()),
    ];
    let signed = signed_headers(
        "POST",
        ENDPOINT,
        &own,
        &body,
        &keys.access,
        &keys.secret,
        None,
        REGION,
        SERVICE,
        SystemTime::now(),
    );
    let all: Vec<(&str, &str)> = own
        .iter()
        .chain(signed.iter())
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    let reply = api.send("POST", ENDPOINT, &all, &body)?;
    answered("POST", ENDPOINT, &reply)?;
    serde_json::from_str(&reply.body)
        .map_err(|e| format!("{} answered text that is not JSON: {e}", call.target))
}

/// A string field of an answer, or why it is missing.
fn text(doc: &Value, field: &str) -> Result<String, String> {
    doc[field]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("the answer names no {field}"))
}

/// A session from a job: its ARN, the device's name (or the job's), and whether it passed.
fn session(job: &Value) -> Session {
    Session {
        id: job["arn"].as_str().unwrap_or_default().to_string(),
        device: job["device"]["name"]
            .as_str()
            .or_else(|| job["name"].as_str())
            .unwrap_or_default()
            .to_string(),
        passed: job["result"].as_str() == Some("PASSED"),
        links: Vec::new(),
        tests: Vec::new(),
        artifacts: Vec::new(),
    }
}

/// Whether a run has finished (its `status` is `COMPLETED` or `STOPPED`).
pub fn finished(run: &Value) -> bool {
    matches!(run["status"].as_str(), Some("COMPLETED") | Some("STOPPED"))
}

/// The `CreateUpload` call for a file of `kind` (`ANDROID_APP`, `INSTRUMENTATION_TEST_PACKAGE`, ...).
pub fn create_upload(project_arn: &str, name: &str, kind: &str) -> Call {
    Call {
        target: "DeviceFarm_20150623.CreateUpload".into(),
        body: serde_json::json!({
            "projectArn": project_arn,
            "name": name,
            "type": kind,
            "contentType": "application/octet-stream",
        }),
    }
}

/// The `GetUpload` call for an upload's ARN.
pub fn get_upload(arn: &str) -> Call {
    Call {
        target: "DeviceFarm_20150623.GetUpload".into(),
        body: serde_json::json!({"arn": arn}),
    }
}

/// The `ScheduleRun` call: the app, the device pool and the test package, with the test's type (`INSTRUMENTATION`).
pub fn schedule_run(
    project_arn: &str,
    app_arn: &str,
    pool_arn: &str,
    test_package_arn: &str,
) -> Call {
    Call {
        target: "DeviceFarm_20150623.ScheduleRun".into(),
        body: serde_json::json!({
            "projectArn": project_arn,
            "appArn": app_arn,
            "devicePoolArn": pool_arn,
            "test": {"type": "INSTRUMENTATION", "testPackageArn": test_package_arn},
        }),
    }
}

/// The `GetRun` call for a run's ARN.
pub fn get_run(arn: &str) -> Call {
    Call {
        target: "DeviceFarm_20150623.GetRun".into(),
        body: serde_json::json!({"arn": arn}),
    }
}

/// The `ListJobs` call for a run's ARN: one job per device.
pub fn list_jobs(arn: &str) -> Call {
    Call {
        target: "DeviceFarm_20150623.ListJobs".into(),
        body: serde_json::json!({"arn": arn}),
    }
}

/// The `ListArtifacts` call for a run's ARN and an artifact type (`VIDEO`, `LOG`, ...).
pub fn list_artifacts(arn: &str, kind: &str) -> Call {
    Call {
        target: "DeviceFarm_20150623.ListArtifacts".into(),
        body: serde_json::json!({"arn": arn, "type": kind}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn each_call_names_its_operation_and_carries_its_fields() {
        let upload = create_upload("arn:p", "app.apk", "ANDROID_APP");
        assert_eq!(upload.target, "DeviceFarm_20150623.CreateUpload");
        assert_eq!(upload.body["type"], "ANDROID_APP");
        let run = schedule_run("arn:p", "arn:a", "arn:pool", "arn:t");
        assert_eq!(run.target, "DeviceFarm_20150623.ScheduleRun");
        assert_eq!(run.body["test"]["type"], "INSTRUMENTATION");
        assert_eq!(run.body["devicePoolArn"], "arn:pool");
        assert_eq!(get_run("arn:r").target, "DeviceFarm_20150623.GetRun");
        assert_eq!(list_jobs("arn:r").target, "DeviceFarm_20150623.ListJobs");
        assert_eq!(list_artifacts("arn:r", "VIDEO").body["type"], "VIDEO");
        assert_eq!(get_upload("arn:u").target, "DeviceFarm_20150623.GetUpload");
        assert!(finished(&json!({"status": "COMPLETED"})));
        assert!(!finished(&json!({"status": "RUNNING"})));
        assert_eq!(ENDPOINT, "https://devicefarm.us-west-2.amazonaws.com/");
        assert_eq!(CONTENT_TYPE, "application/x-amz-json-1.1");
    }
}
