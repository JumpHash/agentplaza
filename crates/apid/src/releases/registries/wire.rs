//! The requests a package's publish sends, per registry (PLAN 22.4, 9.5), built from the package's own bytes. Nothing here
//! sends: each planner returns the requests in order and the page the registry shows the package on, and the step
//! (`mod.rs`) sends them through the glue tier's mediator with the registry's secret.
//!
//! The package's name and version come from its own metadata (a crate's Cargo.toml, an npm tarball's package.json, a
//! wheel's METADATA, a jar's pom.properties, a nupkg's nuspec), and must be the release's version: a package that says
//! another version is refused before anything is sent.

use std::io::{Cursor, Read};

use base64::Engine;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha512};

use crate::releases::signing::sha256_hex;

/// One request of a publish: its method, its address, its headers (lower-case names) and its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    fn new(
        method: &'static str,
        url: impl Into<String>,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> Request {
        Request {
            method,
            url: url.into(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body,
        }
    }
}

/// A package's publish: its requests in order, and the page the registry shows the package on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub page: String,
    pub requests: Vec<Request>,
}

/// What a planner is given about one artifact of a release.
pub struct Artifact<'a> {
    /// The release's target, `cargo:app`, `image:ops/hub`, `go:example.com/acme`.
    pub target: &'a str,
    pub project: &'a str,
    /// The release's version.
    pub version: &'a str,
    pub bytes: &'a [u8],
    /// The SHA-256 of `bytes`, in hex.
    pub sha256: String,
    /// The artifact's download address (the hub's artifact route), when the hub has a public address: a formula's url.
    pub link: Option<String>,
}

/// The planner of a registry connector.
pub fn plan(connector: &str, a: &Artifact) -> Result<Plan, String> {
    match connector {
        "crates-io" => cargo(a),
        "npm" => npm(a),
        "pypi" => pypi(a),
        "maven-central" => maven(a),
        "nuget" => nuget(a),
        "oci-registry" => oci(a),
        "go-proxy" => go(a),
        "homebrew-tap" => brew(a),
        other => Err(format!("{other} is not a publish connector")),
    }
}

/// crates.io's publish: `PUT /api/v1/crates/new` with the crate's metadata and its `.crate` file, both length-prefixed
/// (little-endian u32) as cargo sends them.
pub fn cargo(a: &Artifact) -> Result<Plan, String> {
    let manifest = tar_gz_entry(a.bytes, |p| {
        p.matches('/').count() == 1 && p.ends_with("/Cargo.toml")
    })?;
    let text = String::from_utf8(manifest)
        .map_err(|_| "the crate's Cargo.toml is not UTF-8".to_string())?;
    let doc: toml::Table = text
        .parse()
        .map_err(|e| format!("the crate's Cargo.toml does not parse: {e}"))?;
    let pkg = doc
        .get("package")
        .and_then(|p| p.as_table())
        .ok_or("the crate's Cargo.toml has no [package]")?;
    let name = str_field(pkg.get("name").and_then(|v| v.as_str()), "package.name")?;
    let vers = str_field(
        pkg.get("version").and_then(|v| v.as_str()),
        "package.version",
    )?;
    check_version(a, vers)?;
    let meta = json!({
        "name": name,
        "vers": vers,
        "deps": [],
        "features": {},
        "authors": [],
        "description": pkg.get("description").and_then(|d| d.as_str()),
        "license": pkg.get("license").and_then(|d| d.as_str()),
        "keywords": [],
        "categories": [],
        "badges": {},
        "links": null,
    });
    let meta = serde_json::to_vec(&meta).map_err(|e| e.to_string())?;
    let mut body = Vec::with_capacity(8 + meta.len() + a.bytes.len());
    body.extend_from_slice(&len_prefix(meta.len())?);
    body.extend_from_slice(&meta);
    body.extend_from_slice(&len_prefix(a.bytes.len())?);
    body.extend_from_slice(a.bytes);
    Ok(Plan {
        page: format!("https://crates.io/crates/{name}/{vers}"),
        requests: vec![Request::new(
            "PUT",
            "https://crates.io/api/v1/crates/new",
            &[("content-type", "application/octet-stream")],
            body,
        )],
    })
}

