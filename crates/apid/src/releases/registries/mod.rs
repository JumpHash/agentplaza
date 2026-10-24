//! Package registries as publish connectors (PLAN 22.4, 9.5). After a release is published (`flow::publish_all`), each of
//! its artifacts goes to the registry of its ecosystem: a crate to crates.io, an npm tarball to npm, a wheel to PyPI, a
//! jar to Maven, a nupkg to NuGet, an image layer to an OCI registry, a Go module to the module proxy and a formula to a
//! Homebrew tap. The requests are planned by `wire.rs` from the package's own bytes and sent through the glue tier's
//! mediator (`glue::Mediator`): the host is the one the registry's manifest names (`connectors/manifests/registries`),
//! and the secret the vault holds for it goes in the header the manifest names.
//!
//! The secret is read here and nowhere else: [`Step`] is the only thing that carries a [`Credentials`], and it is built
//! by the publish route alone. The vault refuses a registry's secret to any other binding (the registry's connector,
//! `connector:<id>`, is the only consumer that reads it), and a secret absent from the vault skips its registry rather than failing the release.
//!
//! Each artifact's outcome is a sync row (`ReleaseSync`) keyed `<connector>/<target>`: `published`, `skipped: …` or
//! `failed: …`. A published row is never sent again; publishing the release again sends the rest. A row failed or skipped
//! is retried that way. The real local registries (Verdaccio, devpi, a Maven repository manager) are not run here: the
//! tests fake each registry's publish endpoints (`tests.rs`).

#[cfg(test)]
mod tests;
pub mod wire;

use std::collections::BTreeMap;

use secrecy::ExposeSecret;

use super::entity::{Release, ReleaseArtifact, ReleaseSync};
use super::sbom::ecosystem_of;
use super::signing::sha256_hex;
use crate::connectors::manifest::{self, Manifest};
use crate::glue::{Mediator, Outbound, Secret};
use crate::vault::{Consumer, OnBehalf, VaultError};

/// The most a publish's request body may be: a package's own bytes (512 MiB), not the glue tier's 256 KiB.
pub const BODY_CAP: usize = 512 << 20;

/// The publish connectors, one per registry, by id (their manifests are `connectors::manifest::registries`).
pub const CONNECTORS: [&str; 8] = [
    "crates-io",
    "npm",
    "pypi",
    "maven-central",
    "nuget",
    "oci-registry",
    "go-proxy",
    "homebrew-tap",
];

/// The connector an ecosystem publishes to: the prefix of a release's target (`cargo:app` is `cargo`). `None` for an
/// ecosystem with no registry.
pub fn connector_of(ecosystem: &str) -> Option<&'static str> {
    match ecosystem {
        "cargo" => Some("crates-io"),
        "npm" => Some("npm"),
        "python" => Some("pypi"),
        "gradle" => Some("maven-central"),
        "nuget" => Some("nuget"),
        "image" => Some("oci-registry"),
        "go" => Some("go-proxy"),
        "brew" => Some("homebrew-tap"),
        _ => None,
    }
}

/// The sync row's platform key of one artifact on one registry.
pub fn platform_of(connector: &str, target: &str) -> String {
    format!("{connector}/{target}")
}

/// Whether a sync row belongs to a registry (its key names a connector before the `/`).
pub fn is_registry(platform: &str) -> bool {
    platform
        .split_once('/')
        .is_some_and(|(connector, _)| CONNECTORS.contains(&connector))
}

/// The vault's binding of a connector's secret: the registry's own connector is the only consumer that reads it.
pub fn binding_of(connector: &str) -> String {
    format!("connector:{connector}")
}

/// Reads a registry's secret. Production is the vault, through its binding check (`Credentials` for the vault's store);
/// `Ok(None)` when the vault holds no secret of that name for that binding.
pub trait Credentials {
    fn secret(&self, name: &str, binding: &str) -> Result<Option<String>, String>;
}

impl Credentials for crate::vault::Store {
    fn secret(&self, name: &str, binding: &str) -> Result<Option<String>, String> {
        let consumer = Consumer {
            binding: binding.to_string(),
            on_behalf: OnBehalf::Hub,
        };
        match self.resolve(name, &consumer, apz_plaza::now_ms()) {
            Ok(secret) => Ok(Some(secret.expose_secret().to_string())),
            Err(VaultError::NotFound) => Ok(None),
            Err(e) => Err(e.message()),
        }
    }
}

