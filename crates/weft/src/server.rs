//! `weft serve`: the history server on the hub.
//!
//! It listens on loopback behind the machine's internal token, so only the plaza's own daemons reach it; apid is
//! its door for everyone else and names who each request is for in [`wire::CALLER`]. Writes are signed statements
//! (`POST records`), checked by the store; the hub's own statements are signed here with the hub key, which apid
//! hands over at start and which is held in memory only. Reads follow the path rules. STOP ALL refuses every write.
//!
//! Routes, under `/v1`:
//!
//! | Route | Does |
//! |---|---|
//! | `GET health`, `GET state` | liveness; plaza, owner key, counts |
//! | `GET repos`, `GET repos/{repo}` | repos with heads, queues, rules, connections, releases |
//! | `GET repos/{repo}/log?q=&limit=` | changes matching a query, newest first |
//! | `GET repos/{repo}/queue?ref=` | a landing queue |
//! | `GET repos/{repo}/blame?path=&ref=&at=` | who wrote each line |
//! | `GET repos/{repo}/symbols/{id}?ref=` | a definition's history across renames and moves |
//! | `GET changes/{change}` | a change: revisions, intents, records, queue and export state |
//! | `GET revisions/{rev}`, `…/files`, `…/file?path=`, `…/tree`, `…/diff?path=` | a revision, its files, its change tree, its diff |
//! | `GET ops?since=&limit=`, `GET packs`, `GET keys`, `GET records?subject=` | the op log, packs, keys, signed records |
//! | `GET verify?subject=&deep=` | the verification report |
//! | `GET events` | what happens, as server-sent events |
//! | `POST records` | append a signed statement |
//! | `POST hub {kind, body}` | a statement the hub signs (apid only) |
//! | `POST console {kind, body}` | the owner's ruling or edit from a console, signed by the hub for the owner |
//! | `POST lanes` | open a lane (apid only) |
//! | `POST stop {on}` | STOP ALL on or off (apid only) |
//! | `wire/…` | objects in and out ([`crate::wire`]) |

use std::collections::{BTreeSet, HashMap};
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use apz_http::auth::InternalToken;
use apz_http::{Handle, Limits, Params, Request, Response, Router, Server};
use serde_json::{Value, json};

use crate::derived::{Derived, glob};
use crate::diff;
use crate::hash::Hash;
use crate::keys::KeyPair;
use crate::model::*;
use crate::query;
use crate::record::{Actor, Signed, now_ms};
use crate::state::{Change, ChangeState, State};
use crate::store::Store;
use crate::wire;

/// The hub's Weft: the store, its derived data, and STOP ALL.
pub struct Hub {
    pub store: Store,
    pub derived: Derived,
    pub lander: crate::landing::Lander,
    stopped: AtomicBool,
    /// Every (repo, path) a blob has appeared at, for path rules on object reads.
    paths_of: Mutex<(u64, HashMap<Hash, BTreeSet<(String, String)>>)>,
}

impl Hub {
    pub fn open(dir: &Path, hub_key: Option<KeyPair>) -> Result<Arc<Hub>, String> {
        let store = Store::open(dir, hub_key)?;
        let derived = Derived::new(dir.join("derived"));
        Ok(Arc::new(Hub {
            store,
            derived,
            lander: Default::default(),
            stopped: AtomicBool::new(false),
            paths_of: Mutex::new((0, HashMap::new())),
        }))
    }

    pub fn from_store(store: Store) -> Arc<Hub> {
        let derived = Derived::new(store.dir.join("derived"));
        Arc::new(Hub {
            store,
            derived,
            lander: Default::default(),
            stopped: AtomicBool::new(false),
            paths_of: Mutex::new((0, HashMap::new())),
        })
    }

    pub fn stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    pub fn set_stopped(&self, on: bool) {
        self.stopped.store(on, Ordering::SeqCst);
    }

    /// Derives what new revisions need, as they arrive: change trees for seals, history for landings.
    pub fn start_deriving(self: &Arc<Self>) {
        let rx = self.store.subscribe();
        let hub = Arc::downgrade(self);
        std::thread::spawn(move || {
            while let Ok(ev) = rx.recv() {
                let Some(hub) = hub.upgrade() else { return };
                match ev.kind.as_str() {
                    "revision.seal" => {
                        let rev = format!(
                            "{}.r{}",
                            ev.body["change"].as_str().unwrap_or(""),
                            ev.body["n"]
                        );
                        let _ = hub.derived.change_tree(&hub.store, &rev);
                    }
                    "land" | "import" => {
                        if let (Some(repo), Some(r)) =
                            (ev.body["repo"].as_str(), ev.body["ref"].as_str())
                        {
                            let _ = hub.derived.learn(&hub.store, repo, r);
                        }
                    }
                    _ => {}
                }
                hub.derived.runtime.unload_idle(Duration::from_secs(600));
            }
        });
    }

