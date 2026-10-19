//! The CLI's commands for landing, git, releases and artifacts.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::cli::Ctx;
use crate::hash::Hash;

pub fn run(cx: &Ctx, cmd: &[String]) -> Result<(), String> {
    let word = |i: usize| cmd.get(i).map(String::as_str).unwrap_or("");
    match (word(0), word(1)) {
        ("queue", "add") => {
            let c = cx.client()?;
            let change = cx.args.arg(2, "a change")?;
            let detail = c.get(&format!("/changes/{change}"))?;
            let n = detail["rounds"].as_array().map(|r| r.len()).unwrap_or(0);
            let v = c.post("/hub", &json!({ "kind": "queue.add", "body": { "repo": detail["repo"], "ref": detail["ref"], "change": change, "n": n } }))?;
            println!("{}", v["summary"].as_str().unwrap_or("queued"));
            Ok(())
        }
        ("queue", _) => {
            let c = cx.client()?;
            let repo = repo(cx)?;
            let r = cx.args.opt("ref").unwrap_or_else(|| "main".into());
            let v = c.get(&format!("/repos/{repo}/queue?ref={r}"))?;
            let cands = c.get(&format!("/repos/{repo}/queue/candidates?ref={r}"))?;
            if cx.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({ "queue": v, "candidates": cands }))
                        .unwrap()
                );
                return Ok(());
            }
            println!("{repo}:{r} at {}", v["head"].as_str().unwrap_or("root"));
            for (i, ch) in v["queue"].as_array().into_iter().flatten().enumerate() {
                let cand = cands["candidates"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|c| c["change"] == ch["change"]);
                let state = match cand {
                    Some(c) if c["checked"].is_null() => {
                        "merged, waiting on its re-check".to_string()
                    }
                    Some(c) => format!("merged, checked {}", c["checked"]),
                    None => String::new(),
                };
                println!(
                    "  {}. {} {}  {}",
                    i + 1,
                    ch["change"].as_str().unwrap_or(""),
                    ch["title"].as_str().unwrap_or(""),
                    state
                );
            }
            Ok(())
        }
        ("land", _) => {
            let c = cx.client()?;
            let repo = repo(cx)?;
            let r = cx.args.opt("ref").unwrap_or_else(|| "main".into());
            loop {
                let v = c.post(
                    &format!("/repos/{repo}/queue/step"),
                    &json!({ "ref": r, "recheck": cx.args.has("recheck") }),
                )?;
                match v["outcome"].as_str() {
                    Some("landed") => println!(
                        "{} landed as {}{}",
                        v["change"].as_str().unwrap_or(""),
                        v["revision"].as_str().unwrap_or(""),
                        merged_note(&v)
                    ),
                    Some("empty") => {
                        println!("{repo}:{r}: the queue is empty");
                        return Ok(());
                    }
                    Some("conflict") => {
                        println!(
                            "{} clashes on {}; it went back to its lane with the files marked",
                            v["change"].as_str().unwrap_or(""),
                            list(&v["files"])
                        );
                        return Ok(());
                    }
                    _ => {
                        println!("{}", serde_json::to_string(&v).unwrap_or_default());
                        return Ok(());
                    }
                }
            }
        }
        ("revert", _) => {
            let c = cx.client()?;
            let change = cx.args.arg(1, "a landed change")?;
            let v = c.post(
                &format!("/changes/{change}/revert"),
                &json!({ "reason": cx.args.opt("reason").unwrap_or_default() }),
            )?;
            println!(
                "{} reverts {} (queued)",
                v["change"].as_str().unwrap_or(""),
                change
            );
            Ok(())
        }
        ("bisect", _) => bisect(cx),
        _ => crate::cli_git::run(cx, cmd),
    }
}

fn repo(cx: &Ctx) -> Result<String, String> {
    if let Some(r) = cx.args.opt("repo") {
        return Ok(r);
    }
    crate::workdir::Workdir::find(&cx.cwd)
        .map(|w| w.lane().map(|l| l.repo))
        .unwrap_or(Err("--repo is required".into()))
}

