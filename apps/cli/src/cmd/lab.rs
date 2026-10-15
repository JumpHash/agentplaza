//! `agentplaza lab`: the training lab (the Foundry). The devices, the jobs and their events, the
//! recipes and their vetted commits, the datasets, the models, the Hugging Face imports, and the
//! breeders: the same hub routes the web's Lab uses.

mod datasets;
mod hf;
mod jobs;
mod models;

use clap::Subcommand;
use serde_json::{Value, json};

use crate::cmd::models::items;
use crate::ctx::{self, Ctx, confirm};

/// The law that caps a breeder's job in GPU-hours, and the condition its number is read from.
const CAP_LAW: &str = "S-006c";
/// The per-job cap when the laws cannot be read (the web's fallback, S-006c's own number).
const PER_JOB_CAP_FALLBACK: f64 = 8.0;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The device fleet: each machine's devices, their memory, their load and what they are given to.
    Devices,
    /// Training jobs: list, show, follow the events, plan and submit a job file, cancel.
    #[command(subcommand)]
    Jobs(jobs::Cmd),
    /// Recipes: the training programs, and vetting a commit of one.
    #[command(subcommand)]
    Recipes(RecipesCmd),
    /// Datasets: list them, export them from the plaza's records, upload a JSONL file.
    #[command(subcommand)]
    Datasets(datasets::Cmd),
    /// Models: the registry, importing a model directory, shadow, promote, retire, trying one.
    #[command(subcommand)]
    Models(models::Cmd),
    /// Hugging Face: the token, search, a repo's files, and the import queue.
    #[command(subcommand)]
    Hf(hf::Cmd),
    /// The breeders: the agents with the breeder role, their jobs, and the GPU-hours those used.
    Breeders,
}

#[derive(Subcommand, Debug)]
pub enum RecipesCmd {
    /// The recipes: id, repo, commit, and whether the owner vetted it.
    List,
    /// Vet a recipe at a commit (the owner's pin). The commit is 7 to 64 hex digits.
    Vet {
        /// The recipe id, name@version.
        id: String,
        /// The commit to vet.
        #[arg(long)]
        commit: String,
        /// Vet without asking.
        #[arg(long)]
        yes: bool,
    },
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::Devices => devices(ctx),
        Cmd::Jobs(c) => jobs::run(ctx, c),
        Cmd::Recipes(c) => recipes(ctx, c),
        Cmd::Datasets(c) => datasets::run(ctx, c),
        Cmd::Models(c) => models::run(ctx, c),
        Cmd::Hf(c) => hf::run(ctx, c),
        Cmd::Breeders => breeders(ctx),
    }
}

// ---- Paths, answers and formats ---------------------------------------------------------------

/// A recipe's path. Its id is `name@version`, and the `@` stays raw, as the web sends it.
fn recipe_path(id: &str) -> String {
    format!("/v1/foundry/recipes/{}", ctx::seg(id).replace("%40", "@"))
}

/// A job's path.
fn job_path(id: &str) -> String {
    format!("/v1/foundry/jobs/{}", ctx::seg(id))
}

/// A model's path.
fn model_path(id: &str) -> String {
    format!("/v1/foundry/models/{}", ctx::seg(id))
}

/// A Hugging Face repo's path: `org/name` keeps its slash, and each segment is encoded.
fn repo_path(id: &str) -> String {
    id.split('/').map(ctx::seg).collect::<Vec<_>>().join("/")
}

/// Why a call was refused: the hub's `error` text, else the path and the status.
fn refusal(value: &Value, path: &str, status: u16) -> String {
    value
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("{path}: HTTP {status}"))
}

/// A number for a table: a fraction (GPU-hours and the like) with two decimals, a whole number as it is, null as a dash.
fn amount(v: &Value) -> String {
    match v {
        Value::Number(n) if n.is_f64() => format!("{:.2}", n.as_f64().unwrap_or_default()),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        _ => "—".to_string(),
    }
}

