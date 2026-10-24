//! App Store Connect (9.6): TestFlight's builds and beta groups, and the App Store's version, phased release and review
//! submission. The API is `https://api.appstoreconnect.apple.com/v1`, called with a JWT (ES256, `aud` appstoreconnect-v1)
//! that the team's key signs: the key id and the issuer id go in it, and the app is `APP_STORE_CONNECT_APP_ID`. A build is
//! uploaded as a build upload (Apple names the parts of the file, each is sent to its URL, then the file is marked
//! uploaded), it is located by its version and build number, and it is waited on until it is processed.

use serde_json::{Value, json};

use super::jwt;
use super::{
    Api, Options, Pace, Run, Step, Track, answered, credential, data_id, https_host, manifest_of,
    secrets,
};
use crate::glue::Outbound;
use crate::releases::entity::Release;

const BASE: &str = "https://api.appstoreconnect.apple.com/v1";
/// The domains an upload operation's address may be on: Apple's, and the storage Apple's uploads are kept in.
const UPLOAD_DOMAINS: [&str; 2] = [".apple.com", ".amazonaws.com"];

/// The session of one run: the API with its credential, the app, and the build's version and number.
struct Asc<'a> {
    api: Api<'a>,
    app: String,
    version: String,
    build: String,
}

impl Asc<'_> {
    /// The builds of this version and build number, as Apple lists them (still processing, or processed).
    fn builds(&self) -> Result<Vec<Value>, String> {
        let url = format!(
            "{BASE}/builds?filter%5Bapp%5D={}&filter%5BpreReleaseVersion.version%5D={}&filter%5Bversion%5D={}&filter%5Bplatform%5D=IOS&limit=5",
            self.app, self.version, self.build
        );
        let list = self.api.json("GET", &url, None)?;
        Ok(list["data"].as_array().cloned().unwrap_or_default())
    }
}

/// Submits the iOS build of a release to an App Store Connect track: TestFlight's internal or external group, or the App
/// Store (its version with the build, a phased release if configured, and the review submission).
pub(super) fn submit(
    run: &Run,
    rel: &Release,
    track: Track,
    options: &Options,
    step: &Step,
    bytes: &[u8],
) -> Result<(), String> {
    let asc = run.fresh("auth", || {
        let values = secrets(
            step,
            track.connector(),
            &[
                "APP_STORE_CONNECT_KEY_ID",
                "APP_STORE_CONNECT_ISSUER_ID",
                "APP_STORE_CONNECT_PRIVATE_KEY",
                "APP_STORE_CONNECT_APP_ID",
            ],
        )?;
        let now = step.at_ms / 1000;
        let token = jwt::es256(
            &values[2],
            &values[0],
            &json!({ "iss": values[1], "iat": now, "exp": now + 1200, "aud": "appstoreconnect-v1" }),
        )?;
        let approved = manifest_of(track.connector())?;
        Ok(Asc {
            api: Api {
                outbound: step.outbound,
                egress: approved.egress.clone(),
                secret: Some(credential(&approved, &token)),
            },
            app: values[3].clone(),
            version: rel.version.clone(),
            build: rel.number.unwrap_or(1).to_string(),
        })
    })?;
    run.run("upload", || upload(&asc, bytes))?;
    let build = run.run("processing", || processing(&asc, step.pace))?;
    match track {
        Track::TestFlightInternal => {
            run.run("groups", || groups(&asc, true, &build))?;
        }
        Track::TestFlightExternal => {
            run.run("groups", || groups(&asc, false, &build))?;
            run.run("beta-review", || beta_review(&asc, &build))?;
        }
        _ => {
            let version = run.run("version", || app_store_version(&asc, &build))?;
            if options.phased {
                run.run("phased-release", || phased_release(&asc, &version))?;
            }
            run.run("review", || review(&asc, &version))?;
        }
    }
    Ok(())
}

