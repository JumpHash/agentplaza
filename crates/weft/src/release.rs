//! Releases and artifacts.
//!
//! A release is a version of a repo cut from a landed revision, with notes written from the intents landed since
//! the last release, the targets its build files declare (W20: every one), artifacts with build provenance, and the
//! owner's signature. Exported, it is a signed annotated tag; the platforms get their releases from apid (22.4).

use std::collections::BTreeMap;

use base64::Engine;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::git::{self, Kind, Person};
use crate::gitwire::{self, Remote, Update};
use crate::hash::Hash;
use crate::keys::{KeyPair, PublicKey};
use crate::state::{ChangeState, State};
use crate::store::Store;

/// One thing a release builds.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Target {
    /// `cargo:weft`, `npm:@agentplaza/web`, `gradle:app`, `image:ops/hub`, `go:…`, `python:…`.
    pub name: String,
    pub file: String,
    /// The platforms it is built for (`linux-x86_64`, `macos-aarch64`, `windows-x86_64`, `android-aarch64`, `any`).
    pub platforms: Vec<String>,
}

const DESKTOP: &[&str] = &[
    "linux-x86_64",
    "linux-aarch64",
    "macos-aarch64",
    "windows-x86_64",
];

/// Every target the build files in a revision declare.
pub fn targets(store: &Store, rev: &str) -> Result<Vec<Target>, String> {
    let tree = store.tree(rev)?;
    let read = |p: &str| {
        tree.get(p)
            .and_then(|e| store.blobs.get(&e.hash).ok())
            .map(|b| String::from_utf8_lossy(&b).to_string())
    };
    let mut out = Vec::new();
    for path in tree.keys() {
        let dir = path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        let file = path.rsplit('/').next().unwrap_or(path);
        let at = |f: &str| {
            if dir.is_empty() {
                f.to_string()
            } else {
                format!("{dir}/{f}")
            }
        };
        match file {
            "Cargo.toml" => {
                let Some(text) = read(path) else { continue };
                let Ok(doc) = text.parse::<toml::Table>() else {
                    continue;
                };
                let Some(pkg) = doc.get("package").and_then(|p| p.as_table()) else {
                    continue;
                };
                let name = pkg
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or(dir)
                    .to_string();
                let declared: Option<Vec<String>> = pkg
                    .get("metadata")
                    .and_then(|m| m.get("weft"))
                    .and_then(|w| w.get("targets"))
                    .and_then(|t| t.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    });
                let bins: Vec<String> = doc
                    .get("bin")
                    .and_then(|b| b.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|b| {
                                b.get("name").and_then(|n| n.as_str()).map(str::to_string)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let has_main = tree.contains_key(&at("src/main.rs"));
                let names = if !bins.is_empty() {
                    bins
                } else if has_main {
                    vec![name]
                } else {
                    vec![]
                };
                for n in names {
                    out.push(Target {
                        name: format!("cargo:{n}"),
                        file: path.clone(),
                        platforms: declared
                            .clone()
                            .unwrap_or_else(|| DESKTOP.iter().map(|s| s.to_string()).collect()),
                    });
                }
                // A Homebrew tap named in the package (`[package.metadata.weft] homebrew = "owner/tap"`): its formula takes the
                // package's binaries for macOS and Linux (9.5), one target per tap.
                let tap = pkg
                    .get("metadata")
                    .and_then(|m| m.get("weft"))
                    .and_then(|w| w.get("homebrew"))
                    .and_then(|t| t.as_str())
                    .filter(|t| {
                        t.split_once('/')
                            .is_some_and(|(o, r)| !o.is_empty() && !r.is_empty())
                    });
                if let Some(tap) = tap {
                    out.push(Target {
                        name: format!("brew:{tap}"),
                        file: path.clone(),
                        platforms: ["macos-aarch64", "linux-x86_64", "linux-aarch64"]
                            .iter()
                            .map(|s| s.to_string())
                            .collect(),
                    });
                }
            }
            "package.json" if !path.contains("node_modules/") => {
                let Some(text) = read(path) else { continue };
                let Ok(v) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                if v["private"] == true && v["scripts"]["build"].is_null() {
                    continue;
                }
                if v["scripts"]["build"].is_string() || v["bin"].is_object() || v["bin"].is_string()
                {
                    out.push(Target {
                        name: format!("npm:{}", v["name"].as_str().unwrap_or(dir)),
                        file: path.clone(),
                        platforms: vec!["any".into()],
                    });
                }
            }
            "build.gradle" | "build.gradle.kts"
                if read(path).is_some_and(|t| t.contains("com.android.application")) =>
            {
                out.push(Target {
                    name: format!("gradle:{}", if dir.is_empty() { "app" } else { dir }),
                    file: path.clone(),
                    platforms: vec!["android-aarch64".into(), "android-x86_64".into()],
                });
            }
            "Dockerfile" => out.push(Target {
                name: format!("image:{}", if dir.is_empty() { "." } else { dir }),
                file: path.clone(),
                platforms: vec!["linux-x86_64".into(), "linux-aarch64".into()],
            }),
            "go.mod" => {
                let module = read(path)
                    .and_then(|t| {
                        t.lines()
                            .find_map(|l| l.strip_prefix("module ").map(|m| m.trim().to_string()))
                    })
                    .unwrap_or_else(|| dir.to_string());
                if tree.contains_key(&at("main.go")) {
                    out.push(Target {
                        name: format!("go:{module}"),
                        file: path.clone(),
                        platforms: DESKTOP.iter().map(|s| s.to_string()).collect(),
                    });
                }
            }
            csproj if csproj.ends_with(".csproj") => {
                // A .NET project that names its package id is a NuGet package (9.5); one that does not is not published.
                let id = read(path).and_then(|t| {
                    t.split_once("<PackageId>")
                        .and_then(|(_, rest)| rest.split_once("</PackageId>"))
                        .map(|(id, _)| id.trim().to_string())
                });
                let Some(id) = id.filter(|id| !id.is_empty()) else {
                    continue;
                };
                out.push(Target {
                    name: format!("nuget:{id}"),
                    file: path.clone(),
                    platforms: vec!["any".into()],
                });
            }
            "pyproject.toml" => {
                let name = read(path)
                    .and_then(|t| t.parse::<toml::Table>().ok())
                    .and_then(|d| d.get("project")?.get("name")?.as_str().map(str::to_string))
                    .unwrap_or_else(|| dir.to_string());
                out.push(Target {
                    name: format!("python:{name}"),
                    file: path.clone(),
                    platforms: vec!["any".into()],
                });
            }
            _ => {}
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.name == b.name);
    Ok(out)
}

/// Release notes in plain words: the intents landed after `since` up to `upto`, by kind.
pub fn notes(st: &State, repo: &str, since: Option<&str>, upto: &str) -> Result<String, String> {
    notes_on(st, repo, "main", since, upto)
}

/// The notes of the revisions landed on `reference` after `since`, up to `upto` (a release line walks its own line).
pub fn notes_on(
    st: &State,
    repo: &str,
    reference: &str,
    since: Option<&str>,
    upto: &str,
) -> Result<String, String> {
    let chain = st
        .repo(repo)?
        .refs
        .get(reference)
        .cloned()
        .unwrap_or_default();
    let end = chain
        .iter()
        .position(|r| r == upto)
        .ok_or_else(|| format!("{upto} is not on {reference}"))?;
    let begin = match since {
        Some(s) => chain
            .iter()
            .position(|r| r == s)
            .map(|i| i + 1)
            .unwrap_or(0),
        None => 0,
    };
    let mut groups: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for rev in &chain[begin..=end] {
        let Some(c) = st
            .revisions
            .get(rev)
            .and_then(|r| st.changes.get(&r.change))
        else {
            continue;
        };
        if c.state != ChangeState::Landed {
            continue;
        }
        let intents = c
            .landed
            .as_ref()
            .and_then(|l| st.revisions.get(l))
            .and_then(|l| c.intents.get(&l.n))
            .cloned()
            .or_else(|| c.intents.values().last().cloned())
            .unwrap_or_default();
        if intents.is_empty() {
            let title = match &c.import {
                Some(i) => i.message.lines().next().unwrap_or("").to_string(),
                None => c.title.clone(),
            };
            let kind = if c.kind == "revert" {
                "revert"
            } else {
                "change"
            };
            groups.entry(heading(kind)).or_default().push(title);
        }
        for i in intents {
            groups.entry(heading(&i.kind)).or_default().push(i.title);
        }
    }
    let order = [
        "Features",
        "Fixes",
        "Improvements",
        "Documentation",
        "Research",
        "Reverts",
        "Other changes",
    ];
    let mut out = String::new();
    for h in order {
        if let Some(items) = groups.get(h) {
            out.push_str(&format!("## {h}\n\n"));
            for i in items {
                out.push_str(&format!("- {i}\n"));
            }
            out.push('\n');
        }
    }
    Ok(out.trim_end().to_string() + "\n")
}

fn heading(kind: &str) -> &'static str {
    match kind {
        "feature" => "Features",
        "fix" | "bug" => "Fixes",
        "improvement" | "perf" | "refactor" => "Improvements",
        "docs" => "Documentation",
        "research" => "Research",
        "revert" => "Reverts",
        _ => "Other changes",
    }
}

/// The `release.propose` body for `version` from `from` (main's head when `None`).
pub fn proposal(
    store: &Store,
    repo: &str,
    version: &str,
    from: Option<&str>,
    line: Option<&str>,
) -> Result<Value, String> {
    let st = store.state();
    let r = st.repo(repo)?;
    // A release of main walks main; a maintenance release walks its release line (PLAN 22.3), `release/1.4`.
    let reference = line.filter(|l| *l != "main").unwrap_or("main");
    let from = from
        .map(str::to_string)
        .unwrap_or_else(|| r.head(reference));
    if from == "root" {
        return Err("nothing has landed to release".into());
    }
    // The last release cut from this line before this one.
    let chain = r.refs.get(reference).cloned().unwrap_or_default();
    let at = chain
        .iter()
        .position(|x| *x == from)
        .ok_or_else(|| format!("{from} is not on {reference}"))?;
    let previous = r
        .releases
        .values()
        .filter_map(|x| {
            chain
                .iter()
                .position(|c| *c == x.propose.from)
                .filter(|p| *p < at)
                .map(|p| (p, x.propose.from.clone()))
        })
        .max_by_key(|x| x.0)
        .map(|x| x.1);
    let notes = notes_on(&st, repo, reference, previous.as_deref(), &from)?;
    drop(st);
    let targets: Vec<String> = targets(store, &from)?
        .into_iter()
        .flat_map(|t| {
            t.platforms
                .iter()
                .map(|p| format!("{}@{p}", t.name))
                .collect::<Vec<_>>()
        })
        .collect();
    Ok(
        json!({ "repo": repo, "version": version, "from": from, "line": line, "notes": notes, "targets": targets }),
    )
}

/// What the owner signs: the release's version, the revision's digest and each artifact's digest, in order.
pub fn release_digest(
    st: &State,
    repo: &str,
    version: &str,
    artifacts: &[String],
) -> Result<Hash, String> {
    let rel = st
        .repo(repo)?
        .releases
        .get(version)
        .ok_or_else(|| format!("no release {version}"))?;
    let rev = st.revisions.get(&rel.propose.from).ok_or("no revision")?;
    let mut parts: Vec<Vec<u8>> = vec![
        repo.as_bytes().to_vec(),
        version.as_bytes().to_vec(),
        rev.digest.0.to_vec(),
    ];
    for a in artifacts {
        let info = st
            .artifacts
            .get(a)
            .ok_or_else(|| format!("no artifact {a}"))?;
        parts.push(info.artifact.digest.0.to_vec());
    }
    let refs: Vec<&[u8]> = parts.iter().map(|p| p.as_slice()).collect();
    Ok(Hash::of_parts(&refs))
}

// ── artifacts ────────────────────────────────────────────────────────────────────────────────────────────────────

/// An artifact's `artifact.put` body, with its SLSA v1 provenance (the statement itself is hub-signed in the log).
#[allow(clippy::too_many_arguments)]
pub fn artifact_body(
    store: &Store,
    repo: &str,
    name: &str,
    version: Option<&str>,
    target: &str,
    bytes: &[u8],
    built_from: &str,
    builder: &str,
    command: &str,
) -> Result<Value, String> {
    let digest = store.blobs.put(bytes)?;
    let sha256 = crate::hash::hex(&Sha256::digest(bytes));
    let rev_digest = store
        .state()
        .revisions
        .get(built_from)
        .map(|r| r.digest)
        .ok_or_else(|| format!("no revision {built_from}"))?;
    let id = format!(
        "a-{}-{}",
        &digest.hex()[..12],
        target.replace(['/', ':', '@'], "-")
    );
    let provenance = json!({
        "_type": "https://in-toto.io/Statement/v1",
        "subject": [{ "name": name, "digest": { "sha256": sha256, "blake3": digest.hex() } }],
        "predicateType": "https://slsa.dev/provenance/v1",
        "predicate": {
            "buildDefinition": {
                "buildType": "https://agentplaza.dev/weft/build/v1",
                "externalParameters": { "command": command, "target": target, "repo": repo },
                "resolvedDependencies": [{ "uri": format!("weft:{repo}@{built_from}"), "digest": { "blake3": rev_digest.hex() } }]
            },
            "runDetails": { "builder": { "id": builder }, "metadata": { "invocationId": id } }
        }
    });
    Ok(
        json!({ "id": id, "repo": repo, "name": name, "version": version, "target": target, "digest": digest, "size": bytes.len(), "built_from": built_from, "builder": builder, "command": command, "provenance": provenance }),
    )
}

/// The payload type of an in-toto statement in a DSSE envelope.
pub const IN_TOTO: &str = "application/vnd.in-toto+json";

/// DSSE's pre-authentication encoding of `payload` under the payload type `ty`: what a signature covers.
pub fn pae(ty: &str, payload: &[u8]) -> Vec<u8> {
    [
        b"DSSEv1 ".to_vec(),
        ty.len().to_string().into_bytes(),
        b" ".to_vec(),
        ty.as_bytes().to_vec(),
        b" ".to_vec(),
        payload.len().to_string().into_bytes(),
        b" ".to_vec(),
        payload.to_vec(),
    ]
    .concat()
}

/// A provenance statement in a DSSE envelope signed with `key` (the file that ships beside an artifact).
pub fn dsse(statement: &Value, key: &KeyPair) -> Value {
    let payload = serde_json::to_vec(statement).unwrap_or_default();
    let sig = key.sign(&pae(IN_TOTO, &payload));
    let b64 = base64::engine::general_purpose::STANDARD;
    json!({ "payloadType": IN_TOTO, "payload": b64.encode(&payload), "signatures": [{ "keyid": key.id(), "sig": b64.encode(sig) }] })
}

/// Opens a DSSE envelope of an in-toto statement: the statement it carries, when one of its signatures is `key`'s over
/// the payload. Refuses any other payload type, and an envelope whose payload or signature has changed.
pub fn open_dsse(envelope: &Value, key: &PublicKey) -> Result<Value, String> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let ty = envelope["payloadType"]
        .as_str()
        .ok_or("the envelope names no payload type")?;
    if ty != IN_TOTO {
        return Err(format!(
            "the envelope's payload is {ty}, not an in-toto statement"
        ));
    }
    let payload = b64
        .decode(envelope["payload"].as_str().unwrap_or_default())
        .map_err(|e| format!("the envelope's payload is not base64: {e}"))?;
    let signed = pae(ty, &payload);
    let checked = envelope["signatures"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s["sig"].as_str())
        .filter_map(|sig| b64.decode(sig).ok())
        .any(|sig| key.verify(&signed, &sig).is_ok());
    if !checked {
        return Err("no signature of the envelope is the key's over its payload".into());
    }
    serde_json::from_slice(&payload).map_err(|e| format!("the payload is not JSON: {e}"))
}

