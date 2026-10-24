//! Google Play (9.6): the Android Publisher API. A service account's access token comes from the token endpoint (the JWT
//! bearer grant: an RS256 JWT signed with the account's key is exchanged for it). The bundle goes into an edit: the edit is
//! created, the bundle uploaded (its version code comes back), the track set to that version code (completed, or in progress
//! at a staged rollout's fraction) and the edit committed. The tracks are internal, closed (`alpha`), open (`beta`) and
//! production.

use serde::Deserialize;
use serde_json::{Value, json};

use super::jwt;
use super::{Api, Options, Run, Step, Track, answered, credential, manifest_of, secrets};

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const SCOPE: &str = "https://www.googleapis.com/auth/androidpublisher";
const API: &str = "https://androidpublisher.googleapis.com/androidpublisher/v3";
const UPLOAD: &str = "https://androidpublisher.googleapis.com/upload/androidpublisher/v3";

/// The parts of a service account's key file that the token request needs.
#[derive(Deserialize)]
struct ServiceAccount {
    client_email: String,
    private_key: String,
}

/// A session: the API with the access token, and the app's package name.
struct Play<'a> {
    api: Api<'a>,
    package: String,
}

/// Submits the Android bundle of a release to a Google Play track: an edit holding the bundle on the track, committed.
pub(super) fn submit(
    run: &Run,
    track: Track,
    options: &Options,
    step: &Step,
    bytes: &[u8],
) -> Result<(), String> {
    let play = run.fresh("auth", || {
        let values = secrets(
            step,
            track.connector(),
            &["GOOGLE_PLAY_SERVICE_ACCOUNT", "GOOGLE_PLAY_PACKAGE_NAME"],
        )?;
        let account: ServiceAccount = serde_json::from_str(&values[0]).map_err(|e| {
            format!("GOOGLE_PLAY_SERVICE_ACCOUNT is not a service account key: {e}")
        })?;
        let now = step.at_ms / 1000;
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
        let approved = manifest_of(track.connector())?;
        let exchange = Api {
            outbound: step.outbound,
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
        let token = serde_json::from_str::<Value>(&reply.body)
            .ok()
            .and_then(|v| v["access_token"].as_str().map(str::to_string))
            .ok_or("the token endpoint gave no access token")?;
        Ok(Play {
            api: Api {
                outbound: step.outbound,
                egress: approved.egress.clone(),
                secret: Some(credential(&approved, &token)),
            },
            package: values[1].clone(),
        })
    })?;
    let app = format!("{API}/applications/{}", play.package);
    let edit = run.run("edit", || {
        let created = play.api.json("POST", &format!("{app}/edits"), None)?;
        created["id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "the edit names no id".to_string())
    })?;
    let edit_url = format!("{app}/edits/{edit}");
    let version_code = run.run("upload", || {
        let url = format!(
            "{UPLOAD}/applications/{}/edits/{edit}/bundles?uploadType=media",
            play.package
        );
        let reply = play.api.send(
            "POST",
            &url,
            &[("Content-Type", "application/octet-stream")],
            bytes,
        )?;
        answered("POST", &url, &reply)?;
        let answer: Value = serde_json::from_str(&reply.body)
            .map_err(|e| format!("the upload answered text that is not JSON: {e}"))?;
        answer["versionCode"]
            .as_i64()
            .map(|code| code.to_string())
            .ok_or_else(|| "the upload gave no version code".to_string())
    })?;
    run.run("track", || {
        let code: i64 = version_code
            .parse()
            .map_err(|_| "the recorded version code is not a number".to_string())?;
        let name = track.play_name();
        let release = match options.rollout {
            Some(fraction) => {
                json!({ "versionCodes": [code], "status": "inProgress", "userFraction": fraction })
            }
            None => json!({ "versionCodes": [code], "status": "completed" }),
        };
        play.api.json(
            "PUT",
            &format!("{edit_url}/tracks/{name}"),
            Some(&json!({ "track": name, "releases": [release] })),
        )?;
        Ok(name.to_string())
    })?;
    run.run("commit", || {
        play.api.json("POST", &format!("{edit_url}:commit"), None)?;
        Ok(edit.clone())
    })?;
    Ok(())
}
