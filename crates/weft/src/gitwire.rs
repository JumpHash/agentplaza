//! Talking to a git remote: a local repository (a path or `file://`), or git's smart HTTP (GitHub, GitLab, any
//! git host). Weft pushes (receive-pack) on export and fetches (upload-pack) once, on import.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;

use base64::Engine;

use crate::git::{self, Kind, Oid, Pack, Repo};

#[derive(Clone, Debug)]
pub enum Remote {
    Local(PathBuf),
    Http {
        url: String,
        user: Option<String>,
        secret: Option<String>,
    },
}

impl Remote {
    /// `url` as given; `secret` is the platform token (`x-access-token` is the user GitHub expects for one).
    pub fn parse(url: &str, secret: Option<String>) -> Remote {
        if let Some(p) = url.strip_prefix("file://") {
            return Remote::Local(PathBuf::from(p));
        }
        if url.starts_with("http://") || url.starts_with("https://") {
            let url = url.trim_end_matches('/').to_string();
            let url = if url.ends_with(".git") || url.contains("/git/") {
                url
            } else {
                format!("{url}.git")
            };
            return Remote::Http {
                url,
                user: secret.as_ref().map(|_| "x-access-token".into()),
                secret,
            };
        }
        Remote::Local(PathBuf::from(url))
    }

    /// [`Remote::parse`] with the user git's HTTP sends next to `platform`'s token ([`token_user`]).
    pub fn for_platform(url: &str, platform: &str, secret: Option<String>) -> Remote {
        let mut remote = Remote::parse(url, secret);
        if let Remote::Http {
            user,
            secret: Some(_),
            ..
        } = &mut remote
        {
            *user = Some(token_user(platform).to_string());
        }
        remote
    }

    /// `owner/repo` → `https://github.com/owner/repo.git`.
    pub fn github(slug: &str, secret: Option<String>) -> Remote {
        Remote::parse(
            &format!("https://github.com/{}", slug.trim_end_matches(".git")),
            secret,
        )
    }

    pub fn describe(&self) -> String {
        match self {
            Remote::Local(p) => p.display().to_string(),
            Remote::Http { url, .. } => url.clone(),
        }
    }

    fn auth(&self) -> Option<String> {
        match self {
            Remote::Http {
                user: Some(u),
                secret: Some(s),
                ..
            } => Some(format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(format!("{u}:{s}"))
            )),
            _ => None,
        }
    }
}

/// The user git's HTTP sends with a platform's token: GitHub's `x-access-token`; GitLab needs `oauth2` with an OAuth
/// token and takes it with any other; Forgejo reads only the token.
pub fn token_user(platform: &str) -> &'static str {
    if platform == "github" {
        "x-access-token"
    } else {
        "oauth2"
    }
}

// ── pkt-lines ────────────────────────────────────────────────────────────────────────────────────────────────────

pub fn pkt(line: &[u8]) -> Vec<u8> {
    let mut out = format!("{:04x}", line.len() + 4).into_bytes();
    out.extend_from_slice(line);
    out
}

pub const FLUSH: &[u8] = b"0000";

/// Splits pkt-lines; a flush is an empty line.
pub fn read_pkts(bytes: &[u8]) -> Result<(Vec<Vec<u8>>, usize), String> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 4 <= bytes.len() {
        let n = usize::from_str_radix(
            std::str::from_utf8(&bytes[at..at + 4]).map_err(|_| "bad pkt-line")?,
            16,
        )
        .map_err(|_| "bad pkt-line")?;
        if n == 0 {
            out.push(vec![]);
            at += 4;
            continue;
        }
        if n < 4 || at + n > bytes.len() {
            break;
        }
        out.push(bytes[at + 4..at + n].to_vec());
        at += n;
    }
    Ok((out, at))
}