    /// Lands what is queued, as it comes, without waiting on re-checks (a plaza without apid, and tests).
    pub fn start_auto_land(self: &Arc<Self>) {
        let hub = Arc::downgrade(self);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(300));
                let Some(hub) = hub.upgrade() else { return };
                if hub.stopped() {
                    continue;
                }
                let queues: Vec<(String, String)> = {
                    let st = hub.store.state();
                    st.repos
                        .values()
                        .flat_map(|r| {
                            r.queues
                                .iter()
                                .filter(|(_, q)| !q.is_empty())
                                .map(|(k, _)| (r.name.clone(), k.clone()))
                                .collect::<Vec<_>>()
                        })
                        .collect()
                };
                for (repo, reference) in queues {
                    for _ in 0..8 {
                        match hub.lander.step(
                            &hub.store,
                            &hub.derived,
                            &repo,
                            &reference,
                            crate::landing::Options::default(),
                        ) {
                            Ok(crate::landing::Step::Landed { .. }) => continue,
                            Ok(_) => break,
                            Err(e) => {
                                eprintln!("weft: landing {repo}:{reference}: {e}");
                                break;
                            }
                        }
                    }
                }
            }
        });
    }

    /// Whether `caller` may read `path` in `repo`.
    pub fn may_read(&self, st: &State, caller: &Actor, repo: &str, path: &str) -> bool {
        let Some((role, project)) = self.caller_role(st, caller) else {
            return matches!(caller, Actor::Owner | Actor::Hub);
        };
        let Some(r) = st.repos.get(repo) else {
            return false;
        };
        if r.project.is_some() && project.is_some() && r.project != project {
            return false;
        }
        r.rules
            .iter()
            .filter(|rule| glob(&rule.glob, path))
            .all(|rule| {
                rule.read.is_empty() || role.as_ref().is_some_and(|ro| rule.read.contains(ro))
            })
    }

    /// An agent's or node's role and project from its certificate; `None` for the owner and the hub.
    fn caller_role(&self, st: &State, caller: &Actor) -> Option<(Option<String>, Option<String>)> {
        if matches!(caller, Actor::Owner | Actor::Hub) {
            return None;
        }
        let k = st
            .keys
            .values()
            .filter(|k| &k.holder.actor == caller && k.revoked_seq.is_none())
            .max_by_key(|k| k.issued_seq);
        Some(
            k.map(|k| (k.holder.role.clone(), k.holder.project.clone()))
                .unwrap_or((None, Some("\u{0}none".into()))),
        )
    }

    /// Whether `caller` may read the object `hash`: packs and owner/hub always; others where the object appears at a
    /// path they may read.
    pub fn may_read_object(&self, caller: &Actor, hash: &Hash) -> bool {
        if matches!(caller, Actor::Owner | Actor::Hub) {
            return true;
        }
        let st = self.store.state();
        if st
            .packs
            .values()
            .any(|p| p.pack.wasm == *hash || p.pack.tags == *hash)
        {
            return true;
        }
        let mut idx = self.paths_of.lock().unwrap_or_else(|p| p.into_inner());
        let seen = idx.0;
        let mut newest = seen;
        for rev in st.revisions.values().filter(|r| r.seq > seen) {
            let repo = st
                .changes
                .get(&rev.change)
                .map(|c| c.repo.clone())
                .unwrap_or_default();
            for d in &rev.deltas {
                if let Some(h) = d.after {
                    idx.1
                        .entry(h)
                        .or_default()
                        .insert((repo.clone(), d.path.clone()));
                }
            }
            newest = newest.max(rev.seq);
        }
        for a in st.artifacts.values().filter(|a| a.seq > seen) {
            idx.1.entry(a.artifact.digest).or_default().insert((
                a.artifact.repo.clone(),
                format!("(artifact) {}", a.artifact.name),
            ));
            newest = newest.max(a.seq);
        }
        idx.0 = newest;
        idx.1.get(hash).is_some_and(|places| {
            places
                .iter()
                .any(|(repo, path)| self.may_read(&st, caller, repo, path))
        })
    }
}

/// Serves `hub` on `127.0.0.1:port` (0 picks one) behind `token`.
pub fn spawn(hub: Arc<Hub>, port: u16, token: InternalToken) -> io::Result<Handle> {
    let router = Arc::new(router());
    let handler = move |req: &Request| router.handle(&hub, req);
    Server::bind(port)?
        .limits(Limits {
            max_head: 32 << 10,
            max_body: 24 << 20,
        })
        .spawn(token.guard(handler))
}

type H = Arc<Hub>;

fn ok(v: Value) -> Response {
    Response::json(200, &v)
}

fn fail(status: u16, msg: impl AsRef<str>) -> Response {
    Response::error(status, msg.as_ref())
}

/// A store refusal: 409 for what the state forbids, 403 for who may not, 400 otherwise.
fn refused(e: String) -> Response {
    let status = if e.contains("cannot sign")
        || e.contains("only ")
        || e.contains("revoked")
        || e.contains("unknown key")
        || e.contains("may not")
    {
        403
    } else if e.contains("missing blobs") {
        412
    } else if e.contains("is ")
        || e.contains("expected")
        || e.contains("already")
        || e.contains("next ")
    {
        409
    } else {
        400
    };
    fail(status, e)
}

fn caller(req: &Request) -> Result<Actor, Response> {
    wire::caller(req.header(wire::CALLER)).map_err(|e| fail(400, e))
}

fn hub_only(req: &Request) -> Result<(), Response> {
    match caller(req)? {
        Actor::Hub | Actor::Owner => Ok(()),
        _ => Err(fail(403, "only the hub asks for that")),
    }
}

fn writes_allowed(h: &Hub) -> Result<(), Response> {
    if h.stopped() {
        Err(fail(423, "STOP ALL is on: Weft takes no writes"))
    } else {
        Ok(())
    }
}