/// npm's publish: `PUT /{name}` with the packument that names the version, its tarball attached as base64 (the way the
/// npm CLI publishes). The scope's slash is `%2F` in the path.
pub fn npm(a: &Artifact) -> Result<Plan, String> {
    let manifest = tar_gz_entry(a.bytes, |p| {
        p.matches('/').count() == 1 && p.ends_with("/package.json")
    })?;
    let pkg: Value = serde_json::from_slice(&manifest).map_err(|e| format!("package.json: {e}"))?;
    let name = str_field(pkg.get("name").and_then(Value::as_str), "name")?;
    let version = str_field(pkg.get("version").and_then(Value::as_str), "version")?;
    check_version(a, version)?;
    let file = format!(
        "{}-{version}.tgz",
        name.trim_start_matches('@').replace('/', "-")
    );
    let tarball = format!("https://registry.npmjs.org/{name}/-/{file}");
    let integrity = format!(
        "sha512-{}",
        base64::engine::general_purpose::STANDARD.encode(Sha512::digest(a.bytes))
    );
    let mut versions = Map::new();
    versions.insert(
        version.to_string(),
        json!({ "name": name, "version": version, "dist": { "tarball": tarball, "integrity": integrity } }),
    );
    let mut attachments = Map::new();
    attachments.insert(
        file,
        json!({
            "content_type": "application/octet-stream",
            "data": base64::engine::general_purpose::STANDARD.encode(a.bytes),
            "length": a.bytes.len(),
        }),
    );
    let doc = json!({
        "_id": name,
        "name": name,
        "dist-tags": { "latest": version },
        "versions": versions,
        "_attachments": attachments,
    });
    let body = serde_json::to_vec(&doc).map_err(|e| e.to_string())?;
    let url = format!(
        "https://registry.npmjs.org/{}",
        name.replacen('/', "%2F", 1)
    );
    Ok(Plan {
        page: format!("https://www.npmjs.com/package/{name}/v/{version}"),
        requests: vec![Request::new(
            "PUT",
            url,
            &[("content-type", "application/json")],
            body,
        )],
    })
}

/// PyPI's upload: the legacy form (`POST /legacy/`, multipart), with the wheel's metadata and its digest.
pub fn pypi(a: &Artifact) -> Result<Plan, String> {
    let metadata = zip_entry(a.bytes, |p| {
        p.ends_with(".dist-info/METADATA") && p.matches('/').count() == 1
    })?;
    let text =
        String::from_utf8(metadata).map_err(|_| "the wheel's METADATA is not UTF-8".to_string())?;
    let name = header_of(&text, "Name").ok_or("the wheel's METADATA has no Name")?;
    let version = header_of(&text, "Version").ok_or("the wheel's METADATA has no Version")?;
    check_version(a, &version)?;
    let file = format!("{}-{version}-py3-none-any.whl", name.replace('-', "_"));
    let (content_type, body) = multipart(&[
        Field::Text(":action", "file_upload"),
        Field::Text("protocol_version", "1"),
        Field::Text("metadata_version", "2.1"),
        Field::Text("name", &name),
        Field::Text("version", &version),
        Field::Text("filetype", "bdist_wheel"),
        Field::Text("pyversion", "py3"),
        Field::Text("sha256_digest", &a.sha256),
        Field::File("content", &file, a.bytes),
    ]);
    Ok(Plan {
        page: format!("https://pypi.org/project/{name}/{version}/"),
        requests: vec![Request::new(
            "POST",
            "https://upload.pypi.org/legacy/",
            &[("content-type", content_type.as_str())],
            body,
        )],
    })
}