/// The bands of a side-band-64k answer: (band 1 data, band 2 progress, band 3 error).
fn sideband(pkts: &[Vec<u8>]) -> (Vec<u8>, String, String) {
    let (mut data, mut progress, mut error) = (Vec::new(), String::new(), String::new());
    for p in pkts {
        match p.first() {
            Some(1) => data.extend_from_slice(&p[1..]),
            Some(2) => progress.push_str(&String::from_utf8_lossy(&p[1..])),
            Some(3) => error.push_str(&String::from_utf8_lossy(&p[1..])),
            _ => {}
        }
    }
    (data, progress, error)
}

fn http_get(remote: &Remote, url: &str) -> Result<Vec<u8>, String> {
    let mut req = ureq::get(url)
        .header("User-Agent", "weft/1")
        .header("Git-Protocol", "version=0");
    if let Some(a) = remote.auth() {
        req = req.header("Authorization", &a);
    }
    let resp = req.call().map_err(|e| format!("{url}: {e}"))?;
    let mut out = Vec::new();
    resp.into_body()
        .into_reader()
        .read_to_end(&mut out)
        .map_err(|e| format!("{url}: {e}"))?;
    Ok(out)
}

fn http_post(
    remote: &Remote,
    url: &str,
    content_type: &str,
    body: &[u8],
) -> Result<Vec<u8>, String> {
    let mut req = ureq::post(url)
        .header("User-Agent", "weft/1")
        .header("Content-Type", content_type)
        .header("Accept", content_type.replace("request", "result"));
    if let Some(a) = remote.auth() {
        req = req.header("Authorization", &a);
    }
    let resp = req.send(body).map_err(|e| format!("{url}: {e}"))?;
    let mut out = Vec::new();
    resp.into_body()
        .into_reader()
        .read_to_end(&mut out)
        .map_err(|e| format!("{url}: {e}"))?;
    Ok(out)
}

/// The remote's refs, as the service would see them (`git-receive-pack` or `git-upload-pack`).
pub fn list_refs(remote: &Remote, service: &str) -> Result<BTreeMap<String, Oid>, String> {
    match remote {
        Remote::Local(p) => Repo::open(p)?.refs(),
        Remote::Http { url, .. } => {
            let bytes = http_get(remote, &format!("{url}/info/refs?service={service}"))?;
            let (pkts, _) = read_pkts(&bytes)?;
            let mut out = BTreeMap::new();
            for p in pkts {
                let line = String::from_utf8_lossy(&p);
                let line = line.split('\0').next().unwrap_or("").trim_end();
                if line.starts_with('#') || line.is_empty() {
                    continue;
                }
                if let Some((id, name)) = line.split_once(' ')
                    && name != "capabilities^{}"
                    && !name.ends_with("^{}")
                {
                    out.insert(name.to_string(), git::parse_oid(id)?);
                }
            }
            Ok(out)
        }
    }
}

/// One ref move a push asks for.
#[derive(Clone, Debug)]
pub struct Update {
    pub name: String,
    pub old: Option<Oid>,
    pub new: Oid,
}

/// Pushes `objects` and moves the refs, fast-forward only (each update names the old value it expects).
pub fn push(
    remote: &Remote,
    updates: &[Update],
    objects: &[(Kind, Vec<u8>)],
) -> Result<(), String> {
    match remote {
        Remote::Local(p) => {
            let repo = Repo::open(p)?;
            for (k, d) in objects {
                repo.write(*k, d)?;
            }
            for u in updates {
                repo.update_ref(&u.name, u.old, u.new)?;
            }
            Ok(())
        }
        Remote::Http { url, .. } => {
            let mut body = Vec::new();
            for (i, u) in updates.iter().enumerate() {
                let caps = if i == 0 {
                    "\0report-status side-band-64k agent=weft/1"
                } else {
                    ""
                };
                body.extend(pkt(format!(
                    "{} {} {}{caps}\n",
                    git::hex(&u.old.unwrap_or(git::ZERO)),
                    git::hex(&u.new),
                    u.name
                )
                .as_bytes()));
            }
            body.extend_from_slice(FLUSH);
            body.extend(git::write_pack(objects));
            let answer = http_post(
                remote,
                &format!("{url}/git-receive-pack"),
                "application/x-git-receive-pack-request",
                &body,
            )?;
            let (pkts, _) = read_pkts(&answer)?;
            let (data, _, error) = sideband(&pkts);
            if !error.is_empty() {
                return Err(format!("the remote refused the push: {}", error.trim()));
            }
            let (status, _) = read_pkts(&data)?;
            let lines: Vec<String> = status
                .iter()
                .map(|l| String::from_utf8_lossy(l).trim_end().to_string())
                .filter(|l| !l.is_empty())
                .collect();
            if lines.first().map(String::as_str) != Some("unpack ok") {
                return Err(format!("the remote could not unpack: {}", lines.join("; ")));
            }
            let bad: Vec<&String> = lines.iter().filter(|l| l.starts_with("ng ")).collect();
            if !bad.is_empty() {
                return Err(format!(
                    "the remote refused: {}",
                    bad.iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join("; ")
                ));
            }
            Ok(())
        }
    }
}

