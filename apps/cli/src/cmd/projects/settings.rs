//! `projects settings`: the plaza's settings a project overrides for its agents
//! (`crates/apid/src/project_settings.rs`). An override that is not set leaves the plaza's value, and an agent's own
//! choice still wins over both.

use clap::Subcommand;
use serde_json::{Map, Value, json};

use crate::ctx::{self, Ctx};

/// The kind of value a setting takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Text: a model (`provider/model`) or an effort level.
    Text,
    /// A whole number of 0 or more.
    Number,
    /// One of the permission modes.
    Mode,
}

/// The settings a project overrides, in the order they print (the hub's `ProjectSettings`). `role_models.<role>` is
/// the model of one role's agents besides these.
const KEYS: [(&str, Kind); 9] = [
    ("default_model", Kind::Text),
    ("effort", Kind::Text),
    ("max_tokens", Kind::Number),
    ("max_turns", Kind::Number),
    ("compress_at", Kind::Number),
    ("keep_recent", Kind::Number),
    ("task_budget", Kind::Number),
    ("permission_mode", Kind::Mode),
    ("review_timeout_minutes", Kind::Number),
];

/// The permission modes (the hub's `approvals::Mode`).
const MODES: [&str; 3] = ["autonomous", "ask", "read_only"];

/// The key of the role models, which is set role by role (`role_models.<role>`).
const ROLE_MODELS: &str = "role_models";

#[derive(Subcommand, Debug)]
pub enum Action {
    /// The project's overrides (the default).
    Get,
    /// Every setting the project's agents get, and whether it is the project's or the plaza's.
    Effective,
    /// Override settings, each as `key=value`. The keys: default_model, role_models.<role>, effort, max_tokens,
    /// max_turns, compress_at (bytes, 0 never), keep_recent, task_budget (in the project budget's unit),
    /// permission_mode (autonomous, ask or read_only) and review_timeout_minutes.
    Set {
        /// The settings, such as `effort=high` or `role_models.builder=provider/model`.
        #[arg(required = true, value_name = "KEY=VALUE")]
        pairs: Vec<String>,
    },
    /// Drop overrides, so the plaza's settings hold again: `role_models.<role>` drops one role's model, `role_models`
    /// all of them.
    Unset {
        /// The settings' keys.
        #[arg(required = true, value_name = "KEY")]
        keys: Vec<String>,
    },
}

/// A setting's key as the body names it: one of `KEYS`, one role's model, or every role's.
#[derive(Debug, PartialEq, Eq)]
enum Key<'a> {
    Setting(&'static str, Kind),
    Role(&'a str),
    Roles,
}

fn key_of(key: &str) -> Result<Key<'_>, String> {
    let key = key.trim();
    if key == ROLE_MODELS {
        return Ok(Key::Roles);
    }
    if let Some(role) = key.strip_prefix("role_models.") {
        let role = role.trim();
        if role.is_empty() {
            return Err("role_models.<role> names the role, such as role_models.builder".into());
        }
        return Ok(Key::Role(role));
    }
    KEYS.iter()
        .find(|(name, _)| *name == key)
        .map(|(name, kind)| Key::Setting(name, *kind))
        .ok_or_else(|| {
            let names: Vec<&str> = KEYS.iter().map(|(name, _)| *name).collect();
            format!(
                "no setting {key:?}: a project overrides {}, and role_models.<role>",
                names.join(", ")
            )
        })
}

/// A setting's value as the hub takes it: text, a whole number, or a permission mode.
fn value_of(key: &str, kind: Kind, value: &str) -> Result<Value, String> {
    match kind {
        Kind::Text => Ok(json!(value)),
        Kind::Number => value
            .parse::<u64>()
            .map(|n| json!(n))
            .map_err(|_| format!("{key} is a whole number, not {value:?}")),
        Kind::Mode => {
            let mode = value.replace('-', "_");
            if MODES.contains(&mode.as_str()) {
                Ok(json!(mode))
            } else {
                Err(format!(
                    "{key} is autonomous, ask or read_only, not {value:?}"
                ))
            }
        }
    }
}

