//! Releases synced to the git platforms (PLAN 22.4, 9.4). Weft exports a release's tag (a signed annotated tag on its
//! commit); this module makes the platform's release of that tag: its notes, its assets (each artifact, its SBOM and
//! its provenance, and `SHA256SUMS`; an artifact too large to attach is linked instead), its pre-release flag while
//! it is a candidate of a channel other than stable, the verdicts on its commit as checks, and a withdrawal (a yank)
//! written into its notes. Every later change (edited notes, a yank, a new verdict) is synced again on the next run.
//!
//! [`sync_one`] is the platform side and takes its HTTP as a parameter (the tests give a stand-in); [`sync_release`]
//! is production: it reads the repo's connections, exports the tag through Weft, reads the blobs and records each
//! platform's state in the release's `sync` rows.

use std::collections::HashSet;
use std::sync::Arc;

use super::entity::{Release, ReleaseState, ReleaseSync};
use super::flow::Ledger;
use super::registries;
use super::signing::sha256_hex;
use crate::platforms::forge::{Forge, Repo, Spec, Verdict};
use crate::platforms::http::Http;
use crate::platforms::model::Kind;

/// Artifacts above this size are linked from the release's notes, not attached (a release page stays light).
pub const ASSET_LIMIT: u64 = 256 << 20;

/// One git platform a release goes to.
pub struct Target {
    pub kind: Kind,
    /// The platform's web address (`https://github.com`, a self-managed host).
    pub host: String,
    pub repo: Repo,
    pub token: String,
}

/// What a sync reads of the world: the release, the commit its tag names, the artifacts' bytes by digest, where a
/// too-large artifact can be downloaded (the address up to the artifact's id, `…/{artifact}/content`; none: its entry
/// says it was not attached), and the size above which artifacts are linked.
pub struct Input<'a> {
    pub release: &'a Release,
    pub commit: &'a str,
    pub bytes: &'a dyn Fn(&str) -> Result<Vec<u8>, String>,
    pub link_base: Option<&'a str>,
    pub asset_limit: u64,
}

/// The platform's key in a release's sync rows.
pub fn key_of(kind: Kind) -> &'static str {
    match kind {
        Kind::Github => "github",
        Kind::Gitlab => "gitlab",
        Kind::Forgejo => "forgejo",
    }
}