/// Maven's deploy in the Maven 2 layout: the jar and a pom, `PUT` under `group/artifact/version/`. The coordinates
/// come from the jar's own `pom.properties`. Checksum files are left to the repository manager, which writes them.
pub fn maven(a: &Artifact) -> Result<Plan, String> {
    let props = zip_entry(a.bytes, |p| {
        p.starts_with("META-INF/maven/") && p.ends_with("/pom.properties")
    })?;
    let text = String::from_utf8(props).map_err(|_| "pom.properties is not UTF-8".to_string())?;
    let group = property(&text, "groupId").ok_or("pom.properties has no groupId")?;
    let artifact = property(&text, "artifactId").ok_or("pom.properties has no artifactId")?;
    let version = property(&text, "version").ok_or("pom.properties has no version")?;
    check_version(a, &version)?;
    let dir = format!(
        "https://central.sonatype.com/repository/maven-releases/{}/{artifact}/{version}",
        group.replace('.', "/")
    );
    let pom = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<project xmlns=\"http://maven.apache.org/POM/4.0.0\">\n  \
         <modelVersion>4.0.0</modelVersion>\n  <groupId>{group}</groupId>\n  <artifactId>{artifact}</artifactId>\n  \
         <version>{version}</version>\n  <packaging>jar</packaging>\n</project>\n"
    );
    Ok(Plan {
        page: format!("https://central.sonatype.com/artifact/{group}/{artifact}/{version}"),
        requests: vec![
            Request::new(
                "PUT",
                format!("{dir}/{artifact}-{version}.jar"),
                &[("content-type", "application/java-archive")],
                a.bytes.to_vec(),
            ),
            Request::new(
                "PUT",
                format!("{dir}/{artifact}-{version}.pom"),
                &[("content-type", "application/xml")],
                pom.into_bytes(),
            ),
        ],
    })
}

/// NuGet's push: `PUT /api/v2/package` with the `.nupkg` as the form's `package` part. The id and version are the
/// nuspec's.
pub fn nuget(a: &Artifact) -> Result<Plan, String> {
    let nuspec = zip_entry(a.bytes, |p| p.ends_with(".nuspec") && !p.contains('/'))?;
    let text = String::from_utf8(nuspec).map_err(|_| "the nuspec is not UTF-8".to_string())?;
    let id = xml_text(&text, "id").ok_or("the nuspec has no id")?;
    let version = xml_text(&text, "version").ok_or("the nuspec has no version")?;
    check_version(a, &version)?;
    let (content_type, body) = multipart(&[Field::File(
        "package",
        &format!("{id}.{version}.nupkg"),
        a.bytes,
    )]);
    Ok(Plan {
        page: format!("https://www.nuget.org/packages/{id}/{version}"),
        requests: vec![Request::new(
            "PUT",
            "https://www.nuget.org/api/v2/package",
            &[("content-type", content_type.as_str())],
            body,
        )],
    })
}

/// The OCI distribution API's push of a one-layer image: the config and the layer as blobs (monolithic uploads, the
/// digest in the query), then the manifest under the version as its tag. The layer is the artifact's bytes.
pub fn oci(a: &Artifact) -> Result<Plan, String> {
    let repo = repository(a.target, a.project)?;
    let config = serde_json::to_vec(&json!({
        "architecture": "amd64",
        "os": "linux",
        "rootfs": { "type": "layers", "diff_ids": [format!("sha256:{}", a.sha256)] },
    }))
    .map_err(|e| e.to_string())?;
    let config_sha = sha256_hex(&config);
    let manifest = serde_json::to_vec(&json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "application/vnd.oci.image.config.v1+json",
            "digest": format!("sha256:{config_sha}"),
            "size": config.len(),
        },
        "layers": [{
            "mediaType": "application/vnd.oci.image.layer.v1.tar",
            "digest": format!("sha256:{}", a.sha256),
            "size": a.bytes.len(),
        }],
    }))
    .map_err(|e| e.to_string())?;
    let base = format!("https://ghcr.io/v2/{repo}");
    let version = a.version;
    Ok(Plan {
        page: format!("https://ghcr.io/{repo}:{version}"),
        requests: vec![
            Request::new(
                "POST",
                format!("{base}/blobs/uploads/?digest=sha256:{config_sha}"),
                &[("content-type", "application/octet-stream")],
                config,
            ),
            Request::new(
                "POST",
                format!("{base}/blobs/uploads/?digest=sha256:{}", a.sha256),
                &[("content-type", "application/octet-stream")],
                a.bytes.to_vec(),
            ),
            Request::new(
                "PUT",
                format!("{base}/manifests/{version}"),
                &[("content-type", "application/vnd.oci.image.manifest.v1+json")],
                manifest,
            ),
        ],
    })
}

