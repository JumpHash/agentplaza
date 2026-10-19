//! `acp --repo`'s mirror in Weft: the editor's committed HEAD becomes the latest draft of an editor lane
//! (`editor/<branch>`, the owner's, never landed), so the agents an editor talks to read the owner's code as it is.
//! The files are read from the project's own git repository (Weft reads git itself); only the objects the hub
//! lacks travel; the draft is signed for the owner by the hub (from this machine's console session).

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};
use weft::hash::Hash;
use weft::model::Delta;

use crate::ctx::Client;

/// Weft through the hub, on the CLI's own connection (its pin and owner token).
fn weft_client(client: &Client) -> weft::client::Client {
    weft::client::Client {
        target: client.target.clone(),
        prefix: "/v1/weft".into(),
        timeout: client.timeout.max(std::time::Duration::from_secs(120)),
    }
}

/// Mirrors HEAD of the repository at `top` into the editor lane `branch` of Weft repo `repo`.
pub fn upload(client: &Client, top: &Path, repo: &str, branch: &str) -> Result<String, String> {
    let c = weft_client(client);
    // The editor's committed files.
    let git = weft::git::Repo::open(top)?;
    let head = git.resolve("HEAD")?;
    let commit = weft::git::parse_commit(&git.read(&head)?.1)?;
    let mut local: BTreeMap<String, (u32, Vec<u8>)> = BTreeMap::new();
    for (path, (mode, oid)) in git.flatten(&commit.tree)? {
        let (_, bytes) = git.read(&oid)?;
        local.insert(
            path,
            (
                if mode == 0o100755 || mode == 0o120000 {
                    mode
                } else {
                    0o100644
                },
                bytes,
            ),
        );
    }
    // The editor lane: found among the owner's open lanes by its name, else opened on main.
    let enc = weft::server::percent_encode;
    let open = c.get(&format!(
        "/repos/{}/log?q={}&limit=200",
        enc(repo),
        enc("author(owner) & open()")
    ))?;
    let change = match open["changes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|ch| ch["title"].as_str() == Some(branch))
    {
        Some(ch) => ch["change"].as_str().unwrap_or_default().to_string(),
        None => c.post(
            "/lanes",
            &json!({ "repo": repo, "title": branch, "author": "owner", "kind": "editor" }),
        )?["change"]
            .as_str()
            .ok_or("Weft opened no lane")?
            .to_string(),
    };
    let detail = c.get(&format!("/changes/{change}"))?;
    let base = detail["base"].as_str().unwrap_or("root").to_string();
    let listing = c.post("/wire/want", &json!({ "revision": base }))?;
    let base_files: BTreeMap<String, (Hash, u32)> = listing["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| {
            Some((
                f["path"].as_str()?.to_string(),
                (
                    serde_json::from_value(f["hash"].clone()).ok()?,
                    f["mode"].as_u64().unwrap_or(0o100644) as u32,
                ),
            ))
        })
        .collect();
    let mut deltas: Vec<Delta> = Vec::new();
    for (path, (mode, bytes)) in &local {
        let h = Hash::of(bytes);
        if base_files.get(path) != Some(&(h, *mode)) {
            deltas.push(Delta {
                path: path.clone(),
                after: Some(h),
                mode: *mode,
            });
        }
    }
    for path in base_files.keys().filter(|p| !local.contains_key(*p)) {
        deltas.push(Delta {
            path: path.clone(),
            after: None,
            mode: 0o100644,
        });
    }
    // Nothing new since the last mirror.
    if let Some(last) = detail["drafts"]
        .as_array()
        .and_then(|d| d.last())
        .and_then(Value::as_str)
    {
        let prev = c.get(&format!("/revisions/{last}"))?;
        if serde_json::from_value::<Vec<Delta>>(prev["deltas"].clone()).is_ok_and(|d| d == deltas) {
            return Ok(change);
        }
    }
    let by_hash: BTreeMap<Hash, &Vec<u8>> = local.values().map(|(_, b)| (Hash::of(b), b)).collect();
    let wanted: Vec<Hash> = deltas.iter().filter_map(|d| d.after).collect();
    c.upload(&wanted, &|h| {
        by_hash
            .get(h)
            .map(|b| (*b).clone())
            .ok_or_else(|| format!("no object {}", h.short()))
    })?;
    let n = detail["drafts"].as_array().map(|d| d.len()).unwrap_or(0) + 1;
    c.post("/records/unsigned", &json!({ "kind": "revision.draft", "body": { "change": change, "n": n, "deltas": deltas, "tool_calls": [format!("editor:{}", weft::git::hex(&head))] } }))?;
    Ok(change)
}
