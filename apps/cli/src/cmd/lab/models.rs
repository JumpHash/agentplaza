//! `agentplaza lab models`: the model registry. A model is a trained or imported network with its card.
//! Shadow runs it alongside the live decisions and counts how often it agrees; promote makes it live;
//! retire takes it out of service.

use clap::Subcommand;
use serde_json::{Value, json};

use super::{model_path, size_text};
use crate::cmd::models::{items, joined, none_if_empty};
use crate::ctx::{self, Ctx, confirm};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The registry: id, state, name, version, parameters and size.
    List {
        /// Only the Hugging Face base models a recipe can fine-tune.
        #[arg(long)]
        base: bool,
    },
    /// One model in full: its card, its source, its shadow totals.
    Show {
        /// The model's id.
        id: String,
    },
    /// Register a model directory (it holds model.onnx and model_card.json). The path is made absolute first.
    Import {
        /// The model directory.
        dir: String,
    },
    /// Put a model in shadow: it runs alongside the live decisions and its agreement is counted. Asks first unless --yes.
    Shadow {
        /// The model's id.
        id: String,
        /// Go ahead without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Promote a model to live. Asks first unless --yes.
    Promote {
        /// The model's id.
        id: String,
        /// Go ahead without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Retire a model: it leaves service. Asks first unless --yes.
    Retire {
        /// The model's id.
        id: String,
        /// Go ahead without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Run a model on some text (`-` reads stdin). Each run is a real call to the model.
    Invoke {
        /// The model's id.
        id: String,
        /// The text to run it on; `-` reads stdin.
        #[arg(long)]
        text: String,
    },
    /// Re-register an imported model from its download. Asks first unless --yes.
    Rederive {
        /// The model's id.
        id: String,
        /// Go ahead without asking.
        #[arg(long)]
        yes: bool,
    },
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List { base } => {
            let path = if base {
                "/v1/foundry/models?kind=base"
            } else {
                "/v1/foundry/models"
            };
            let view = ctx.client.get(path)?;
            ctx.out(&view, |v| {
                let rows: Vec<Vec<String>> = items(&v["models"])
                    .map(|m| {
                        vec![
                            ctx::s(m, "id").to_string(),
                            ctx::s(m, "state").to_string(),
                            ctx::text(&m["card"], "name"),
                            ctx::text(&m["card"], "version"),
                            ctx::text(m, "parameters"),
                            size_text(m["size_bytes"].as_f64().unwrap_or(0.0)),
                            ctx::when(&m["created_ms"]),
                        ]
                    })
                    .collect();
                ctx::table(
                    &[
                        "id",
                        "state",
                        "name",
                        "version",
                        "parameters",
                        "size",
                        "created",
                    ],
                    &rows,
                );
            });
            Ok(())
        }
        Cmd::Show { id } => {
            let model = ctx.client.get(&model_path(&id))?;
            ctx.out(&model, print_model);
            Ok(())
        }
        Cmd::Import { dir } => {
            let path = absolute(&dir)?;
            let model = ctx
                .client
                .post("/v1/foundry/models/import", &json!({ "path": path }))?;
            ctx.out(&model, |m| {
                println!("registered {} as {}", ctx::s(m, "id"), ctx::s(m, "state"))
            });
            Ok(())
        }
        Cmd::Shadow { id, yes } => act(
            ctx,
            &id,
            "shadow",
            yes,
            &format!("Put model {id} in shadow."),
        ),
        Cmd::Promote { id, yes } => act(
            ctx,
            &id,
            "promote",
            yes,
            &format!("Promote model {id} to live."),
        ),
        Cmd::Retire { id, yes } => act(ctx, &id, "retire", yes, &format!("Retire model {id}.")),
        Cmd::Invoke { id, text } => {
            let text = ctx::text_arg(&text)?;
            let reply = ctx.client.post(
                &format!("{}/invoke", model_path(&id)),
                &json!({ "text": text }),
            )?;
            ctx.out(&reply, |r| {
                match r.get("output") {
                    Some(Value::String(text)) => println!("{text}"),
                    Some(other) => println!(
                        "{}",
                        serde_json::to_string_pretty(other).unwrap_or_default()
                    ),
                    None => {}
                }
                println!("{} ms", ctx::text(r, "ms"));
            });
            Ok(())
        }
        Cmd::Rederive { id, yes } => {
            if !confirm(yes, &format!("Re-derive model {id} from its download."))? {
                return Err("cancelled".into());
            }
            let reply = ctx
                .client
                .call("POST", &format!("{}/rederive", model_path(&id)), None)?;
            ctx.out(&reply, |r| {
                println!(
                    "model {} is {}",
                    ctx::text(&r["model"], "id"),
                    ctx::s(r, "state")
                )
            });
            Ok(())
        }
    }
}

/// A state action (shadow, promote, retire): asked first, then sent, and the model's new state shown.
fn act(ctx: &Ctx, id: &str, action: &str, yes: bool, question: &str) -> Result<(), String> {
    if !confirm(yes, question)? {
        return Err("cancelled".into());
    }
    let model = ctx
        .client
        .call("POST", &format!("{}/{action}", model_path(id)), None)?;
    ctx.out(&model, |m| {
        println!("model {} is {}", ctx::s(m, "id"), ctx::s(m, "state"))
    });
    Ok(())
}

/// A model directory as an absolute path, with symbolic links followed. It must be a directory.
fn absolute(dir: &str) -> Result<String, String> {
    let path = std::fs::canonicalize(dir).map_err(|e| format!("{dir}: {e}"))?;
    if !path.is_dir() {
        return Err(format!("{dir} is not a directory"));
    }
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| format!("{dir}: the path is not UTF-8"))
}

/// A model in full: its identity, its card, and its shadow totals.
fn print_model(m: &Value) {
    let shadow = &m["shadow"];
    let agreement = match m["agreement_rate"].as_f64() {
        Some(rate) => format!("{:.1}%", rate * 100.0),
        None => "—".to_string(),
    };
    ctx::fields(&[
        ("id", ctx::s(m, "id").to_string()),
        ("state", ctx::s(m, "state").to_string()),
        ("name", ctx::text(&m["card"], "name")),
        ("version", ctx::text(&m["card"], "version")),
        ("labels", none_if_empty(joined(m["card"].get("labels")))),
        ("source job", none_if_empty(ctx::text(m, "source_job"))),
        ("agent", none_if_empty(ctx::text(m, "agent_cid"))),
        ("parameters", ctx::text(m, "parameters")),
        ("size", size_text(m["size_bytes"].as_f64().unwrap_or(0.0))),
        ("shadow runs", ctx::text(shadow, "runs")),
        ("shadow agreed", ctx::text(shadow, "agreed")),
        ("agreement", agreement),
        ("created", ctx::when(&m["created_ms"])),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_directory_is_taken_as_an_absolute_path() {
        let dir = std::env::temp_dir();
        let path = absolute(dir.to_str().expect("utf-8 temp dir")).expect("an existing directory");
        assert!(std::path::Path::new(&path).is_absolute(), "{path}");
    }

    #[test]
    fn a_missing_or_plain_file_is_refused_as_a_model_directory() {
        assert!(absolute("/definitely/not/a/model/dir").is_err());
        let file = std::env::temp_dir().join(format!("apz-lab-models-{}", std::process::id()));
        std::fs::write(&file, b"x").expect("a temp file");
        let refused = absolute(file.to_str().expect("utf-8 temp file"));
        let _ = std::fs::remove_file(&file);
        assert!(refused.is_err(), "a file is not a model directory");
    }
}
