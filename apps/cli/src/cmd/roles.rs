//! `agentplaza roles`: the seat templates, the roles a task's seats are filled from. Bare, it lists the built-in
//! and the custom ones; `save` adds a custom one (or changes it) from its JSON definition, and `delete` removes a
//! custom one that no agent runs.

use std::path::PathBuf;

use clap::Subcommand;
use serde_json::Value;

use crate::ctx::{self, Ctx, confirm};

/// `agentplaza roles`: lists the seat templates when no subcommand is given.
#[derive(clap::Args, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The seat templates, built in and custom, with the model each runs on (the plaza's default when it names none).
    List,
    /// Save a seat template from its JSON definition (a file, or - for stdin): a new custom role, or the change of one.
    Save {
        /// The role definition as JSON, or - to read it from stdin.
        file: PathBuf,
    },
    /// Delete a custom seat template. Asks to confirm unless --yes.
    Delete {
        /// The role id.
        id: String,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
}

/// The seat templates as rows: id, title, source, and the model (the plaza's default when the role names none).
fn role_rows(roles: &Value) -> Vec<Vec<String>> {
    roles
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|role| {
            let model = role
                .get("def")
                .map(|def| ctx::s(def, "model"))
                .unwrap_or_default();
            vec![
                ctx::s(role, "id").to_string(),
                ctx::s(role, "title").to_string(),
                ctx::s(role, "source").to_string(),
                if model.is_empty() {
                    "plaza default".to_string()
                } else {
                    model.to_string()
                },
            ]
        })
        .collect()
}

pub fn run(ctx: &Ctx, args: Args) -> Result<(), String> {
    match args.command.unwrap_or(Cmd::List) {
        Cmd::List => {
            let roles = ctx.client.get("/v1/roles")?;
            ctx.out(&roles, |v| {
                ctx::table(&["id", "title", "source", "model"], &role_rows(v));
            });
        }
        Cmd::Save { file } => {
            let text = if file.as_os_str() == "-" {
                std::io::read_to_string(std::io::stdin()).map_err(|e| format!("stdin: {e}"))?
            } else {
                std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?
            };
            let def: Value =
                serde_json::from_str(&text).map_err(|e| format!("the role is not JSON: {e}"))?;
            let saved = ctx.client.post("/v1/roles", &def)?;
            ctx.out(&saved, |v| println!("saved role {}", ctx::s(v, "id")));
        }
        Cmd::Delete { id, yes } => {
            if !confirm(
                yes,
                &format!("Delete role {id}? The hub refuses it while agents of the role run."),
            )? {
                println!("cancelled");
                return Ok(());
            }
            let v = ctx.client.delete(&format!("/v1/roles/{}", ctx::seg(&id)))?;
            ctx.out(&v, |_| println!("deleted role {id}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use serde_json::json;

    /// The `roles` arguments as the CLI parses them: the subcommand, or none for a bare `roles`; `None` when refused.
    fn parsed(args: &[&str]) -> Option<Option<Cmd>> {
        let mut all = vec!["agentplaza", "roles"];
        all.extend_from_slice(args);
        match crate::Cli::try_parse_from(all).ok()?.command {
            crate::Command::Roles(roles) => Some(roles.command),
            _ => None,
        }
    }

    #[test]
    fn roles_lists_bare_and_saves_and_deletes_with_their_arguments() {
        assert!(matches!(parsed(&[]), Some(None)), "a bare roles lists them");
        assert!(matches!(parsed(&["list"]), Some(Some(Cmd::List))));
        assert!(matches!(
            parsed(&["save", "-"]),
            Some(Some(Cmd::Save { file })) if file.as_os_str() == "-"
        ));
        assert!(matches!(
            parsed(&["delete", "analyst"]),
            Some(Some(Cmd::Delete { yes: false, .. }))
        ));
        assert!(matches!(
            parsed(&["delete", "analyst", "--yes"]),
            Some(Some(Cmd::Delete { yes: true, .. }))
        ));
        assert!(parsed(&["save"]).is_none(), "a save needs a file or -");
        assert!(parsed(&["delete"]).is_none(), "a delete needs the role id");
    }

    #[test]
    fn a_seat_template_lists_its_id_title_source_and_model_or_the_plaza_default() {
        let roles = json!([
            { "id": "builder", "title": "Builder", "source": "builtin", "agents": 2,
              "def": { "id": "builder", "title": "Builder", "model": "" } },
            { "id": "analyst", "title": "Analyst", "source": "custom", "agents": 0,
              "def": { "id": "analyst", "title": "Analyst", "model": "openai/gpt-6" } }
        ]);
        assert_eq!(
            role_rows(&roles),
            [
                ["builder", "Builder", "builtin", "plaza default"],
                ["analyst", "Analyst", "custom", "openai/gpt-6"],
            ]
            .map(|row| row.map(String::from).to_vec())
        );
        assert!(role_rows(&json!({ "error": "x" })).is_empty());
    }
}
