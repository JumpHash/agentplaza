//! Platform signatures and the check of a candidate's artifacts (PLAN 22.2, 9.3).
//!
//! Each artifact carries one line per signature in its `signatures` (`scheme` then its fields):
//! - `ed25519 <hub key> <sig>`: the hub's signature over the artifact's SHA-256 (every artifact has one);
//! - `codesign adhoc`: an Apple Mach-O signed with `codesign -s -` (no identity: ad hoc), checked with `codesign -v`;
//! - `notary <ticket> <sha256>`: the notarisation ticket of the signed bytes, from a [`Notary`] (a stand-in until 9.6);
//! - `authenticode-standin <key> <sig>` and `apksigner-standin <key> <sig>`: Windows and Android signatures from a
//!   stand-in signer, made with the platform's key in the vault (osslsigncode and apksigner are not on this Mac, so
//!   nothing here runs them: the real signers are named as not run until a node with the SDKs signs).
//!
//! The platform is the bytes' own format (Mach-O, PE, an APK), so a target's signature cannot be left out by naming it
//! differently. [`check`] verifies every line and requires the ones the format needs.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use sha2::{Digest, Sha256};
use weft::keys::{KeyPair, PublicKey};

use super::attest;
use super::entity::ReleaseArtifact;
use super::sbom;

/// The keys and services the release step signs with. Production reads the keys from the vault (`routes.rs`); the
/// tests generate their own. Nothing else reads them.
pub trait Signing {
    /// The hub's key: it signs each artifact's digest and its provenance. Weft's own key (`WEFT_HUB_KEY`).
    fn hub(&self) -> Result<KeyPair, String>;
    /// The key of a stand-in platform signer (`authenticode-standin`, `apksigner-standin`).
    fn platform(&self, scheme: &str) -> Result<KeyPair, String>;
    /// The notary a Mach-O is submitted to.
    fn notary(&self) -> &dyn Notary;
}

/// Notarisation: submits a signed artifact's digest and returns the ticket the notary issued for it.
pub trait Notary {
    fn submit(&self, sha256: &str) -> Result<String, String>;
    /// Whether `ticket` is the one issued for `sha256`.
    fn accepted(&self, ticket: &str, sha256: &str) -> bool;
}

/// The notary of this release until the App Store Connect client exists (9.6): a stand-in whose ticket is the digest's
/// prefix. The check still compares the ticket to the signed bytes, so a changed artifact is refused.
pub struct StandInNotary;

impl Notary for StandInNotary {
    fn submit(&self, sha256: &str) -> Result<String, String> {
        Ok(ticket_of(sha256))
    }

    fn accepted(&self, ticket: &str, sha256: &str) -> bool {
        ticket == ticket_of(sha256)
    }
}

fn ticket_of(sha256: &str) -> String {
    format!("standin-{}", &sha256[..16.min(sha256.len())])
}

/// The format of an artifact's bytes, which decides its platform signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// A 64-bit Mach-O (thin: the arm64 binaries this release builds).
    MachO,
    /// A Windows PE file.
    Pe,
    /// An Android package: a zip with an `AndroidManifest.xml`.
    Apk,
    Other,
}