// ── export ───────────────────────────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ReleaseExportReport {
    pub tag: String,
    pub tag_id: String,
    pub commit: String,
}

/// Exports a signed release: a signed annotated tag on the release's commit. The platforms' releases (their notes,
/// assets, SBOMs, provenance and checks) are synced by apid (`releases/sync.rs`, 22.4) from this report.
pub fn export_release(
    store: &Store,
    repo: &str,
    version: &str,
    connection: &str,
    token: Option<String>,
    signing: Option<&KeyPair>,
) -> Result<ReleaseExportReport, String> {
    let (rel, conn) = {
        let st = store.state();
        let r = st.repo(repo)?;
        let rel = r
            .releases
            .get(version)
            .cloned()
            .ok_or_else(|| format!("no release {version}"))?;
        let conn = r
            .connections
            .get(connection)
            .cloned()
            .ok_or_else(|| format!("no connection {connection}"))?;
        if rel.signed.is_none() {
            return Err("only a signed release is exported".into());
        }
        (rel, conn)
    };
    let commit_hex = crate::export::commit_of(&store.state(), repo, connection, &rel.propose.from)
        .ok_or("the release's revision has not been exported to this connection yet")?;
    let commit = git::parse_oid(&commit_hex)?;
    let tagger = Person {
        name: conn.conn.author_name.clone(),
        email: conn.conn.author_email.clone(),
        time: (crate::record::now_ms() / 1000) as i64,
        tz: "+0000".into(),
    };
    let message = format!("{version}\n\n{}", rel.propose.notes);
    let unsigned = git::write_tag(&commit, Kind::Commit, version, &tagger, &message, None);
    let bytes = match signing {
        Some(k) => git::write_tag(
            &commit,
            Kind::Commit,
            version,
            &tagger,
            &message,
            Some(&crate::sshsig::sign(k, "git", &unsigned)),
        ),
        None => unsigned,
    };
    let tag_id = git::oid(Kind::Tag, &bytes);
    let remote = Remote::for_platform(&conn.conn.remote, &conn.conn.platform, token.clone());
    let name = format!("refs/tags/{version}");
    let existing = gitwire::list_refs(&remote, "git-receive-pack")?
        .get(&name)
        .copied();
    if existing.is_some() && existing != Some(tag_id) {
        return Err(format!("the remote already has a different tag {version}"));
    }
    if existing.is_none() {
        gitwire::push(
            &remote,
            &[Update {
                name: name.clone(),
                old: None,
                new: tag_id,
            }],
            &[(Kind::Tag, bytes)],
        )?;
    }
    let report = ReleaseExportReport {
        tag: version.into(),
        tag_id: git::hex(&tag_id),
        commit: commit_hex,
    };
    store.hub_append(
        "release.export",
        json!({ "repo": repo, "version": version, "connection": connection, "tag": version }),
    )?;
    Ok(report)
}
