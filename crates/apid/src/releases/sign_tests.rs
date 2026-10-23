//! Signing, SBOMs and provenance of a candidate's artifacts (PLAN 22.2, 9.3): every artifact of a candidate verifies (its
//! hub signature, its provenance, its SBOM, and the platform's signature by its format), publish verifies them first,
//! and each of a tampered artifact, SBOM or provenance fails. The keys are generated in the tests (`tests::TestSigning`).
//! A Mach-O is signed ad hoc with the real `codesign` on macOS and verified with `codesign -v`.

use base64::Engine;
use serde_json::{Value, json};
use weft::keys::KeyPair;

use super::entity::{ReleaseArtifact, ReleaseState};
use super::flow::{self, Builder, Built, Ledger};
use super::signing::{self, Format};
use super::tests::{World, cut, lab};
use crate::recipes::runs::RunState;

/// A builder whose every target is the same bytes: the format decides the platform signature.
struct BytesBuilder(Vec<u8>);

impl Builder for BytesBuilder {
    fn build(&self, _release: &super::entity::Release, _target: &str) -> Result<Built, String> {
        Ok(Built {
            bytes: self.0.clone(),
            pool: "fake-pool".into(),
            base: "fake-base".into(),
            builder: "fake-builder".into(),
            command: "cargo build --release".into(),
        })
    }
}

fn bytes_of(world: &World, art: &ReleaseArtifact) -> Vec<u8> {
    let hash: weft::hash::Hash = art.digest.parse().unwrap();
    world.ledger.blob(&hash).unwrap()
}

/// Cuts a candidate of the lab's release from `bytes` and returns its id and the world it lives in.
fn candidate_of(world: &World, bytes: Vec<u8>) -> String {
    let d = world.deps();
    let drafted = flow::draft(&d, "app", "app", &cut("v0.1.0")).unwrap();
    flow::candidate(&d, &drafted.id, &BytesBuilder(bytes), None).unwrap();
    drafted.id
}

/// The first artifact of a release, as the entity holds it now.
fn first_artifact(world: &World, id: &str) -> ReleaseArtifact {
    world.releases.get(id).unwrap().artifacts[0].clone()
}

#[test]
fn every_artifact_of_a_candidate_verifies_and_publish_checks_them_first() {
    let (client, _handle, _dir, _change, _agent) = lab("sign-verify");
    let world = World::new(client, "every_artifact_of_a_candidate_verifies");
    let d = world.deps();
    let drafted = flow::draft(&d, "app", "app", &cut("v0.1.0")).unwrap();
    let candidate = flow::candidate(
        &d,
        &drafted.id,
        &BytesBuilder(b"the app's bytes".to_vec()),
        Some(1),
    )
    .unwrap();
    assert_eq!(candidate.artifacts.len(), 2, "one artifact per target");
    for art in &candidate.artifacts {
        signing::check(art, &bytes_of(&world, art), &world.signing).unwrap();
        assert!(art.signatures.starts_with("ed25519 "), "{}", art.signatures);
        let sbom: Value = serde_json::from_str(&art.sbom).unwrap();
        assert_eq!(sbom["bomFormat"], "CycloneDX");
        assert!(
            sbom["components"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["purl"] == "pkg:cargo/serde@1.0.219"),
            "the SBOM comes from the lock file at the release's revision"
        );
        let envelope: Value = serde_json::from_str(&art.provenance).unwrap();
        assert_eq!(envelope["payloadType"], weft::release::IN_TOTO);
    }
    flow::verify(&d, &drafted.id).unwrap();
    flow::checks(&d, &drafted.id, true, "").unwrap();
    let published = flow::publish(&d, &drafted.id).unwrap();
    assert_eq!(published.state, ReleaseState::Published);
}

#[test]
fn a_tampered_artifact_does_not_verify() {
    let (client, _handle, _dir, _change, _agent) = lab("sign-tamper-artifact");
    let world = World::new(client, "a_tampered_artifact_does_not_verify");
    let id = candidate_of(&world, b"the app's bytes".to_vec());
    let art = first_artifact(&world, &id);
    let mut bytes = bytes_of(&world, &art);
    bytes.push(b'!');
    let err = signing::check(&art, &bytes, &world.signing).unwrap_err();
    assert!(err.contains("not the artifact Weft recorded"), "{err}");
}