pub fn format_of(bytes: &[u8]) -> Format {
    if bytes.starts_with(&[0xCF, 0xFA, 0xED, 0xFE]) || bytes.starts_with(&[0xFE, 0xED, 0xFA, 0xCF])
    {
        Format::MachO
    } else if bytes.starts_with(b"MZ") {
        Format::Pe
    } else if bytes.starts_with(b"PK\x03\x04") && contains(bytes, b"AndroidManifest.xml") {
        Format::Apk
    } else {
        Format::Other
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// The signature schemes an artifact of this format must carry.
fn required(format: Format) -> &'static [&'static str] {
    match format {
        Format::MachO => &["codesign", "notary"],
        Format::Pe => &["authenticode-standin"],
        Format::Apk => &["apksigner-standin"],
        Format::Other => &[],
    }
}

/// Bytes after platform signing, and the lines that record its signatures (the platform part of `signatures`).
pub struct Signed {
    pub bytes: Vec<u8>,
    pub lines: Vec<String>,
}

/// Signs `bytes` for their platform (by format). Other formats carry no platform signature.
pub fn sign_platform(signing: &dyn Signing, bytes: Vec<u8>) -> Result<Signed, String> {
    match format_of(&bytes) {
        Format::MachO => {
            let signed = codesign_adhoc(&bytes)?;
            let sha = sha256_hex(&signed);
            let ticket = signing.notary().submit(&sha)?;
            Ok(Signed {
                bytes: signed,
                lines: vec!["codesign adhoc".into(), format!("notary {ticket} {sha}")],
            })
        }
        Format::Pe => standin(signing, "authenticode-standin", bytes),
        Format::Apk => standin(signing, "apksigner-standin", bytes),
        Format::Other => Ok(Signed {
            bytes,
            lines: Vec::new(),
        }),
    }
}

fn standin(signing: &dyn Signing, scheme: &str, bytes: Vec<u8>) -> Result<Signed, String> {
    let sha = sha256_hex(&bytes);
    let key = signing.platform(scheme)?;
    let line = format!(
        "{scheme} {} {}",
        key.public().hex(),
        b64(&key.sign(&message(scheme, &sha)))
    );
    Ok(Signed {
        bytes,
        lines: vec![line],
    })
}

/// The `ed25519` line of the hub: its signature over the artifact's SHA-256.
pub fn hub_line(hub: &KeyPair, sha256: &str) -> String {
    format!(
        "ed25519 {} {}",
        hub.public().hex(),
        b64(&hub.sign(&message("ed25519", sha256)))
    )
}

/// What a signature covers: its scheme and the artifact's SHA-256, so one scheme's signature is no other's.
fn message(scheme: &str, sha256: &str) -> Vec<u8> {
    format!("{scheme}\n{sha256}").into_bytes()
}

/// Checks an artifact record against its bytes: the bytes are the ones Weft recorded, every signature line verifies
/// and the format's required signatures are present, the provenance is the hub's and binds this artifact and its SBOM,
/// and the SBOM describes the artifact.
pub fn check(record: &ReleaseArtifact, bytes: &[u8], signing: &dyn Signing) -> Result<(), String> {
    let at = |why: &str| format!("{}: {why}", record.target);
    if weft::hash::Hash::of(bytes).hex() != record.digest {
        return Err(at("the bytes are not the artifact Weft recorded"));
    }
    let sha = sha256_hex(bytes);
    let hub = signing.hub()?.public();
    let mut schemes = Vec::new();
    for line in record.signatures.lines().filter(|l| !l.trim().is_empty()) {
        let scheme = check_line(line, bytes, &sha, &hub, signing).map_err(|e| at(&e))?;
        schemes.push(scheme);
    }
    if !schemes.iter().any(|s| s == "ed25519") {
        return Err(at("it has no signature of the hub"));
    }
    for need in required(format_of(bytes)) {
        if !schemes.iter().any(|s| s == need) {
            return Err(at(&format!("it is unsigned: no {need} signature")));
        }
    }
    let statement = attest::open(&record.provenance, &hub).map_err(|e| at(&e))?;
    let sbom_sha = sha256_hex(record.sbom.as_bytes());
    attest::check_subjects(&statement, &sha, &sbom_sha).map_err(|e| at(&e))?;
    sbom::check(&record.sbom, &sha).map_err(|e| at(&e))
}

/// Verifies one signature line; returns its scheme.
fn check_line(
    line: &str,
    bytes: &[u8],
    sha: &str,
    hub: &PublicKey,
    signing: &dyn Signing,
) -> Result<String, String> {
    let mut fields = line.split(' ');
    let scheme = fields.next().unwrap_or_default();
    let field = |fields: &mut std::str::Split<'_, char>, what: &str| {
        fields
            .next()
            .map(str::to_string)
            .ok_or_else(|| format!("a {scheme} signature has no {what}"))
    };
    match scheme {
        "ed25519" => {
            let key = field(&mut fields, "key")?;
            if key != hub.hex() {
                return Err("its signature is not the hub's key".into());
            }
            verify_sig(
                hub,
                &message("ed25519", sha),
                &field(&mut fields, "signature")?,
            )?;
        }
        "codesign" => {
            if field(&mut fields, "mode")? != "adhoc" {
                return Err("codesign signatures are ad hoc here".into());
            }
            codesign_verify(bytes)?;
        }
        "notary" => {
            let ticket = field(&mut fields, "ticket")?;
            if field(&mut fields, "digest")? != sha {
                return Err("the notary's ticket is for other bytes".into());
            }
            if !signing.notary().accepted(&ticket, sha) {
                return Err("the notary did not issue this ticket".into());
            }
        }
        "authenticode-standin" | "apksigner-standin" => {
            let key = field(&mut fields, "key")?;
            let expected = signing.platform(scheme)?.public();
            if key != expected.hex() {
                return Err(format!("its signature is not the {scheme} key"));
            }
            verify_sig(
                &expected,
                &message(scheme, sha),
                &field(&mut fields, "signature")?,
            )?;
        }
        other => return Err(format!("an unknown signature scheme {other:?}")),
    }
    Ok(scheme.to_string())
}

