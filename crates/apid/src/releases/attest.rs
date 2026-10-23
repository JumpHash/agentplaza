//! Provenance (PLAN 22.2, 9.3): an in-toto statement (SLSA v1) per artifact, sealed in a DSSE envelope with the key Weft
//! signs with (`weft::release::dsse`). Its subjects are the artifact and its SBOM, by SHA-256, so the signature covers
//! both: a changed SBOM or a changed artifact no longer matches what the statement says.

use serde_json::{Value, json};
use weft::keys::{KeyPair, PublicKey};

/// The SBOM's name as a subject of the statement.
pub const SBOM_SUBJECT: &str = "sbom.cdx.json";

/// What a build is attested to be: its artifact, its SBOM and the run and machine that made it.
pub struct Built<'a> {
    pub repo: &'a str,
    /// The release's target (`cargo:app@linux-x86_64`).
    pub target: &'a str,
    /// The revision the build was cut from, in Weft.
    pub from: &'a str,
    pub command: &'a str,
    pub builder: &'a str,
    pub pool: &'a str,
    pub base: &'a str,
    pub run: &'a str,
    pub artifact_sha256: &'a str,
    pub sbom_sha256: &'a str,
}

/// The in-toto statement of a build.
pub fn statement(b: &Built) -> Value {
    json!({
        "_type": "https://in-toto.io/Statement/v1",
        "subject": [
            { "name": b.target, "digest": { "sha256": b.artifact_sha256 } },
            { "name": SBOM_SUBJECT, "digest": { "sha256": b.sbom_sha256 } },
        ],
        "predicateType": "https://slsa.dev/provenance/v1",
        "predicate": {
            "buildDefinition": {
                "buildType": "https://agentplaza.dev/weft/build/v1",
                "externalParameters": { "command": b.command, "target": b.target, "repo": b.repo },
                "internalParameters": { "pool": b.pool, "base": b.base },
                "resolvedDependencies": [{ "uri": format!("weft:{}@{}", b.repo, b.from), "digest": { "weft": b.from } }],
            },
            "runDetails": { "builder": { "id": b.builder }, "metadata": { "invocationId": b.run } },
        },
    })
}

/// The statement sealed in a DSSE envelope with `key`, as the text that ships beside the artifact.
pub fn seal(statement: &Value, key: &KeyPair) -> String {
    serde_json::to_string_pretty(&weft::release::dsse(statement, key)).unwrap_or_default()
}

/// Opens a sealed provenance with the hub's public key: the statement, when its signature is the key's.
pub fn open(envelope: &str, key: &PublicKey) -> Result<Value, String> {
    let envelope: Value =
        serde_json::from_str(envelope).map_err(|e| format!("the provenance is not JSON: {e}"))?;
    weft::release::open_dsse(&envelope, key)
}

/// Checks an opened statement binds the artifact and the SBOM the release recorded: one subject is the artifact with
/// `artifact_sha256`, and the SBOM subject has `sbom_sha256`.
pub fn check_subjects(
    statement: &Value,
    artifact_sha256: &str,
    sbom_sha256: &str,
) -> Result<(), String> {
    if statement["predicateType"] != "https://slsa.dev/provenance/v1" {
        return Err("the provenance is not a SLSA v1 statement".into());
    }
    let subjects = statement["subject"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let sha = |s: &Value| {
        s["digest"]["sha256"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    if !subjects
        .iter()
        .any(|s| s["name"] == SBOM_SUBJECT && sha(s) == sbom_sha256)
    {
        return Err("the provenance was signed for another SBOM".into());
    }
    if !subjects
        .iter()
        .any(|s| s["name"] != SBOM_SUBJECT && sha(s) == artifact_sha256)
    {
        return Err("the provenance was signed for another artifact".into());
    }
    Ok(())
}
