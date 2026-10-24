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
    /// The owner's own Maven repository, when one is set: its Maven 2 layout takes the files, not the Portal's bundle.
    pub repository: Option<&'a str>,
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

/// Maven's publish of one jar and its pom, with a checksum file for each (`.md5` and `.sha1`). With the Central Portal
/// (the default) the files go in one bundle zip under `group/artifact/version/`, uploaded as the Portal's `bundle`. With
/// the owner's own repository, the files are `PUT` in the Maven 2 layout under its address. The coordinates come from the
/// jar's own `pom.properties`. The bundle carries no `.asc` signatures: the Portal requires them, and the owner's key
/// signs nothing here (named as not run).
pub fn maven(a: &Artifact) -> Result<Plan, String> {
    let props = zip_entry(a.bytes, |p| {
        p.starts_with("META-INF/maven/") && p.ends_with("/pom.properties")
    })?;
    let text = String::from_utf8(props).map_err(|_| "pom.properties is not UTF-8".to_string())?;
    let group = property(&text, "groupId").ok_or("pom.properties has no groupId")?;
    let artifact = property(&text, "artifactId").ok_or("pom.properties has no artifactId")?;
    let version = property(&text, "version").ok_or("pom.properties has no version")?;
    check_version(a, &version)?;
    let pom = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<project xmlns=\"http://maven.apache.org/POM/4.0.0\">\n  \
         <modelVersion>4.0.0</modelVersion>\n  <groupId>{group}</groupId>\n  <artifactId>{artifact}</artifactId>\n  \
         <version>{version}</version>\n  <packaging>jar</packaging>\n</project>\n"
    );
    let dir = format!("{}/{artifact}/{version}", group.replace('.', "/"));
    let jar = format!("{artifact}-{version}.jar");
    let pom_name = format!("{artifact}-{version}.pom");
    let mut files = checksummed(&jar, a.bytes);
    files.extend(checksummed(&pom_name, pom.as_bytes()));
    let page = format!("https://central.sonatype.com/artifact/{group}/{artifact}/{version}");
    let requests = match a.repository {
        Some(base) => files
            .iter()
            .map(|(name, bytes)| {
                Request::new(
                    "PUT",
                    format!("{}/{dir}/{name}", base.trim_end_matches('/')),
                    &[("content-type", content_type_of(name))],
                    bytes.clone(),
                )
            })
            .collect(),
        None => {
            let bundle = bundle(
                &files
                    .iter()
                    .map(|(name, bytes)| (format!("{dir}/{name}"), bytes.clone()))
                    .collect::<Vec<_>>(),
            )?;
            let (content_type, body) = multipart(&[Field::File(
                "bundle",
                &format!("{artifact}-{version}-bundle.zip"),
                &bundle,
            )]);
            vec![Request::new(
                "POST",
                "https://central.sonatype.com/api/v1/publisher/upload?publishingType=USER_MANAGED",
                &[("content-type", content_type.as_str())],
                body,
            )]
        }
    };
    Ok(Plan { page, requests })
}

/// A file and its `.md5` and `.sha1` checksum files, as Maven reads them.
fn checksummed(name: &str, bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    use md5::Digest as _;
    let md5 = weft::hash::hex(&md5::Md5::digest(bytes));
    let sha1 = weft::hash::hex(&sha1::Sha1::digest(bytes));
    vec![
        (name.to_string(), bytes.to_vec()),
        (format!("{name}.md5"), md5.into_bytes()),
        (format!("{name}.sha1"), sha1.into_bytes()),
    ]
}

fn content_type_of(name: &str) -> &'static str {
    if name.ends_with(".jar") {
        "application/java-archive"
    } else if name.ends_with(".pom") {
        "application/xml"
    } else {
        "text/plain"
    }
}

/// A zip of `files` (each at its path), the bundle the Central Portal takes.
fn bundle(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    use std::io::Write;
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (path, bytes) in files {
        writer
            .start_file(path, zip::write::SimpleFileOptions::default())
            .map_err(|e| format!("the bundle: {e}"))?;
        writer
            .write_all(bytes)
            .map_err(|e| format!("the bundle: {e}"))?;
    }
    writer
        .finish()
        .map(Cursor::into_inner)
        .map_err(|e| format!("the bundle: {e}"))
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

/// One platform binary of a formula: its platform, its download address and its SHA-256 in hex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub platform: String,
    pub url: String,
    pub sha256: String,
}

/// A tap's formula: `Formula/<project>.rb` in the tap repository, its content and the commit's message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Formula {
    pub owner: String,
    pub tap: String,
    pub project: String,
    pub page: String,
    pub content: String,
    pub message: String,
}