/// The `PATCH /v1/projects/{id}/settings` body that sets `pairs` (`key=value` each): every value of the kind its
/// setting takes, and a role's model under `role_models`. An unknown key, or a value that is missing or of the wrong
/// kind, is refused before anything is sent.
fn set_body(pairs: &[String]) -> Result<Value, String> {
    let mut body = Map::new();
    let mut roles = Map::new();
    for pair in pairs {
        let (key, value) = pair
            .split_once('=')
            .ok_or_else(|| format!("{pair:?} is not key=value"))?;
        let value = value.trim();
        if value.is_empty() {
            return Err(format!(
                "{pair:?} has no value: `projects settings ID unset {}` sets it back to the plaza's",
                key.trim()
            ));
        }
        match key_of(key)? {
            Key::Setting(name, kind) => {
                body.insert(name.to_string(), value_of(name, kind, value)?);
            }
            Key::Role(role) => {
                roles.insert(role.to_string(), json!(value));
            }
            Key::Roles => {
                return Err(
                    "role_models is set role by role: role_models.<role>=provider/model".into(),
                );
            }
        }
    }
    if !roles.is_empty() {
        body.insert(ROLE_MODELS.to_string(), Value::Object(roles));
    }
    Ok(Value::Object(body))
}

/// The `PATCH` body that unsets `keys`: each to null, which sets it back to the plaza's; a role's model under
/// `role_models`, and `role_models` alone all of them.
fn unset_body(keys: &[String]) -> Result<Value, String> {
    let mut body = Map::new();
    let mut roles = Map::new();
    let mut all_roles = false;
    for key in keys {
        match key_of(key)? {
            Key::Setting(name, _) => {
                body.insert(name.to_string(), Value::Null);
            }
            Key::Role(role) => {
                roles.insert(role.to_string(), Value::Null);
            }
            Key::Roles => all_roles = true,
        }
    }
    if all_roles {
        body.insert(ROLE_MODELS.to_string(), Value::Null);
    } else if !roles.is_empty() {
        body.insert(ROLE_MODELS.to_string(), Value::Object(roles));
    }
    Ok(Value::Object(body))
}