#[test]
fn a_tampered_sbom_fails_verification() {
    let (client, _handle, _dir, _change, _agent) = lab("sign-tamper-sbom");
    let world = World::new(client, "a_tampered_sbom_fails_verification");
    let id = candidate_of(&world, b"the app's bytes".to_vec());
    flow::verify(&world.deps(), &id).unwrap();
    world.releases.update(&id, |r| {
        let art = &mut r.artifacts[0];
        art.sbom = art.sbom.replace("1.0.219", "1.0.220");
    });
    let err = flow::verify(&world.deps(), &id).unwrap_err();
    assert!(err.contains("another SBOM"), "{err}");
}

#[test]
fn a_tampered_provenance_fails_verification_and_publish_refuses_it() {
    let (client, _handle, _dir, _change, _agent) = lab("sign-tamper-provenance");
    let world = World::new(client, "a_tampered_provenance_fails_verification");
    let id = candidate_of(&world, b"the app's bytes".to_vec());
    let b64 = base64::engine::general_purpose::STANDARD;
    let art = first_artifact(&world, &id);
    let mut envelope: Value = serde_json::from_str(&art.provenance).unwrap();
    let mut statement: Value =
        serde_json::from_slice(&b64.decode(envelope["payload"].as_str().unwrap()).unwrap())
            .unwrap();
    statement["predicate"]["runDetails"]["builder"]["id"] = json!("someone-else");

    // Edited payload, the signature kept: the hub's signature no longer holds.
    envelope["payload"] = json!(b64.encode(serde_json::to_vec(&statement).unwrap()));
    world
        .releases
        .update(&id, |r| r.artifacts[0].provenance = envelope.to_string());
    let err = flow::verify(&world.deps(), &id).unwrap_err();
    assert!(err.contains("the key's"), "{err}");
    let refused = flow::publish(&world.deps(), &id).unwrap_err();
    assert!(refused.contains("does not verify"), "{refused}");
    assert_eq!(
        world.releases.get(&id).unwrap().state,
        ReleaseState::Candidate
    );

    // The same statement sealed by another key is refused too.
    let forged = weft::release::dsse(&statement, &KeyPair::generate());
    world
        .releases
        .update(&id, |r| r.artifacts[0].provenance = forged.to_string());
    assert!(
        flow::verify(&world.deps(), &id)
            .unwrap_err()
            .contains("the key's")
    );
}

#[test]
fn a_windows_artifact_carries_a_stand_in_authenticode_signature_that_verifies() {
    let (client, _handle, _dir, _change, _agent) = lab("sign-pe");
    let world = World::new(client, "a_windows_artifact_carries_a_stand_in_authenticode");
    let pe = [b"MZ\x90\x00".to_vec(), b"the windows build".to_vec()].concat();
    let id = candidate_of(&world, pe.clone());
    let art = first_artifact(&world, &id);
    assert!(
        art.signatures.contains("authenticode-standin "),
        "{}",
        art.signatures
    );
    signing::check(&art, &bytes_of(&world, &art), &world.signing).unwrap();

    // Without its platform signature a PE is unsigned, whatever its hub signature says.
    let mut unsigned = art.clone();
    unsigned.signatures = unsigned
        .signatures
        .lines()
        .filter(|l| !l.starts_with("authenticode"))
        .collect::<Vec<_>>()
        .join("\n");
    let err = signing::check(&unsigned, &pe, &world.signing).unwrap_err();
    assert!(err.contains("unsigned"), "{err}");

    // A changed PE is not the artifact Weft recorded.
    let mut changed = pe;
    changed.push(0);
    assert!(signing::check(&art, &changed, &world.signing).is_err());
}

