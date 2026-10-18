//! Connecting a repo to an existing git remote (W18: Weft never creates one).
//!
//! On GitHub the exported commits' author is the owner as the connector's authorization knows them (W19): the
//! account's name and its primary verified email, or the account's noreply address when the owner prefers it. With
//! the owner's consent the hub's SSH signing key is registered on that account, so exported commits and tags show
//! as verified. A plain git URL with no account behind it uses the name and email from the plaza's settings.

use std::io::Read;

use serde::Serialize;
use serde_json::{Value, json};

/// The GitHub API's base (`WEFT_GITHUB_API` points elsewhere, for tests and GitHub Enterprise).
pub fn github_api() -> String {
    std::env::var("WEFT_GITHUB_API").unwrap_or_else(|_| "https://api.github.com".into())
}

#[derive(Clone, Debug, Serialize)]
pub struct Account {
    pub login: String,
    pub id: u64,
    pub name: String,
    pub email: String,
    /// `primary` (a verified address) or `noreply`.
    pub email_kind: String,
}

pub fn api_get(api: &str, token: &str, path: &str) -> Result<Value, String> {
    let url = format!("{api}{path}");
    let resp = ureq::get(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "weft/1")
        .call()
        .map_err(|e| format!("{url}: {e}"))?;
    let mut body = Vec::new();
    resp.into_body()
        .into_reader()
        .take(8 << 20)
        .read_to_end(&mut body)
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&body).map_err(|e| format!("{url}: {e}"))
}

pub fn api_post(api: &str, token: &str, path: &str, body: &Value) -> Result<Value, String> {
    let url = if path.starts_with("http") {
        path.to_string()
    } else {
        format!("{api}{path}")
    };
    let resp = ureq::post(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "weft/1")
        .send_json(body)
        .map_err(|e| format!("{url}: {e}"))?;
    let mut out = Vec::new();
    resp.into_body()
        .into_reader()
        .take(8 << 20)
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    Ok(serde_json::from_slice(&out).unwrap_or(Value::Null))
}

pub fn api_upload(
    token: &str,
    url: &str,
    content_type: &str,
    bytes: &[u8],
) -> Result<Value, String> {
    let resp = ureq::post(url)
        .header("Authorization", &format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "weft/1")
        .header("Content-Type", content_type)
        .send(bytes)
        .map_err(|e| format!("{url}: {e}"))?;
    let mut out = Vec::new();
    resp.into_body()
        .into_reader()
        .take(8 << 20)
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    Ok(serde_json::from_slice(&out).unwrap_or(Value::Null))
}

/// The account a GitHub token belongs to, and the address its exported commits carry.
pub fn github_account(api: &str, token: &str, prefer_noreply: bool) -> Result<Account, String> {
    let user = api_get(api, token, "/user")?;
    let login = user["login"]
        .as_str()
        .ok_or("the token names no GitHub user")?
        .to_string();
    let id = user["id"].as_u64().unwrap_or(0);
    let name = user["name"]
        .as_str()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or(&login)
        .to_string();
    let noreply = format!("{id}+{login}@users.noreply.github.com");
    if prefer_noreply {
        return Ok(Account {
            login,
            id,
            name,
            email: noreply,
            email_kind: "noreply".into(),
        });
    }
    // The primary verified address, when the token may read it (`user:email`); else the noreply address.
    let primary = api_get(api, token, "/user/emails").ok().and_then(|v| {
        v.as_array()?
            .iter()
            .find(|e| e["primary"] == true && e["verified"] == true)
            .and_then(|e| e["email"].as_str().map(str::to_string))
    });
    Ok(match primary {
        Some(email) => Account {
            login,
            id,
            name,
            email,
            email_kind: "primary".into(),
        },
        None => Account {
            login,
            id,
            name,
            email: noreply,
            email_kind: "noreply".into(),
        },
    })
}

/// Registers the hub's SSH signing key on the account (once; an existing registration is left as it is).
pub fn register_signing_key(
    api: &str,
    token: &str,
    title: &str,
    key_line: &str,
) -> Result<String, String> {
    let key_part: String = key_line
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    if let Ok(list) = api_get(api, token, "/user/ssh_signing_keys")
        && list.as_array().is_some_and(|keys| {
            keys.iter()
                .any(|k| k["key"].as_str().is_some_and(|k| k.starts_with(&key_part)))
        })
    {
        return Ok("already registered".into());
    }
    api_post(
        api,
        token,
        "/user/ssh_signing_keys",
        &json!({ "title": title, "key": key_part }),
    )
    .map(|_| "registered".into())
}

/// `owner/repo` of a remote URL: its last two path segments (github.com, GitHub Enterprise, a mirror).
pub fn github_slug(remote: &str) -> Option<String> {
    let rest = remote
        .split_once("://")
        .map(|(_, r)| r)
        .or_else(|| remote.split_once(':').map(|(_, r)| r))
        .unwrap_or(remote);
    let parts: Vec<&str> = rest
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .split('/')
        .filter(|p| !p.is_empty())
        .collect();
    (parts.len() >= 2).then(|| format!("{}/{}", parts[parts.len() - 2], parts[parts.len() - 1]))
}

/// The `connection.add` body.
#[allow(clippy::too_many_arguments)]
pub fn connection_body(
    repo: &str,
    id: &str,
    remote: &str,
    platform: &str,
    branch: &str,
    name: &str,
    email: &str,
    credential: Option<&str>,
) -> Value {
    json!({ "repo": repo, "id": id, "remote": remote, "platform": platform, "branch": branch, "author_name": name, "author_email": email, "credential": credential })
}
