//! The CLI's git, release and artifact commands.

use std::path::Path;

use serde_json::{Value, json};

use crate::cli::Ctx;
use crate::hash::Hash;

fn repo(cx: &Ctx) -> Result<String, String> {
    cx.args
        .opt("repo")
        .ok_or_else(|| "--repo is required".into())
}

/// The git platform's token, from the variable `--git-token-env` names (default `WEFT_GIT_TOKEN`).
fn git_token(cx: &Ctx) -> Option<String> {
    let var = cx
        .args
        .opt("git-token-env")
        .unwrap_or_else(|| "WEFT_GIT_TOKEN".into());
    std::env::var(var).ok().filter(|t| !t.is_empty())
}

fn show(cx: &Ctx, v: &Value, human: impl FnOnce(&Value) -> String) {
    if cx.json {
        println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
    } else {
        print!("{}", human(v));
    }
}

fn split_version(s: &str) -> Result<(String, String), String> {
    s.split_once('@')
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .ok_or_else(|| format!("{s}: give <repo>@<version>"))
}

pub fn run(cx: &Ctx, cmd: &[String]) -> Result<(), String> {
    let word = |i: usize| cmd.get(i).map(String::as_str).unwrap_or("");
    match (word(0), word(1)) {
        ("connect", _) => {
            let c = cx.client()?;
            let remote = cx.args.arg(1, "a remote (owner/repo or a git URL)")?;
            let repo = repo(cx)?;
            let platform = cx.args.opt("platform").unwrap_or_else(|| {
                if remote.contains("github.com")
                    || (!remote.contains("://")
                        && remote.matches('/').count() == 1
                        && !Path::new(&remote).exists())
                {
                    "github".into()
                } else {
                    "git".into()
                }
            });
            let url = if platform == "github" && !remote.contains("://") {
                format!("https://github.com/{}.git", remote.trim_end_matches(".git"))
            } else {
                remote.clone()
            };
            let token = git_token(cx);
            // Who the exported commits are by: the account the token belongs to, else --author.
            let (name, email) =
                match (&platform[..], &token, cx.args.opt("author")) {
                    (_, _, Some(a)) => {
                        let p = crate::git::Person::parse(&format!("{a} 0 +0000"))?;
                        (p.name, p.email)
                    }
                    ("github", Some(t), None) => {
                        let acct = c.post(
                            "/github/account",
                            &json!({ "token": t, "noreply": cx.args.has("noreply") }),
                        )?;
                        (
                            acct["name"].as_str().unwrap_or("").to_string(),
                            acct["email"].as_str().unwrap_or("").to_string(),
                        )
                    }
                    _ => return Err(
                        "give --author \"Name <email>\" (a plain git remote has no account to ask)"
                            .into(),
                    ),
                };
            let id = cx.args.opt("id").unwrap_or_else(|| {
                if platform == "github" {
                    "github".into()
                } else {
                    "origin".into()
                }
            });
            let body = crate::connect::connection_body(
                &repo,
                &id,
                &url,
                &platform,
                &cx.args.opt("branch").unwrap_or_else(|| "main".into()),
                &name,
                &email,
                cx.args.opt("credential").as_deref(),
            );
            cx.say(&c, "connection.add", body)?;
            println!("{repo} is connected to {url}; exported commits are by {name} <{email}>");
            // A repo with no history yet takes the remote's, once.
            let detail = c.get(&format!("/repos/{repo}"))?;
            if detail["landed"] == 0 {
                let r = c.post(&format!("/repos/{repo}/import"), &json!({ "source": url, "branch": cx.args.opt("branch").unwrap_or_else(|| "main".into()), "token": token }))?;
                println!(
                    "imported {} commits ({} releases) from {url}",
                    r["commits"],
                    r["releases"].as_array().map(|a| a.len()).unwrap_or(0)
                );
            }
            if cx.args.has("register-key")
                && let Some(t) = &token
            {
                let r = c.post("/github/signing-key", &json!({ "token": t }))?;
                println!(
                    "the hub's signing key: {}",
                    r["result"].as_str().unwrap_or("")
                );
            }
            Ok(())
        }
        ("connections", _) => {
            let c = cx.client()?;
            let v = c.get(&format!("/repos/{}", repo(cx)?))?;
            show(cx, &v["connections"], |list| {
                let mut out = String::new();
                for k in list.as_array().into_iter().flatten() {
                    out.push_str(&format!(
                        "{:<8} {:<48} {:<9} {} exported, head {}  by {}\n",
                        k["id"].as_str().unwrap_or(""),
                        k["remote"].as_str().unwrap_or(""),
                        k["state"].as_str().unwrap_or(""),
                        k["exported"],
                        k["head"]
                            .as_str()
                            .map(|h| &h[..12.min(h.len())])
                            .unwrap_or("-"),
                        k["author"].as_str().unwrap_or("")
                    ));
                    if let Some(d) = k["detail"].as_str().filter(|d| !d.is_empty()) {
                        out.push_str(&format!("         {d}\n"));
                    }
                }
                out
            });
            Ok(())
        }
        ("export", _) => {
            let c = cx.client()?;
            let repo = cx.args.arg(1, "a repo")?;
            let detail = c.get(&format!("/repos/{repo}"))?;
            let conns: Vec<String> = match cx.args.opt("connection") {
                Some(x) => vec![x],
                None => detail["connections"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|k| k["id"].as_str().map(str::to_string))
                    .collect(),
            };
            if conns.is_empty() {
                return Err(format!("{repo} has no connection"));
            }
            for conn in conns {
                let v = c.post(&format!("/repos/{repo}/export"), &json!({ "connection": conn, "token": git_token(cx), "dry_run": cx.args.has("dry-run") }))?;
                show(cx, &v, |v| {
                    let mut out = format!(
                        "{} → {} ({}): {}\n",
                        conn,
                        v["remote"].as_str().unwrap_or(""),
                        v["branch"].as_str().unwrap_or(""),
                        v["state"].as_str().unwrap_or("")
                    );
                    if let Some(d) = v["detail"].as_str().filter(|d| !d.is_empty()) {
                        out.push_str(&format!("  {d}\n"));
                    }
                    for ch in v["changes"].as_array().into_iter().flatten() {
                        for k in ch["commits"].as_array().into_iter().flatten() {
                            out.push_str(&format!(
                                "  {} {}  {}\n",
                                ch["change"].as_str().unwrap_or(""),
                                &k["git"].as_str().unwrap_or("")[..12],
                                k["intent"].as_str().unwrap_or("")
                            ));
                        }
                    }
                    out
                });
            }
            Ok(())
        }
        ("adopt", _) => {
            let c = cx.client()?;
            let repo = cx.args.arg(1, "a repo")?;
            let conn = cx.args.opt("connection").unwrap_or_else(|| "github".into());
            let v = c.post(
                &format!("/repos/{repo}/adopt"),
                &json!({ "connection": conn, "token": git_token(cx) }),
            )?;
            show(cx, &v, |v| {
                format!(
                    "{} adopts {} commits ({} files) from the remote; it is reviewed and landed like any change\n",
                    v["change"].as_str().unwrap_or(""),
                    v["commits"].as_array().map(|a| a.len()).unwrap_or(0),
                    v["files"]
                )
            });
            Ok(())
        }
        ("import", _) => {
            let c = cx.client()?;
            let source = cx.args.arg(1, "a git URL or path")?;
            let repo = repo(cx)?;
            let exists = c.get(&format!("/repos/{repo}")).is_ok();
            if !exists {
                cx.say(&c, "repo.create", json!({ "repo": repo, "project": cx.args.opt("project"), "description": cx.args.opt("description").unwrap_or_default() }))?;
            }
            let source = if Path::new(&source).exists() {
                Path::new(&source)
                    .canonicalize()
                    .map_err(|e| e.to_string())?
                    .display()
                    .to_string()
            } else {
                source
            };
            let v = c.post(&format!("/repos/{repo}/import"), &json!({ "source": source, "branch": cx.args.opt("branch").unwrap_or_else(|| "main".into()), "token": git_token(cx) }))?;
            show(cx, &v, |v| {
                format!(
                    "imported {} commits into {repo} ({} already there, {} file changes, trees checked {} times); releases: {}\n",
                    v["commits"],
                    v["skipped"],
                    v["files"],
                    v["checked"],
                    v["releases"]
                        .as_array()
                        .map(|a| a
                            .iter()
                            .filter_map(|x| x.as_str())
                            .collect::<Vec<_>>()
                            .join(", "))
                        .unwrap_or_default()
                )
            });
            Ok(())
        }
        ("release", "sign") => {
            let c = cx.client()?;
            let (repo, version) = split_version(&cx.args.arg(2, "<repo>@<version>")?)?;
            let artifacts = cx.args.all("artifact");
            let d = c.post(
                &format!("/repos/{repo}/releases/{version}/digest"),
                &json!({ "artifacts": artifacts }),
            )?;
            let v = cx.say(&c, "release.sign", json!({ "repo": repo, "version": version, "artifacts": artifacts, "digest": d["digest"] }))?;
            println!("{}", v["summary"].as_str().unwrap_or("signed"));
            Ok(())
        }
        ("release", "export") => {
            let c = cx.client()?;
            let (repo, version) = split_version(&cx.args.arg(2, "<repo>@<version>")?)?;
            let conn = cx.args.opt("connection").unwrap_or_else(|| "github".into());
            let v = c.post(
                &format!("/repos/{repo}/releases/{version}/export"),
                &json!({ "connection": conn, "token": git_token(cx) }),
            )?;
            show(cx, &v, |v| {
                format!(
                    "tag {} ({}) on {}{}\n",
                    v["tag"].as_str().unwrap_or(""),
                    &v["tag_id"].as_str().unwrap_or("")[..12],
                    &v["commit"].as_str().unwrap_or("")[..12],
                    v["url"]
                        .as_str()
                        .map(|u| format!("; release at {u}"))
                        .unwrap_or_default()
                )
            });
            Ok(())
        }
        ("release", _) => {
            let c = cx.client()?;
            let version = cx.args.arg(1, "a version")?;
            let repo = repo(cx)?;
            let notes = match cx.args.opt("notes-file") {
                Some(f) => Some(std::fs::read_to_string(&f).map_err(|e| format!("{f}: {e}"))?),
                None => None,
            };
            let v = c.post(&format!("/repos/{repo}/releases"), &json!({ "version": version, "from": cx.args.opt("from"), "line": cx.args.opt("line"), "notes": notes }))?;
            show(cx, &v, |v| {
                format!(
                    "{repo} {version} from {}\n\n{}\ntargets: {}\n(the owner signs it with: weft release sign {repo}@{version} --artifact …)\n",
                    v["from"].as_str().unwrap_or(""),
                    v["notes"].as_str().unwrap_or(""),
                    v["targets"]
                        .as_array()
                        .map(|a| a
                            .iter()
                            .filter_map(|x| x.as_str())
                            .collect::<Vec<_>>()
                            .join(", "))
                        .unwrap_or_default()
                )
            });
            Ok(())
        }
        ("releases", _) => {
            let c = cx.client()?;
            let v = c.get(&format!("/repos/{}/releases", repo(cx)?))?;
            show(cx, &v, |v| {
                let mut out = String::new();
                for r in v["releases"].as_array().into_iter().flatten() {
                    let state = if !r["imported"].is_null() {
                        "imported"
                    } else if r["signed"].is_null() {
                        "proposed"
                    } else {
                        "signed"
                    };
                    out.push_str(&format!(
                        "{:<12} {:<9} from {:<10} {} artifacts, {} exports\n",
                        r["version"].as_str().unwrap_or(""),
                        state,
                        r["from"].as_str().unwrap_or(""),
                        r["artifacts"].as_array().map(|a| a.len()).unwrap_or(0),
                        r["exports"].as_array().map(|a| a.len()).unwrap_or(0)
                    ));
                }
                out
            });
            Ok(())
        }
        ("artifact", "put") => {
            let c = cx.client()?;
            let file = cx.args.arg(2, "a file")?;
            let bytes = std::fs::read(&file).map_err(|e| format!("{file}: {e}"))?;
            let hash = Hash::of(&bytes);
            let b2 = bytes.clone();
            c.upload(&[hash], &|_| Ok(b2.clone()))?;
            let repo = repo(cx)?;
            let from = match cx.args.opt("from") {
                Some(f) => f,
                None => c.get(&format!("/repos/{repo}"))?["refs"]["main"]
                    .as_str()
                    .unwrap_or("root")
                    .to_string(),
            };
            let name = cx.args.opt("name").unwrap_or_else(|| {
                Path::new(&file)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            });
            let v = c.post("/artifacts", &json!({ "repo": repo, "name": name, "version": cx.args.opt("version"), "target": cx.args.opt("target").unwrap_or_else(|| "any".into()), "blob": hash, "built_from": from, "builder": cx.args.opt("builder").unwrap_or_else(|| "weft-cli".into()), "command": cx.args.opt("command").unwrap_or_default(), "change": cx.args.opt("change") }))?;
            show(cx, &v, |v| {
                format!(
                    "{}  {} {} bytes from {}\n",
                    v["id"].as_str().unwrap_or(""),
                    v["name"].as_str().unwrap_or(""),
                    v["size"],
                    v["built_from"].as_str().unwrap_or("")
                )
            });
            Ok(())
        }
        ("artifact", "get") => {
            let c = cx.client()?;
            let id = cx.args.arg(2, "an artifact id")?;
            let bytes = c.get_bytes(&format!("/artifacts/{id}/content"))?;
            let out = cx.args.opt("out").unwrap_or_else(|| id.clone());
            std::fs::write(&out, &bytes).map_err(|e| format!("{out}: {e}"))?;
            println!("{out}: {} bytes", bytes.len());
            Ok(())
        }
        ("artifact", _) | ("artifacts", _) => {
            let c = cx.client()?;
            let mut url = "/artifacts".to_string();
            if let Some(r) = cx.args.opt("revision") {
                url.push_str(&format!("?revision={r}"));
            } else if let Some(r) = cx.args.opt("release") {
                url.push_str(&format!("?release={r}"));
            }
            let v = c.get(&url)?;
            show(cx, &v, |v| {
                let mut out = String::new();
                for a in v["artifacts"].as_array().into_iter().flatten() {
                    let a = &a["artifact"];
                    out.push_str(&format!(
                        "{:<34} {:<16} {:<16} {:>10}  {}\n",
                        a["id"].as_str().unwrap_or(""),
                        a["name"].as_str().unwrap_or(""),
                        a["target"].as_str().unwrap_or(""),
                        a["size"],
                        a["built_from"].as_str().unwrap_or("")
                    ));
                }
                out
            });
            Ok(())
        }
        ("trace", _) => {
            let c = cx.client()?;
            let v = c.get(&format!("/changes/{}/trace", cx.args.arg(1, "a change")?))?;
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            Ok(())
        }
        _ => Err(format!(
            "no command `{}` (weft help lists them)",
            cmd.join(" ")
        )),
    }
}