impl Formula {
    /// The formula's address in the tap on the GitHub contents API: read for its sha, then written.
    pub fn contents_url(&self) -> String {
        format!(
            "https://api.github.com/repos/{}/homebrew-{}/contents/Formula/{}.rb",
            self.owner, self.tap, self.project
        )
    }
}

/// A release's formula (9.5, Homebrew): the formula of `brew:<owner>/<tap>` names each platform binary the release built
/// for macOS and Linux, with its url and sha256, and installs the one for the machine. The class is the project's name in
/// camel case, and the binary is installed under the project's name.
pub fn formula(
    project: &str,
    version: &str,
    tap_target: &str,
    entries: &[Entry],
) -> Result<Formula, String> {
    let (owner, tap) = tap_target
        .strip_prefix("brew:")
        .and_then(|t| t.split_once('/'))
        .ok_or("a Homebrew target names its tap as brew:<owner>/<tap>")?;
    if project.is_empty()
        || !project
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!(
            "{project} is not a formula name (lower-case letters, digits and -)"
        ));
    }
    if entries.is_empty() {
        return Err("a formula needs at least one platform binary".into());
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
    let mut stanzas = String::new();
    for entry in entries {
        let (os, cpu) = match entry.platform.as_str() {
            "macos-aarch64" => ("on_macos", "arm?"),
            "linux-x86_64" => ("on_linux", "intel?"),
            "linux-aarch64" => ("on_linux", "arm?"),
            other => return Err(format!("{other} has no Homebrew formula stanza")),
        };
        stanzas.push_str(&format!(
            "  {os} do\n    if Hardware::CPU.{cpu}\n      url \"{}\"\n      sha256 \"{}\"\n    end\n  end\n",
            entry.url, entry.sha256
        ));
    }
    let content = format!(
        "class {class} < Formula\n  version \"{version}\"\n\n{stanzas}\n  def install\n    \
         bin.install Dir[\"*\"].first => \"{project}\"\n  end\nend\n"
    );
    Ok(Formula {
        owner: owner.to_string(),
        tap: tap.to_string(),
        project: project.to_string(),
        page: format!("https://github.com/{owner}/homebrew-{tap}/blob/main/Formula/{project}.rb"),
        content,
        message: format!("{project} {version}"),
    })
}

/// The commit of a formula to the tap: a `PUT` of its content, with the sha of the file it replaces (none for a new
/// file: the contents API refuses an update without it).
pub fn contents_put(formula: &Formula, existing_sha: Option<&str>) -> Result<Request, String> {
    let mut doc = json!({
        "message": formula.message,
        "content": base64::engine::general_purpose::STANDARD.encode(&formula.content),
    });
    if let Some(sha) = existing_sha {
        doc["sha"] = json!(sha);
    }
    Ok(Request::new(
        "PUT",
        formula.contents_url(),
        &[
            ("accept", "application/vnd.github+json"),
            ("content-type", "application/json"),
        ],
        serde_json::to_vec(&doc).map_err(|e| e.to_string())?,
    ))
}

