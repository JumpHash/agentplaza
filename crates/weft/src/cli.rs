//! The `weft` command line.
//!
//! Where it talks: `--hub <url>` (or `WEFT_HUB`, or the lane's own hub). How it proves itself: the machine's
//! internal token (`--token-file`, `WEFT_TOKEN_FILE`) to reach `weft serve` directly, or an owner token
//! (`--bearer`, `WEFT_BEARER`) through apid. How it signs: with a key (`--key`, `WEFT_KEY`: the owner's device key,
//! or a test agent's), or, in an agent VM, not at all: apid's gateway signs with the agent's key from the vault.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::client::Client;
use crate::hash::Hash;
use crate::keys::KeyPair;
use crate::model::{Entry, IntentDef};
use crate::record::{Actor, Statement};
use crate::workdir::{self, Lane, Workdir};

pub const USAGE: &str = "weft: AgentPlaza's version control

  weft serve --state <dir> [--port N] [--token-file F] [--hub-key F | --hub-key-stdin] [--signing-key F|-] [--port-file F] [--auto-land]
  weft serve --cache --dir <dir> --upstream <url> [--port N] [--token-file F] [--upstream-token-file F] [--max-mb N]
  weft init --state <dir> --plaza <name> --owner-key <file> --hub-key <file>
  weft keys new <file> | keys show <file> | keys certify <public-hex> --actor agent:<cid> [--name N --role R --project P]
  weft repo create <name> [--project P] [--description D] | repos
  weft lane open --repo R --title T --author agent:<cid> [--task T] [--stacked-on w-N]
  weft checkout <change> [--dir D] [--paths a/**,b] | checkout --candidate <digest> [--dir D]    weft status    weft diff [--symbols] [path]
  weft snapshot [--tool-call ID]… [--turn N] [--model M]    weft restore <revision>
  weft describe --intent <title> [--kind K] [--summary S] <path or path::symbol>…
  weft done [--model M]
  weft log [query] [--repo R] [--limit N]    weft show <change>    weft tree <revision>
  weft blame <path>[::symbol] [--repo R] [--at REV]    weft history <path::symbol> [--repo R]
  weft op log [--since N]    weft undo <seq> [--reason R]
  weft verify [subject] [--deep]
  weft pack ls | pack add <pack dir> [--wasm F]
  weft map [question] [--repo R] | map draft [--repo R]
  weft queue [--repo R] | queue add <change> | land [--repo R]
  weft revert <change> [--reason R] | bisect --repo R --check \"<command>\" [--good REV] [--bad REV]
  weft connect <remote> --repo R [--platform github|git] [--branch main] [--author \"Name <email>\"] [--credential NAME]
  weft connections [--repo R] | export <repo> [--dry-run] [--connection ID] | adopt <repo> [--connection ID]
  weft import <git url or path> --repo R [--project P] [--branch main]
  weft release <version> --repo R [--from REV] | releases [--repo R] | release sign <repo>@<version>
  weft artifact ls [--revision R | --release repo@v] | artifact get <id> [--out F] | artifact put <file> --repo R --name N [--target T] [--from REV]
  weft trace <change>    weft state
  Add --json for the raw answer.";

/// Parsed arguments: positionals, `--name value` options (repeatable) and `--switch`es.
pub struct Args {
    pub pos: Vec<String>,
    pub opts: BTreeMap<String, Vec<String>>,
}

const SWITCHES: &[&str] = &[
    "json",
    "deep",
    "symbols",
    "cache",
    "hub-key-stdin",
    "dry-run",
    "force",
    "help",
    "auto-land",
    "recheck",
];

impl Args {
    pub fn parse(raw: &[String]) -> Args {
        let mut pos = Vec::new();
        let mut opts: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut i = 0;
        while i < raw.len() {
            let a = &raw[i];
            if let Some(name) = a.strip_prefix("--") {
                if let Some((n, v)) = name.split_once('=') {
                    opts.entry(n.to_string()).or_default().push(v.to_string());
                } else if SWITCHES.contains(&name) || i + 1 >= raw.len() {
                    opts.entry(name.to_string())
                        .or_default()
                        .push(String::new());
                } else {
                    opts.entry(name.to_string())
                        .or_default()
                        .push(raw[i + 1].clone());
                    i += 1;
                }
            } else {
                pos.push(a.clone());
            }
            i += 1;
        }
        Args { pos, opts }
    }

    pub fn opt(&self, name: &str) -> Option<String> {
        self.opts.get(name).and_then(|v| v.last().cloned())
    }

    pub fn all(&self, name: &str) -> Vec<String> {
        self.opts.get(name).cloned().unwrap_or_default()
    }

    pub fn has(&self, name: &str) -> bool {
        self.opts.contains_key(name)
    }

    pub fn need(&self, name: &str) -> Result<String, String> {
        self.opt(name)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| format!("--{name} is required"))
    }

    pub fn arg(&self, i: usize, what: &str) -> Result<String, String> {
        self.pos
            .get(i)
            .cloned()
            .ok_or_else(|| format!("{what} is required"))
    }
}

/// Who the CLI is and where it talks.
pub struct Ctx {
    pub args: Args,
    pub cwd: PathBuf,
    pub json: bool,
    /// Variables that win over the process environment (an embedded CLI, a test playing a VM).
    pub env: BTreeMap<String, String>,
}

impl Ctx {
    fn env(&self, opt: &str, var: &str) -> Option<String> {
        self.args
            .opt(opt)
            .or_else(|| self.env.get(var).cloned())
            .or_else(|| std::env::var(var).ok())
            .filter(|s| !s.is_empty())
    }

    pub fn hub_url(&self) -> Result<String, String> {
        if let Some(h) = self.env("hub", "WEFT_HUB") {
            return Ok(h);
        }
        if let Some(w) = Workdir::find(&self.cwd) {
            return Ok(w.lane()?.hub);
        }
        Err("no hub: give --hub <url> or set WEFT_HUB".into())
    }

    pub fn client(&self) -> Result<Client, String> {
        let mut headers = Vec::new();
        if let Some(f) = self.env("token-file", "WEFT_TOKEN_FILE") {
            let t = std::fs::read_to_string(&f).map_err(|e| format!("{f}: {e}"))?;
            headers.push((apz_http::auth::HEADER.to_string(), t.trim().to_string()));
        }
        if let Some(a) = self.env("as", "WEFT_AS") {
            headers.push((crate::wire::CALLER.to_string(), a));
        }
        let mut c = Client::new(&self.hub_url()?, self.env("bearer", "WEFT_BEARER"), headers)?;
        // A hub with its own certificate: the SHA-256 of its DER, in hex (`--pin`, `WEFT_PIN`).
        if let Some(pin) = self.env("pin", "WEFT_PIN") {
            let bytes = crate::hash::unhex(pin.trim()).map_err(|_| "the pin is not hex")?;
            c.target.pin = Some(
                bytes
                    .try_into()
                    .map_err(|_| "the pin is a SHA-256 (64 hex digits)")?,
            );
        }
        Ok(c)
    }

    pub fn key(&self) -> Result<Option<KeyPair>, String> {
        match self.env("key", "WEFT_KEY") {
            Some(p) => KeyPair::load(Path::new(&p)).map(Some),
            None => Ok(None),
        }
    }

    /// Says something: signed here with the key, or handed to apid's gateway to sign with the agent's key.
    pub fn say(&self, c: &Client, kind: &str, body: Value) -> Result<Value, String> {
        match self.key()? {
            Some(key) => {
                let actor = self.actor_of(c, &key)?;
                c.post(
                    "/records",
                    &Statement::new(kind, actor, body).sign(&key).to_json(),
                )
            }
            None => c.post("/records/unsigned", &json!({ "kind": kind, "body": body })),
        }
    }

    /// The actor a key signs as: `--as`, else whoever its certificate names (the owner for the owner's key).
    pub fn actor_of(&self, c: &Client, key: &KeyPair) -> Result<Actor, String> {
        if let Some(a) = self.env("as", "WEFT_AS") {
            return Actor::parse(&a);
        }
        let keys = c.get("/keys")?;
        if keys["owner"].as_str() == Some(&key.id()) {
            return Ok(Actor::Owner);
        }
        for k in keys["keys"].as_array().into_iter().flatten() {
            if k["id"].as_str() == Some(&key.id()) {
                return serde_json::from_value(k["holder"]["actor"].clone())
                    .map_err(|e| e.to_string());
            }
        }
        Err("this key is not certified on the hub".into())
    }

    fn print(&self, v: &Value, human: impl FnOnce(&Value) -> String) {
        if self.json {
            println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
        } else {
            print!("{}", human(v));
        }
    }

    fn workdir(&self) -> Result<Workdir, String> {
        Workdir::find(&self.cwd)
            .ok_or_else(|| "not in a lane's directory (run weft checkout first)".into())
    }

    fn repo(&self) -> Result<String, String> {
        if let Some(r) = self.args.opt("repo") {
            return Ok(r);
        }
        if let Some(w) = Workdir::find(&self.cwd) {
            return Ok(w.lane()?.repo);
        }
        Err("--repo is required".into())
    }
}

pub fn run(raw: Vec<String>) -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    run_in(raw, cwd, BTreeMap::new())
}

/// [`run`] from `cwd`, with `env` before the process environment.
pub fn run_in(raw: Vec<String>, cwd: PathBuf, env: BTreeMap<String, String>) -> Result<(), String> {
    let args = Args::parse(&raw);
    let json = args.has("json");
    let cmd: Vec<String> = args.pos.clone();
    let cx = Ctx {
        args,
        cwd,
        json,
        env,
    };
    let word = |i: usize| cmd.get(i).map(String::as_str).unwrap_or("");
    match (word(0), word(1)) {
        ("" | "help", _) => {
            println!("{USAGE}");
            Ok(())
        }
        ("serve", _) => serve(&cx),
        ("init", _) => init(&cx),
        ("keys", "new") => {
            let p = cx.args.arg(2, "a key file")?;
            let k = KeyPair::generate();
            k.save(Path::new(&p))?;
            println!("{} {}", k.id(), k.public().hex());
            Ok(())
        }
        ("keys", "show") => {
            let k = KeyPair::load(Path::new(&cx.args.arg(2, "a key file")?))?;
            println!("{} {}", k.id(), k.public().hex());
            Ok(())
        }
        ("keys", "certify") => {
            let c = cx.client()?;
            let public = cx.args.arg(2, "the public key (hex)")?;
            let actor = Actor::parse(&cx.args.need("actor")?)?;
            let holder = json!({ "actor": actor, "name": cx.args.opt("name"), "role": cx.args.opt("role"), "project": cx.args.opt("project") });
            let caps: Vec<String> = cx.args.all("cap");
            let v = cx.say(
                &c,
                "cert.issue",
                json!({ "key": public, "holder": holder, "caps": caps }),
            )?;
            cx.print(&v, |v| {
                format!("{}\n", v["summary"].as_str().unwrap_or("certified"))
            });
            Ok(())
        }
        ("keys", _) => {
            let c = cx.client()?;
            let v = c.get("/keys")?;
            cx.print(&v, |v| {
                let mut out = format!("owner key {}\n", v["owner"].as_str().unwrap_or("?"));
                for k in v["keys"].as_array().into_iter().flatten() {
                    let state = if k["revoked_seq"].is_null() {
                        "valid"
                    } else {
                        "revoked"
                    };
                    out.push_str(&format!(
                        "  {}  {:<12} {:<10} {:<8} {}\n",
                        k["id"].as_str().unwrap_or(""),
                        label_of(&k["holder"]["actor"]),
                        k["holder"]["name"].as_str().unwrap_or(""),
                        k["holder"]["role"].as_str().unwrap_or(""),
                        state
                    ));
                }
                out
            });
            Ok(())
        }
        ("repo", "create") => {
            let c = cx.client()?;
            let name = cx.args.arg(2, "a repo name")?;
            let v = cx.say(&c, "repo.create", json!({ "repo": name, "project": cx.args.opt("project"), "description": cx.args.opt("description").unwrap_or_default() }))?;
            cx.print(&v, |v| {
                format!("{}\n", v["summary"].as_str().unwrap_or("created"))
            });
            Ok(())
        }
        ("repos", _) => {
            let c = cx.client()?;
            let v = c.get("/repos")?;
            cx.print(&v, |v| {
                let mut out = String::new();
                for r in v["repos"].as_array().into_iter().flatten() {
                    out.push_str(&format!(
                        "{:<20} {:<8} main {:<10} {} landed, {} open\n",
                        r["name"].as_str().unwrap_or(""),
                        r["project"].as_str().unwrap_or("-"),
                        r["refs"]["main"].as_str().unwrap_or("root"),
                        r["landed"],
                        r["open_changes"]
                    ));
                }
                out
            });
            Ok(())
        }
        ("lane", "open") => {
            let c = cx.client()?;
            let v = c.post("/lanes", &json!({ "repo": cx.args.need("repo")?, "ref": cx.args.opt("ref").unwrap_or_else(|| "main".into()), "task": cx.args.opt("task"), "title": cx.args.need("title")?, "author": cx.args.need("author")?, "stacked_on": cx.args.opt("stacked-on"), "kind": cx.args.opt("kind").unwrap_or_else(|| "normal".into()) }))?;
            cx.print(&v, |v| format!("{}\n", v["change"].as_str().unwrap_or("")));
            Ok(())
        }
        ("checkout", _) => checkout(&cx),
        ("status", _) => status(&cx),
        ("diff", _) => diff(&cx),
        ("snapshot", _) => {
            let w = cx.workdir()?;
            let tool_calls = cx.args.all("tool-call");
            let turn = cx.args.opt("turn").and_then(|t| t.parse::<u64>().ok());
            let r = snapshot(&cx, &w, tool_calls, turn, cx.args.opt("model"))?;
            println!("{r}");
            Ok(())
        }
        ("restore", _) => restore(&cx),
        ("describe", _) => describe(&cx),
        ("done", _) => done(&cx),
        ("log", _) => log(&cx),
        ("show", _) => show(&cx),
        ("tree", _) => {
            let c = cx.client()?;
            let rev = cx.args.arg(1, "a revision")?;
            let v = c.get(&format!("/revisions/{rev}/tree"))?;
            cx.print(&v, render_tree);
            Ok(())
        }
        ("blame", _) => blame(&cx),
        ("history", _) => {
            let c = cx.client()?;
            let id = cx.args.arg(1, "a definition (path::symbol) or lineage")?;
            let v = c.get(&format!(
                "/repos/{}/symbols/{}",
                cx.repo()?,
                crate::server::percent_encode(&id)
            ))?;
            cx.print(&v, |v| {
                let mut out = format!("{}  ({})\n", id, v["lineage"].as_str().unwrap_or(""));
                for e in v["history"].as_array().into_iter().flatten() {
                    out.push_str(&format!(
                        "  {}  {} {}{}\n",
                        e["change"].as_str().unwrap_or(""),
                        e["op"].as_str().unwrap_or(""),
                        e["id"].as_str().unwrap_or(""),
                        e["from"]
                            .as_str()
                            .map(|f| format!("  (from {f})"))
                            .unwrap_or_default()
                    ));
                }
                out
            });
            Ok(())
        }
        ("op", _) => {
            let c = cx.client()?;
            let v = c.get(&format!(
                "/ops?since={}",
                cx.args.opt("since").unwrap_or_else(|| "0".into())
            ))?;
            cx.print(&v, |v| {
                let mut out = String::new();
                for o in v["ops"].as_array().into_iter().flatten() {
                    out.push_str(&format!(
                        "{:>6}  {:<16} {:<14} {}{}\n",
                        o["seq"],
                        o["kind"].as_str().unwrap_or(""),
                        label_of(&o["actor"]),
                        o["summary"].as_str().unwrap_or(""),
                        if o["undone_by"].is_null() {
                            String::new()
                        } else {
                            format!("  (undone by {})", o["undone_by"])
                        }
                    ));
                }
                out
            });
            Ok(())
        }
        ("undo", _) => {
            let c = cx.client()?;
            let seq: u64 = cx
                .args
                .arg(1, "an op's seq")?
                .parse()
                .map_err(|_| "not a seq")?;
            let v = cx.say(
                &c,
                "undo",
                json!({ "seq": seq, "reason": cx.args.opt("reason").unwrap_or_default() }),
            )?;
            cx.print(&v, |v| {
                format!("{}\n", v["summary"].as_str().unwrap_or("undone"))
            });
            Ok(())
        }
        ("verify", _) => verify(&cx),
        ("pack", "add") => {
            let c = cx.client()?;
            let dir = PathBuf::from(cx.args.arg(2, "a pack folder")?);
            let cache = dirs_cache().join("npm");
            let prepared = crate::packsrc::prepare(
                &dir,
                cx.args.opt("wasm").as_deref().map(Path::new),
                Some(&cache),
            )?;
            let (wasm, query) = (prepared.wasm.clone(), prepared.query.clone());
            c.upload(&[prepared.pack.wasm, prepared.pack.tags], &|h| {
                Ok(if *h == prepared.pack.wasm {
                    wasm.clone()
                } else {
                    query.clone()
                })
            })?;
            let v = cx.say(
                &c,
                "pack.add",
                serde_json::to_value(&prepared.pack).unwrap(),
            )?;
            cx.print(&v, |v| {
                format!("{}\n", v["summary"].as_str().unwrap_or("added"))
            });
            Ok(())
        }
        ("pack", _) | ("packs", _) => {
            let c = cx.client()?;
            let v = c.get("/packs")?;
            cx.print(&v, |v| {
                let loaded: Vec<&str> = v["loaded"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|x| x.as_str())
                    .collect();
                let mut out = String::new();
                for p in v["packs"].as_array().into_iter().flatten() {
                    let name = p["pack"]["name"].as_str().unwrap_or("");
                    out.push_str(&format!(
                        "{:<12} v{:<3} {:<28} {}\n",
                        name,
                        p["pack"]["version"],
                        p["pack"]["extensions"]
                            .as_array()
                            .map(|e| e
                                .iter()
                                .filter_map(|x| x.as_str())
                                .collect::<Vec<_>>()
                                .join(" "))
                            .unwrap_or_default(),
                        if loaded.contains(&name) { "loaded" } else { "" }
                    ));
                }
                out
            });
            Ok(())
        }
        ("map", "draft") => {
            let c = cx.client()?;
            let v = c.get(&format!("/repos/{}/map/draft", cx.repo()?))?;
            cx.print(&v, |v| v["toml"].as_str().unwrap_or("").to_string());
            Ok(())
        }
        ("map", _) => {
            let c = cx.client()?;
            let repo = cx.repo()?;
            let question: Vec<String> = cx.args.pos[1..].to_vec();
            if question.is_empty() {
                let v = c.get(&format!("/repos/{repo}/map"))?;
                cx.print(&v, render_map);
            } else {
                let v = c.get(&format!(
                    "/repos/{repo}/map/where?q={}",
                    crate::server::percent_encode(&question.join(" "))
                ))?;
                cx.print(&v, |v| {
                    let mut out = String::new();
                    for h in v["hits"].as_array().into_iter().flatten() {
                        out.push_str(&format!(
                            "{}{}  — {}{}\n",
                            h["path"].as_str().unwrap_or(""),
                            h["symbol"]
                                .as_str()
                                .map(|s| format!("::{s}"))
                                .unwrap_or_default(),
                            h["why"].as_str().unwrap_or(""),
                            h["component"]
                                .as_str()
                                .map(|c| format!(", in {c}"))
                                .unwrap_or_default()
                        ));
                    }
                    if out.is_empty() {
                        out = "nothing matches\n".into();
                    }
                    out
                });
            }
            Ok(())
        }
        ("state", _) => {
            let c = cx.client()?;
            let v = c.get("/state")?;
            cx.print(&v, |v| {
                format!("{}\n", serde_json::to_string_pretty(v).unwrap_or_default())
            });
            Ok(())
        }
        _ => crate::cli_more::run(&cx, &cmd),
    }
}

fn label_of(actor: &Value) -> String {
    serde_json::from_value::<Actor>(actor.clone())
        .map(|a| a.label())
        .unwrap_or_else(|_| actor.as_str().unwrap_or("").to_string())
}

pub fn dirs_cache() -> PathBuf {
    std::env::var("WEFT_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".cache").join("weft"))
                .unwrap_or_else(|_| std::env::temp_dir().join("weft-cache"))
        })
}

fn serve(cx: &Ctx) -> Result<(), String> {
    let a = &cx.args;
    let port: u16 = a.opt("port").and_then(|p| p.parse().ok()).unwrap_or(0);
    let token = match a.opt("token-file") {
        Some(f) => apz_http::auth::InternalToken::file(f),
        None => apz_http::auth::InternalToken::none(),
    };
    let handle = if a.has("cache") {
        let dir = PathBuf::from(a.need("dir")?);
        let mut headers = Vec::new();
        if let Some(f) = a.opt("upstream-token-file") {
            let t = std::fs::read_to_string(&f).map_err(|e| format!("{f}: {e}"))?;
            headers.push((apz_http::auth::HEADER.to_string(), t.trim().to_string()));
        }
        let upstream = Client::new(&a.need("upstream")?, a.opt("upstream-bearer"), headers)?;
        let max = a
            .opt("max-mb")
            .and_then(|m| m.parse::<u64>().ok())
            .unwrap_or(2048)
            << 20;
        let cache = crate::cache::Cache::open(&dir, upstream, max)?;
        crate::cache::spawn(cache, port, token).map_err(|e| e.to_string())?
    } else {
        let state = PathBuf::from(a.need("state")?);
        let hub_key = if a.has("hub-key-stdin") {
            let mut line = String::new();
            std::io::stdin()
                .read_line(&mut line)
                .map_err(|e| e.to_string())?;
            Some(KeyPair::from_hex(line.trim())?)
        } else {
            match a.opt("hub-key") {
                Some(f) => Some(KeyPair::load(Path::new(&f))?),
                None => None,
            }
        };
        // The SSH signing key for exported commits: a file, or the second line on stdin.
        let signing = match a.opt("signing-key") {
            Some(f) if f == "-" => {
                let mut line = String::new();
                std::io::stdin()
                    .read_line(&mut line)
                    .map_err(|e| e.to_string())?;
                Some(KeyPair::from_hex(line.trim())?)
            }
            Some(f) => Some(KeyPair::load(Path::new(&f))?),
            None => None,
        };
        let hub = crate::server::Hub::open_with(&state, hub_key, signing)?;
        hub.start_deriving();
        if a.has("auto-land") {
            hub.start_auto_land();
        }
        crate::server::spawn(hub, port, token).map_err(|e| e.to_string())?
    };
    let port = handle.port();
    if let Some(f) = a.opt("port-file") {
        workdir::write_atomic(Path::new(&f), port.to_string().as_bytes())?;
    }
    eprintln!("weft: serving on 127.0.0.1:{port}");
    handle.join().map_err(|e| e.to_string())
}

fn init(cx: &Ctx) -> Result<(), String> {
    let a = &cx.args;
    let state = PathBuf::from(a.need("state")?);
    let owner = KeyPair::load(Path::new(&a.need("owner-key")?))?;
    let hub = KeyPair::load(Path::new(&a.need("hub-key")?))?;
    let hub_id = hub.id();
    crate::store::Store::init(
        &state,
        &a.opt("plaza").unwrap_or_else(|| "plaza".into()),
        &owner,
        hub,
        &["certify-agents", "rule-for-owner"],
    )?;
    println!(
        "weft: a new store in {} (owner key {}, hub key {hub_id})",
        state.display(),
        owner.id()
    );
    Ok(())
}

/// `weft checkout <change>`: the lane's files, from its latest draft or round (or its base when it has none).
fn checkout(cx: &Ctx) -> Result<(), String> {
    let c = cx.client()?;
    // A landing candidate (for a re-check): its files, laid out once, with no lane behind them.
    if let Some(digest) = cx.args.opt("candidate") {
        let dir = cx
            .args
            .opt("dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| cx.cwd.clone());
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let w = Workdir { root: dir };
        let v = c.post("/wire/want", &json!({ "candidate": digest }))?;
        let files: Vec<(String, Hash, u32)> = v["files"]
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
        fetch(&c, &w, files.iter().map(|f| f.1).collect())?;
        for (p, h, mode) in &files {
            w.write_file(p, &w.object(h).ok_or("an object did not arrive")?, *mode)?;
        }
        println!(
            "candidate {digest}: {} files in {}",
            files.len(),
            w.root.display()
        );
        return Ok(());
    }
    let change = cx.args.arg(1, "a change (w-N) or task")?;
    let detail = c.get(&format!("/changes/{change}"))?;
    let name = detail["change"]
        .as_str()
        .ok_or("no such change")?
        .to_string();
    let base = detail["base"].as_str().unwrap_or("root").to_string();
    let at = detail["drafts"]
        .as_array()
        .and_then(|d| d.last())
        .or_else(|| detail["rounds"].as_array().and_then(|r| r.last()))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| base.clone());
    let at = if detail["state"] == "landed" {
        detail["landed"].as_str().map(str::to_string).unwrap_or(at)
    } else {
        at
    };
    let dir = cx
        .args
        .opt("dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| cx.cwd.clone());
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let w = Workdir {
        root: dir.canonicalize().map_err(|e| e.to_string())?,
    };
    let sparse: Vec<String> = cx
        .args
        .opt("paths")
        .map(|p| {
            p.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let want = |rev: &str| -> Result<BTreeMap<String, Entry>, String> {
        let body = if sparse.is_empty() {
            json!({ "revision": rev })
        } else {
            json!({ "revision": rev, "paths": sparse })
        };
        let v = c.post("/wire/want", &body)?;
        let mut files = BTreeMap::new();
        for f in v["files"].as_array().into_iter().flatten() {
            files.insert(
                f["path"].as_str().unwrap_or("").to_string(),
                Entry {
                    hash: serde_json::from_value(f["hash"].clone()).map_err(|e| e.to_string())?,
                    mode: f["mode"].as_u64().unwrap_or(0o100644) as u32,
                },
            );
        }
        Ok(files)
    };
    let base_files = want(&base)?;
    let files = if at == base {
        base_files.clone()
    } else {
        want(&at)?
    };
    fetch(&c, &w, files.values().map(|e| e.hash).collect())?;
    // Files this directory had from an earlier checkout and the lane no longer has.
    if let Ok(old) = w.lane() {
        for p in old.files.keys().filter(|p| !files.contains_key(*p)) {
            w.remove_file(p);
        }
    }
    for (p, e) in &files {
        let bytes = w
            .object(&e.hash)
            .ok_or_else(|| format!("object for {p} did not arrive"))?;
        w.write_file(p, &bytes, e.mode)?;
    }
    let lane = Lane {
        hub: cx.hub_url()?,
        repo: detail["repo"].as_str().unwrap_or("").to_string(),
        change: name.clone(),
        base,
        at: at.clone(),
        base_files,
        files,
        next_draft: detail["drafts"]
            .as_array()
            .map(|d| d.len() as u32 + 1)
            .unwrap_or(1),
        sparse,
    };
    w.save_lane(&lane)?;
    println!(
        "{name} at {at}: {} files in {}",
        lane.files.len(),
        w.root.display()
    );
    Ok(())
}

/// Fetches the objects the directory lacks.
fn fetch(c: &Client, w: &Workdir, hashes: Vec<Hash>) -> Result<usize, String> {
    let mut lacking: Vec<Hash> = hashes
        .into_iter()
        .filter(|h| w.object(h).is_none())
        .collect();
    lacking.sort();
    lacking.dedup();
    c.download(&lacking, &mut |h, b| w.put_object(h, b))
}

fn status(cx: &Ctx) -> Result<(), String> {
    let w = cx.workdir()?;
    let lane = w.lane()?;
    let (now, _) = w.scan()?;
    let since_base = workdir::deltas(&lane.base_files, &now);
    let since_snap = workdir::deltas(&lane.files, &now);
    let pending = w.outbox()?.len();
    let v = json!({ "change": lane.change, "repo": lane.repo, "base": lane.base, "at": lane.at, "changed": since_base, "unsnapshotted": since_snap, "outbox": pending });
    cx.print(&v, |_| {
        let mark = |d: &crate::model::Delta, base: &BTreeMap<String, Entry>| {
            if d.after.is_none() {
                '-'
            } else if base.contains_key(&d.path) {
                '~'
            } else {
                '+'
            }
        };
        let mut out = format!(
            "{} on {} (base {}, last {})\n",
            lane.change, lane.repo, lane.base, lane.at
        );
        if since_base.is_empty() {
            out.push_str("  no changes from the base\n");
        }
        for d in &since_base {
            let fresh = since_snap.iter().any(|s| s.path == d.path);
            out.push_str(&format!(
                "  {} {}{}\n",
                mark(d, &lane.base_files),
                d.path,
                if fresh { "   (not snapshotted)" } else { "" }
            ));
        }
        for d in since_snap
            .iter()
            .filter(|s| !since_base.iter().any(|b| b.path == s.path))
        {
            out.push_str(&format!(
                "  ↺ {}   (back to the base, not snapshotted)\n",
                d.path
            ));
        }
        if pending > 0 {
            out.push_str(&format!("  {pending} snapshot(s) waiting in the outbox\n"));
        }
        out
    });
    Ok(())
}

fn diff(cx: &Ctx) -> Result<(), String> {
    let w = cx.workdir()?;
    let lane = w.lane()?;
    let c = cx.client()?;
    if cx.args.has("symbols") {
        let v = c.get(&format!("/revisions/{}/tree", lane.at))?;
        cx.print(&v, render_tree);
        return Ok(());
    }
    let (now, paths) = w.scan()?;
    let ds = workdir::deltas(&lane.base_files, &now);
    let only = cx.args.pos.get(1);
    let olds: Vec<Hash> = ds
        .iter()
        .filter_map(|d| lane.base_files.get(&d.path).map(|e| e.hash))
        .collect();
    let _ = fetch(&c, &w, olds);
    let mut out = String::new();
    for d in ds.iter().filter(|d| only.is_none_or(|o| &d.path == o)) {
        let old = lane
            .base_files
            .get(&d.path)
            .and_then(|e| w.object(&e.hash))
            .unwrap_or_default();
        let new = paths
            .get(&d.path)
            .and_then(|p| std::fs::read(p).ok())
            .unwrap_or_default();
        if crate::diff::is_text(&old) && crate::diff::is_text(&new) {
            out.push_str(&crate::diff::unified(
                &d.path,
                &String::from_utf8_lossy(&old),
                &String::from_utf8_lossy(&new),
            ));
        } else {
            out.push_str(&format!(
                "binary {}: {} → {} bytes\n",
                d.path,
                old.len(),
                new.len()
            ));
        }
    }
    print!("{out}");
    Ok(())
}

/// Sends what waits in the outbox, oldest first.
pub fn flush(cx: &Ctx, c: &Client, w: &Workdir) -> Result<usize, String> {
    let mut sent = 0;
    for (path, item) in w.outbox()? {
        let hashes: Vec<Hash> = serde_json::from_value(item["objects"].clone()).unwrap_or_default();
        c.upload(&hashes, &|h| {
            w.object(h)
                .ok_or_else(|| format!("object {} is not in .weft/objects", h.short()))
        })?;
        cx.say(c, item["kind"].as_str().unwrap_or(""), item["body"].clone())?;
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        sent += 1;
    }
    Ok(sent)
}

/// `weft snapshot`: this turn's draft. Only new objects travel; offline, it waits in the outbox.
pub fn snapshot(
    cx: &Ctx,
    w: &Workdir,
    tool_calls: Vec<String>,
    turn: Option<u64>,
    model: Option<String>,
) -> Result<String, String> {
    let mut lane = w.lane()?;
    let (now, paths) = w.scan()?;
    if workdir::deltas(&lane.files, &now).is_empty() && lane.at != lane.base {
        return Ok(format!("{}: nothing new since {}", lane.change, lane.at));
    }
    let ds = workdir::deltas(&lane.base_files, &now);
    // Keep every changed file's object here, so an outbox can be sent later.
    for d in &ds {
        if let (Some(h), Some(p)) = (d.after, paths.get(&d.path))
            && w.object(&h).is_none()
        {
            {
                let bytes = std::fs::read(p).map_err(|e| e.to_string())?;
                if Hash::of(&bytes) == h {
                    w.put_object(&h, &bytes)?;
                } else {
                    // A symlink: its target is its content.
                    let target = std::fs::read_link(p)
                        .map_err(|e| e.to_string())?
                        .to_string_lossy()
                        .as_bytes()
                        .to_vec();
                    w.put_object(&h, &target)?;
                }
            }
        }
    }
    let objects: Vec<Hash> = ds.iter().filter_map(|d| d.after).collect();
    let n = lane.next_draft;
    let body = json!({ "change": lane.change, "n": n, "deltas": ds, "tool_calls": tool_calls, "turn": turn, "model": model });
    let item = json!({ "kind": "revision.draft", "body": body, "objects": objects });
    let id = format!("{}.d{n}", lane.change);
    let sent = (|| -> Result<(), String> {
        let c = cx.client()?;
        flush(cx, &c, w)?;
        c.upload(&objects, &|h| {
            w.object(h)
                .ok_or_else(|| format!("object {} is missing", h.short()))
        })?;
        cx.say(&c, "revision.draft", body.clone())?;
        Ok(())
    })();
    let note = match sent {
        Ok(()) => format!("{id}: {} files from the base", ds.len()),
        Err(e)
            if e.contains("refused")
                || e.contains("Connection")
                || e.contains("connect")
                || e.contains("timed out")
                || e.contains("502") =>
        {
            w.queue(&item)?;
            format!("{id}: the hub is out of reach ({e}); kept in the outbox")
        }
        Err(e) => return Err(e),
    };
    lane.files = now;
    lane.at = id;
    lane.next_draft = n + 1;
    w.save_lane(&lane)?;
    Ok(note)
}

fn restore(cx: &Ctx) -> Result<(), String> {
    let w = cx.workdir()?;
    let mut lane = w.lane()?;
    let c = cx.client()?;
    let rev = cx.args.arg(1, "a revision (w-N.dK)")?;
    let v = c.post("/wire/want", &json!({ "revision": rev }))?;
    let mut files = BTreeMap::new();
    for f in v["files"].as_array().into_iter().flatten() {
        files.insert(
            f["path"].as_str().unwrap_or("").to_string(),
            Entry {
                hash: serde_json::from_value(f["hash"].clone()).map_err(|e| e.to_string())?,
                mode: f["mode"].as_u64().unwrap_or(0o100644) as u32,
            },
        );
    }
    fetch(&c, &w, files.values().map(|e| e.hash).collect())?;
    let (now, _) = w.scan()?;
    for p in now.keys().filter(|p| !files.contains_key(*p)) {
        w.remove_file(p);
    }
    for (p, e) in &files {
        if now.get(p) != Some(e) {
            w.write_file(
                p,
                &w.object(&e.hash).ok_or("an object did not arrive")?,
                e.mode,
            )?;
        }
    }
    println!(
        "restored {rev}: {} files (snapshot to keep it as the next draft)",
        files.len()
    );
    lane.base_files.retain(|_, _| true);
    w.save_lane(&lane)?;
    Ok(())
}

fn intents_path(w: &Workdir) -> PathBuf {
    w.meta().join("intents.json")
}

fn describe(cx: &Ctx) -> Result<(), String> {
    let w = cx.workdir()?;
    let lane = w.lane()?;
    let title = cx.args.need("intent")?;
    let covers: Vec<String> = cx.args.pos[1..].to_vec();
    if covers.is_empty() {
        return Err("name the paths or definitions this intent covers".into());
    }
    let mut intents: Vec<IntentDef> = std::fs::read_to_string(intents_path(&w))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    intents.retain(|i| i.title != title);
    intents.push(IntentDef {
        title: title.clone(),
        summary: cx.args.opt("summary").unwrap_or_default(),
        kind: cx.args.opt("kind").unwrap_or_else(|| "change".into()),
        covers,
    });
    workdir::write_atomic(
        &intents_path(&w),
        serde_json::to_string_pretty(&intents).unwrap().as_bytes(),
    )?;
    let c = cx.client()?;
    let detail = c.get(&format!("/changes/{}", lane.change))?;
    let n = detail["rounds"].as_array().map(|r| r.len()).unwrap_or(0) + 1;
    cx.say(
        &c,
        "intent.set",
        json!({ "change": lane.change, "n": n, "intents": intents }),
    )?;
    println!("{}: {} intents for round {n}", lane.change, intents.len());
    Ok(())
}

fn done(cx: &Ctx) -> Result<(), String> {
    let w = cx.workdir()?;
    let note = snapshot(cx, &w, vec![], None, cx.args.opt("model"))?;
    if note.contains("outbox") {
        return Err(format!(
            "{note}\nweft: a lane is not sealed while its outbox is not empty"
        ));
    }
    let c = cx.client()?;
    flush(cx, &c, &w)?;
    let lane = w.lane()?;
    let ds = workdir::deltas(&lane.base_files, &lane.files);
    let detail = c.get(&format!("/changes/{}", lane.change))?;
    let n = detail["rounds"].as_array().map(|r| r.len()).unwrap_or(0) + 1;
    cx.say(
        &c,
        "revision.seal",
        json!({ "change": lane.change, "n": n, "deltas": ds, "model": cx.args.opt("model") }),
    )?;
    let intents: Vec<IntentDef> = std::fs::read_to_string(intents_path(&w))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    if !intents.is_empty() {
        cx.say(
            &c,
            "intent.set",
            json!({ "change": lane.change, "n": n, "intents": intents }),
        )?;
    }
    println!(
        "{}.r{n} sealed: {} files, {} intents; it goes to review",
        lane.change,
        ds.len(),
        intents.len()
    );
    Ok(())
}

fn log(cx: &Ctx) -> Result<(), String> {
    let c = cx.client()?;
    let repo = cx.repo()?;
    let q = cx.args.pos[1..].join(" ");
    let v = c.get(&format!(
        "/repos/{repo}/log?q={}&limit={}",
        crate::server::percent_encode(&q),
        cx.args.opt("limit").unwrap_or_else(|| "50".into())
    ))?;
    cx.print(&v, |v| {
        let mut out = String::new();
        for ch in v["changes"].as_array().into_iter().flatten() {
            let intents: Vec<String> = ch["intents"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|i| {
                    format!(
                        "{}: {}",
                        i["kind"].as_str().unwrap_or(""),
                        i["title"].as_str().unwrap_or("")
                    )
                })
                .collect();
            out.push_str(&format!(
                "{:<7} {:<9} {:<12} {:<6} {}\n",
                ch["change"].as_str().unwrap_or(""),
                ch["state"].as_str().unwrap_or(""),
                ch["author"].as_str().unwrap_or(""),
                ch["task"].as_str().unwrap_or("-"),
                ch["title"].as_str().unwrap_or("")
            ));
            for i in intents {
                out.push_str(&format!("{:<36}· {i}\n", ""));
            }
        }
        out.push_str(&format!(
            "{} of {} changes\n",
            v["changes"].as_array().map(|a| a.len()).unwrap_or(0),
            v["total"]
        ));
        out
    });
    Ok(())
}

fn show(cx: &Ctx) -> Result<(), String> {
    let c = cx.client()?;
    let change = cx.args.arg(1, "a change")?;
    let v = c.get(&format!("/changes/{change}"))?;
    cx.print(&v, |v| {
        let mut out = format!(
            "{} {} — {}\n",
            v["change"].as_str().unwrap_or(""),
            v["state"].as_str().unwrap_or(""),
            v["title"].as_str().unwrap_or("")
        );
        out.push_str(&format!(
            "  repo {}  ref {}  task {}  author {}  base {}\n",
            v["repo"].as_str().unwrap_or(""),
            v["ref"].as_str().unwrap_or(""),
            v["task"].as_str().unwrap_or("-"),
            v["author"].as_str().unwrap_or(""),
            v["base"].as_str().unwrap_or("")
        ));
        out.push_str("  revisions\n");
        for r in v["revisions"].as_array().into_iter().flatten() {
            out.push_str(&format!(
                "    {:<10} {:<8} {} files  by {}{}\n",
                r["id"].as_str().unwrap_or(""),
                r["kind"].as_str().unwrap_or(""),
                r["files"],
                r["actor"].as_str().unwrap_or(""),
                r["turn"]
                    .as_u64()
                    .map(|t| format!(", turn {t}"))
                    .unwrap_or_default()
            ));
        }
        if let Some(intents) = v["intents"].as_object() {
            for (round, list) in intents {
                for i in list.as_array().into_iter().flatten() {
                    out.push_str(&format!(
                        "  intent (round {round}) {}: {}\n",
                        i["kind"].as_str().unwrap_or(""),
                        i["title"].as_str().unwrap_or("")
                    ));
                }
            }
        }
        out.push_str("  sign-off\n");
        for r in v["records"].as_array().into_iter().flatten() {
            out.push_str(&format!(
                "    {:<10} {:<12} {}{}\n",
                r["type"].as_str().unwrap_or(""),
                r["actor"].as_str().unwrap_or(""),
                r["revision"].as_str().unwrap_or(""),
                if r["via_console"] == true {
                    "  (from the console)"
                } else {
                    ""
                }
            ));
        }
        out
    });
    Ok(())
}

fn blame(cx: &Ctx) -> Result<(), String> {
    let c = cx.client()?;
    let target = cx.args.arg(1, "a path")?;
    let (path, symbol) = match target.split_once("::") {
        Some((p, s)) => (p.to_string(), Some(s.to_string())),
        None => (target.clone(), None),
    };
    let mut url = format!(
        "/repos/{}/blame?path={}",
        cx.repo()?,
        crate::server::percent_encode(&path)
    );
    if let Some(s) = &symbol {
        url.push_str(&format!("&symbol={}", crate::server::percent_encode(s)));
    }
    if let Some(at) = cx.args.opt("at") {
        url.push_str(&format!("&at={at}"));
    }
    let v = c.get(&url)?;
    cx.print(&v, |v| {
        let mut out = String::new();
        for l in v["lines"].as_array().into_iter().flatten() {
            let by = &l["by"];
            let who = if by.is_null() {
                String::new()
            } else {
                format!(
                    "{} {} {}",
                    by["change"].as_str().unwrap_or(""),
                    label_of(&by["by"]),
                    by["author"].as_str().or(by["model"].as_str()).unwrap_or("")
                )
            };
            out.push_str(&format!(
                "{:>5} {:<38} │ {}\n",
                l["line"],
                who.chars().take(38).collect::<String>(),
                l["text"].as_str().unwrap_or("")
            ));
        }
        out
    });
    Ok(())
}

fn verify(cx: &Ctx) -> Result<(), String> {
    let c = cx.client()?;
    let v = match cx.args.pos.get(1) {
        Some(s) => c.get(&format!(
            "/verify?subject={}",
            crate::server::percent_encode(s)
        ))?,
        None => c.get(&format!(
            "/verify?deep={}",
            if cx.args.has("deep") { "1" } else { "0" }
        ))?,
    };
    cx.print(&v, |v| {
        if v["records"].is_array() {
            let mut out = format!(
                "{}: {}\n",
                v["subject"].as_str().unwrap_or(""),
                if v["ok"] == true {
                    "verified"
                } else {
                    "NOT verified"
                }
            );
            for r in v["records"].as_array().into_iter().flatten() {
                out.push_str(&format!(
                    "  {:>5} {:<16} {:<12} {:<24} {}{}\n",
                    r["seq"],
                    r["kind"].as_str().unwrap_or(""),
                    r["actor"].as_str().unwrap_or(""),
                    r["signer"].as_str().unwrap_or(""),
                    if r["ok"] == true {
                        "ok"
                    } else {
                        r["problem"].as_str().unwrap_or("bad")
                    },
                    if r["via_console"] == true {
                        " (console)"
                    } else {
                        ""
                    }
                ));
            }
            out
        } else {
            format!(
                "the log verifies: {} records, {} signatures, {} changes, {} blobs\n",
                v["records"], v["signatures"], v["changes"], v["blobs"]
            )
        }
    });
    Ok(())
}

pub fn render_tree(v: &Value) -> String {
    let mut out = format!(
        "{} (base {}): +{} −{}\n",
        v["revision"].as_str().unwrap_or(""),
        v["base"].as_str().unwrap_or(""),
        v["add"],
        v["del"]
    );
    for i in v["intents"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "  ◆ {}: {}\n",
            i["kind"].as_str().unwrap_or(""),
            i["title"].as_str().unwrap_or("")
        ));
        for it in i["items"].as_array().into_iter().flatten() {
            out.push_str(&format!("      {}\n", it.as_str().unwrap_or("")));
        }
    }
    if let Some(u) = v["unsorted"].as_array().filter(|u| !u.is_empty()) {
        out.push_str("  ◇ not in an intent\n");
        for it in u {
            out.push_str(&format!("      {}\n", it.as_str().unwrap_or("")));
        }
    }
    for f in v["files"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "  {} {}  {}  +{} −{}\n",
            f["op"].as_str().unwrap_or(""),
            f["path"].as_str().unwrap_or(""),
            f["pack"].as_str().unwrap_or(if f["binary"] == true {
                "binary"
            } else {
                "text"
            }),
            f["add"],
            f["del"]
        ));
        for s in f["symbols"].as_array().into_iter().flatten() {
            let by: Vec<String> = s["provenance"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| p["source"].as_str().unwrap_or("").to_string())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            out.push_str(&format!(
                "      {} {}{}{}\n",
                s["op"].as_str().unwrap_or(""),
                s["qualified"].as_str().unwrap_or(""),
                s["from"]
                    .as_str()
                    .map(|f| format!("  (was {f})"))
                    .unwrap_or_default(),
                if by.is_empty() {
                    String::new()
                } else {
                    format!("  [{}]", by.join(", "))
                }
            ));
            for t in s["touches"].as_array().into_iter().flatten() {
                out.push_str(&format!(
                    "          {} {}\n",
                    t["how"].as_str().unwrap_or(""),
                    t["to"].as_str().unwrap_or("")
                ));
            }
        }
    }
    for d in v["divergence"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "  ⚠ diverges from {} ({}): {}\n",
            d["change"].as_str().unwrap_or(""),
            d["task"].as_str().unwrap_or("-"),
            d["symbols"]
                .as_array()
                .filter(|s| !s.is_empty())
                .or(d["files"].as_array())
                .map(|x| x
                    .iter()
                    .filter_map(|y| y.as_str())
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_default()
        ));
    }
    out
}

pub fn render_map(v: &Value) -> String {
    let mut out = format!(
        "repo {}  {}\n",
        v["repo"].as_str().unwrap_or(""),
        v["map"]["description"].as_str().unwrap_or("")
    );
    if v["drafted"] == true {
        out.push_str("  (no map.toml yet; this is a draft)\n");
    }
    for c in v["map"]["component"]
        .as_array()
        .or(v["map"]["components"].as_array())
        .into_iter()
        .flatten()
    {
        out.push_str(&format!(
            "  {:<32} {:<28} {}\n",
            c["paths"]
                .as_array()
                .map(|p| p
                    .iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_default(),
            c["title"]
                .as_str()
                .unwrap_or(c["name"].as_str().unwrap_or("")),
            c["languages"]
                .as_array()
                .map(|p| p
                    .iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_default()
        ));
    }
    for c in v["connections"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "  connected to {} ({}, {})\n",
            c["remote"].as_str().unwrap_or(""),
            c["state"].as_str().unwrap_or(""),
            c["branch"].as_str().unwrap_or("")
        ));
    }
    out
}