/// A count of bytes as a short size: B, KiB, MiB or GiB.
pub(crate) fn size_text(bytes: f64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value:.0} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// The first twelve characters of a hash or a commit.
fn short(hex: &str) -> String {
    hex.chars().take(12).collect()
}

/// `yes`, `no`, or a dash when the hub did not say.
fn yes_no(flag: Option<bool>) -> &'static str {
    match flag {
        Some(true) => "yes",
        Some(false) => "no",
        None => "—",
    }
}

// ---- Devices and recipes ----------------------------------------------------------------------

fn devices(ctx: &Ctx) -> Result<(), String> {
    let view = ctx.client.get("/v1/foundry/devices")?;
    ctx.out(&view, |v| {
        let rows: Vec<Vec<String>> = items(&v["machines"]).flat_map(device_rows).collect();
        ctx::table(
            &[
                "machine",
                "online",
                "trainer",
                "index",
                "device",
                "backend",
                "memory",
                "load",
                "allocated",
            ],
            &rows,
        );
    });
    Ok(())
}

/// A machine's table rows: one per device, or one row for a machine with none.
fn device_rows(m: &Value) -> Vec<Vec<String>> {
    let machine = ctx::s(m, "id").to_string();
    let online = yes_no(m["online"].as_bool()).to_string();
    let trainer = yes_no(m["trainer_ok"].as_bool()).to_string();
    let devices: Vec<&Value> = items(&m["devices"]).collect();
    if devices.is_empty() {
        let dash = || "—".to_string();
        return vec![vec![
            machine,
            online,
            trainer,
            dash(),
            "no devices".to_string(),
            dash(),
            dash(),
            dash(),
            dash(),
        ]];
    }
    devices
        .into_iter()
        .map(|d| {
            vec![
                machine.clone(),
                online.clone(),
                trainer.clone(),
                ctx::text(d, "index"),
                format!("{} {}", ctx::s(d, "vendor"), ctx::s(d, "name"))
                    .trim()
                    .to_string(),
                ctx::s(d, "backend").to_string(),
                format!(
                    "{} of {} MiB",
                    ctx::text(d, "mem_used_mib"),
                    ctx::text(d, "memory_mib")
                ),
                format!(
                    "{}% {}",
                    ctx::text(d, "util_pct"),
                    if d["busy"].as_bool() == Some(true) {
                        "busy"
                    } else {
                        "idle"
                    }
                ),
                d["allocated_to"].as_str().unwrap_or("—").to_string(),
            ]
        })
        .collect()
}

fn recipes(ctx: &Ctx, cmd: RecipesCmd) -> Result<(), String> {
    match cmd {
        RecipesCmd::List => {
            let list = ctx.client.get("/v1/foundry/recipes")?;
            ctx.out(&list, |v| {
                let rows: Vec<Vec<String>> = items(v)
                    .map(|r| {
                        vec![
                            ctx::s(r, "id").to_string(),
                            ctx::s(r, "repo").to_string(),
                            short(ctx::s(r, "commit")),
                            vetted(r),
                            yes_no(r["local_dev"].as_bool()).to_string(),
                        ]
                    })
                    .collect();
                ctx::table(&["recipe", "repo", "commit", "vetted", "local dev"], &rows);
            });
            Ok(())
        }
        RecipesCmd::Vet { id, commit, yes } => {
            if !confirm(yes, &format!("Vet recipe {id} at commit {commit}."))? {
                return Err("cancelled".into());
            }
            let recipe = ctx.client.post(
                &format!("{}/vet", recipe_path(&id)),
                &json!({ "commit": commit }),
            )?;
            ctx.out(&recipe, |r| {
                println!(
                    "vetted {} at {}",
                    ctx::s(r, "id"),
                    short(ctx::s(r, "commit"))
                )
            });
            Ok(())
        }
    }
}

/// Who vetted a recipe and when, or "not vetted".
fn vetted(r: &Value) -> String {
    match r["vetted_by"].as_str() {
        Some(by) => format!("{by} {}", ctx::when(&r["vetted_ms"])),
        None => "not vetted".to_string(),
    }
}