fn list(v: &Value) -> String {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn merged_note(v: &Value) -> String {
    let m = list(&v["merged"]);
    if m.is_empty() {
        String::new()
    } else {
        format!(" (merged: {m})")
    }
}

/// `weft bisect`: the first landed change on a ref after which `--check` fails, by halving.
fn bisect(cx: &Ctx) -> Result<(), String> {
    let c = cx.client()?;
    let repo = repo(cx)?;
    let r = cx.args.opt("ref").unwrap_or_else(|| "main".into());
    let check = cx.args.need("check")?;
    let v = c.get(&format!("/repos/{repo}/refs/{r}"))?;
    let revs: Vec<Value> = v["revisions"].as_array().cloned().unwrap_or_default();
    if revs.is_empty() {
        return Err("nothing has landed".into());
    }
    let pos = |id: &str| revs.iter().position(|x| x["revision"].as_str() == Some(id));
    let mut good: i64 = match cx.args.opt("good") {
        Some(g) => pos(&g).ok_or_else(|| format!("{g} is not on {r}"))? as i64,
        None => -1,
    };
    let mut bad: usize = match cx.args.opt("bad") {
        Some(b) => pos(&b).ok_or_else(|| format!("{b} is not on {r}"))?,
        None => revs.len() - 1,
    };
    let work = std::env::temp_dir().join(format!("weft-bisect-{}", std::process::id()));
    let objects = crate::cli::dirs_cache().join("objects");
    let run = |i: usize| -> Result<bool, String> {
        let rev = revs[i]["revision"].as_str().unwrap_or("");
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
        let listing = c.post("/wire/want", &json!({ "revision": rev }))?;
        let files: Vec<(String, Hash, u32)> = listing["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| {
                Some((
                    f["path"].as_str()?.to_string(),
                    serde_json::from_value(f["hash"].clone()).ok()?,
                    f["mode"].as_u64().unwrap_or(0o100644) as u32,
                ))
            })
            .collect();
        let w = crate::workdir::Workdir { root: work.clone() };
        let have = |h: &Hash| {
            std::fs::read(objects.join(h.hex()))
                .ok()
                .filter(|b| Hash::of(b) == *h)
        };
        let lacking: Vec<Hash> = files
            .iter()
            .map(|f| f.1)
            .filter(|h| have(h).is_none())
            .collect();
        std::fs::create_dir_all(&objects).map_err(|e| e.to_string())?;
        c.download(&lacking, &mut |h, b| {
            std::fs::write(objects.join(h.hex()), b).map_err(|e| e.to_string())
        })?;
        for (p, h, mode) in &files {
            w.write_file(p, &have(h).ok_or("an object did not arrive")?, *mode)?;
        }
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(&check)
            .current_dir(&work)
            .status()
            .map_err(|e| e.to_string())?;
        println!(
            "  {} {}  {}",
            if status.success() { "good" } else { "bad " },
            rev,
            revs[i]["title"].as_str().unwrap_or("")
        );
        Ok(status.success())
    };
    if run(bad)? {
        return Err(format!(
            "the check passes at {}; nothing to find",
            revs[bad]["revision"].as_str().unwrap_or("")
        ));
    }
    while (bad as i64) - good > 1 {
        let mid = ((good + bad as i64) / 2) as usize;
        if run(mid)? {
            good = mid as i64;
        } else {
            bad = mid;
        }
    }
    let _ = std::fs::remove_dir_all(&work);
    let first = &revs[bad];
    println!(
        "first bad: {} ({}, task {})",
        first["change"].as_str().unwrap_or(""),
        first["title"].as_str().unwrap_or(""),
        first["task"].as_str().unwrap_or("-")
    );
    let _ = PathBuf::new();
    Ok(())
}
