//! The publish step against fake registries (PLAN 22.4, 9.5). Each registry's publish endpoints are a fake HTTP server on
//! 127.0.0.1 that checks what it is sent and records it with the credential it was given; the transport sends a
//! registry's host to its fake and nowhere else. Credentials are a real vault in a temp directory, bound as the owner's
//! registry secrets are. The real local registries (Verdaccio, devpi, a Maven repository manager) are not run.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine;
use secrecy::SecretString;
use serde_json::{Value, json};

use super::{Step, is_registry, platform_of, publish_release};
use crate::glue::{HttpsOutbound, Outbound, Outgoing, Reply};
use crate::releases::entity::{Release, ReleaseArtifact, ReleaseState};
use crate::releases::flow::{self, Builder, Built, Outcome};
use crate::releases::signing::sha256_hex;
use crate::releases::tests::{World, cut, lab_with};
use crate::vault::{Consumer, NewSecret, OnBehalf, Scope, Store as Vault};

/// The cargo package's manifest: a crate, declared as a package (`any`) and not as binaries.
const CARGO_ANY: &str = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[package.metadata.weft]\ntargets = [\"any\"]\nhomebrew = \"acme/tools\"\n";

/// The test's files. Weft declares a target for each (`<name>@<platform>`); only the Cargo crate, the npm package, the
/// Python project and the NuGet package are built for `any`. The rest are platform builds, not packages.
const FILES: [(&str, &str); 8] = [
    ("Cargo.toml", CARGO_ANY),
    (
        "Acme.Lib.csproj",
        "<Project><PropertyGroup><PackageId>Acme.Lib</PackageId></PropertyGroup></Project>",
    ),
    (
        "package.json",
        r#"{"name":"@acme/web","version":"0.1.0","scripts":{"build":"tsc"}}"#,
    ),
    (
        "pyproject.toml",
        "[project]\nname = \"acme\"\nversion = \"0.1.0\"\n",
    ),
    ("Dockerfile", "FROM scratch\n"),
    ("go.mod", "module example.com/acme\n\ngo 1.22\n"),
    ("main.go", "package main\n\nfunc main() {}\n"),
    ("build.gradle", "plugins { id 'com.android.application' }\n"),
];

/// A gzipped tar of one file.
pub(super) fn tgz(path: &str, text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let encoder = flate2::write::GzEncoder::new(&mut out, flate2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(text.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, path, text.as_bytes())
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();
    }
    out
}

/// A zip of the given files.
pub(super) fn zipped(files: &[(&str, &str)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, text) in files {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(text.as_bytes()).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

/// The package each target builds, at `version`: what its ecosystem publishes.
fn package_of(target: &str, version: &str) -> Vec<u8> {
    let ecosystem = target.split_once(':').map_or("", |(e, _)| e);
    match ecosystem {
        "cargo" => tgz(
            "app-0.1.0/Cargo.toml",
            &format!("[package]\nname = \"app\"\nversion = \"{version}\"\n"),
        ),
        "npm" => tgz(
            "package/package.json",
            &format!(r#"{{"name":"@acme/web","version":"{version}"}}"#),
        ),
        "python" => {
            let meta = format!("Metadata-Version: 2.1\nName: acme\nVersion: {version}\n");
            zipped(&[("acme-0.1.0.dist-info/METADATA", meta.as_str())])
        }
        "gradle" => {
            let props = format!("groupId=com.acme\nartifactId=app\nversion={version}\n");
            zipped(&[("META-INF/maven/com.acme/app/pom.properties", props.as_str())])
        }
        "nuget" => {
            let nuspec = format!(
                "<package><metadata><id>Acme.Lib</id><version>{version}</version></metadata></package>"
            );
            zipped(&[("Acme.Lib.nuspec", nuspec.as_str())])
        }
        "image" => format!("the image's layer for {version}").into_bytes(),
        _ => format!("{ecosystem} at {version}").into_bytes(),
    }
}

/// Builds every target as its package.
struct Packages;

impl Builder for Packages {
    fn build(&self, release: &Release, target: &str) -> Result<Outcome, String> {
        Ok(Outcome::Built(Built {
            bytes: package_of(target, &release.version),
            pool: "fake-pool".into(),
            base: "fake-base".into(),
            builder: "fake-builder".into(),
            command: "package".into(),
        }))
    }
}

/// One request a fake registry received, as it arrived.
#[derive(Clone, Debug)]
pub(crate) struct Seen {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) headers: BTreeMap<String, String>,
    pub(crate) body: Vec<u8>,
}

impl Seen {
    fn authorization(&self) -> Option<&str> {
        self.headers.get("authorization").map(String::as_str)
    }
}

pub(crate) type Log = Arc<Mutex<Vec<Seen>>>;

/// Reads one request: its head, then its body by `content-length`.
fn read_request(stream: &mut TcpStream) -> Option<Seen> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).ok()?;
        head.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&head).into_owned();
    let mut lines = text.lines();
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let path = first.next()?.to_string();
    let headers: BTreeMap<String, String> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    stream.read_exact(&mut body).ok()?;
    Some(Seen {
        method,
        path,
        headers,
        body,
    })
}