/// A file name that a platform takes as an asset: the target's letters, digits, dashes, dots and underscores.
fn safe(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Whether the release is on the platforms at all, and whether as a pre-release. Drafts, failed and abandoned ones are
/// not; a candidate is a pre-release where its channel is not stable (the project's choice of what is visible).
fn visibility(rel: &Release) -> Option<bool> {
    match rel.state {
        ReleaseState::Draft | ReleaseState::Failed | ReleaseState::Abandoned => None,
        ReleaseState::Candidate | ReleaseState::Qualifying | ReleaseState::Qualified => {
            (rel.channel != "stable").then_some(true)
        }
        _ => Some(false),
    }
}

/// The release's notes as the platform shows them: its notes, what was linked or not attached, and the withdrawal.
fn body(rel: &Release, link_base: Option<&str>, asset_limit: u64) -> String {
    let mut out = rel.notes.trim_end().to_string();
    out.push('\n');
    let big: Vec<_> = rel
        .artifacts
        .iter()
        .filter(|a| a.size > asset_limit && !a.is_ux())
        .collect();
    if !big.is_empty() {
        out.push_str("\n## Assets\n\n");
        for a in big {
            let place = match link_base {
                Some(base) => format!("{base}/{}/content", a.artifact),
                None => "not attached: too large for a release asset".to_string(),
            };
            out.push_str(&format!(
                "- `{}` ({} bytes): {place}\n",
                safe(&a.target),
                a.size
            ));
        }
    }
    if rel.state == ReleaseState::Yanked {
        out.push_str("\n> **Withdrawn**: this release was yanked by its owner. Do not use it.\n");
    }
    out
}

/// Syncs one release to one platform: its release (created with its assets, or updated), and the verdicts on its
/// commit posted as checks that were not posted before (a verdict is marked by its statement's sequence number).
/// Returns the sync rows the platform now has: its release's row (replacing the old one) and the new checks.
pub fn sync_one(
    http: Arc<dyn Http>,
    input: &Input,
    target: &Target,
    at_ms: u64,
) -> Result<Vec<ReleaseSync>, String> {
    let rel = input.release;
    let Some(prerelease) = visibility(rel) else {
        return Ok(vec![]);
    };
    let key = key_of(target.kind);
    let forge = Forge::new(target.kind, &target.host, http);
    let yanked = rel.state == ReleaseState::Yanked;
    let spec = Spec {
        tag: rel.version.clone(),
        name: rel.version.clone(),
        body: body(rel, input.link_base, input.asset_limit),
        prerelease,
    };
    let existing = forge.release_of_tag(&target.token, &target.repo, &rel.version)?;
    let remote = match existing {
        Some(remote) => {
            forge.update_release(&target.token, &target.repo, &remote, &spec)?;
            remote
        }
        None => {
            let remote = forge.create_release(&target.token, &target.repo, &spec)?;
            // The release's files: each artifact under the limit, its SBOM and provenance, and the sums of the artifacts.
            let mut sums = String::new();
            for art in rel
                .artifacts
                .iter()
                .filter(|a| a.size <= input.asset_limit && !a.is_ux())
            {
                let bytes = (input.bytes)(&art.digest)?;
                let name = safe(&art.target);
                sums.push_str(&format!("{}  {name}\n", sha256_hex(&bytes)));
                forge.attach(&target.token, &target.repo, &remote, &name, &bytes)?;
            }
            for art in rel.artifacts.iter().filter(|a| !a.is_ux()) {
                let name = safe(&art.target);
                if !art.sbom.is_empty() {
                    forge.attach(
                        &target.token,
                        &target.repo,
                        &remote,
                        &format!("{name}.cdx.json"),
                        art.sbom.as_bytes(),
                    )?;
                }
                if !art.provenance.is_empty() {
                    forge.attach(
                        &target.token,
                        &target.repo,
                        &remote,
                        &format!("{name}.intoto.jsonl"),
                        art.provenance.as_bytes(),
                    )?;
                }
            }
            if !sums.is_empty() {
                forge.attach(
                    &target.token,
                    &target.repo,
                    &remote,
                    "SHA256SUMS",
                    sums.as_bytes(),
                )?;
            }
            remote
        }
    };
    let state = if yanked {
        "yanked"
    } else if prerelease {
        "pre-release"
    } else {
        "published"
    };
    let mut rows = vec![ReleaseSync {
        platform: key.into(),
        state: state.into(),
        url: remote.url.clone(),
        at_ms,
    }];
    let posted: HashSet<&str> = rel
        .sync
        .iter()
        .filter(|s| s.platform == format!("{key}:check"))
        .map(|s| s.state.as_str())
        .collect();
    for v in &rel.verdicts {
        let verdict = match v.verdict.as_str() {
            "passed" => Verdict::Pass,
            "failed" => Verdict::Fail,
            _ => continue,
        };
        let mark = format!("{}:{}", v.seq, v.verdict);
        if posted.contains(mark.as_str()) {
            continue;
        }
        let summary = if v.reason.is_empty() {
            format!("{} {}", v.source, v.verdict)
        } else {
            v.reason.clone()
        };
        forge.check(
            &target.token,
            &target.repo,
            input.commit,
            &format!("agentplaza/{}", v.source),
            verdict,
            &summary,
        )?;
        rows.push(ReleaseSync {
            platform: format!("{key}:check"),
            state: mark,
            url: None,
            at_ms,
        });
    }
    Ok(rows)
}

/// The platform's address and the repository's path on it, from a remote: `https://host/owner/name.git`,
/// `ssh://git@host/owner/name` or `git@host:owner/name.git`.
pub fn split_remote(remote: &str) -> Option<(String, String)> {
    let rest = match remote.split_once("://") {
        Some((_, rest)) => rest.split_once('/')?,
        None => remote.split_once(':')?,
    };
    let host = rest.0.rsplit('@').next()?.to_string();
    if host.is_empty() {
        return None;
    }
    Some((format!("https://{host}"), Repo::new(rest.1).path))
}

/// The platform a connection's platform name is, when it is one of the three git platforms.
pub fn kind_of(platform: &str) -> Option<Kind> {
    match platform {
        "github" => Some(Kind::Github),
        "gitlab" => Some(Kind::Gitlab),
        "forgejo" | "gitea" => Some(Kind::Forgejo),
        _ => None,
    }
}

/// Syncs a release to every git platform its repo's connections name (`github`, `gitlab`, `forgejo`): the tag is
/// exported through Weft first (signed, on the release's commit), then the platform's release is made or updated and
/// its verdicts posted. Each platform's state is its sync row; a failure is recorded as `failed: …` and the next run
/// tries again. A release that is not on the platforms (a draft, a stable candidate) is left as it is.
pub fn sync_release(app: &crate::state::App, id: &str) -> Result<Release, String> {
    let rel = app.releases.get(id).ok_or("no such release")?;
    if visibility(&rel).is_none() {
        return Ok(rel);
    }
    let detail = app.weft.get(&format!("/repos/{}", rel.repo))?;
    let http = app.platforms.http();
    let now = apz_plaza::now_ms();
    let mut rows = Vec::new();
    let mut touched: Vec<&str> = Vec::new();
    // The hub's artifact download route, under its public address: a platform's link to an artifact too large to attach.
    let public = app.config.public_url.trim_end_matches('/');
    let link_base = (!public.is_empty()).then(|| format!("{public}/v1/weft/artifacts"));
    for c in detail["connections"].as_array().into_iter().flatten() {
        let Some(kind) = kind_of(c["platform"].as_str().unwrap_or("")) else {
            continue;
        };
        let key = key_of(kind);
        touched.push(key);
        let conn = c["id"].as_str().unwrap_or("").to_string();
        let result = (|| -> Result<Vec<ReleaseSync>, String> {
            let (host, path) = split_remote(c["remote"].as_str().unwrap_or(""))
                .ok_or("the remote is not a platform's")?;
            let token = crate::weft_hub::credential(app, c["credential"].as_str())
                .ok_or("no token reaches the platform")?;
            let tagged = app.weft.post(
                &format!("/repos/{}/releases/{}/export", rel.repo, rel.version),
                &serde_json::json!({ "connection": conn, "token": token }),
            )?;
            let commit = tagged["commit"]
                .as_str()
                .ok_or("Weft gave no commit for the tag")?
                .to_string();
            let bytes = |digest: &str| -> Result<Vec<u8>, String> {
                let hash = digest
                    .parse::<weft::hash::Hash>()
                    .map_err(|e| e.to_string())?;
                app.weft.blob(&hash)
            };
            let input = Input {
                release: &rel,
                commit: &commit,
                bytes: &bytes,
                link_base: link_base.as_deref(),
                asset_limit: ASSET_LIMIT,
            };
            let target = Target {
                kind,
                host,
                repo: Repo::new(&path),
                token,
            };
            sync_one(http.clone(), &input, &target, now)
        })();
        match result {
            Ok(new) => rows.extend(new),
            Err(e) => rows.push(ReleaseSync {
                platform: key.into(),
                state: format!("failed: {e}"),
                url: None,
                at_ms: now,
            }),
        }
    }
    let updated = app
        .releases
        .update(id, |r| {
            r.sync.retain(|s| !touched.contains(&s.platform.as_str()));
            r.sync.extend(rows);
            r.updated_ms = now;
        })
        .ok_or("no such release")?;
    app.bus
        .publish("release_sync", &serde_json::json!({ "release": id }));
    Ok(updated)
}

/// Syncs the releases that changed since their platforms last heard of them (an edited note, a yank, a verdict), and
/// those whose last attempt failed more than five minutes ago. The scheduler calls it on each pass.
pub fn tick(app: &crate::state::App) {
    let now = apz_plaza::now_ms();
    for project in app.projects.list() {
        for rel in app.releases.list(&project.id) {
            if visibility(&rel).is_none() {
                continue;
            }
            let last = rel.sync.iter().map(|s| s.at_ms).max().unwrap_or(0);
            let failed = rel.sync.iter().any(|s| {
                !registries::is_registry(&s.platform)
                    && s.state.starts_with("failed")
                    && s.at_ms + 300_000 < now
            });
            if (rel.updated_ms > last || failed)
                && let Err(e) = sync_release(app, &rel.id)
            {
                eprintln!(
                    "apid: syncing release {} to its platforms: {e}",
                    rel.version
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remote_names_its_host_and_its_path() {
        assert_eq!(
            split_remote("https://github.com/irfan/app.git"),
            Some(("https://github.com".into(), "irfan/app".into()))
        );
        assert_eq!(
            split_remote("git@github.com:irfan/app.git"),
            Some(("https://github.com".into(), "irfan/app".into()))
        );
        assert_eq!(
            split_remote("https://gitlab.example.com/team/sub/app.git"),
            Some(("https://gitlab.example.com".into(), "team/sub/app".into()))
        );
    }

    #[test]
    fn a_platform_name_is_one_of_three() {
        assert_eq!(kind_of("gitlab"), Some(Kind::Gitlab));
        assert_eq!(kind_of("gitea"), Some(Kind::Forgejo));
        assert_eq!(kind_of("git"), None);
    }
}
