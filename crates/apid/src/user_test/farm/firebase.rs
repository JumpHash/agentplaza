//! Firebase Test Lab (PLAN 19.3, 8.5): the request shapes of a test matrix, built and tested here; the run is not wired (see
//! `farm::run`). ASSUMED from Google's public Firebase Test Lab (Testing API v1) reference: a matrix is created with
//! `POST https://testing.googleapis.com/v1/projects/{project}/testMatrices` whose body holds the `testSpecification` (the app
//! and test packages as `gs://` locations, in `androidInstrumentationTest`) and the `environmentMatrix` (`androidDeviceList`
//! of `androidModelId`, `androidVersionId`, `locale` and `orientation`), and answers the matrix with its `testMatrixId`,
//! `state` and `outcome`. A matrix is read with `GET` on the same path and id. The packages reach Cloud Storage first (not
//! built here). The credential is a service account's token (the store's JWT exchange, not built here).

use serde_json::{Value, json};

pub const API: &str = "https://testing.googleapis.com/v1";

/// The create request of a test matrix for `project`: the packages' locations and the devices by model and API level.
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

/// The address of a matrix's create call, and of a matrix read by its id.
pub fn matrix_url(project: &str) -> String {
    format!("{API}/projects/{project}/testMatrices")
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
        assert_eq!(
            matrix_url("acme"),
            "https://testing.googleapis.com/v1/projects/acme/testMatrices"
        );
        assert!(finished(&json!({"state": "FINISHED"})));
        assert!(!finished(&json!({"state": "RUNNING"})));
    }
}