/// A fake registry on 127.0.0.1: each request is recorded, and `answer` says its status and body.
pub(crate) fn fake(answer: impl Fn(&Seen) -> (u16, String) + Send + Sync + 'static) -> (u16, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&log);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let Some(seen) = read_request(&mut stream) else {
                continue;
            };
            let (status, body) = answer(&seen);
            kept.lock().unwrap().push(seen);
            let reply = format!(
                "HTTP/1.1 {status} Answer\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    (port, log)
}

/// The transport of a test: a request to a registry's host goes to that registry's fake on 127.0.0.1, with the same
/// path, headers and body. Every request is kept as the mediator sent it; a host with no fake is an error.
pub(crate) struct Redirect {
    hosts: Vec<(&'static str, u16)>,
    sent: Mutex<Vec<Outgoing>>,
}

impl Redirect {
    pub(crate) fn new(hosts: Vec<(&'static str, u16)>) -> Redirect {
        Redirect {
            hosts,
            sent: Mutex::new(Vec::new()),
        }
    }
}

impl Outbound for Redirect {
    fn send(&self, request: &Outgoing) -> Result<Reply, String> {
        self.sent.lock().unwrap().push(request.clone());
        let rest = request
            .url
            .strip_prefix("https://")
            .ok_or("only https reaches a registry")?;
        let (host, path) = match rest.split_once('/') {
            Some((host, path)) => (host, format!("/{path}")),
            None => (rest, "/".to_string()),
        };
        let port = self
            .hosts
            .iter()
            .find(|(h, _)| *h == host)
            .map(|(_, p)| *p)
            .ok_or_else(|| format!("no fake registry answers for {host}"))?;
        HttpsOutbound.send(&Outgoing {
            url: format!("http://127.0.0.1:{port}{path}"),
            ..request.clone()
        })
    }
}

/// A vault of its own in a temp directory, booted, with each secret bound to its binding.
pub(crate) fn test_vault(name: &str, secrets: &[(&str, &str, &str)]) -> Vault {
    let dir =
        std::env::temp_dir().join(format!("apid-registry-vault-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("config")).unwrap();
    let vault = Vault::open_in(&dir, &dir.join("config").join("vault.key"));
    vault.boot(apz_plaza::now_ms()).unwrap();
    for (secret, binding, value) in secrets {
        add(&vault, secret, binding, value);
    }
    vault
}

pub(crate) fn add(vault: &Vault, name: &str, binding: &str, value: &str) {
    vault
        .create(
            NewSecret {
                name: name.to_string(),
                kind: "token".into(),
                label: name.to_string(),
                value: SecretString::from(value.to_string()),
                scope: Scope::Plaza,
                bindings: vec![binding.to_string()],
            },
            "owner",
            apz_plaza::now_ms(),
        )
        .unwrap();
}

/// The crates.io index path of a crate name (the index's layout for its name length).
fn index_path(name: &str) -> String {
    match name.len() {
        1 => format!("1/{name}"),
        2 => format!("2/{name}"),
        3 => format!("3/{}/{name}", &name[..1]),
        _ => format!("{}/{}/{name}", &name[..2], &name[2..4]),
    }
}

/// A fresh directory for a fake crates.io's index.
fn index_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("apid-registry-index-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The lines a crate's index file holds, parsed.
fn index_lines(index: &Path, name: &str) -> Vec<Value> {
    std::fs::read_to_string(index.join(index_path(name)))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// A fake crates.io: `PUT /api/v1/crates/new` reads the length-prefixed metadata and `.crate`, and writes the version's
/// line into a file-based index at `index`, as crates.io updates its index on publish.
fn fake_crates(index: PathBuf) -> (u16, Log) {
    fake(move |seen| {
        if seen.method != "PUT" || seen.path != "/api/v1/crates/new" {
            return (404, "{}".into());
        }
        let body = &seen.body;
        let meta_len = u32::from_le_bytes(body[..4].try_into().unwrap()) as usize;
        let meta: Value = serde_json::from_slice(&body[4..4 + meta_len]).unwrap();
        let rest = &body[4 + meta_len..];
        let crate_len = u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize;
        let krate = &rest[4..4 + crate_len];
        let name = meta["name"].as_str().unwrap().to_string();
        let line = json!({
            "name": name,
            "vers": meta["vers"],
            "deps": [],
            "cksum": sha256_hex(krate),
            "features": {},
            "yanked": false,
        });
        let file = index.join(index_path(&name));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let mut text = std::fs::read_to_string(&file).unwrap_or_default();
        text.push_str(&format!("{line}\n"));
        std::fs::write(&file, text).unwrap();
        (200, json!({ "warnings": {} }).to_string())
    })
}

/// The fake npm: `PUT /{name}` with the packument; the attached tarball must be the package. With `first`, the first
/// request is answered with that status and is not recorded as a publish.
fn fake_npm(first: Option<u16>) -> (u16, Log) {
    let calls = AtomicUsize::new(0);
    fake(move |seen| {
        let n = calls.fetch_add(1, Ordering::SeqCst);
        if let Some(status) = first.filter(|_| n == 0) {
            return (status, "{}".into());
        }
        if seen.method != "PUT" || seen.path != "/@acme%2Fweb" {
            return (404, "{}".into());
        }
        let doc: Value = serde_json::from_slice(&seen.body).unwrap();
        let version = doc["dist-tags"]["latest"].as_str().unwrap();
        if !doc["versions"][version]["dist"]["tarball"].is_string() {
            return (400, "no tarball for the version".into());
        }
        let attachment = doc["_attachments"]["acme-web-0.1.0.tgz"]["data"]
            .as_str()
            .unwrap();
        let tarball = base64::engine::general_purpose::STANDARD
            .decode(attachment)
            .unwrap();
        if tarball != package_of("npm:@acme/web", "0.1.0") {
            return (400, "the attachment is not the package".into());
        }
        (200, json!({ "success": true }).to_string())
    })
}

/// The fake PyPI: the legacy upload form, with the wheel's name and its file type in the form.
fn fake_pypi() -> (u16, Log) {
    fake(|seen| {
        let form = String::from_utf8_lossy(&seen.body);
        let ok = seen.method == "POST"
            && seen.path == "/legacy/"
            && form.contains("name=\"name\"\r\n\r\nacme")
            && form.contains("name=\"filetype\"\r\n\r\nbdist_wheel");
        if ok {
            (200, "".into())
        } else {
            (400, "bad upload".into())
        }
    })
}

/// The fake NuGet push: `PUT /api/v2/package` with the nupkg as the `package` part, keyed by `X-NuGet-ApiKey`.
fn fake_nuget() -> (u16, Log) {
    fake(|seen| {
        let form = String::from_utf8_lossy(&seen.body);
        let key = seen.headers.get("x-nuget-apikey").map(String::as_str) == Some("nuget-key");
        let ok = seen.method == "PUT"
            && seen.path == "/api/v2/package"
            && key
            && form.contains("name=\"package\"; filename=\"Acme.Lib.0.1.0.nupkg\"");
        if ok {
            (201, "".into())
        } else {
            (400, "".into())
        }
    })
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The bytes of a multipart form's file part named `field`, split at the form's boundary (its first line).
pub(super) fn form_file(body: &[u8], field: &str) -> Option<Vec<u8>> {
    let first_line = find(body, b"\r\n")?;
    let delimiter = &body[..first_line];
    let marker = format!("name=\"{field}\"");
    let mut rest = &body[first_line + 2..];
    loop {
        let next = find(rest, delimiter)?;
        let part = &rest[..next];
        if let Some(head_end) = find(part, b"\r\n\r\n")
            .filter(|&end| String::from_utf8_lossy(&part[..end]).contains(&marker))
        {
            let data = &part[head_end + 4..];
            return Some(data.strip_suffix(b"\r\n").unwrap_or(data).to_vec());
        }
        rest = &rest[next + delimiter.len()..];
        if rest.starts_with(b"--") {
            return None;
        }
    }
}

/// The entries of a zip, by name, with their bytes.
pub(super) fn zip_entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    (0..archive.len())
        .map(|i| {
            let mut file = archive.by_index(i).unwrap();
            let name = file.name().unwrap().to_string();
            let mut data = Vec::new();
            file.read_to_end(&mut data).unwrap();
            (name, data)
        })
        .collect()
}

/// The fake Central Portal: `POST /api/v1/publisher/upload` takes a bundle, and the fake checks it is the jar and the pom
/// of the release, each with its `.md5` and `.sha1` digest, and answers with a deployment id as the Portal does.
fn fake_central() -> (u16, Log) {
    fake(|seen| {
        if seen.method != "POST" || !seen.path.starts_with("/api/v1/publisher/upload") {
            return (404, "".into());
        }
        let Some(bundle) = form_file(&seen.body, "bundle") else {
            return (400, "no bundle".into());
        };
        if !bundle_is_whole(&zip_entries(&bundle)) {
            return (
                400,
                "the bundle is not a jar and a pom with their checksums".into(),
            );
        }
        (201, "deployment-1".into())
    })
}

/// A bundle is six files: a jar and a pom under one path, each with a `.md5` and a `.sha1` that are its digests.
fn bundle_is_whole(files: &[(String, Vec<u8>)]) -> bool {
    use md5::Digest as _;
    let get = |name: &str| {
        files
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, b)| b.as_slice())
    };
    files.len() == 6
        && [
            "com/acme/app/0.1.0/app-0.1.0.jar",
            "com/acme/app/0.1.0/app-0.1.0.pom",
        ]
        .iter()
        .all(|file| {
            let Some(bytes) = get(file) else {
                return false;
            };
            let md5 = weft::hash::hex(&md5::Md5::digest(bytes));
            let sha1 = weft::hash::hex(&sha1::Sha1::digest(bytes));
            get(&format!("{file}.md5")) == Some(md5.as_bytes())
                && get(&format!("{file}.sha1")) == Some(sha1.as_bytes())
        })
}

/// The fake tap: the contents API for `Formula/app.rb`. The file is `existing` (its sha) or absent, and a write must carry
/// the sha of the file it replaces, as the contents API requires.
fn fake_tap(existing: Option<&'static str>) -> (u16, Log) {
    fake(move |seen| {
        if seen.path != "/repos/acme/homebrew-tools/contents/Formula/app.rb" {
            return (404, "{}".into());
        }
        match (seen.method.as_str(), existing) {
            ("GET", Some(sha)) => (200, json!({ "sha": sha }).to_string()),
            ("GET", None) => (404, json!({ "message": "Not Found" }).to_string()),
            ("PUT", _) => {
                let doc: Value = serde_json::from_slice(&seen.body).unwrap();
                if doc.get("sha").and_then(Value::as_str) != existing {
                    return (422, json!({ "message": "sha does not match" }).to_string());
                }
                (201, json!({ "content": { "sha": "new" } }).to_string())
            }
            _ => (405, "".into()),
        }
    })
}

/// The fake OCI registry: monolithic blob uploads (the digest checked against the body) and the manifest, which must
/// name only blobs already uploaded.
fn fake_oci() -> (u16, Log) {
    let uploaded: Mutex<Vec<String>> = Mutex::new(Vec::new());
    fake(move |seen| {
        if seen.method == "POST" && seen.path.starts_with("/v2/app/blobs/uploads/") {
            let digest = seen
                .path
                .split_once("digest=sha256:")
                .map(|(_, d)| d.to_string())
                .unwrap_or_default();
            if sha256_hex(&seen.body) != digest {
                return (400, "the digest is not the body's".into());
            }
            uploaded.lock().unwrap().push(digest);
            return (201, "".into());
        }
        if seen.method == "PUT" && seen.path == "/v2/app/manifests/0.1.0" {
            let manifest: Value = serde_json::from_slice(&seen.body).unwrap();
            let have = uploaded.lock().unwrap();
            let config = manifest["config"]["digest"]
                .as_str()
                .unwrap()
                .trim_start_matches("sha256:");
            let layer = manifest["layers"][0]["digest"]
                .as_str()
                .unwrap()
                .trim_start_matches("sha256:");
            if have.iter().any(|d| d == config) && have.iter().any(|d| d == layer) {
                return (201, "".into());
            }
            return (400, "a blob of the manifest is not uploaded".into());
        }
        (404, "".into())
    })
}

/// The fake Go module proxy: a version's `.info`, which it has once the module's tag is there.
fn fake_go() -> (u16, Log) {
    fake(|seen| {
        if seen.method == "GET" && seen.path == "/example.com/acme/@v/v0.1.0.info" {
            (200, json!({ "Version": "v0.1.0" }).to_string())
        } else {
            (404, "not found".into())
        }
    })
}

fn b64(text: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(text)
}

/// A released library: a candidate of every target, checked, in a Weft of its own. `_handle` keeps that Weft served.
struct Library {
    world: World,
    id: String,
    _handle: apz_http::Handle,
}

fn library(name: &str) -> Library {
    let (client, handle, _dir, _change, _agent) = lab_with(name, &FILES);
    let world = World::new(client, name);
    let d = world.deps();
    let drafted = flow::draft(&d, "app", "app", &cut("0.1.0")).unwrap();
    let candidate = flow::candidate(&d, &drafted.id, &Packages, Some(1)).unwrap();
    let mut targets: Vec<&str> = candidate
        .artifacts
        .iter()
        .map(|a| a.target.as_str())
        .collect();
    targets.sort_unstable();
    assert_eq!(
        targets,
        [
            "brew:acme/tools@linux-aarch64",
            "brew:acme/tools@linux-x86_64",
            "brew:acme/tools@macos-aarch64",
            "cargo:app@any",
            "go:example.com/acme@linux-aarch64",
            "go:example.com/acme@linux-x86_64",
            "go:example.com/acme@macos-aarch64",
            "go:example.com/acme@windows-x86_64",
            "gradle:app@android-aarch64",
            "gradle:app@android-x86_64",
            "image:.@linux-aarch64",
            "image:.@linux-x86_64",
            "npm:@acme/web@any",
            "nuget:Acme.Lib@any",
            "python:acme@any",
        ]
    );
    flow::checks(&d, &drafted.id, true, "").unwrap();
    Library {
        world,
        id: drafted.id,
        _handle: handle,
    }
}

/// A published release by hand: one artifact per `(target, bytes)`, with the digests `d0`, `d1`, …, and the bytes the
/// publish step reads by digest.
fn release_by_hand(
    project: &str,
    version: &str,
    built: &[(&str, Vec<u8>)],
) -> (Release, Vec<(String, Vec<u8>)>) {
    let artifacts: Vec<ReleaseArtifact> = built
        .iter()
        .enumerate()
        .map(|(i, (target, bytes))| ReleaseArtifact {
            target: target.to_string(),
            artifact: format!("a{i}"),
            digest: format!("d{i}"),
            size: bytes.len() as u64,
            signatures: String::new(),
            sbom: String::new(),
            provenance: String::new(),
            pool: String::new(),
            base: String::new(),
            run: String::new(),
        })
        .collect();
    let blobs = built
        .iter()
        .enumerate()
        .map(|(i, (_, bytes))| (format!("d{i}"), bytes.clone()))
        .collect();
    let rel = Release {
        id: "rel-1".into(),
        project: project.into(),
        repo: project.into(),
        version: version.into(),
        channel: "stable".into(),
        base: None,
        lane: "main".into(),
        from: "rev".into(),
        state: ReleaseState::Published,
        number: None,
        notes: String::new(),
        run: None,
        changes: vec![],
        artifacts,
        not_run: vec![],
        verdicts: vec![],
        promotions: vec![],
        sync: vec![],
        store_steps: vec![],
        compatibility: vec![],
        created_ms: 1,
        updated_ms: 1,
    };
    (rel, blobs)
}

fn rows_of(world: &World, id: &str) -> Vec<(String, String)> {
    let rel = world.releases.get(id).unwrap();
    rel.sync
        .iter()
        .filter(|s| is_registry(&s.platform))
        .map(|s| (s.platform.clone(), s.state.clone()))
        .collect()
}

#[test]
fn a_library_release_publishes_to_each_local_registry_from_the_publish_step() {
    let index = index_dir("library");
    let (crates, crates_log) = fake_crates(index.clone());
    let (npm, npm_log) = fake_npm(None);
    let (pypi, pypi_log) = fake_pypi();
    let (nuget, nuget_log) = fake_nuget();
    let (github, github_log) = fake_tap(None);
    let redirect = Redirect::new(vec![
        ("crates.io", crates),
        ("registry.npmjs.org", npm),
        ("upload.pypi.org", pypi),
        ("www.nuget.org", nuget),
        ("api.github.com", github),
    ]);
    let vault = test_vault(
        "library",
        &[
            ("CRATES_IO_TOKEN", "connector:crates-io", "crates-token"),
            ("NPM_TOKEN", "connector:npm", "npm-token"),
            ("PYPI_TOKEN", "connector:pypi", "pypi-token"),
            ("NUGET_API_KEY", "connector:nuget", "nuget-key"),
            ("HOMEBREW_TAP_TOKEN", "connector:homebrew-tap", "tap-token"),
        ],
    );
    let lib = library("library-publish");
    let d = lib.world.deps();
    // Before the publish step, nothing has reached a registry.
    assert!(redirect.sent.lock().unwrap().is_empty());
    let step = Step {
        outbound: &redirect,
        vault: &vault,
        link_base: Some("https://hub.example/v1/weft/artifacts"),
        at_ms: 1,
    };
    let published = flow::publish_all(&d, &lib.id, &step).unwrap();
    assert_eq!(published.state, ReleaseState::Published);

    // Four artifacts are packages; the eight platform builds are not sent anywhere.
    let rows = rows_of(&lib.world, &lib.id);
    assert_eq!(rows.len(), 5, "{rows:?}");
    for (connector, target) in [
        ("crates-io", "cargo:app@any"),
        ("npm", "npm:@acme/web@any"),
        ("pypi", "python:acme@any"),
        ("nuget", "nuget:Acme.Lib@any"),
        ("homebrew-tap", "brew:acme/tools"),
    ] {
        let want = (platform_of(connector, target), "published".to_string());
        assert!(rows.contains(&want), "{want:?} is not among {rows:?}");
    }

    // Each fake received its own publish, with the credential its registry takes.
    let crate_seen = crates_log.lock().unwrap();
    assert_eq!(crate_seen.len(), 1);
    assert_eq!(crate_seen[0].authorization(), Some("crates-token"));
    let index_entry = index_lines(&index, "app");
    assert_eq!(
        index_entry.len(),
        1,
        "the file-based index holds the version"
    );
    assert_eq!(index_entry[0]["vers"], "0.1.0");
    assert_eq!(
        index_entry[0]["cksum"],
        sha256_hex(&package_of("cargo:app", "0.1.0"))
    );

    let npm_seen = npm_log.lock().unwrap();
    assert_eq!(npm_seen.len(), 1);
    assert_eq!(npm_seen[0].authorization(), Some("Bearer npm-token"));
    let pypi_seen = pypi_log.lock().unwrap();
    assert_eq!(
        pypi_seen[0].authorization(),
        Some(format!("Basic {}", b64("__token__:pypi-token")).as_str())
    );
    let nuget_seen = nuget_log.lock().unwrap();
    assert_eq!(
        nuget_seen[0]
            .headers
            .get("x-nuget-apikey")
            .map(String::as_str),
        Some("nuget-key")
    );
    assert_eq!(
        nuget_seen[0].authorization(),
        None,
        "NuGet takes its key in its own header"
    );

    // The formula is one commit to the tap, with each macOS and Linux binary's url and digest.
    let github_seen = github_log.lock().unwrap();
    let put = github_seen
        .iter()
        .rev()
        .find(|s| s.method == "PUT")
        .expect("a formula commit");
    assert_eq!(put.authorization(), Some("Bearer tap-token"));
    let doc: Value = serde_json::from_slice(&put.body).unwrap();
    assert!(
        doc.get("sha").is_none(),
        "the tap had no formula to replace"
    );
    let content = String::from_utf8(
        base64::engine::general_purpose::STANDARD
            .decode(doc["content"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    assert!(content.contains("version \"0.1.0\""), "{content}");
    let brew: Vec<_> = published
        .artifacts
        .iter()
        .filter(|a| a.target.starts_with("brew:"))
        .collect();
    assert_eq!(brew.len(), 3, "one binary per macOS and Linux platform");
    for art in &brew {
        assert!(
            content.contains(&format!(
                "url \"https://hub.example/v1/weft/artifacts/{}/content\"",
                art.artifact
            )),
            "{content}"
        );
    }
    let digest = sha256_hex(&package_of("brew:acme/tools", "0.1.0"));
    assert_eq!(content.matches(&digest).count(), 3, "{content}");
    drop(github_seen);

    // Every request went to the host its manifest names, and no other.
    let sent_count = {
        let sent = redirect.sent.lock().unwrap();
        for out in sent.iter() {
            let host = out
                .url
                .trim_start_matches("https://")
                .split('/')
                .next()
                .unwrap();
            assert!(
                [
                    "crates.io",
                    "registry.npmjs.org",
                    "upload.pypi.org",
                    "www.nuget.org",
                    "api.github.com"
                ]
                .contains(&host),
                "{host}"
            );
        }
        sent.len()
    };

    // A published row is not sent again: publishing the release again sends nothing new.
    flow::publish_all(&d, &lib.id, &step).unwrap();
    assert_eq!(redirect.sent.lock().unwrap().len(), sent_count);
    let _ = std::fs::remove_dir_all(&index);
}

#[test]
fn each_registry_publish_endpoint_takes_its_package_with_its_credential() {
    let (maven, maven_log) = fake_central();
    let (oci, oci_log) = fake_oci();
    let (go, go_log) = fake_go();
    let redirect = Redirect::new(vec![
        ("central.sonatype.com", maven),
        ("ghcr.io", oci),
        ("proxy.golang.org", go),
    ]);
    let vault = test_vault(
        "endpoints",
        &[
            (
                "MAVEN_CREDENTIALS",
                "connector:maven-central",
                "deployer:maven-pass",
            ),
            ("OCI_TOKEN", "connector:oci-registry", "oci-token"),
        ],
    );
    let jar = zipped(&[(
        "META-INF/maven/com.acme/app/pom.properties",
        "groupId=com.acme\nartifactId=app\nversion=0.1.0\n",
    )]);
    let (rel, blobs) = release_by_hand(
        "app",
        "0.1.0",
        &[
            ("gradle:app@any", jar),
            ("image:.@any", b"the image's layer".to_vec()),
            ("go:example.com/acme@any", b"the module".to_vec()),
        ],
    );
    let step = Step {
        outbound: &redirect,
        vault: &vault,
        link_base: None,
        at_ms: 1,
    };
    let rows = publish_release(&step, &rel, &|digest: &str| {
        blobs
            .iter()
            .find(|(d, _)| d == digest)
            .map(|(_, bytes)| bytes.clone())
            .ok_or_else(|| format!("no blob {digest}"))
    });
    let states: Vec<(&str, &str)> = rows
        .iter()
        .map(|r| (r.platform.as_str(), r.state.as_str()))
        .collect();
    assert_eq!(
        states,
        [
            ("maven-central/gradle:app@any", "published"),
            ("oci-registry/image:.@any", "published"),
            ("go-proxy/go:example.com/acme@any", "published"),
        ],
        "{states:?}"
    );

    // Maven: the jar and its pom, with the credential as basic auth.
    let maven_seen = maven_log.lock().unwrap();
    assert_eq!(maven_seen.len(), 1, "one bundle to the Portal");
    assert_eq!(
        zip_entries(&form_file(&maven_seen[0].body, "bundle").unwrap()).len(),
        6
    );
    assert_eq!(
        maven_seen[0].authorization(),
        Some(format!("Bearer {}", b64("deployer:maven-pass")).as_str())
    );
    // OCI: two blobs, then the manifest that names them, with the bearer token.
    let oci_seen = oci_log.lock().unwrap();
    assert_eq!(oci_seen.len(), 3, "two blobs and the manifest");
    assert_eq!(oci_seen[2].authorization(), Some("Bearer oci-token"));
    // The module proxy: the version's info is asked for; no credential is sent.
    let go_seen = go_log.lock().unwrap();
    assert_eq!(go_seen.len(), 1);
    assert_eq!(go_seen[0].authorization(), None);
}

#[test]
fn a_registry_secret_is_read_only_by_the_binding_of_its_registry() {
    let vault = test_vault(
        "bindings",
        &[("CRATES_IO_TOKEN", "connector:crates-io", "crates-token")],
    );
    let read = |binding: &str| {
        vault.resolve(
            "CRATES_IO_TOKEN",
            &Consumer {
                binding: binding.to_string(),
                on_behalf: OnBehalf::Hub,
            },
            apz_plaza::now_ms(),
        )
    };
    assert!(read("connector:crates-io").is_ok());
    // The release's other steps, and the other connectors, are refused.
    for binding in [
        "release:candidate",
        "release:verify",
        "release:sync",
        "connector:github",
        "connector:npm",
    ] {
        assert!(
            read(binding).is_err(),
            "{binding} reached the registry secret"
        );
    }
    // The publish step's credential source gives the value to its own binding, and nothing to another one.
    use super::Credentials;
    assert_eq!(
        vault
            .secret("CRATES_IO_TOKEN", "connector:crates-io")
            .unwrap()
            .as_deref(),
        Some("crates-token")
    );
    assert_eq!(
        vault
            .secret("CRATES_IO_TOKEN", "release:candidate")
            .unwrap(),
        None
    );
}

#[test]
fn a_release_is_not_sent_to_any_registry_until_its_publish_step_runs() {
    let index = index_dir("not-yet");
    let (_port, crates_log) = fake_crates(index.clone());
    let lib = library("library-not-yet");
    let d = lib.world.deps();
    // The statement of publication alone is Weft's; no registry is reached by it.
    flow::publish(&d, &lib.id).unwrap();
    assert_eq!(
        lib.world.releases.get(&lib.id).unwrap().state,
        ReleaseState::Published
    );
    assert!(rows_of(&lib.world, &lib.id).is_empty());
    assert!(crates_log.lock().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&index);
}

#[test]
fn a_registry_without_its_secret_is_skipped_then_sent_once_the_secret_is_there() {
    let index = index_dir("skipped");
    let (crates, crates_log) = fake_crates(index.clone());
    let (npm, npm_log) = fake_npm(None);
    let redirect = Redirect::new(vec![("crates.io", crates), ("registry.npmjs.org", npm)]);
    let vault = test_vault(
        "skipped",
        &[("CRATES_IO_TOKEN", "connector:crates-io", "crates-token")],
    );
    let lib = library("library-skipped");
    let d = lib.world.deps();
    let step = Step {
        outbound: &redirect,
        vault: &vault,
        link_base: None,
        at_ms: 1,
    };
    flow::publish_all(&d, &lib.id, &step).unwrap();
    let rows = rows_of(&lib.world, &lib.id);
    assert!(
        rows.contains(&(
            platform_of("npm", "npm:@acme/web@any"),
            "skipped: NPM_TOKEN is not in the vault".into()
        )),
        "{rows:?}"
    );
    assert!(rows.contains(&(
        platform_of("crates-io", "cargo:app@any"),
        "published".into()
    )));
    assert!(
        npm_log.lock().unwrap().is_empty(),
        "no npm request without its secret"
    );

    // The owner adds the secret; publishing again sends the skipped registry and not the published one.
    add(&vault, "NPM_TOKEN", "connector:npm", "npm-token");
    let crates_before = crates_log.lock().unwrap().len();
    flow::publish_all(&d, &lib.id, &step).unwrap();
    let rows = rows_of(&lib.world, &lib.id);
    assert!(
        rows.contains(&(platform_of("npm", "npm:@acme/web@any"), "published".into())),
        "{rows:?}"
    );
    assert_eq!(npm_log.lock().unwrap().len(), 1);
    assert_eq!(crates_log.lock().unwrap().len(), crates_before);
    let _ = std::fs::remove_dir_all(&index);
}

#[test]
fn a_refused_send_is_a_failed_row_and_publishing_again_sends_it() {
    let (npm, npm_log) = fake_npm(Some(500));
    let redirect = Redirect::new(vec![("registry.npmjs.org", npm)]);
    let vault = test_vault("retry", &[("NPM_TOKEN", "connector:npm", "npm-token")]);
    let lib = library("library-retry");
    let d = lib.world.deps();
    let step = Step {
        outbound: &redirect,
        vault: &vault,
        link_base: None,
        at_ms: 1,
    };
    flow::publish_all(&d, &lib.id, &step).unwrap();
    let rows = rows_of(&lib.world, &lib.id);
    let (platform, state) = rows
        .iter()
        .find(|(p, _)| p.starts_with("npm/"))
        .expect("an npm row");
    assert_eq!(platform, &platform_of("npm", "npm:@acme/web@any"));
    assert!(
        state.starts_with("failed: PUT") && state.contains("answered 500"),
        "{state}"
    );

    flow::publish_all(&d, &lib.id, &step).unwrap();
    let rows = rows_of(&lib.world, &lib.id);
    assert!(
        rows.contains(&(platform_of("npm", "npm:@acme/web@any"), "published".into())),
        "{rows:?}"
    );
    assert_eq!(
        npm_log.lock().unwrap().len(),
        2,
        "the refused send and the retry"
    );
}

#[test]
fn a_package_whose_version_is_not_the_releases_is_refused_before_it_is_sent() {
    let (npm, npm_log) = fake_npm(None);
    let redirect = Redirect::new(vec![("registry.npmjs.org", npm)]);
    let vault = test_vault("mismatch", &[("NPM_TOKEN", "connector:npm", "npm-token")]);
    let lib = library("library-mismatch");
    let d = lib.world.deps();
    // The release is recorded at 0.2.0, but its npm package says 0.1.0.
    lib.world
        .releases
        .update(&lib.id, |r| r.version = "0.2.0".into());
    let step = Step {
        outbound: &redirect,
        vault: &vault,
        link_base: None,
        at_ms: 1,
    };
    let rel = lib.world.releases.get(&lib.id).unwrap();
    let rows = publish_release(&step, &rel, &|digest: &str| {
        let hash = digest
            .parse::<weft::hash::Hash>()
            .map_err(|e| e.to_string())?;
        d.weft.blob(&hash)
    });
    let npm_row = rows
        .iter()
        .find(|s| s.platform.starts_with("npm/"))
        .expect("an npm row");
    assert!(
        npm_row
            .state
            .starts_with("failed: the package is version 0.1.0 and the release is 0.2.0"),
        "{}",
        npm_row.state
    );
    assert!(
        npm_log.lock().unwrap().is_empty(),
        "nothing is sent for a mismatched package"
    );
    assert!(redirect.sent.lock().unwrap().is_empty());
}

#[test]
fn the_registry_keys_are_the_connectors_and_no_git_platform_is_one() {
    use super::CONNECTORS;
    let ids: Vec<String> = crate::connectors::manifest::registries()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(ids, CONNECTORS);
    assert!(is_registry("npm/npm:@acme/web@any"));
    assert!(!is_registry("github"));
    assert!(!is_registry("github:check"));
    assert!(!is_registry("gitlab"));
}

#[test]
fn a_formula_is_written_to_the_tap_and_an_existing_one_is_replaced_with_its_sha() {
    let bytes = b"the tap's binary".to_vec();
    let built = [
        ("brew:acme/tools@macos-aarch64", bytes.clone()),
        ("brew:acme/tools@linux-x86_64", bytes.clone()),
    ];
    let (rel, blobs) = release_by_hand("app", "0.1.0", &built);
    let vault = test_vault(
        "tap",
        &[("HOMEBREW_TAP_TOKEN", "connector:homebrew-tap", "tap-token")],
    );
    let lookup = |digest: &str| {
        blobs
            .iter()
            .find(|(d, _)| d == digest)
            .map(|(_, bytes)| bytes.clone())
            .ok_or_else(|| format!("no blob {digest}"))
    };
    let sha = sha256_hex(&bytes);
    // First the tap has no formula; then it has one, whose sha the update must carry.
    for existing in [None, Some("old-sha")] {
        let (github, log) = fake_tap(existing);
        let redirect = Redirect::new(vec![("api.github.com", github)]);
        let step = Step {
            outbound: &redirect,
            vault: &vault,
            link_base: Some("https://hub.example/v1/weft/artifacts"),
            at_ms: 1,
        };
        let rows = publish_release(&step, &rel, &lookup);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].platform, "homebrew-tap/brew:acme/tools");
        assert_eq!(rows[0].state, "published", "{rows:?}");
        assert_eq!(
            rows[0].url.as_deref(),
            Some("https://github.com/acme/homebrew-tools/blob/main/Formula/app.rb")
        );
        let seen = log.lock().unwrap();
        assert_eq!(
            seen.iter().filter(|s| s.method == "GET").count(),
            1,
            "the sha is read first"
        );
        let put = seen.iter().rev().find(|s| s.method == "PUT").unwrap();
        let doc: Value = serde_json::from_slice(&put.body).unwrap();
        assert_eq!(doc.get("sha").and_then(Value::as_str), existing);
        let content = String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(doc["content"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
        assert!(content.contains("url \"https://hub.example/v1/weft/artifacts/a0/content\""));
        assert!(content.contains("url \"https://hub.example/v1/weft/artifacts/a1/content\""));
        assert_eq!(content.matches(&sha).count(), 2, "{content}");
    }
}