/// The build's upload, unless Apple already has the build: the file's parts are sent to the addresses Apple names, and
/// the file is marked uploaded with the MD5 of what was sent.
fn upload(asc: &Asc, bytes: &[u8]) -> Result<String, String> {
    if !asc.builds()?.is_empty() {
        return Ok("located".into());
    }
    let created = asc.api.json(
        "POST",
        &format!("{BASE}/buildUploads"),
        Some(&json!({
            "data": {
                "type": "buildUploads",
                "attributes": {
                    "cfBundleShortVersionString": asc.version,
                    "cfBundleVersion": asc.build,
                    "platform": "IOS"
                },
                "relationships": { "app": { "data": { "type": "apps", "id": asc.app } } }
            }
        })),
    )?;
    let upload = data_id(&created)?;
    let file = asc.api.json(
        "POST",
        &format!("{BASE}/buildUploadFiles"),
        Some(&json!({
            "data": {
                "type": "buildUploadFiles",
                "attributes": {
                    "fileName": "app.ipa",
                    "fileSize": bytes.len(),
                    "assetType": "ASSET",
                    "uti": "com.apple.ipa"
                },
                "relationships": { "buildUpload": { "data": { "type": "buildUploads", "id": upload } } }
            }
        })),
    )?;
    let file_id = data_id(&file)?;
    let operations = file["data"]["attributes"]["uploadOperations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if operations.is_empty() {
        return Err("Apple named no upload operation for the build".into());
    }
    for operation in &operations {
        send_part(asc.api.outbound, operation, bytes)?;
    }
    let md5 = {
        use md5::Digest as _;
        weft::hash::hex(&md5::Md5::digest(bytes))
    };
    asc.api.json(
        "PATCH",
        &format!("{BASE}/buildUploadFiles/{file_id}"),
        Some(&json!({
            "data": {
                "type": "buildUploadFiles",
                "id": file_id,
                "attributes": {
                    "uploaded": true,
                    "sourceFileChecksums": { "file": { "hash": md5, "algorithm": "MD5" } }
                }
            }
        })),
    )?;
    Ok("uploaded".into())
}

/// One part of an upload: the bytes its operation names, sent to its address with the headers it names. The address must be
/// on Apple's domains or their storage, and it carries no credential (the address is its own authority).
pub(super) fn send_part(
    outbound: &dyn Outbound,
    operation: &Value,
    bytes: &[u8],
) -> Result<(), String> {
    let url = operation["url"]
        .as_str()
        .ok_or("an upload operation names no address")?;
    let host = https_host(url)?;
    if !UPLOAD_DOMAINS.iter().any(|domain| host.ends_with(domain)) {
        return Err(format!(
            "an upload operation names {host}, which is not Apple's"
        ));
    }
    let offset = operation["offset"].as_u64().unwrap_or(0) as usize;
    let length = operation["length"]
        .as_u64()
        .map_or(bytes.len(), |n| n as usize);
    let part = bytes
        .get(offset..offset.saturating_add(length))
        .ok_or("an upload operation is past the end of the build")?;
    let method = operation["method"].as_str().unwrap_or("PUT");
    let headers: Vec<(String, String)> = operation["requestHeaders"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|h| {
            Some((
                h["name"].as_str()?.to_string(),
                h["value"].as_str()?.to_string(),
            ))
        })
        .collect();
    let refs: Vec<(&str, &str)> = headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let api = Api {
        outbound,
        egress: vec![host],
        secret: None,
    };
    let reply = api.send(method, url, &refs, part)?;
    answered(method, url, &reply)
}

/// Waits until Apple has processed the build; the build's id is the step's reference.
fn processing(asc: &Asc, pace: Pace) -> Result<String, String> {
    let checks = pace.polls.max(1);
    for attempt in 0..checks {
        if attempt > 0 {
            std::thread::sleep(pace.interval);
        }
        if let Some(build) = asc.builds()?.first() {
            let id = super::item_id(build)?;
            match build["attributes"]["processingState"].as_str() {
                Some("VALID") => return Ok(id),
                Some(state @ ("FAILED" | "INVALID")) => {
                    return Err(format!(
                        "Apple did not accept build {id}: its processing is {state}"
                    ));
                }
                _ => {}
            }
        }
    }
    Err(format!(
        "Apple is still processing the build after {checks} checks: a retry resumes the wait"
    ))
}

