//! Repository provisioning through the same authorization the owner granted for Git exports.
//! Creation is private-only and requires the exact repository name, even when a namespace grants
//! access to all existing repositories. A failed create is never interpreted as an existing repo.

use serde::Deserialize;
use serde_json::{Value, json};

use super::{Integration, Kind, Level};
use crate::agents::Agent;
use crate::state::App;

pub const TOOL: &str = "plaza_repository";

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    List,
    Create {
        authorization: String,
        repo: String,
    },
    Connect {
        authorization: String,
        repo: String,
        project: String,
    },
}

pub fn spec(name: &str) -> Option<(&'static str, Value)> {
    (name == TOOL).then(|| (
        "List Git authorizations, create a new private GitHub repository explicitly granted by the owner, or connect a granted repository to a project. Create requires an exact named grant; namespace-wide access alone is insufficient. Create never adopts an existing repository. Connect after a successful create, before the first task lands, so landed work exports to GitHub. Credentials stay on the hub.",
        json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["list", "create", "connect"]},
                "authorization": {"type": "string"},
                "repo": {"type": "string", "description": "Exact owner/name granted by the owner."},
                "project": {"type": "string"}
            },
            "required": ["action"], "additionalProperties": false
        }),
    ))
}

/// The orchestrator can provision only what the owner explicitly named. Workers cannot call this
/// service even if a future tool-library alias accidentally exposes it.
pub fn run(app: &App, agent: &Agent, input: &Value) -> Result<Value, String> {
    if agent.role != crate::settings::ORCHESTRATOR_ROLE {
        return Err("only the orchestrator may provision repositories".into());
    }
    let action: Action = serde_json::from_value(input.clone())
        .map_err(|_| "repository action is list, create {authorization, repo}, or connect {authorization, repo, project}")?;
    match action {
        Action::List => Ok(json!(
            app.platforms
                .integrations()
                .iter()
                .map(|i| json!({
                    "id": i.id, "kind": i.kind, "host": i.host,
                    "account": i.account.login, "grants": i.grants
                }))
                .collect::<Vec<_>>()
        )),
        Action::Create {
            authorization,
            repo,
        } => create(app, &authorization, &repo),
        Action::Connect {
            authorization,
            repo,
            project,
        } => super::connect_project(app, &authorization, &repo, &project, false)
            .map_err(|(status, why)| format!("connecting the repository ({status}): {why}")),
    }
}

/// Checks all local authorization and input constraints before reading credentials or doing HTTP.
fn checked<'a>(integration: &Integration, repo: &'a str) -> Result<(&'a str, &'a str), String> {
    let (owner, name) = repo.split_once('/').ok_or("a repository is owner/name")?;
    let component = |s: &str, limit: usize| {
        !s.is_empty()
            && s.len() <= limit
            && s != "."
            && s != ".."
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    if !component(owner, 100) || !component(name, 100) {
        return Err(
            "a repository must have a valid owner and name, without URL or path components".into(),
        );
    }
    if integration.kind != Kind::Github {
        return Err("repository creation currently supports GitHub only".into());
    }
    match integration.level {
        Level::Repo => {
            return Err("a repository-scoped authorization cannot create repositories".into());
        }
        Level::Org if integration.target.as_deref() != Some(owner) => {
            return Err("the repository is outside this authorization's organization".into());
        }
        Level::Org | Level::User => {}
    }
    if !integration
        .grants
        .namespaces
        .get(owner)
        .is_some_and(|g| g.repos.contains(name))
    {
        return Err(format!(
            "the owner must explicitly grant {repo} by name before it can be created; all-repositories access does not authorize creation"
        ));
    }
    Ok((owner, name))
}

pub fn create(app: &App, id: &str, repo: &str) -> Result<Value, String> {
    let integration = app
        .platforms
        .integration(id)
        .ok_or("no such Git authorization")?;
    let (owner, name) = checked(&integration, repo)?;
    let namespace = super::namespaces(app, &integration)?
        .into_iter()
        .find(|n| n.name == owner)
        .ok_or("the authorization cannot reach the requested namespace")?;
    let auth = super::access(app, &integration)?;
    let created = app
        .platforms
        .github(&integration.host)
        .create_private_repository(&auth, &namespace, name)?;
    if created.full_name != repo || !created.private {
        return Err("GitHub returned an unexpected repository identity or visibility; inspect the platform before retrying".into());
    }
    super::publish(app, "repository_created", id);
    Ok(json!({"created": true, "authorization": id, "repository": created}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn integration() -> Integration {
        serde_json::from_value(json!({
            "id": "gh", "kind": "github", "host": "https://github.com",
            "via": "token", "level": "user", "account": {"login": "owner"},
            "created_ms": 0, "grants": {"namespaces": {"owner": {"all": true, "repos": []}}}
        }))
        .unwrap()
    }

    #[test]
    fn namespace_access_alone_cannot_create_and_exact_grants_do_not_escape_scope() {
        let mut i = integration();
        assert!(
            checked(&i, "owner/shop")
                .unwrap_err()
                .contains("explicitly grant")
        );
        i.grants
            .namespaces
            .get_mut("owner")
            .unwrap()
            .repos
            .insert("shop".into());
        assert_eq!(checked(&i, "owner/shop").unwrap(), ("owner", "shop"));
        for invalid in [
            "owner/other",
            "other/shop",
            "owner/../shop",
            "owner/..",
            "https://github.com/owner/shop",
        ] {
            assert!(checked(&i, invalid).is_err(), "{invalid}");
        }
        i.level = Level::Repo;
        assert!(checked(&i, "owner/shop").is_err());
        i.level = Level::Org;
        i.target = Some("other".into());
        assert!(checked(&i, "owner/shop").is_err());
        i.target = Some("owner".into());
        assert!(checked(&i, "owner/shop").is_ok());
    }

    #[test]
    fn actions_cannot_smuggle_visibility_or_missing_fields() {
        assert!(serde_json::from_value::<Action>(json!({"action":"create", "authorization":"gh", "repo":"owner/shop", "private":false})).is_err());
        assert!(
            serde_json::from_value::<Action>(
                json!({"action":"connect", "authorization":"gh", "repo":"owner/shop"})
            )
            .is_err()
        );
    }
}