// ---- Breeders ---------------------------------------------------------------------------------

fn breeders(ctx: &Ctx) -> Result<(), String> {
    let agents = ctx.client.get("/v1/agents")?;
    let jobs = ctx.client.get("/v1/foundry/jobs")?;
    // The laws are read for the cap only. When they cannot be read, the fallback cap is shown and marked as one.
    let cap = ctx
        .client
        .get("/v1/laws")
        .ok()
        .and_then(|laws| job_cap(&laws));
    let summary = breeder_summary(&agents, &jobs, cap);
    ctx.out(&summary, print_breeders);
    Ok(())
}

/// S-006c's per-job cap in GPU-hours; None when the law or its number is not there.
fn job_cap(laws: &Value) -> Option<f64> {
    let law = items(laws).find(|l| ctx::s(l, "id") == CAP_LAW)?;
    let text: Vec<&str> = items(&law["conditions"])
        .filter_map(Value::as_str)
        .collect();
    parse_cap(&text.join(" "))
}

/// The number after `context.gpu_hours <=` in a condition's text.
fn parse_cap(text: &str) -> Option<f64> {
    let rest = text.split("context.gpu_hours").nth(1)?;
    let rest = rest.trim_start().strip_prefix("<=")?.trim_start();
    let number: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    number.parse().ok()
}

/// The breeders (the agents with the breeder role), each with its own jobs and the GPU-hours they used.
/// A job belongs to a breeder when the hub says it was submitted with the breeder's CID.
fn breeder_summary(agents: &Value, jobs: &Value, cap: Option<f64>) -> Value {
    let mut mine: Vec<&Value> = items(agents)
        .filter(|a| ctx::s(a, "role") == "breeder")
        .collect();
    mine.sort_by_key(|a| a["cid"].as_u64().unwrap_or(0));
    let breeders: Vec<Value> = mine
        .into_iter()
        .map(|a| {
            let cid = a["cid"].as_u64();
            let own: Vec<Value> = items(jobs).filter(|j| cid.is_some() && j["agent"].as_u64() == cid).cloned().collect();
            let used: f64 = own.iter().map(|j| j["gpu_hours"].as_f64().unwrap_or(0.0)).sum();
            json!({ "cid": a["cid"].clone(), "name": ctx::s(a, "name"), "status": ctx::s(a, "status"), "jobs": own, "gpu_hours": used })
        })
        .collect();
    json!({ "cap": cap.unwrap_or(PER_JOB_CAP_FALLBACK), "cap_read": cap.is_some(), "breeders": breeders })
}

fn print_breeders(s: &Value) {
    let list: Vec<&Value> = items(&s["breeders"]).collect();
    let jobs: usize = list.iter().map(|b| items(&b["jobs"]).count()).sum();
    let used: f64 = list
        .iter()
        .map(|b| b["gpu_hours"].as_f64().unwrap_or(0.0))
        .sum();
    let source = if s["cap_read"].as_bool() == Some(true) {
        CAP_LAW
    } else {
        "fallback: the laws could not be read"
    };
    println!(
        "{} breeders · {jobs} jobs · {used:.2} GPU-h used · per-job cap {} GPU-h ({source})",
        list.len(),
        amount(&s["cap"])
    );
    let rows: Vec<Vec<String>> = list
        .iter()
        .map(|b| {
            let own: f64 = items(&b["jobs"])
                .map(|j| j["gpu_hours"].as_f64().unwrap_or(0.0))
                .sum();
            vec![
                ctx::text(b, "cid"),
                breeder_name(b),
                ctx::s(b, "status").to_string(),
                items(&b["jobs"]).count().to_string(),
                format!("{own:.2}"),
            ]
        })
        .collect();
    ctx::table(&["cid", "name", "status", "jobs", "gpu-h"], &rows);
    let job_rows: Vec<Vec<String>> = list
        .iter()
        .flat_map(|b| {
            items(&b["jobs"]).map(move |j| {
                vec![
                    ctx::text(b, "cid"),
                    ctx::s(j, "id").to_string(),
                    ctx::s(j, "status").to_string(),
                    ctx::s(j, "recipe").to_string(),
                    amount(&j["gpu_hours"]),
                    ctx::when(&j["created_ms"]),
                ]
            })
        })
        .collect();
    println!();
    ctx::table(
        &["breeder", "job", "status", "recipe", "gpu-h", "created"],
        &job_rows,
    );
}