fn q(req: &Request, name: &str) -> Option<String> {
    req.query_param(name).map(percent_decode)
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

pub fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~/:".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn router() -> Router<H> {
    Router::new()
        .get("/v1/health", |h: &H, _, _| {
            let st = h.store.state();
            ok(json!({ "ok": true, "plaza": st.plaza, "records": st.ops.len(), "stopped": h.stopped(), "packs_loaded": h.derived.runtime.loaded() }))
        })
        .get("/v1/state", |h: &H, _, _| {
            let st = h.store.state();
            let (blobs, bytes) = h.store.blobs.stats();
            ok(json!({
                "plaza": st.plaza, "owner_key": st.owner.as_ref().map(|k| k.id()), "repos": st.repos.len(), "changes": st.changes.len(),
                "records": st.records.len(), "ops": st.ops.len(), "packs": st.packs.keys().collect::<Vec<_>>(), "blobs": blobs, "blob_bytes": bytes,
                "log_bytes": h.store.log_size(), "stopped": h.stopped(), "hub_key": h.store.hub_public().map(|k| k.id()),
            }))
        })
        .get("/v1/repos", |h: &H, req, _| {
            let who = match caller(req) {
                Ok(c) => c,
                Err(r) => return r,
            };
            let st = h.store.state();
            let repos: Vec<Value> = st.repos.values().filter(|r| h.may_read(&st, &who, &r.name, "")).map(|r| repo_view(&st, r)).collect();
            ok(json!({ "repos": repos }))
        })
        .get("/v1/repos/{repo}", |h: &H, req, p| {
            let who = match caller(req) {
                Ok(c) => c,
                Err(r) => return r,
            };
            let st = h.store.state();
            match st.repos.get(p.get("repo")) {
                Some(r) if h.may_read(&st, &who, &r.name, "") => ok(repo_view(&st, r)),
                _ => fail(404, "no such repo"),
            }
        })
        .get("/v1/repos/{repo}/log", |h: &H, req, p| log_route(h, req, p))
        .get("/v1/repos/{repo}/queue", |h: &H, req, p| {
            let st = h.store.state();
            let Some(r) = st.repos.get(p.get("repo")) else { return fail(404, "no such repo") };
            let reference = q(req, "ref").unwrap_or_else(|| "main".into());
            let items: Vec<Value> = r.queues.get(&reference).into_iter().flatten().filter_map(|id| st.changes.get(id)).map(|c| change_summary(&st, c)).collect();
            ok(json!({ "repo": r.name, "ref": reference, "head": r.head(&reference), "queue": items }))
        })
        .get("/v1/repos/{repo}/blame", |h: &H, req, p| {
            let who = match caller(req) {
                Ok(c) => c,
                Err(r) => return r,
            };
            let Some(path) = q(req, "path") else { return fail(400, "blame needs ?path=") };
            if !h.may_read(&h.store.state(), &who, p.get("repo"), &path) {
                return fail(404, "no such file");
            }
            let reference = q(req, "ref").unwrap_or_else(|| "main".into());
            match h.derived.blame(&h.store, p.get("repo"), &reference, &path, q(req, "at").as_deref()) {
                Ok(lines) => {
                    let symbol = q(req, "symbol");
                    let lines = match symbol {
                        Some(sym) => {
                            let head = h.store.state().repos.get(p.get("repo")).map(|r| r.head(&reference)).unwrap_or_default();
                            let parsed = h.store.tree(&head).ok().and_then(|t| t.get(&path).map(|e| e.hash)).map(|hh| h.derived.parse(&h.store, &path, &hh));
                            let range = parsed.and_then(|pp| pp.symbols.iter().find(|s| s.qualified == sym || s.qualified.ends_with(&format!(" {sym}"))).map(|s| (s.start_line, s.end_line)));
                            match range {
                                Some((a, b)) => lines.into_iter().filter(|l| l.line >= a && l.line <= b).collect(),
                                None => return fail(404, format!("no definition {sym} in {path}")),
                            }
                        }
                        None => lines,
                    };
                    ok(json!({ "path": path, "ref": reference, "lines": lines }))
                }
                Err(e) => fail(404, e),
            }
        })
        .get("/v1/repos/{repo}/symbols/{id}", |h: &H, req, p| {
            let reference = q(req, "ref").unwrap_or_else(|| "main".into());
            match h.derived.history(&h.store, p.get("repo"), &reference, p.get("id")) {
                Ok(hist) => ok(json!({ "id": p.get("id"), "lineage": if p.get("id").starts_with("s:") { p.get("id").to_string() } else { h.derived.lineage(p.get("repo"), p.get("id")) }, "history": hist })),
                Err(e) => fail(404, e),
            }
        })
        .get("/v1/changes/{change}", |h: &H, req, p| {
            let who = match caller(req) {
                Ok(c) => c,
                Err(r) => return r,
            };
            let st = h.store.state();
            match st.change(p.get("change")) {
                Ok(c) if h.may_read(&st, &who, &c.repo, "") => ok(change_view(&st, c)),
                Ok(_) => fail(404, "no such change"),
                Err(e) => fail(404, e),
            }
        })
        .get("/v1/revisions/{rev}", |h: &H, _, p| {
            let st = h.store.state();
            match st.revisions.get(p.get("rev")) {
                Some(r) => ok(json!(r)),
                None => fail(404, format!("no revision {}", p.get("rev"))),
            }
        })
        .get("/v1/revisions/{rev}/files", |h: &H, req, p| {
            let who = match caller(req) {
                Ok(c) => c,
                Err(r) => return r,
            };
            let repo = match rev_repo(h, p.get("rev")) {
                Ok(r) => r,
                Err(e) => return e,
            };
            match h.store.tree(p.get("rev")) {
                Ok(tree) => {
                    let st = h.store.state();
                    let files: Vec<Value> = tree
                        .iter()
                        .filter(|(path, _)| h.may_read(&st, &who, &repo, path))
                        .map(|(path, e)| json!({ "path": path, "hash": e.hash, "mode": e.mode, "size": h.store.blobs.size(&e.hash) }))
                        .collect();
                    ok(json!({ "revision": p.get("rev"), "repo": repo, "files": files }))
                }
                Err(e) => fail(404, e),
            }
        })
        .get("/v1/revisions/{rev}/file", |h: &H, req, p| {
            let who = match caller(req) {
                Ok(c) => c,
                Err(r) => return r,
            };
            let Some(path) = q(req, "path") else { return fail(400, "file needs ?path=") };
            let repo = match rev_repo(h, p.get("rev")) {
                Ok(r) => r,
                Err(e) => return e,
            };
            if !h.may_read(&h.store.state(), &who, &repo, &path) {
                return fail(404, "no such file");
            }
            match h.store.read(p.get("rev"), &path) {
                Ok(Some(b)) => Response::new(200, "application/octet-stream", b),
                Ok(None) => fail(404, "no such file"),
                Err(e) => fail(404, e),
            }
        })
        .get("/v1/revisions/{rev}/tree", |h: &H, req, p| {
            let who = match caller(req) {
                Ok(c) => c,
                Err(r) => return r,
            };
            let repo = match rev_repo(h, p.get("rev")) {
                Ok(r) => r,
                Err(e) => return e,
            };
            match h.derived.change_tree(&h.store, p.get("rev")) {
                Ok(mut t) => {
                    let st = h.store.state();
                    t.files.retain(|f| h.may_read(&st, &who, &repo, &f.path));
                    ok(json!(t))
                }
                Err(e) => fail(404, e),
            }
        })
        .get("/v1/revisions/{rev}/diff", |h: &H, req, p| diff_route(h, req, p))
        .get("/v1/ops", |h: &H, req, _| {
            let st = h.store.state();
            let since: u64 = q(req, "since").and_then(|s| s.parse().ok()).unwrap_or(0);
            let limit: usize = q(req, "limit").and_then(|s| s.parse().ok()).unwrap_or(200);
            let ops: Vec<&crate::state::OpInfo> = st.ops.iter().filter(|o| o.seq > since).collect();
            let skip = ops.len().saturating_sub(limit);
            ok(json!({ "ops": ops[skip..] }))
        })
        .get("/v1/packs", |h: &H, _, _| {
            let st = h.store.state();
            ok(json!({ "packs": st.packs.values().collect::<Vec<_>>(), "loaded": h.derived.runtime.loaded(), "loads": h.derived.runtime.loads() }))
        })
        .get("/v1/keys", |h: &H, _, _| {
            let st = h.store.state();
            let mut keys: Vec<&crate::state::KeyInfo> = st.keys.values().collect();
            keys.sort_by_key(|k| k.issued_seq);
            ok(json!({ "owner": st.owner.as_ref().map(|k| k.id()), "keys": keys }))
        })
        .get("/v1/records", |h: &H, req, _| {
            let Some(subject) = q(req, "subject") else { return fail(400, "records needs ?subject=") };
            match crate::verify::records_of(&h.store, &subject) {
                Ok(recs) => ok(json!({ "subject": subject, "records": recs })),
                Err(e) => fail(404, e),
            }
        })
        .get("/v1/verify", |h: &H, req, _| {
            let deep = q(req, "deep").is_some_and(|d| d == "1" || d == "true");
            match q(req, "subject") {
                Some(s) => match crate::verify::verify_subject(&h.store, &s) {
                    Ok(r) => ok(json!(r)),
                    Err(e) => fail(409, e),
                },
                None if deep => match h.store.verify_deep() {
                    Ok(r) => ok(json!({ "ok": true, "deep": r })),
                    Err(e) => fail(409, e),
                },
                None => match crate::verify::verify_chain(&h.store) {
                    Ok(r) => ok(json!(r)),
                    Err(e) => fail(409, e),
                },
            }
        })
        .get("/v1/events", |h: &H, _, _| {
            let rx = h.store.subscribe();
            Response::sse(move |sse| {
                loop {
                    match rx.recv_timeout(Duration::from_secs(15)) {
                        Ok(ev) => sse.event(&format!("weft_{}", ev.kind.replace('.', "_")), &json!({ "seq": ev.seq, "kind": ev.kind, "summary": ev.summary, "body": ev.body }).to_string())?,
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => sse.heartbeat()?,
                        Err(_) => return Ok(()),
                    }
                }
            })
        })
        .post("/v1/records", |h: &H, req, _| {
            if let Err(r) = writes_allowed(h) {
                return r;
            }
            let v: Value = match req.json() {
                Ok(v) => v,
                Err(e) => return fail(400, format!("not JSON: {e}")),
            };
            let signed = match Signed::from_json(&v) {
                Ok(s) => s,
                Err(e) => return fail(400, e),
            };
            match h.store.append(signed) {
                Ok((rec, summary)) => ok(json!({ "seq": rec.seq, "summary": summary })),
                Err(e) => refused(e),
            }
        })
        .post("/v1/records/unsigned", |_: &H, _, _| fail(403, "weft serve takes signed statements only; an agent's go through apid, which signs with the agent's key"))
        .get("/v1/repos/{repo}/map", |h: &H, _, p| {
            let repo = p.get("repo");
            let st_repo = match h.store.state().repo(repo) {
                Ok(r) => repo_view(&h.store.state(), r),
                Err(e) => return fail(404, e),
            };
            let (map, drafted) = match crate::map::load(&h.store, repo, None) {
                Ok(Some(m)) => (m, false),
                Ok(None) => {
                    let head = h.store.state().repos.get(repo).map(|r| r.head("main")).unwrap_or_else(|| "root".into());
                    match h.store.tree(&head) {
                        Ok(t) => (crate::map::draft(&h.store, &t), true),
                        Err(e) => return fail(404, e),
                    }
                }
                Err(e) => return fail(409, e),
            };
            ok(json!({ "repo": repo, "project": st_repo["project"], "map": map, "drafted": drafted, "connections": st_repo["connections"], "releases": st_repo["releases"] }))
        })
        .get("/v1/repos/{repo}/map/draft", |h: &H, _, p| {
            let head = match h.store.state().repo(p.get("repo")) {
                Ok(r) => r.head("main"),
                Err(e) => return fail(404, e),
            };
            match h.store.tree(&head) {
                Ok(t) => {
                    let m = crate::map::draft(&h.store, &t);
                    ok(json!({ "map": m, "toml": m.to_toml() }))
                }
                Err(e) => fail(404, e),
            }
        })
        .get("/v1/repos/{repo}/map/where", |h: &H, req, p| {
            let question = q(req, "q").unwrap_or_default();
            match crate::map::answer(&h.store, &h.derived, p.get("repo"), &question) {
                Ok(hits) => ok(json!({ "question": question, "hits": hits })),
                Err(e) => fail(404, e),
            }
        })
        .post("/v1/repos/{repo}/queue/step", |h: &H, req, p| {
            if let Err(r) = hub_only(req).and_then(|_| writes_allowed(h)) {
                return r;
            }
            let v: Value = req.json().unwrap_or(Value::Null);
            let reference = v["ref"].as_str().unwrap_or("main").to_string();
            let mut opts = crate::landing::Options { recheck: v["recheck"].as_bool().unwrap_or(false), ..Default::default() };
            if let Some(ms) = v["stall_ms"].as_u64() {
                opts.stall_ms = ms;
            }
            match h.lander.step(&h.store, &h.derived, p.get("repo"), &reference, opts) {
                Ok(step) => ok(json!(step)),
                Err(e) => refused(e),
            }
        })
        .post("/v1/repos/{repo}/queue/report", |h: &H, req, _| {
            if let Err(r) = hub_only(req) {
                return r;
            }
            let v: Value = req.json().unwrap_or(Value::Null);
            let Ok(digest) = serde_json::from_value::<Hash>(v["digest"].clone()) else { return fail(400, "report needs a digest") };
            if h.lander.report(&digest, v["ok"].as_bool().unwrap_or(false)) { ok(json!({ "reported": digest })) } else { fail(404, "no such candidate") }
        })
        .get("/v1/repos/{repo}/queue/candidates", |h: &H, req, p| {
            let reference = q(req, "ref").unwrap_or_else(|| "main".into());
            ok(json!({ "candidates": h.lander.candidates(p.get("repo"), &reference) }))
        })
        .get("/v1/repos/{repo}/refs/{ref}", |h: &H, _, p| match crate::landing::chain(&h.store, p.get("repo"), p.get("ref")) {
            Ok(c) => ok(json!({ "repo": p.get("repo"), "ref": p.get("ref"), "revisions": c })),
            Err(e) => fail(404, e),
        })
        .post("/v1/changes/{change}/revert", |h: &H, req, p| {
            if let Err(r) = hub_only(req).and_then(|_| writes_allowed(h)) {
                return r;
            }
            let v: Value = req.json().unwrap_or(Value::Null);
            let author = match caller(req) {
                Ok(a) => a,
                Err(r) => return r,
            };
            match crate::landing::revert(&h.store, &h.derived, p.get("change"), author, v["reason"].as_str().unwrap_or("")) {
                Ok(name) => ok(json!({ "change": name, "reverts": p.get("change") })),
                Err(e) => refused(e),
            }
        })
        .post("/v1/hub", |h: &H, req, _| {
            if let Err(r) = hub_only(req).and_then(|_| writes_allowed(h)) {
                return r;
            }
            let v: Value = match req.json() {
                Ok(v) => v,
                Err(e) => return fail(400, format!("not JSON: {e}")),
            };
            let Some(kind) = v["kind"].as_str() else { return fail(400, "kind is required") };
            match h.store.hub_append(kind, v["body"].clone()) {
                Ok((rec, summary)) => ok(json!({ "seq": rec.seq, "summary": summary })),
                Err(e) => refused(e),
            }
        })
        .post("/v1/console", |h: &H, req, _| {
            if let Err(r) = writes_allowed(h) {
                return r;
            }
            if !matches!(caller(req), Ok(Actor::Owner)) {
                return fail(403, "only the owner's console signs as the owner");
            }
            let v: Value = match req.json() {
                Ok(v) => v,
                Err(e) => return fail(400, format!("not JSON: {e}")),
            };
            let Some(kind) = v["kind"].as_str() else { return fail(400, "kind is required") };
            match h.store.console_owner_append(kind, v["body"].clone()) {
                Ok((rec, summary)) => ok(json!({ "seq": rec.seq, "summary": summary })),
                Err(e) => refused(e),
            }
        })
        .post("/v1/lanes", |h: &H, req, _| {
            if let Err(r) = hub_only(req).and_then(|_| writes_allowed(h)) {
                return r;
            }
            let v: Value = match req.json() {
                Ok(v) => v,
                Err(e) => return fail(400, format!("not JSON: {e}")),
            };
            let author = match serde_json::from_value::<Actor>(v["author"].clone()).or_else(|_| Actor::parse(v["author"].as_str().unwrap_or(""))) {
                Ok(a) => a,
                Err(e) => return fail(400, format!("author: {e}")),
            };
            let s = |k: &str| v[k].as_str().map(str::to_string);
            match h.store.open_lane(
                &s("repo").unwrap_or_default(),
                &s("ref").unwrap_or_else(|| "main".into()),
                s("task").as_deref(),
                &s("title").unwrap_or_default(),
                author,
                s("stacked_on").as_deref(),
                &s("kind").unwrap_or_else(|| "normal".into()),
            ) {
                Ok(change) => {
                    let st = h.store.state();
                    let c = st.change(&change).map(|c| change_view(&st, c)).unwrap_or(Value::Null);
                    ok(json!({ "change": change, "detail": c }))
                }
                Err(e) => refused(e),
            }
        })
        .post("/v1/stop", |h: &H, req, _| {
            if let Err(r) = hub_only(req) {
                return r;
            }
            let on = req.json::<Value>().ok().and_then(|v| v["on"].as_bool()).unwrap_or(true);
            h.set_stopped(on);
            ok(json!({ "stopped": on }))
        })
        // The wire.
        .post("/v1/wire/missing", |h: &H, req, _| {
            let v: Value = match req.json() {
                Ok(v) => v,
                Err(e) => return fail(400, format!("not JSON: {e}")),
            };
            let hashes: Vec<Hash> = match serde_json::from_value(v["hashes"].clone()) {
                Ok(x) => x,
                Err(e) => return fail(400, format!("hashes: {e}")),
            };
            ok(json!({ "missing": h.store.blobs.missing(&hashes) }))
        })
        .route("PUT", "/v1/wire/objects/{hash}", |h: &H, req, p| {
            let Ok(hash) = p.get("hash").parse::<Hash>() else { return fail(400, "not a hash") };
            match h.store.blobs.put_checked(&hash, &req.body) {
                Ok(()) => ok(json!({ "stored": hash })),
                Err(e) => fail(400, e),
            }
        })
        .post("/v1/wire/manifest", |h: &H, req, _| {
            let v: Value = match req.json() {
                Ok(v) => v,
                Err(e) => return fail(400, format!("not JSON: {e}")),
            };
            let (Ok(hash), Ok(chunks)) = (serde_json::from_value::<Hash>(v["hash"].clone()), serde_json::from_value::<Vec<(Hash, u64)>>(v["chunks"].clone())) else {
                return fail(400, "manifest needs hash and chunks");
            };
            match h.store.blobs.put_manifest(&hash, &chunks) {
                Ok(()) => ok(json!({ "stored": hash })),
                Err(e) => fail(400, e),
            }
        })
        .post("/v1/wire/info", |h: &H, req, _| {
            let who = match caller(req) {
                Ok(c) => c,
                Err(r) => return r,
            };
            let hashes: Vec<Hash> = match req.json::<Value>().ok().and_then(|v| serde_json::from_value(v["hashes"].clone()).ok()) {
                Some(x) => x,
                None => return fail(400, "info needs hashes"),
            };
            let mut out = Vec::with_capacity(hashes.len());
            for hash in hashes {
                if !h.may_read_object(&who, &hash) {
                    return fail(404, format!("no object {}", hash.short()));
                }
                match h.store.blobs.chunks(&hash) {
                    Ok(chunks) => out.push(json!({ "hash": hash, "size": h.store.blobs.size(&hash), "chunks": chunks })),
                    Err(e) => return fail(404, e),
                }
            }
            ok(json!({ "objects": out }))
        })
        .get("/v1/wire/chunks/{hash}", |h: &H, req, p| {
            let Ok(hash) = p.get("hash").parse::<Hash>() else { return fail(400, "not a hash") };
            let who = match caller(req) {
                Ok(c) => c,
                Err(r) => return r,
            };
            if !h.may_read_object(&who, &hash) {
                return fail(404, "no such object");
            }
            match h.store.blobs.chunks(&hash) {
                Ok(chunks) => ok(json!({ "hash": hash, "size": h.store.blobs.size(&hash), "chunks": chunks })),
                Err(e) => fail(404, e),
            }
        })
        .post("/v1/wire/bundle", |h: &H, req, _| {
            let who = match caller(req) {
                Ok(c) => c,
                Err(r) => return r,
            };
            let v: Value = match req.json() {
                Ok(v) => v,
                Err(e) => return fail(400, format!("not JSON: {e}")),
            };
            let hashes: Vec<Hash> = match serde_json::from_value(v["hashes"].clone()) {
                Ok(x) => x,
                Err(e) => return fail(400, format!("hashes: {e}")),
            };
            let mut out = Vec::new();
            let mut bytes = 0;
            for hash in hashes {
                if !h.may_read_object(&who, &hash) {
                    return fail(404, format!("no object {}", hash.short()));
                }
                let b = match h.store.blobs.get(&hash) {
                    Ok(b) => b,
                    Err(e) => return fail(404, e),
                };
                if !out.is_empty() && bytes + b.len() > wire::BUNDLE_BYTES {
                    break;
                }
                bytes += b.len();
                out.push((hash, b));
            }
            Response::new(200, "application/octet-stream", wire::encode_bundle(&out)).with_header("Cache-Control", "no-store")
        })
        .get("/v1/wire/objects/{hash}", |h: &H, req, p| {
            let Ok(hash) = p.get("hash").parse::<Hash>() else { return fail(400, "not a hash") };
            let who = match caller(req) {
                Ok(c) => c,
                Err(r) => return r,
            };
            if !h.may_read_object(&who, &hash) {
                return fail(404, "no such object");
            }
            match h.store.blobs.get(&hash) {
                Ok(b) => match req.header("range").and_then(|r| wire::parse_range(r, b.len())) {
                    Some((a, z)) => Response::new(206, "application/octet-stream", b[a..z].to_vec()).with_header("Content-Range", &format!("bytes {a}-{}/{}", z - 1, b.len())),
                    None => Response::new(200, "application/octet-stream", b).with_header("Cache-Control", "public, max-age=31536000, immutable"),
                },
                Err(e) => fail(404, e),
            }
        })
        .post("/v1/wire/want", |h: &H, req, _| {
            // A revision's files, so the client can ask for the objects it lacks.
            let who = match caller(req) {
                Ok(c) => c,
                Err(r) => return r,
            };
            let v: Value = match req.json() {
                Ok(v) => v,
                Err(e) => return fail(400, format!("not JSON: {e}")),
            };
            if let Some(d) = v["candidate"].as_str() {
                // A landing candidate's tree, for the re-check.
                let Ok(digest) = d.parse::<Hash>() else { return fail(400, "not a digest") };
                return match h.lander.candidate_tree(&h.store, &digest) {
                    Ok(tree) => ok(json!({ "candidate": d, "files": tree.iter().map(|(path, e)| json!({ "path": path, "hash": e.hash, "mode": e.mode })).collect::<Vec<_>>() })),
                    Err(e) => fail(404, e),
                };
            }
            let Some(rev) = v["revision"].as_str() else { return fail(400, "want needs a revision") };
            let repo = match rev_repo(h, rev) {
                Ok(r) => r,
                Err(e) => return e,
            };
            let paths: Option<Vec<String>> = serde_json::from_value(v["paths"].clone()).ok();
            match h.store.tree(rev) {
                Ok(tree) => {
                    let st = h.store.state();
                    let files: Vec<Value> = tree
                        .iter()
                        .filter(|(path, _)| h.may_read(&st, &who, &repo, path))
                        .filter(|(path, _)| paths.as_ref().is_none_or(|ps| ps.iter().any(|g| glob(g, path) || path.starts_with(&format!("{}/", g.trim_end_matches('/'))))))
                        .map(|(path, e)| json!({ "path": path, "hash": e.hash, "mode": e.mode }))
                        .collect();
                    ok(json!({ "revision": rev, "repo": repo, "files": files }))
                }
                Err(e) => fail(404, e),
            }
        })
}