/// Fetches what `wants` need (everything, without `haves`) as one pack.
pub fn fetch(remote: &Remote, wants: &[Oid], haves: &[Oid]) -> Result<Pack, String> {
    match remote {
        Remote::Local(_) => Err("a local repository is read where it is".into()),
        Remote::Http { url, .. } => {
            let mut body = Vec::new();
            for (i, w) in wants.iter().enumerate() {
                let caps = if i == 0 {
                    " side-band-64k ofs-delta agent=weft/1"
                } else {
                    ""
                };
                body.extend(pkt(format!("want {}{caps}\n", git::hex(w)).as_bytes()));
            }
            body.extend_from_slice(FLUSH);
            for h in haves {
                body.extend(pkt(format!("have {}\n", git::hex(h)).as_bytes()));
            }
            body.extend(pkt(b"done\n"));
            let answer = http_post(
                remote,
                &format!("{url}/git-upload-pack"),
                "application/x-git-upload-pack-request",
                &body,
            )?;
            let (pkts, _) = read_pkts(&answer)?;
            // NAK or ACK lines come first, then the side bands.
            let start = pkts
                .iter()
                .position(|p| matches!(p.first(), Some(1..=3)))
                .unwrap_or(pkts.len());
            let (data, _, error) = sideband(&pkts[start..]);
            if !error.is_empty() {
                return Err(format!("the remote refused the fetch: {}", error.trim()));
            }
            Pack::from_bytes(data)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkt_lines_round_trip() {
        let mut b = pkt(b"hello\n");
        b.extend_from_slice(FLUSH);
        b.extend(pkt(b"x"));
        let (lines, used) = read_pkts(&b).unwrap();
        assert_eq!(lines, vec![b"hello\n".to_vec(), vec![], b"x".to_vec()]);
        assert_eq!(used, b.len());
    }

    #[test]
    fn remotes_parse() {
        assert!(matches!(
            Remote::parse("file:///tmp/x.git", None),
            Remote::Local(_)
        ));
        match Remote::github("JumpHash/agentplaza", Some("t".into())) {
            Remote::Http { url, user, .. } => {
                assert_eq!(url, "https://github.com/JumpHash/agentplaza.git");
                assert_eq!(user.as_deref(), Some("x-access-token"));
            }
            _ => panic!(),
        }
        // GitLab takes an OAuth token only as `oauth2`'s password; without a token there is no user.
        let user_of = |r: Remote| match r {
            Remote::Http { user, .. } => user,
            Remote::Local(_) => None,
        };
        assert_eq!(
            user_of(Remote::for_platform(
                "https://gitlab.com/g/r.git",
                "gitlab",
                Some("t".into())
            ))
            .as_deref(),
            Some("oauth2")
        );
        assert_eq!(
            user_of(Remote::for_platform(
                "https://codeberg.org/o/r",
                "forgejo",
                Some("t".into())
            ))
            .as_deref(),
            Some("oauth2")
        );
        assert_eq!(
            user_of(Remote::for_platform(
                "https://gitlab.com/g/r.git",
                "gitlab",
                None
            )),
            None
        );
    }
}