/// Adds the build to the app's beta groups of the kind (internal, or external); the groups' ids are the step's reference.
fn groups(asc: &Asc, internal: bool, build: &str) -> Result<String, String> {
    let list = asc.api.json(
        "GET",
        &format!("{BASE}/betaGroups?filter%5Bapp%5D={}&limit=200", asc.app),
        None,
    )?;
    let mut ids = Vec::new();
    for group in list["data"].as_array().into_iter().flatten() {
        if group["attributes"]["isInternalGroup"].as_bool() == Some(internal) {
            ids.push(super::item_id(group)?);
        }
    }
    if ids.is_empty() {
        let kind = if internal { "internal" } else { "external" };
        return Err(format!("the app has no {kind} beta group"));
    }
    for id in &ids {
        asc.api.json(
            "POST",
            &format!("{BASE}/betaGroups/{id}/relationships/builds"),
            Some(&json!({ "data": [{ "type": "builds", "id": build }] })),
        )?;
    }
    Ok(ids.join(","))
}

/// The external group's beta review of the build; its submission's id is the step's reference.
fn beta_review(asc: &Asc, build: &str) -> Result<String, String> {
    let created = asc.api.json(
        "POST",
        &format!("{BASE}/betaAppReviewSubmissions"),
        Some(&json!({
            "data": {
                "type": "betaAppReviewSubmissions",
                "relationships": { "build": { "data": { "type": "builds", "id": build } } }
            }
        })),
    )?;
    data_id(&created)
}

/// The App Store version of the build's version string, created when it is not there, with the build attached; its id is
/// the step's reference.
fn app_store_version(asc: &Asc, build: &str) -> Result<String, String> {
    let existing = asc.api.json(
        "GET",
        &format!(
            "{BASE}/apps/{}/appStoreVersions?filter%5BversionString%5D={}&filter%5Bplatform%5D=IOS&limit=1",
            asc.app, asc.version
        ),
        None,
    )?;
    let version = match existing["data"].as_array().and_then(|list| list.first()) {
        Some(found) => super::item_id(found)?,
        None => {
            let created = asc.api.json(
                "POST",
                &format!("{BASE}/appStoreVersions"),
                Some(&json!({
                    "data": {
                        "type": "appStoreVersions",
                        "attributes": {
                            "platform": "IOS",
                            "versionString": asc.version,
                            "releaseType": "AFTER_APPROVAL"
                        },
                        "relationships": { "app": { "data": { "type": "apps", "id": asc.app } } }
                    }
                })),
            )?;
            data_id(&created)?
        }
    };
    asc.api.json(
        "PATCH",
        &format!("{BASE}/appStoreVersions/{version}/relationships/build"),
        Some(&json!({ "data": { "type": "builds", "id": build } })),
    )?;
    Ok(version)
}

/// The phased release of the App Store version: Apple's seven-day rollout, started when the version is released.
fn phased_release(asc: &Asc, version: &str) -> Result<String, String> {
    let created = asc.api.json(
        "POST",
        &format!("{BASE}/appStoreVersionPhasedReleases"),
        Some(&json!({
            "data": {
                "type": "appStoreVersionPhasedReleases",
                "attributes": { "phasedReleaseState": "ACTIVE" },
                "relationships": {
                    "appStoreVersion": { "data": { "type": "appStoreVersions", "id": version } }
                }
            }
        })),
    )?;
    data_id(&created)
}

/// The review submission of the App Store version, submitted for review; its id is the step's reference.
fn review(asc: &Asc, version: &str) -> Result<String, String> {
    let created = asc.api.json(
        "POST",
        &format!("{BASE}/reviewSubmissions"),
        Some(&json!({
            "data": {
                "type": "reviewSubmissions",
                "attributes": { "platform": "IOS" },
                "relationships": { "app": { "data": { "type": "apps", "id": asc.app } } }
            }
        })),
    )?;
    let submission = data_id(&created)?;
    asc.api.json(
        "POST",
        &format!("{BASE}/reviewSubmissionItems"),
        Some(&json!({
            "data": {
                "type": "reviewSubmissionItems",
                "relationships": {
                    "reviewSubmission": { "data": { "type": "reviewSubmissions", "id": submission } },
                    "appStoreVersion": { "data": { "type": "appStoreVersions", "id": version } }
                }
            }
        })),
    )?;
    asc.api.json(
        "PATCH",
        &format!("{BASE}/reviewSubmissions/{submission}"),
        Some(&json!({
            "data": {
                "type": "reviewSubmissions",
                "id": submission,
                "attributes": { "submitted": true }
            }
        })),
    )?;
    Ok(submission)
}