/// A breeder's name, or `agent <cid>` when it has none.
fn breeder_name(b: &Value) -> String {
    let name = ctx::s(b, "name").trim();
    if name.is_empty() {
        format!("agent {}", ctx::text(b, "cid"))
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    /// The `lab` command alone, so its subcommands parse the way the CLI parses them.
    #[derive(Parser, Debug)]
    struct Probe {
        #[command(subcommand)]
        cmd: Cmd,
    }

    /// `lab ARGS`, parsed.
    fn parse(args: &[&str]) -> Result<Cmd, clap::Error> {
        let mut argv = vec!["lab"];
        argv.extend_from_slice(args);
        Probe::try_parse_from(argv).map(|p| p.cmd)
    }

    #[test]
    fn the_groups_are_all_there() {
        let cmd = Probe::command();
        for group in [
            "devices", "jobs", "recipes", "datasets", "models", "hf", "breeders",
        ] {
            assert!(cmd.find_subcommand(group).is_some(), "{group} is missing");
        }
    }

    #[test]
    fn a_recipe_keeps_its_at_sign_and_other_segments_are_encoded() {
        assert_eq!(recipe_path("cls@1.2"), "/v1/foundry/recipes/cls@1.2");
        assert_eq!(recipe_path("a b@1"), "/v1/foundry/recipes/a%20b@1");
        assert_eq!(job_path("job-1"), "/v1/foundry/jobs/job-1");
        assert_eq!(
            model_path("hf-org-x-1a2b"),
            "/v1/foundry/models/hf-org-x-1a2b"
        );
        assert_eq!(model_path("a/b"), "/v1/foundry/models/a%2Fb");
    }

    #[test]
    fn a_repo_keeps_its_slash_and_encodes_each_segment() {
        assert_eq!(repo_path("org/name"), "org/name");
        assert_eq!(repo_path("org/a b"), "org/a%20b");
        assert_eq!(repo_path("name"), "name");
    }

    #[test]
    fn the_per_job_cap_is_read_from_the_law_condition() {
        assert_eq!(
            parse_cap("context.gpu_hours <= 8 and context.kind == 'job'"),
            Some(8.0)
        );
        assert_eq!(parse_cap("context.gpu_hours<=2.5"), Some(2.5));
        assert_eq!(parse_cap("no cap here"), None);
        let laws = json!([
            { "id": "S-006d", "conditions": ["context.gpu_hours <= 99"] },
            { "id": "S-006c", "conditions": ["context.gpu_hours <= 4"] }
        ]);
        assert_eq!(job_cap(&laws), Some(4.0));
        assert_eq!(job_cap(&json!([])), None);
    }

    #[test]
    fn the_token_is_never_an_argument_and_the_limits_hold() {
        assert!(
            parse(&["hf", "token", "set", "hf_secret"]).is_err(),
            "a token is read, never passed"
        );
        assert!(parse(&["hf", "token", "set"]).is_ok());
        assert!(parse(&["hf", "search", "--limit", "101"]).is_err());
        assert!(parse(&["hf", "search", "--limit", "100"]).is_ok());
        assert!(parse(&["hf", "import", "org/m", "--as", "bogus"]).is_err());
        assert!(parse(&["jobs", "list", "--status", "bogus"]).is_err());
        assert!(parse(&["datasets", "export", "--kind", "upload", "--name", "x"]).is_err());
        assert!(parse(&["recipes", "vet", "cls@1", "--commit", "abc1234"]).is_ok());
    }
}