fn rev_repo(h: &Hub, rev: &str) -> Result<String, Response> {
    if rev == "root" {
        return Ok(String::new());
    }
    let st = h.store.state();
    let r = st
        .revisions
        .get(rev)
        .ok_or_else(|| fail(404, format!("no revision {rev}")))?;
    Ok(st
        .changes
        .get(&r.change)
        .map(|c| c.repo.clone())
        .unwrap_or_default())
}

fn log_route(h: &Hub, req: &Request, p: &Params) -> Response {
    let repo = p.get("repo").to_string();
    if !h.store.state().repos.contains_key(&repo) {
        return fail(404, "no such repo");
    }
    let text = q(req, "q")
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "all()".into());
    let parsed = match query::parse(&text) {
        Ok(x) => x,
        Err(e) => return fail(400, format!("query: {e}")),
    };
    let limit: usize = q(req, "limit").and_then(|s| s.parse().ok()).unwrap_or(100);
    // What the query needs from derived data, computed before the state is locked for evaluation.
    let latest = |st: &State, id: u64| {
        st.changes.get(&id).and_then(|c| {
            c.landed
                .clone()
                .or_else(|| c.rounds.last().cloned())
                .or_else(|| c.drafts.last().cloned())
        })
    };
    let mut symbols: HashMap<u64, Vec<String>> = HashMap::new();
    if query::calls(&parsed, "touches")
        .iter()
        .any(|a| a.contains("::"))
    {
        let revs: Vec<(u64, String)> = {
            let st = h.store.state();
            st.changes
                .values()
                .filter(|c| c.repo == repo)
                .filter_map(|c| latest(&st, c.id).map(|r| (c.id, r)))
                .collect()
        };
        for (id, rev) in revs {
            if let Ok(t) = h.derived.cached_tree(&h.store, &rev) {
                symbols.insert(
                    id,
                    t.files
                        .iter()
                        .flat_map(|f| f.symbols.iter().map(|s| s.id.clone()))
                        .collect(),
                );
            }
        }
    }
    let components: HashMap<String, Vec<String>> = query::calls(&parsed, "component")
        .into_iter()
        .map(|c| {
            (
                c.to_string(),
                crate::map::component_globs(&h.store, &repo, c),
            )
        })
        .collect();
    let st = h.store.state();
    let paths = |st: &State, id: u64| -> Vec<String> {
        latest(st, id)
            .and_then(|r| st.revisions.get(&r))
            .map(|r| r.deltas.iter().map(|d| d.path.clone()).collect())
            .unwrap_or_default()
    };
    let syms = |id: u64| symbols.get(&id).cloned().unwrap_or_default();
    let comp = |name: &str| components.get(name).cloned().unwrap_or_default();
    let cx = query::Context {
        state: &st,
        repo: Some(&repo),
        now_ms: now_ms(),
        paths: &paths,
        symbols: &syms,
        component: &comp,
    };
    match query::eval(&parsed, &cx) {
        Ok(ids) => {
            let mut changes: Vec<&Change> =
                ids.iter().filter_map(|id| st.changes.get(id)).collect();
            changes.sort_by_key(|c| {
                std::cmp::Reverse(
                    c.landed
                        .as_ref()
                        .and_then(|l| st.revisions.get(l))
                        .map(|r| r.seq)
                        .unwrap_or(c.opened_seq),
                )
            });
            let total = changes.len();
            let list: Vec<Value> = changes
                .into_iter()
                .take(limit)
                .map(|c| change_summary(&st, c))
                .collect();
            ok(json!({ "repo": repo, "query": text, "total": total, "changes": list }))
        }
        Err(e) => fail(400, format!("query: {e}")),
    }
}