#[test]
fn an_android_package_is_recognised_and_signed_by_the_apksigner_stand_in() {
    let apk = [
        b"PK\x03\x04".to_vec(),
        b"....AndroidManifest.xml....".to_vec(),
    ]
    .concat();
    assert_eq!(signing::format_of(&apk), Format::Apk);
    let keys = super::tests::TestSigning::new();
    let signed = signing::sign_platform(&keys, apk.clone()).unwrap();
    assert_eq!(
        signed.bytes, apk,
        "a stand-in signature leaves the bytes as they are"
    );
    assert_eq!(signed.lines.len(), 1);
    assert!(
        signed.lines[0].starts_with("apksigner-standin "),
        "{:?}",
        signed.lines
    );
    assert_eq!(signing::format_of(b"plain text"), Format::Other);
}

#[test]
fn a_candidate_refuses_a_build_whose_lock_file_is_not_in_its_revision() {
    let (client, _handle, _dir, _change, _agent) = lab("sign-no-lock");
    let world = World::new(client, "a_candidate_refuses_a_build_without_a_lock_file");
    let d = world.deps();
    let drafted = flow::draft(&d, "app", "app", &cut("v0.1.0")).unwrap();
    // A release of a revision with no lock file is what 9.3 refuses: the SBOM has nothing to list.
    world
        .releases
        .update(&drafted.id, |r| r.from = "no-such-revision".into());
    let err = flow::candidate(&d, &drafted.id, &BytesBuilder(b"x".to_vec()), None).unwrap_err();
    assert!(err.contains("no-such-revision"), "{err}");
    assert_eq!(
        world.releases.get(&drafted.id).unwrap().state,
        ReleaseState::Draft
    );
    let run = world
        .releases
        .get(&drafted.id)
        .unwrap()
        .run
        .clone()
        .unwrap();
    assert_eq!(d.runs.view(&run).unwrap().run.state, RunState::Failed);
}

/// A small Mach-O built here by rustc: a whole test executable is larger than one of Weft's answers.
#[cfg(target_os = "macos")]
fn built_binary() -> Vec<u8> {
    let dir = std::env::temp_dir().join(format!("apz-built-binary-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join("hello.rs");
    std::fs::write(&source, "fn main() { println!(\"built here\"); }\n").unwrap();
    let binary = dir.join("hello");
    let built = std::process::Command::new("rustc")
        .arg("-o")
        .arg(&binary)
        .arg(&source)
        .status()
        .unwrap();
    assert!(built.success(), "rustc builds the test binary");
    let bytes = std::fs::read(&binary).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    bytes
}

/// A built macOS binary is signed ad hoc with the real `codesign -s -`, its digest is notarised by the stand-in, and
/// `codesign -v --strict` checks the signed bytes, both by the release's publish and by a direct run here.
#[cfg(target_os = "macos")]
#[test]
fn a_built_macos_binary_is_signed_ad_hoc_and_checked_with_codesign() {
    let bytes = built_binary();
    assert_eq!(signing::format_of(&bytes), Format::MachO);
    let (client, _handle, _dir, _change, _agent) = lab("sign-macho");
    let world = World::new(client, "a_built_macos_binary_is_signed_ad_hoc");
    let id = candidate_of(&world, bytes);
    let art = first_artifact(&world, &id);
    assert!(
        art.signatures.contains("codesign adhoc"),
        "{}",
        art.signatures
    );
    let signed = bytes_of(&world, &art);
    assert!(
        world
            .signing
            .notary
            .submitted
            .lock()
            .unwrap()
            .contains(&signing::sha256_hex(&signed)),
        "the notary was sent the digest of the signed bytes"
    );
    flow::verify(&world.deps(), &id).unwrap();

    let dir = std::env::temp_dir().join(format!("apz-codesign-check-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("signed-binary");
    std::fs::write(&file, &signed).unwrap();
    let verify = std::process::Command::new("codesign")
        .args(["-v", "--strict"])
        .arg(&file)
        .output()
        .unwrap();
    assert!(
        verify.status.success(),
        "{}",
        String::from_utf8_lossy(&verify.stderr)
    );
    let info = std::process::Command::new("codesign")
        .args(["-dv"])
        .arg(&file)
        .output()
        .unwrap();
    let info = String::from_utf8_lossy(&info.stderr).to_string();
    assert!(info.contains("Signature=adhoc"), "{info}");
    let _ = std::fs::remove_dir_all(&dir);
}