/// A setting's value as text: strings bare, null as "-", anything else as its JSON.
fn value_text(value: &Value) -> String {
    match value {
        Value::Null => "-".to_string(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// A project's overrides as `(key, value)` pairs, as `set` takes them: the settings in the order of `KEYS`, then each
/// role's model as `role_models.<role>`, then any other the hub sends. Empty when none is set.
pub(crate) fn pairs(settings: &Value) -> Vec<(String, String)> {
    let Some(map) = settings.as_object() else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = KEYS
        .iter()
        .filter_map(|(key, _)| {
            let value = map.get(*key).filter(|v| !v.is_null())?;
            Some((key.to_string(), value_text(value)))
        })
        .collect();
    if let Some(roles) = map.get(ROLE_MODELS).and_then(Value::as_object) {
        for (role, model) in roles {
            out.push((format!("role_models.{role}"), value_text(model)));
        }
    }
    for (key, value) in map {
        let known = key == ROLE_MODELS || KEYS.iter().any(|(name, _)| name == key);
        if !known && !value.is_null() {
            out.push((key.clone(), value_text(value)));
        }
    }
    out
}

/// The effective settings as rows: each setting, the value the project's agents get and where it comes from (`project`
/// or `plaza`), in the order of `KEYS`; then each role's model.
fn effective_rows(effective: &Value) -> Vec<Vec<String>> {
    let row = |key: String, entry: &Value| {
        vec![
            key,
            value_text(entry.get("value").unwrap_or(&Value::Null)),
            ctx::s(entry, "source").to_string(),
        ]
    };
    let mut rows = Vec::new();
    if let Some(settings) = effective.get("settings").and_then(Value::as_object) {
        for (key, _) in KEYS {
            if let Some(entry) = settings.get(key) {
                rows.push(row(key.to_string(), entry));
            }
        }
        for (key, entry) in settings {
            if !KEYS.iter().any(|(name, _)| name == key) {
                rows.push(row(key.clone(), entry));
            }
        }
    }
    if let Some(roles) = effective.get(ROLE_MODELS).and_then(Value::as_object) {
        for (role, entry) in roles {
            rows.push(row(format!("role_models.{role}"), entry));
        }
    }
    rows
}

/// `projects settings ID`: the project's overrides; `effective`, `set` and `unset` show or change them.
pub fn settings(ctx: &Ctx, id: &str, action: Option<Action>) -> Result<(), String> {
    let path = format!("/v1/projects/{}/settings", ctx::seg(id));
    let saved = match action.unwrap_or(Action::Get) {
        Action::Get => {
            let settings = ctx.client.get(&path)?;
            ctx.out(&settings, |s| print_overrides(id, s));
            return Ok(());
        }
        Action::Effective => {
            let effective = ctx.client.get(&format!("{path}/effective"))?;
            ctx.out(&effective, |e| {
                ctx::table(&["setting", "value", "from"], &effective_rows(e));
            });
            return Ok(());
        }
        Action::Set { pairs } => ctx.client.patch(&path, &set_body(&pairs)?)?,
        Action::Unset { keys } => ctx.client.patch(&path, &unset_body(&keys)?)?,
    };
    ctx.out(&saved, |s| {
        println!("saved the settings of project {id}");
        print_overrides(id, s);
    });
    Ok(())
}

fn print_overrides(id: &str, settings: &Value) {
    let pairs = pairs(settings);
    if pairs.is_empty() {
        println!("project {id} overrides none of the plaza's settings");
        return;
    }
    let fields: Vec<(&str, String)> = pairs
        .iter()
        .map(|(key, value)| (key.as_str(), value.clone()))
        .collect();
    ctx::fields(&fields);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn set_sends_each_value_as_its_setting_takes_it() {
        let body = set_body(&strings(&[
            "effort=high",
            "max_turns=40",
            "permission_mode=read-only",
            "role_models.builder=anthropic/claude-sonnet-5-5",
            "role_models.qa = openai/gpt-6-luna",
        ]))
        .unwrap();
        assert_eq!(
            body,
            json!({
                "effort": "high",
                "max_turns": 40,
                "permission_mode": "read_only",
                "role_models": { "builder": "anthropic/claude-sonnet-5-5", "qa": "openai/gpt-6-luna" }
            })
        );
    }

    #[test]
    fn set_refuses_what_the_hub_would_not_take() {
        let refused = |pair: &str| set_body(&strings(&[pair])).unwrap_err();
        assert!(refused("colour=red").contains("no setting \"colour\""));
        assert!(refused("max_turns=many").contains("whole number"));
        assert!(refused("max_turns=-1").contains("whole number"));
        assert!(refused("permission_mode=sometimes").contains("autonomous, ask or read_only"));
        assert!(refused("effort").contains("not key=value"));
        assert!(refused("effort=").contains("unset effort"));
        assert!(refused("role_models=x").contains("role by role"));
        assert!(refused("role_models.=x").contains("names the role"));
    }

    #[test]
    fn unset_sends_null_and_role_models_alone_drops_every_role() {
        assert_eq!(
            unset_body(&strings(&["effort", "role_models.qa"])).unwrap(),
            json!({ "effort": null, "role_models": { "qa": null } })
        );
        assert_eq!(
            unset_body(&strings(&["role_models.qa", "role_models"])).unwrap(),
            json!({ "role_models": null })
        );
        assert!(unset_body(&strings(&["colour"])).is_err());
    }

    #[test]
    fn overrides_print_in_the_settings_order_with_each_roles_model() {
        let settings = json!({
            "task_budget": 3000,
            "effort": "high",
            "role_models": { "builder": "anthropic/claude-sonnet-5-5" },
            "later_setting": true
        });
        let shown: Vec<(String, String)> = pairs(&settings);
        assert_eq!(
            shown,
            [
                ("effort", "high"),
                ("task_budget", "3000"),
                ("role_models.builder", "anthropic/claude-sonnet-5-5"),
                ("later_setting", "true"),
            ]
            .map(|(k, v)| (k.to_string(), v.to_string()))
        );
        assert!(pairs(&json!({})).is_empty());
    }

    #[test]
    fn effective_settings_say_where_each_value_comes_from() {
        let effective = json!({
            "project": "pr3",
            "settings": {
                "max_turns": { "value": 40, "source": "project" },
                "default_model": { "value": "openai/gpt-6", "source": "plaza" },
                "effort": { "value": null, "source": "plaza" }
            },
            "role_models": { "qa": { "value": "xai/grok-4.7", "source": "project" } }
        });
        assert_eq!(
            effective_rows(&effective),
            [
                ["default_model", "openai/gpt-6", "plaza"],
                ["effort", "-", "plaza"],
                ["max_turns", "40", "project"],
                ["role_models.qa", "xai/grok-4.7", "project"],
            ]
            .map(|row| row.map(String::from).to_vec())
        );
    }
}
