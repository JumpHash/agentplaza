//! AWS Device Farm (PLAN 19.3, 8.5): the request shapes of an upload, a run and its read, built and tested here; the run is not
//! wired (see `farm::run`). ASSUMED from the Device Farm API reference: each call is a JSON `POST` to
//! `https://devicefarm.us-west-2.amazonaws.com/` with `Content-Type: application/x-amz-json-1.1` and an `X-Amz-Target` of
//! `DeviceFarm_20150623.<Operation>`: `CreateUpload` ({projectArn, name, type, contentType}, answering the upload's `arn` and a
//! presigned `url` to PUT the file to), `ScheduleRun` ({projectArn, appArn, devicePoolArn, test: {type, testPackageArn}},
//! answering the run's `arn`), and `GetRun` ({arn}, answering a run with its `status` and `result`). Each request is signed
//! (AWS SigV4) per request, which a single static header cannot carry: the signing is not built, and the run is not wired.

use serde_json::{Value, json};

pub const ENDPOINT: &str = "https://devicefarm.us-west-2.amazonaws.com/";
pub const CONTENT_TYPE: &str = "application/x-amz-json-1.1";

/// One call: its `X-Amz-Target` and its JSON body.
pub struct Call {
    pub target: String,
    pub body: Value,
}

/// The `CreateUpload` call for a file of `kind` (`ANDROID_APP`, `INSTRUMENTATION_TEST_PACKAGE`, ...).
pub fn create_upload(project_arn: &str, name: &str, kind: &str) -> Call {
    Call {
        target: "DeviceFarm_20150623.CreateUpload".into(),
        body: json!({
            "projectArn": project_arn,
            "name": name,
            "type": kind,
            "contentType": "application/octet-stream",
        }),
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
        body: json!({
            "projectArn": project_arn,
            "appArn": app_arn,
            "devicePoolArn": pool_arn,
            "test": {"type": "INSTRUMENTATION", "testPackageArn": test_package_arn},
        }),
    }
}

/// The `GetRun` call for a run's `arn`.
pub fn get_run(arn: &str) -> Call {
    Call {
        target: "DeviceFarm_20150623.GetRun".into(),
        body: json!({"arn": arn}),
    }
}

/// Whether a run has finished (its `status` is `COMPLETED` or `STOPPED`).
pub fn finished(run: &Value) -> bool {
    matches!(run["status"].as_str(), Some("COMPLETED") | Some("STOPPED"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(finished(&json!({"status": "COMPLETED"})));
        assert!(!finished(&json!({"status": "RUNNING"})));
        assert_eq!(ENDPOINT, "https://devicefarm.us-west-2.amazonaws.com/");
        assert_eq!(CONTENT_TYPE, "application/x-amz-json-1.1");
    }
}