/// The publish step's dependencies: the transport (the hub's, in production), the vault and the hub's public address
/// (a formula's url is the artifact's download address there).
pub struct Step<'a> {
    pub outbound: &'a dyn Outbound,
    pub vault: &'a dyn Credentials,
    pub link_base: Option<&'a str>,
    pub at_ms: u64,
}

/// Publishes the artifacts of `rel` to their registries and returns the sync rows of every registry artifact: the rows
/// already `published` are carried forward, the others are sent now. `bytes_of` reads an artifact by its digest.
pub fn publish_release(
    step: &Step,
    rel: &Release,
    bytes_of: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) -> Vec<ReleaseSync> {
    let mut rows = Vec::new();
    for art in &rel.artifacts {
        // A package is the artifact built for `any` (Weft names a target `<name>@<platform>`): a platform build is not one.
        let Some((name, "any")) = art.target.rsplit_once('@') else {
            continue;
        };
        let Some(connector) = connector_of(ecosystem_of(name)) else {
            continue;
        };
        let platform = platform_of(connector, &art.target);
        if let Some(done) = rel
            .sync
            .iter()
            .find(|s| s.platform == platform && s.state == "published")
        {
            rows.push(done.clone());
            continue;
        }
        let (state, url) = match send_one(step, rel, art, name, connector, bytes_of) {
            Ok(Sent::Published(page)) => ("published".to_string(), Some(page)),
            Ok(Sent::Skipped(why)) => (format!("skipped: {why}"), None),
            Err(e) => (format!("failed: {e}"), None),
        };
        rows.push(ReleaseSync {
            platform,
            state,
            url,
            at_ms: step.at_ms,
        });
    }
    rows
}

enum Sent {
    Published(String),
    Skipped(String),
}

/// One artifact to one registry: its secret (skipped when the vault has none), its plan, and each request through the
/// mediator. A 409 is the version already being there, which is published as far as the registry is concerned.
fn send_one(
    step: &Step,
    rel: &Release,
    art: &ReleaseArtifact,
    name: &str,
    connector: &str,
    bytes_of: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<Sent, String> {
    let manifest = manifest_of(connector)?;
    let secret = match manifest.secrets.first() {
        None => None,
        Some(name) => match step.vault.secret(name, &binding_of(connector))? {
            Some(raw) => Some(raw),
            None => return Ok(Sent::Skipped(format!("{name} is not in the vault"))),
        },
    };
    let bytes = bytes_of(&art.digest)?;
    let link = step
        .link_base
        .map(|base| format!("{base}/{}/content", art.artifact));
    let plan = wire::plan(
        connector,
        &wire::Artifact {
            target: name,
            project: &rel.project,
            version: &rel.version,
            bytes: &bytes,
            sha256: sha256_hex(&bytes),
            link,
        },
    )?;
    let secret = secret.map(|raw| Secret {
        header: manifest.auth_header.clone(),
        value: manifest
            .auth_format
            .replace("${SECRET}", &encoded(connector, &raw)),
    });
    let mediator = Mediator {
        egress: &manifest.egress,
        secret: secret.as_ref(),
        outbound: step.outbound,
        loopback_http: false,
        body_cap: BODY_CAP,
    };
    for req in &plan.requests {
        let headers: BTreeMap<String, String> = req.headers.iter().cloned().collect();
        let reply = mediator.send_bytes(req.method, &req.url, &headers, &req.body)?;
        if !((200..300).contains(&reply.status) || reply.status == 409) {
            return Err(format!(
                "{} {} answered {}: {}",
                req.method,
                req.url.split('?').next().unwrap_or(""),
                reply.status,
                reply.body.chars().take(200).collect::<String>()
            ));
        }
    }
    Ok(Sent::Published(plan.page))
}

/// The credential as the registry's header carries it: a PyPI token is `__token__:<token>` in basic auth, a Maven
/// credential is `user:password` in basic auth, and the others are the raw token.
fn encoded(connector: &str, raw: &str) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    match connector {
        "pypi" => b64.encode(format!("__token__:{raw}")),
        "maven-central" => b64.encode(raw),
        _ => raw.to_string(),
    }
}

fn manifest_of(connector: &str) -> Result<Manifest, String> {
    manifest::registries()
        .into_iter()
        .find(|m| m.id == connector)
        .ok_or_else(|| format!("no manifest for the {connector} connector"))
}
