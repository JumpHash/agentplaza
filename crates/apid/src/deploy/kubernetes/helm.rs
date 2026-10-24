//! A Helm chart's deploy (PLAN 23, 11.2). A project that declares a chart (`KUBERNETES_HELM_CHART`) is installed with
//! `helm upgrade --install`, the release's image set through the chart's `image` value, and its status read with
//! `helm status -o json`. A failed stage rolls back to the previous revision with `helm rollback`.
//!
//! helm is detected on the hub's PATH and never installed: without it the stage fails with "needs helm". helm reads the
//! cluster from a kubeconfig the hub writes for each call to a private file (server, CA and token), removed when that call
//! ends. The token is in that file and in no argument, log or error.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

use super::{Client, Tools, private_name};

/// A helm to run: its binary, the chart, and the release's name in the namespace.
#[derive(Debug, Clone, PartialEq)]
pub struct Helm {
    bin: PathBuf,
    pub chart: String,
    pub release: String,
}

/// A helm release as `helm status` reports it: its revision and its status (`deployed`, `failed`, …).
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub revision: u64,
    pub status: String,
}

/// The helm for `chart`, when helm is on the hub's PATH.
pub fn locate(tools: &Tools, chart: &str, release: &str) -> Result<Helm, String> {
    let bin = tools.find("helm").ok_or_else(|| {
        format!("needs helm: {chart} is a Helm chart, and helm is not on the hub's PATH")
    })?;
    Ok(Helm {
        bin,
        chart: chart.to_string(),
        release: release.to_string(),
    })
}

/// The release's status in `namespace`, or None when helm has no such release.
pub fn status(helm: &Helm, client: &Client, namespace: &str) -> Result<Option<Status>, String> {
    match run(
        helm,
        client,
        &[
            "status",
            &helm.release,
            "--namespace",
            namespace,
            "-o",
            "json",
        ],
    ) {
        Ok(text) => {
            let doc: Value = serde_json::from_str(&text)
                .map_err(|e| format!("helm status answered no JSON: {e}"))?;
            let revision = doc["version"]
                .as_u64()
                .ok_or("helm status names no revision")?;
            Ok(Some(Status {
                revision,
                status: doc["info"]["status"].as_str().unwrap_or("").to_string(),
            }))
        }
        Err(e) if e.contains("not found") => Ok(None),
        Err(e) => Err(e),
    }
}

/// `helm upgrade --install`: the chart as the release, with the release's image as its `image` value, waiting for the
/// rollout up to `timeout`.
pub fn upgrade(
    helm: &Helm,
    client: &Client,
    namespace: &str,
    image: &str,
    timeout: std::time::Duration,
) -> Result<(), String> {
    let set = format!("image={image}");
    let timeout = format!("{}s", timeout.as_secs());
    run(
        helm,
        client,
        &[
            "upgrade",
            "--install",
            &helm.release,
            &helm.chart,
            "--namespace",
            namespace,
            "--set",
            &set,
            "--wait",
            "--timeout",
            &timeout,
        ],
    )
    .map(|_| ())
}

/// `helm rollback <release> <revision>`: the release goes back to `revision`, waiting up to `timeout`.
pub fn rollback(
    helm: &Helm,
    client: &Client,
    namespace: &str,
    revision: u64,
    timeout: std::time::Duration,
) -> Result<(), String> {
    let revision = revision.to_string();
    let timeout = format!("{}s", timeout.as_secs());
    run(
        helm,
        client,
        &[
            "rollback",
            &helm.release,
            &revision,
            "--namespace",
            namespace,
            "--wait",
            "--timeout",
            &timeout,
        ],
    )
    .map(|_| ())
}

/// One helm call: a kubeconfig is written for it, helm runs with `KUBECONFIG` pointing at it, and the file is removed when
/// the call has ended. Its output is returned; a failure is helm's own words, without the token.
fn run(helm: &Helm, client: &Client, args: &[&str]) -> Result<String, String> {
    let kubeconfig = Kubeconfig::write(client)?;
    let output = Command::new(&helm.bin)
        .args(args)
        .env("KUBECONFIG", &kubeconfig.path)
        .output();
    drop(kubeconfig);
    let output = output.map_err(|e| format!("helm did not start: {e}"))?;
    if !output.status.success() {
        let why = String::from_utf8_lossy(&output.stderr);
        return Err(client.redact(&format!("helm {} failed: {}", args[0], why.trim())));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The kubeconfig a helm call reads: a private file (0600 on Unix), removed when it is dropped.
struct Kubeconfig {
    path: PathBuf,
}

impl Kubeconfig {
    fn write(client: &Client) -> Result<Kubeconfig, String> {
        let path = std::env::temp_dir().join(format!("{}.json", private_name("kubeconfig")));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&path)
            .map_err(|e| format!("the hub could not write helm's kubeconfig: {e}"))?;
        // The guard exists before the write, so a failed write removes the file too.
        let kubeconfig = Kubeconfig { path };
        let body = serde_json::to_vec(&client.kubeconfig()).map_err(|e| e.to_string())?;
        file.write_all(&body)
            .map_err(|e| format!("the hub could not write helm's kubeconfig: {e}"))?;
        Ok(kubeconfig)
    }
}

impl Drop for Kubeconfig {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