/// The Go module proxy. A module is published by its signed tag (9.4); the proxy learns of the version when it is asked
/// for it, so the step asks for the version's `.info` and takes the answer as the version being there. The module path is
/// escaped as the proxy's protocol says (an upper-case letter is `!` and the lower-case letter).
pub fn go(a: &Artifact) -> Result<Plan, String> {
    let module = a
        .target
        .strip_prefix("go:")
        .filter(|m| !m.is_empty())
        .ok_or("a Go target names its module")?;
    let version = if a.version.starts_with('v') {
        a.version.to_string()
    } else {
        format!("v{}", a.version)
    };
    Ok(Plan {
        page: format!("https://pkg.go.dev/{module}@{version}"),
        requests: vec![Request::new(
            "GET",
            format!(
                "https://proxy.golang.org/{}/@v/{version}.info",
                escape_module(module)
            ),
            &[],
            Vec::new(),
        )],
    })
}

/// A Homebrew tap's publish: the formula `Formula/<project>.rb` is written into the tap repository
/// (`homebrew-<tap>`, the target's `brew:<owner>/<tap>`) with the GitHub contents API. The formula's url is the
/// artifact's download address and its sha256 the artifact's digest.
pub fn brew(a: &Artifact) -> Result<Plan, String> {
    let (owner, tap) = a
        .target
        .strip_prefix("brew:")
        .and_then(|t| t.split_once('/'))
        .ok_or("a Homebrew target names its tap as brew:<owner>/<tap>")?;
    let link = a.link.as_deref().ok_or(
        "a formula's url is the artifact's download address: the hub has no public address",
    )?;
    let project = a.project;
    if project.is_empty()
        || !project
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!(
            "{project} is not a formula name (lower-case letters, digits and -)"
        ));
    }
    let class: String = project
        .split('-')
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|c| c.to_ascii_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        })
        .collect();
    let version = a.version;
    let formula = format!(
        "class {class} < Formula\n  version \"{version}\"\n  url \"{link}\"\n  sha256 \"{}\"\n\n  def install\n    \
         prefix.install Dir[\"*\"]\n  end\nend\n",
        a.sha256
    );
    let body = serde_json::to_vec(&json!({
        "message": format!("{project} {version}"),
        "content": base64::engine::general_purpose::STANDARD.encode(formula),
    }))
    .map_err(|e| e.to_string())?;
    Ok(Plan {
        page: format!("https://github.com/{owner}/homebrew-{tap}/blob/main/Formula/{project}.rb"),
        requests: vec![Request::new(
            "PUT",
            format!(
                "https://api.github.com/repos/{owner}/homebrew-{tap}/contents/Formula/{project}.rb"
            ),
            &[
                ("accept", "application/vnd.github+json"),
                ("content-type", "application/json"),
            ],
            body,
        )],
    })
}

