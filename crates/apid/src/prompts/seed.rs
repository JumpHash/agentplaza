//! The built-in role prompts imported into the library at start (docs/design/prompt-library.md, "Built-in prompts").
//!
//! Each built-in role prompt becomes the prompt `role-<role>`, kind system, bound to `role:<role>`, written by
//! `builtin` and published. When a release changes a built-in file, its text is added as a new `builtin` version,
//! published only while the live version is itself a `builtin` one: the owner's edits are never overwritten, and an
//! owner who unpublished the prompt keeps it unpublished.

use std::collections::BTreeMap;

use apz_prompt::Kind;

use super::{sha256_hex, NewPrompt, Prompts, StoreError};

pub const AUTHOR: &str = "builtin";

/// What `import` did for one role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Created { published: bool },
    Updated { published: bool },
    Unchanged,
    Failed(String),
}

/// The library id of a built-in role's prompt.
pub fn id_of(role: &str) -> String {
    apz_prompt::slug(&format!("role-{role}"))
}

/// Imports every built-in role prompt (`role` → text). Empty texts are skipped. Never fails start-up: a role that
/// cannot be imported is reported and keeps its built-in prompt.
pub fn import(prompts: &Prompts, builtins: &BTreeMap<String, String>, label: impl Fn(&str) -> String, now_ms: u64)
    -> BTreeMap<String, Outcome> {
    let mut out = BTreeMap::new();
    for (role, text) in builtins {
        if text.trim().is_empty() {
            continue;
        }
        let outcome = import_one(prompts, role, text, &label(role), now_ms).unwrap_or_else(|e| Outcome::Failed(format!("{e:?}")));
        if let Outcome::Failed(why) = &outcome {
            eprintln!("apid: prompt library: built-in prompt for {role} not imported: {why}");
        }
        out.insert(role.clone(), outcome);
    }
    out
}

fn import_one(prompts: &Prompts, role: &str, text: &str, label: &str, now_ms: u64) -> Result<Outcome, StoreError> {
    let id = id_of(role);
    let binding = format!("role:{role}");
    let store = prompts.store();
    let Some(prompt) = store.get(&id)? else {
        // Bind it unless another live prompt already holds the role (the owner made their own).
        let taken = store.list(false)?.iter().any(|p| p.binding.as_deref() == Some(&binding));
        let new = NewPrompt {
            id: id.clone(),
            kind: Kind::System,
            title: format!("{label} role"),
            description: format!("The built-in system prompt of the {label} role."),
            tags: vec!["built-in".into()],
            binding: (!taken && super::valid_binding(Kind::System, &binding)).then_some(binding),
        };
        prompts.create(&new, text.to_string(), "Imported from the built-in role prompt".into(), AUTHOR, now_ms)?;
        let published = publish(prompts, &id, 1, now_ms);
        return Ok(Outcome::Created { published });
    };
    let versions = store.versions(&id)?;
    let sha = sha256_hex(text);
    if versions.iter().any(|v| v.sha256 == sha && v.author == AUTHOR) {
        return Ok(Outcome::Unchanged);
    }
    let (meta, created) =
        prompts.add_version(&id, prompt.latest_version, text.to_string(), "Updated built-in role prompt".into(), AUTHOR, now_ms)?;
    if !created {
        return Ok(Outcome::Unchanged);
    }
    let live_is_builtin = prompt
        .active_version
        .and_then(|live| versions.iter().find(|v| v.version == live))
        .is_some_and(|v| v.author == AUTHOR);
    let published = live_is_builtin && publish(prompts, &id, meta.version, now_ms);
    Ok(Outcome::Updated { published })
}

/// Publishes, logging (not failing) when the version has error-level findings.
fn publish(prompts: &Prompts, id: &str, version: u32, now_ms: u64) -> bool {
    match prompts.publish(id, version, AUTHOR, now_ms) {
        Ok(_) => true,
        Err(e) => {
            eprintln!("apid: prompt library: built-in {id} v{version} not published: {e:?}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::prompts::store_mem::MemoryPrompts;

    const T0: u64 = 1_800_000_000_000;
    const V1: &str = "# Builder\n\n## Purpose\nYou build.\n\n## Responsibilities\n- Write code.\n\n## Deliverables\n- Code.\n\n## Boundaries\n- Stay in your VM.\n";

    fn builtins(text: &str) -> BTreeMap<String, String> {
        BTreeMap::from([("builder".to_string(), text.to_string())])
    }

    #[test]
    fn imports_publishes_and_follows_releases_until_the_owner_edits() {
        let prompts = Prompts::new(Arc::new(MemoryPrompts::in_memory()));
        let label = |r: &str| r.to_string();
        assert_eq!(import(&prompts, &builtins(V1), label, T0)["builder"], Outcome::Created { published: true });
        assert_eq!(prompts.bound_body("role:builder").as_deref(), Some(V1));
        // Restarting with the same file changes nothing.
        assert_eq!(import(&prompts, &builtins(V1), label, T0 + 1)["builder"], Outcome::Unchanged);
        // A release changes the file: a new builtin version, published.
        let v2 = format!("{V1}\nShip small changes.\n");
        assert_eq!(import(&prompts, &builtins(&v2), label, T0 + 2)["builder"], Outcome::Updated { published: true });
        assert_eq!(prompts.bound_body("role:builder").as_deref(), Some(v2.as_str()));
        // The owner publishes an edit; the next release is added but not published.
        let owner = format!("{v2}\nThe owner's line.\n");
        prompts.add_version("role-builder", 2, owner.clone(), String::new(), "owner", T0 + 3).unwrap();
        prompts.publish("role-builder", 3, "owner", T0 + 4).unwrap();
        let v4 = format!("{V1}\nA later release.\n");
        assert_eq!(import(&prompts, &builtins(&v4), label, T0 + 5)["builder"], Outcome::Updated { published: false });
        assert_eq!(prompts.bound_body("role:builder").as_deref(), Some(owner.as_str()));
    }
}