fn diff_route(h: &Hub, req: &Request, p: &Params) -> Response {
    let who = match caller(req) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let rev = p.get("rev");
    let repo = match rev_repo(h, rev) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let base_id = match q(req, "base") {
        Some(b) => b,
        None => match h.store.state().revisions.get(rev) {
            Some(r) => r.base.clone(),
            None => return fail(404, "no such revision"),
        },
    };
    let (base, after) = match (h.store.tree(&base_id), h.store.tree(rev)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return fail(404, e),
    };
    let only = q(req, "path");
    let deltas: Vec<Delta> = {
        let st = h.store.state();
        Store::deltas_between(&base, &after)
            .into_iter()
            .filter(|d| {
                only.as_ref().is_none_or(|o| o == &d.path) && h.may_read(&st, &who, &repo, &d.path)
            })
            .collect()
    };
    let mut files = Vec::new();
    for d in deltas {
        let old = base
            .get(&d.path)
            .and_then(|e| h.store.blobs.get(&e.hash).ok())
            .unwrap_or_default();
        let new = d
            .after
            .and_then(|x| h.store.blobs.get(&x).ok())
            .unwrap_or_default();
        if diff::is_text(&old) && diff::is_text(&new) {
            let (o, n) = (String::from_utf8_lossy(&old), String::from_utf8_lossy(&new));
            let (add, del) = diff::counts(&o, &n);
            files.push(json!({ "path": d.path, "add": add, "del": del, "unified": diff::unified(&d.path, &o, &n), "hunks": diff::hunks(&o, &n, 3) }));
        } else {
            files.push(
                json!({ "path": d.path, "binary": true, "before": old.len(), "after": new.len() }),
            );
        }
    }
    ok(json!({ "revision": rev, "base": base_id, "files": files }))
}