fn verify_sig(key: &PublicKey, message: &[u8], sig: &str) -> Result<(), String> {
    let sig = base64::engine::general_purpose::STANDARD
        .decode(sig)
        .map_err(|e| format!("a signature is not base64: {e}"))?;
    key.verify(message, &sig)
        .map_err(|_| "a signature does not verify over the artifact".to_string())
}

/// The SHA-256 of `bytes` in hex: the digest of the artifact in the provenance and the signatures.
pub fn sha256_hex(bytes: &[u8]) -> String {
    weft::hash::hex(&Sha256::digest(bytes))
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

// ── codesign ─────────────────────────────────────────────────────────────────────────────────────────────────────

/// A scratch folder holding one file for a tool to read or sign; removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn with(bytes: &[u8]) -> Result<(Scratch, PathBuf), String> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("apz-sign-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let file = dir.join("artifact");
        std::fs::write(&file, bytes).map_err(|e| format!("{}: {e}", file.display()))?;
        Ok((Scratch(dir), file))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn require_macos() -> Result<(), String> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err(
            "codesign runs only on macOS: a Mach-O artifact cannot be signed or checked here"
                .into(),
        )
    }
}

fn run(tool: &mut Command) -> Result<(), String> {
    let out = tool
        .output()
        .map_err(|e| format!("could not run the tool: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let why = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(format!("the tool refused: {why}"))
}

/// The bytes of a Mach-O signed ad hoc with `codesign -s -` (no identity, no keychain).
fn codesign_adhoc(bytes: &[u8]) -> Result<Vec<u8>, String> {
    require_macos()?;
    let (scratch, file) = Scratch::with(bytes)?;
    run(Command::new("codesign").args(["-s", "-", "-f"]).arg(&file))
        .map_err(|e| format!("codesign -s -: {e}"))?;
    let signed = std::fs::read(&file).map_err(|e| format!("{}: {e}", scratch.path().display()))?;
    Ok(signed)
}

/// `codesign -v --strict` on the bytes: whether their signature holds.
fn codesign_verify(bytes: &[u8]) -> Result<(), String> {
    require_macos()?;
    let (_scratch, file) = Scratch::with(bytes)?;
    run(Command::new("codesign").args(["-v", "--strict"]).arg(&file))
        .map_err(|e| format!("codesign -v: {e}"))
}
