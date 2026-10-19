//! `agentplaza recipes`: Review recipes, the steps a sealed round is reviewed with (code, docs, research). The
//! recipes and their versions, the orchestrator's proposals for new ones, and a new version of a recipe from a file.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde_json::{Value, json};

use crate::ctx::{self, Ctx};

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The recipes, with their version, what they apply to, and their rounds and steps.
    List,
    /// One recipe as its TOML (`--version N` for an earlier version).
    #[command(disable_version_flag = true)]
    Show {
        /// The recipe's id, such as `code`.
        id: String,
        /// An earlier version, from 1.
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        version: Option<u64>,
    },
    /// Save a new version of a recipe from a TOML file; `-` reads stdin.
    Put {
        /// The recipe's id.
        id: String,
        /// The recipe's TOML file.
        file: PathBuf,
    },
    /// The recipes the orchestrator proposes, with its reason and each one's state.
    Proposals,
    /// Approve a proposed recipe.
    Approve {
        /// The proposal's id, such as `rp1`.
        proposal: String,
    },
    /// Decline a proposed recipe.
    Decline {
        /// The proposal's id.
        proposal: String,
    },
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List => {
            let book = ctx.client.get("/v1/recipes")?;
            ctx.out(&book, |b| {
                ctx::table(
                    &["id", "version", "title", "applies to", "rounds", "steps"],
                    &recipe_rows(b),
                );
            });
            Ok(())
        }
        Cmd::Show { id, version } => {
            let path = match version {
                Some(version) => format!("/v1/recipes/{}?version={version}", ctx::seg(&id)),
                None => format!("/v1/recipes/{}", ctx::seg(&id)),
            };
            let shown = ctx.client.get(&path)?;
            ctx.out(&shown, |s| println!("{}", ctx::s(s, "toml").trim_end()));
            Ok(())
        }
        Cmd::Put { id, file } => {
            let toml = read_toml(&file)?;
            let saved = ctx.client.put(
                &format!("/v1/recipes/{}", ctx::seg(&id)),
                &json!({ "toml": toml }),
            )?;
            ctx.out(&saved, |s| {
                println!(
                    "saved recipe {id} as version {}",
                    ctx::text(&s["recipe"], "version")
                )
            });
            Ok(())
        }
        Cmd::Proposals => {
            let book = ctx.client.get("/v1/recipes")?;
            let proposals = Value::Array(array_at(&book, "proposals").to_vec());
            ctx.out(&proposals, |p| {
                ctx::table(
                    &["id", "recipe", "title", "by", "state", "why"],
                    &proposal_rows(p),
                );
            });
            Ok(())
        }
        Cmd::Approve { proposal } => decide(ctx, &proposal, true),
        Cmd::Decline { proposal } => decide(ctx, &proposal, false),
    }
}

/// Approves or declines a proposed recipe; the hub answers with the proposal as it stands now.
fn decide(ctx: &Ctx, proposal: &str, approve: bool) -> Result<(), String> {
    let decided = ctx.client.post(
        &format!("/v1/recipes/proposals/{}", ctx::seg(proposal)),
        &json!({ "approve": approve }),
    )?;
    ctx.out(&decided, |_| {
        println!(
            "{} proposal {proposal}",
            if approve { "approved" } else { "declined" }
        )
    });
    Ok(())
}

/// A recipe's TOML from a file, or from stdin for `-`. An empty file is refused.
fn read_toml(file: &Path) -> Result<String, String> {
    if file == Path::new("-") {
        return ctx::text_arg("-");
    }
    let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    if text.trim().is_empty() {
        return Err(format!("{} is empty: a recipe is TOML", file.display()));
    }
    Ok(text)
}

/// The objects of a JSON array under `key`.
fn array_at<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// What a recipe applies to, by its kinds of work (`code, docs`).
fn applies_to(recipe: &Value) -> String {
    recipe["applies"]["kinds"]
        .as_array()
        .map(|kinds| {
            kinds
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

/// The recipes of `GET /v1/recipes` as table rows.
fn recipe_rows(book: &Value) -> Vec<Vec<String>> {
    array_at(book, "recipes")
        .iter()
        .map(|r| {
            vec![
                ctx::s(r, "id").to_string(),
                ctx::text(r, "version"),
                ctx::s(r, "title").to_string(),
                applies_to(r),
                ctx::text(r, "rounds"),
                ctx::text(r, "nodes"),
            ]
        })
        .collect()
}

/// The proposals as table rows: the recipe each one would add, by whom and when it was made.
fn proposal_rows(proposals: &Value) -> Vec<Vec<String>> {
    proposals
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|p| {
            let recipe = &p["recipe"];
            vec![
                ctx::s(p, "id").to_string(),
                format!("{}@{}", ctx::s(recipe, "id"), ctx::text(recipe, "version")),
                ctx::s(recipe, "title").to_string(),
                ctx::text(p, "by"),
                ctx::s(p, "state").to_string(),
                ctx::s(p, "why").to_string(),
            ]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `GET /v1/recipes` as the hub answers it: the built-in recipes and one proposal.
    fn book() -> Value {
        json!({
            "recipes": [
                { "id": "code", "version": 3, "title": "Code", "applies": { "kinds": ["code"], "roles": [] },
                  "axes": ["fea", "tst"], "rounds": 2, "nodes": 9 },
                { "id": "docs", "version": 1, "title": "Documentation", "applies": { "kinds": ["docs", "research"], "roles": [] },
                  "axes": ["fea"], "rounds": 2, "nodes": 5 }
            ],
            "proposals": [
                { "id": "rp1", "by": 301, "why": "tests want a separate step", "state": "pending", "at_ms": 1,
                  "recipe": { "id": "code", "version": 4, "title": "Code, with tests", "applies": { "kinds": ["code"], "roles": [] } } }
            ]
        })
    }

    #[test]
    fn the_recipes_list_as_rows_with_what_they_apply_to() {
        assert_eq!(
            recipe_rows(&book()),
            [
                vec!["code", "3", "Code", "code", "2", "9"],
                vec!["docs", "1", "Documentation", "docs, research", "2", "5"],
            ]
        );
    }

    #[test]
    fn a_proposal_names_the_recipe_it_would_add() {
        assert_eq!(
            proposal_rows(&book()["proposals"]),
            [vec![
                "rp1",
                "code@4",
                "Code, with tests",
                "301",
                "pending",
                "tests want a separate step",
            ]]
        );
    }
}