pub fn repo_view(st: &State, r: &crate::state::Repo) -> Value {
    let refs: serde_json::Map<String, Value> = r
        .refs
        .keys()
        .map(|k| (k.clone(), json!(r.head(k))))
        .collect();
    let mut refs = refs;
    refs.entry("main").or_insert(json!(r.head("main")));
    let queues: serde_json::Map<String, Value> = r
        .queues
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                json!(
                    v.iter()
                        .filter_map(|id| st.changes.get(id).map(|c| c.name.clone()))
                        .collect::<Vec<_>>()
                ),
            )
        })
        .collect();
    let open = st
        .changes
        .values()
        .filter(|c| {
            c.repo == r.name && !matches!(c.state, ChangeState::Landed | ChangeState::Abandoned)
        })
        .count();
    let landed = r.refs.get("main").map(|v| v.len()).unwrap_or(0);
    json!({
        "name": r.name, "project": r.project, "description": r.description, "refs": refs, "queues": queues, "rules": r.rules,
        "connections": r.connections.values().map(|c| json!({ "id": c.conn.id, "remote": c.conn.remote, "platform": c.conn.platform, "branch": c.conn.branch, "state": c.state, "detail": c.detail, "head": c.head, "exported": c.exported.len(), "author": format!("{} <{}>", c.conn.author_name, c.conn.author_email) })).collect::<Vec<_>>(),
        "releases": r.releases.values().map(|x| json!({ "version": x.propose.version, "signed": x.signed.is_some(), "from": x.propose.from })).collect::<Vec<_>>(),
        "open_changes": open, "landed": landed,
    })
}