/// The Go module proxy's escaping of a module path: each upper-case letter becomes `!` and its lower-case letter.
pub fn escape_module(module: &str) -> String {
    module
        .chars()
        .map(|c| {
            if c.is_ascii_uppercase() {
                format!("!{}", c.to_ascii_lowercase())
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// One field of a multipart form.
pub enum Field<'a> {
    Text(&'a str, &'a str),
    File(&'a str, &'a str, &'a [u8]),
}

const BOUNDARY: &str = "apz-publish-7f3a9c";

/// A multipart/form-data body: its content type and its bytes.
pub fn multipart(fields: &[Field]) -> (String, Vec<u8>) {
    let mut body = Vec::new();
    for field in fields {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        match field {
            Field::Text(name, value) => body.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                    .as_bytes(),
            ),
            Field::File(name, file, bytes) => {
                body.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"{file}\"\r\n\
                         Content-Type: application/octet-stream\r\n\r\n"
                    )
                    .as_bytes(),
                );
                body.extend_from_slice(bytes);
                body.extend_from_slice(b"\r\n");
            }
        }
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={BOUNDARY}"), body)
}

/// The OCI repository a target publishes under: `image:<path>` (lower case), or the project's own name for `image:.`.
fn repository(target: &str, project: &str) -> Result<String, String> {
    let path = target.strip_prefix("image:").unwrap_or(project);
    let repo = if path == "." || path.is_empty() {
        project.to_string()
    } else {
        path.to_string()
    }
    .to_ascii_lowercase();
    let shaped = !repo.is_empty()
        && repo.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-' | '/')
        });
    if shaped {
        Ok(repo)
    } else {
        Err(format!("{repo} is not a repository name"))
    }
}

fn len_prefix(len: usize) -> Result<[u8; 4], String> {
    u32::try_from(len)
        .map(u32::to_le_bytes)
        .map_err(|_| "a package is larger than 4 GiB".to_string())
}

fn check_version(a: &Artifact, package: &str) -> Result<(), String> {
    if package.trim_start_matches('v') == a.version.trim_start_matches('v') {
        Ok(())
    } else {
        Err(format!(
            "the package is version {package} and the release is {}",
            a.version
        ))
    }
}

fn str_field<'v>(value: Option<&'v str>, what: &str) -> Result<&'v str, String> {
    value.ok_or_else(|| format!("the package has no {what}"))
}

fn header_of(text: &str, key: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.strip_prefix(key).and_then(|rest| rest.strip_prefix(": ")))
        .map(|v| v.trim().to_string())
}

fn property(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == key).then(|| v.trim().to_string())
    })
}

fn xml_text(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(close.as_str())? + start;
    Some(text[start..end].trim().to_string())
}

/// The bytes of the first file of a gzipped tar whose path satisfies `wants`.
fn tar_gz_entry(bytes: &[u8], wants: impl Fn(&str) -> bool) -> Result<Vec<u8>, String> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let entries = archive
        .entries()
        .map_err(|e| format!("the package is not a gzipped tar: {e}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("the package does not read: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("the package does not read: {e}"))?
            .to_string_lossy()
            .into_owned();
        if wants(&path) {
            let mut out = Vec::new();
            entry
                .read_to_end(&mut out)
                .map_err(|e| format!("the package does not read: {e}"))?;
            return Ok(out);
        }
    }
    Err("the package holds no file where its ecosystem keeps its metadata".into())
}