/// The read of a formula's file on the contents API, for its sha.
pub fn contents_get(formula: &Formula) -> Request {
    Request::new(
        "GET",
        formula.contents_url(),
        &[("accept", "application/vnd.github+json")],
        Vec::new(),
    )
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
    use crate::releases::registries::tests::{form_file, tgz, zip_entries, zipped};

    fn artifact<'a>(target: &'a str, version: &'a str, bytes: &'a [u8]) -> Artifact<'a> {
        Artifact {
            target,
            project: "app",
            version,
            bytes,
            sha256: sha256_hex(bytes),
            repository: None,
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
    fn a_jar_goes_to_the_central_portal_as_a_bundle_with_its_checksums() {
        let jar = zipped(&[(
            "META-INF/maven/com.acme/app/pom.properties",
            "groupId=com.acme\nartifactId=app\nversion=0.1.0\n",
        )]);
        let plan = maven(&artifact("gradle:app", "0.1.0", &jar)).unwrap();
        let [upload] = &plan.requests[..] else {
            panic!("one upload: {plan:?}");
        };
        assert_eq!(upload.method, "POST");
        assert_eq!(
            upload.url,
            "https://central.sonatype.com/api/v1/publisher/upload?publishingType=USER_MANAGED"
        );
        let bundle = form_file(&upload.body, "bundle").expect("a bundle part");
        let files = zip_entries(&bundle);
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "com/acme/app/0.1.0/app-0.1.0.jar",
                "com/acme/app/0.1.0/app-0.1.0.jar.md5",
                "com/acme/app/0.1.0/app-0.1.0.jar.sha1",
                "com/acme/app/0.1.0/app-0.1.0.pom",
                "com/acme/app/0.1.0/app-0.1.0.pom.md5",
                "com/acme/app/0.1.0/app-0.1.0.pom.sha1",
            ]
        );
        // Each checksum file holds the hex digest of its file.
        use md5::Digest as _;
        let entry = |name: &str| &files.iter().find(|(n, _)| n == name).unwrap().1;
        let jar_bytes = entry("com/acme/app/0.1.0/app-0.1.0.jar");
        assert_eq!(
            entry("com/acme/app/0.1.0/app-0.1.0.jar.md5"),
            weft::hash::hex(&md5::Md5::digest(jar_bytes)).as_bytes()
        );
        assert_eq!(
            entry("com/acme/app/0.1.0/app-0.1.0.jar.sha1"),
            weft::hash::hex(&sha1::Sha1::digest(jar_bytes)).as_bytes()
        );
        assert!(
            String::from_utf8_lossy(entry("com/acme/app/0.1.0/app-0.1.0.pom"))
                .contains("<artifactId>app</artifactId>")
        );
    }

    #[test]
    fn a_jar_goes_to_the_owners_maven_repository_in_the_maven2_layout_when_one_is_set() {
        let jar = zipped(&[(
            "META-INF/maven/com.acme/app/pom.properties",
            "groupId=com.acme\nartifactId=app\nversion=0.1.0\n",
        )]);
        let mut a = artifact("gradle:app", "0.1.0", &jar);
        a.repository = Some("https://repo.example/maven/");
        let plan = maven(&a).unwrap();
        let urls: Vec<&str> = plan.requests.iter().map(|r| r.url.as_str()).collect();
        let dir = "https://repo.example/maven/com/acme/app/0.1.0/app-0.1.0";
        assert_eq!(
            urls,
            [
                format!("{dir}.jar"),
                format!("{dir}.jar.md5"),
                format!("{dir}.jar.sha1"),
                format!("{dir}.pom"),
                format!("{dir}.pom.md5"),
                format!("{dir}.pom.sha1"),
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
    fn a_formula_names_each_platform_binary_and_is_written_to_the_tap_with_its_sha() {
        let entries = [
            Entry {
                platform: "macos-aarch64".into(),
                url: "https://hub.example/a/content".into(),
                sha256: "aa".into(),
            },
            Entry {
                platform: "linux-x86_64".into(),
                url: "https://hub.example/b/content".into(),
                sha256: "bb".into(),
            },
        ];
        let f = formula("app", "0.1.0", "brew:acme/tools", &entries).unwrap();
        assert_eq!(
            f.page,
            "https://github.com/acme/homebrew-tools/blob/main/Formula/app.rb"
        );
        assert_eq!(f.message, "app 0.1.0");
        assert!(
            f.content
                .starts_with("class App < Formula\n  version \"0.1.0\"\n")
        );
        assert!(f.content.contains(
            "on_macos do\n    if Hardware::CPU.arm?\n      url \"https://hub.example/a/content\"\n      sha256 \"aa\""
        ));
        assert!(f.content.contains(
            "on_linux do\n    if Hardware::CPU.intel?\n      url \"https://hub.example/b/content\"\n      sha256 \"bb\""
        ));
        // A new file is written without a sha; an existing one carries the sha it replaces.
        let new = contents_put(&f, None).unwrap();
        assert_eq!(new.method, "PUT");
        assert_eq!(
            new.url,
            "https://api.github.com/repos/acme/homebrew-tools/contents/Formula/app.rb"
        );
        let doc: Value = serde_json::from_slice(&new.body).unwrap();
        assert!(doc.get("sha").is_none());
        let put = contents_put(&f, Some("old-sha")).unwrap();
        let doc: Value = serde_json::from_slice(&put.body).unwrap();
        assert_eq!(doc["sha"], "old-sha");
        let written = base64::engine::general_purpose::STANDARD
            .decode(doc["content"].as_str().unwrap())
            .unwrap();
        assert_eq!(written, f.content.as_bytes());
        assert_eq!(contents_get(&f).method, "GET");
    }

    #[test]
    fn a_formula_refuses_a_platform_it_has_no_stanza_for_and_a_tap_without_an_owner() {
        let windows = [Entry {
            platform: "windows-x86_64".into(),
            url: "https://hub.example/a/content".into(),
            sha256: "aa".into(),
        }];
        assert!(formula("app", "0.1.0", "brew:acme/tools", &windows).is_err());
        assert!(formula("app", "0.1.0", "acme/tools", &windows).is_err());
        assert!(formula("app", "0.1.0", "brew:acme/tools", &[]).is_err());
    }

    #[test]
    fn a_planner_for_an_unknown_connector_is_refused() {
        assert!(plan("myspace", &artifact("cargo:app", "0.1.0", b"x")).is_err());
    }
}