pub fn change_summary(st: &State, c: &Change) -> Value {
    let latest = c
        .landed
        .clone()
        .or_else(|| c.rounds.last().cloned())
        .or_else(|| c.drafts.last().cloned());
    let files = latest
        .as_ref()
        .and_then(|r| st.revisions.get(r))
        .map(|r| r.deltas.len())
        .unwrap_or(0);
    let at = latest
        .as_ref()
        .and_then(|r| st.revisions.get(r))
        .map(|r| r.at_ms)
        .unwrap_or(c.opened_ms);
    json!({
        "change": c.name, "repo": c.repo, "ref": c.reference, "task": c.task, "title": c.title, "author": c.author.label(), "state": c.state,
        "kind": c.kind, "rounds": c.rounds.len(), "drafts": c.drafts.len(), "files": files, "latest": latest, "landed": c.landed, "at_ms": at,
        "intents": c.intents.values().last().map(|v| v.iter().map(|i| json!({ "title": i.title, "kind": i.kind })).collect::<Vec<_>>()).unwrap_or_default(),
        "imported": c.import.as_ref().map(|i| json!({ "git": i.git, "author": format!("{} <{}>", i.author_name, i.author_email) })),
    })
}

pub fn change_view(st: &State, c: &Change) -> Value {
    let revs: Vec<Value> = c
        .drafts
        .iter()
        .chain(&c.rounds)
        .chain(c.landed.iter())
        .filter_map(|r| st.revisions.get(r))
        .map(|r| json!({ "id": r.id, "kind": r.kind, "n": r.n, "base": r.base, "files": r.deltas.len(), "digest": r.digest, "actor": r.actor.label(), "model": r.model, "turn": r.turn, "tool_calls": r.tool_calls, "seq": r.seq, "at_ms": r.at_ms }))
        .collect();
    let records: Vec<Value> = c
        .records
        .iter()
        .filter_map(|s| st.records.get(s))
        .map(|r| json!({ "seq": r.seq, "type": r.attest.predicate_type, "revision": r.attest.revision, "actor": r.actor.label(), "key": r.key_id, "via_console": r.via_console, "at_ms": r.at_ms, "predicate": r.attest.predicate }))
        .collect();
    let queue_pos = st
        .repos
        .get(&c.repo)
        .and_then(|r| r.queues.get(&c.reference))
        .and_then(|q| q.iter().position(|id| *id == c.id));
    json!({
        "change": c.name, "id": c.id, "repo": c.repo, "ref": c.reference, "task": c.task, "title": c.title, "author": c.author.label(), "state": c.state,
        "kind": c.kind, "base": c.base, "stacked_on": c.stacked_on.and_then(|s| st.changes.get(&s).map(|x| x.name.clone())), "reverts": c.reverts.and_then(|s| st.changes.get(&s).map(|x| x.name.clone())),
        "revisions": revs, "drafts": c.drafts, "rounds": c.rounds, "landed": c.landed, "intents": c.intents, "records": records,
        "conflict": c.conflict, "queue_position": queue_pos, "import": c.import, "exported": c.exported, "opened_ms": c.opened_ms,
    })
}