/// The bytes of the first file of a zip whose name satisfies `wants`.
fn zip_entry(bytes: &[u8], wants: impl Fn(&str) -> bool) -> Result<Vec<u8>, String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| format!("the package is not a zip: {e}"))?;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|e| format!("the package does not read: {e}"))?;
        if file.name().is_ok_and(|n| wants(&n)) {
            let mut out = Vec::new();
            file.read_to_end(&mut out)
                .map_err(|e| format!("the package does not read: {e}"))?;
            return Ok(out);
        }
    }
    Err("the package holds no file where its ecosystem keeps its metadata".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::releases::registries::tests::{tgz, zipped};

    fn artifact<'a>(target: &'a str, version: &'a str, bytes: &'a [u8]) -> Artifact<'a> {
        Artifact {
            target,
            project: "app",
            version,
            bytes,
            sha256: sha256_hex(bytes),
            link: Some("https://hub.example/v1/weft/artifacts/a1/content".into()),
        }
    }

    #[test]
    fn a_crate_is_put_as_cargo_sends_it_with_its_metadata_in_front() {
        let krate = tgz(
            "app-0.1.0/Cargo.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\ndescription = \"the app\"\n",
        );
        let plan = cargo(&artifact("cargo:app", "0.1.0", &krate)).unwrap();
        assert_eq!(plan.page, "https://crates.io/crates/app/0.1.0");
        let req = &plan.requests[0];
        assert_eq!(
            (req.method, req.url.as_str()),
            ("PUT", "https://crates.io/api/v1/crates/new")
        );
        let meta_len = u32::from_le_bytes(req.body[..4].try_into().unwrap()) as usize;
        let meta: Value = serde_json::from_slice(&req.body[4..4 + meta_len]).unwrap();
        assert_eq!(meta["name"], "app");
        assert_eq!(meta["vers"], "0.1.0");
        assert_eq!(meta["description"], "the app");
        let rest = &req.body[4 + meta_len..];
        assert_eq!(
            u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize,
            krate.len()
        );
        assert_eq!(&rest[4..], krate.as_slice());
    }

    #[test]
    fn a_package_of_another_version_than_the_release_is_refused_before_anything_is_sent() {
        let krate = tgz(
            "app-0.2.0/Cargo.toml",
            "[package]\nname = \"app\"\nversion = \"0.2.0\"\n",
        );
        let err = cargo(&artifact("cargo:app", "0.1.0", &krate)).unwrap_err();
        assert!(
            err.contains("version 0.2.0 and the release is 0.1.0"),
            "{err}"
        );
    }

    #[test]
    fn an_npm_scope_is_escaped_in_the_path_and_its_tarball_is_attached() {
        let tarball = tgz(
            "package/package.json",
            r#"{"name":"@acme/web","version":"0.1.0"}"#,
        );
        let plan = npm(&artifact("npm:@acme/web", "0.1.0", &tarball)).unwrap();
        let req = &plan.requests[0];
        assert_eq!(req.url, "https://registry.npmjs.org/@acme%2Fweb");
        let doc: Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(doc["dist-tags"]["latest"], "0.1.0");
        assert_eq!(
            doc["versions"]["0.1.0"]["dist"]["tarball"],
            "https://registry.npmjs.org/@acme/web/-/acme-web-0.1.0.tgz"
        );
        let data = base64::engine::general_purpose::STANDARD
            .decode(
                doc["_attachments"]["acme-web-0.1.0.tgz"]["data"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(data, tarball);
    }

    #[test]
    fn a_wheel_is_posted_to_the_legacy_form_with_its_digest() {
        let wheel = zipped(&[
            ("acme/__init__.py", "x = 1\n"),
            (
                "acme-0.1.0.dist-info/METADATA",
                "Metadata-Version: 2.1\nName: acme\nVersion: 0.1.0\n",
            ),
        ]);
        let plan = pypi(&artifact("python:acme", "0.1.0", &wheel)).unwrap();
        let req = &plan.requests[0];
        assert_eq!(
            (req.method, req.url.as_str()),
            ("POST", "https://upload.pypi.org/legacy/")
        );
        let body = String::from_utf8_lossy(&req.body);
        assert!(body.contains("name=\"sha256_digest\"\r\n\r\n"), "{body}");
        assert!(body.contains(&sha256_hex(&wheel)));
        assert!(body.contains("filename=\"acme-0.1.0-py3-none-any.whl\""));
    }

    #[test]
    fn a_jar_is_deployed_in_the_maven_layout_with_a_pom() {
        let jar = zipped(&[(
            "META-INF/maven/com.acme/app/pom.properties",
            "groupId=com.acme\nartifactId=app\nversion=0.1.0\n",
        )]);
        let plan = maven(&artifact("gradle:app", "0.1.0", &jar)).unwrap();
        let urls: Vec<&str> = plan.requests.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://central.sonatype.com/repository/maven-releases/com/acme/app/0.1.0/app-0.1.0.jar",
                "https://central.sonatype.com/repository/maven-releases/com/acme/app/0.1.0/app-0.1.0.pom",
            ]
        );
        assert!(plan.requests.iter().all(|r| r.method == "PUT"));
    }

    #[test]
    fn a_nupkg_is_pushed_with_its_id_and_version_from_the_nuspec() {
        let nupkg = zipped(&[(
            "Acme.Lib.nuspec",
            "<package><metadata><id>Acme.Lib</id><version>0.1.0</version></metadata></package>",
        )]);
        let plan = nuget(&artifact("nuget:Acme.Lib", "0.1.0", &nupkg)).unwrap();
        let req = &plan.requests[0];
        assert_eq!(
            (req.method, req.url.as_str()),
            ("PUT", "https://www.nuget.org/api/v2/package")
        );
        assert!(String::from_utf8_lossy(&req.body).contains("filename=\"Acme.Lib.0.1.0.nupkg\""));
    }

    #[test]
    fn an_image_is_pushed_as_two_blobs_then_its_manifest_under_the_version() {
        let layer = b"the image's layer".to_vec();
        let plan = oci(&Artifact {
            target: "image:.",
            ..artifact("image:.", "0.1.0", &layer)
        })
        .unwrap();
        let [config, blob, manifest] = &plan.requests[..] else {
            panic!("three requests: {plan:?}");
        };
        assert_eq!(config.method, "POST");
        assert!(
            config
                .url
                .starts_with("https://ghcr.io/v2/app/blobs/uploads/?digest=sha256:")
        );
        assert!(!config.body.is_empty());
        assert_eq!(
            blob.url,
            format!(
                "https://ghcr.io/v2/app/blobs/uploads/?digest=sha256:{}",
                sha256_hex(&layer)
            )
        );
        assert_eq!(blob.body, layer);
        assert_eq!(
            (manifest.method, manifest.url.as_str()),
            ("PUT", "https://ghcr.io/v2/app/manifests/0.1.0")
        );
        let doc: Value = serde_json::from_slice(&manifest.body).unwrap();
        assert_eq!(
            doc["layers"][0]["digest"],
            format!("sha256:{}", sha256_hex(&layer))
        );
        assert_eq!(
            doc["config"]["digest"],
            format!("sha256:{}", sha256_hex(&config.body))
        );
    }

    #[test]
    fn an_image_repository_is_its_target_path_in_lower_case() {
        assert_eq!(repository("image:Ops/Hub", "app").unwrap(), "ops/hub");
        assert_eq!(repository("image:.", "app").unwrap(), "app");
        assert!(repository("image:bad name", "app").is_err());
    }

    #[test]
    fn a_go_module_is_asked_for_its_version_at_the_proxy_with_upper_case_escaped() {
        assert_eq!(
            escape_module("github.com/Acme/Lib"),
            "github.com/!acme/!lib"
        );
        let plan = go(&artifact("go:github.com/Acme/Lib", "0.1.0", b"x")).unwrap();
        let req = &plan.requests[0];
        assert_eq!(req.method, "GET");
        assert_eq!(
            req.url,
            "https://proxy.golang.org/github.com/!acme/!lib/@v/v0.1.0.info"
        );
        assert!(req.body.is_empty());
    }

    #[test]
    fn a_homebrew_formula_is_written_to_its_tap_with_the_artifact_url_and_digest() {
        let bytes = b"the tarball".to_vec();
        let plan = brew(&Artifact {
            target: "brew:acme/tools",
            ..artifact("brew:acme/tools", "0.1.0", &bytes)
        })
        .unwrap();
        let req = &plan.requests[0];
        assert_eq!(req.method, "PUT");
        assert_eq!(
            req.url,
            "https://api.github.com/repos/acme/homebrew-tools/contents/Formula/app.rb"
        );
        let doc: Value = serde_json::from_slice(&req.body).unwrap();
        let formula = String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(doc["content"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
        assert!(formula.starts_with("class App < Formula\n"), "{formula}");
        assert!(formula.contains("url \"https://hub.example/v1/weft/artifacts/a1/content\""));
        assert!(formula.contains(&format!("sha256 \"{}\"", sha256_hex(&bytes))));
        assert!(
            brew(&Artifact {
                link: None,
                ..artifact("brew:acme/tools", "0.1.0", &bytes)
            })
            .is_err()
        );
    }

    #[test]
    fn a_planner_for_an_unknown_connector_is_refused() {
        assert!(plan("myspace", &artifact("cargo:app", "0.1.0", b"x")).is_err());
    }
}
