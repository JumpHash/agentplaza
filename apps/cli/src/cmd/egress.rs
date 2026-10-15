//! `agentplaza egress`: the egress credentials. A rule has the hub add a vault secret to an agent's
//! outbound HTTPS request, so the secret never enters the VM. The owner writes the rules here; a rule
//! names its secret and never shows its value.

use clap::Subcommand;
use serde_json::{Value, json};

use crate::ctx::{self, Ctx, confirm, seg};

/// The rules collection on the hub.
const RULES: &str = "/v1/egress/inject";

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The egress rules: which roles reach which hosts, and the secret each adds.
    List,
    /// Add an egress rule. The secret is a vault secret's name (bound to the egress consumer); its value is never shown.
    Add {
        /// The host: exact (api.example.com) or a suffix (*.example.com).
        #[arg(long)]
        host: String,
        /// The port. 443 by default: the hub refuses 80, since a credential only goes over HTTPS.
        #[arg(long, default_value_t = 443)]
        port: u16,
        /// A method the rule covers (repeatable). None means any method.
        #[arg(long = "method", value_name = "METHOD")]
        methods: Vec<String>,
        /// The path prefix the rule covers.
        #[arg(long, default_value = "/")]
        path_prefix: String,
        /// A role the rule covers (repeatable). None means every role.
        #[arg(long = "role", value_name = "ROLE")]
        roles: Vec<String>,
        /// The header the secret goes into.
        #[arg(long, default_value = "Authorization")]
        header: String,
        /// The header's value, with ${SECRET} where the secret goes.
        #[arg(long, default_value = "Bearer ${SECRET}")]
        format: String,
        /// The vault secret's name.
        #[arg(long)]
        secret: String,
    },
    /// Turn a rule on.
    Enable {
        /// The rule's id, from `egress list`.
        id: String,
    },
    /// Turn a rule off. It is kept, and can be turned back on.
    Disable {
        /// The rule's id, from `egress list`.
        id: String,
    },
    /// Remove a rule. Asks to confirm unless --yes.
    Rm {
        /// The rule's id, from `egress list`.
        id: String,
        /// Skip the confirmation.
        #[arg(long)]
        yes: bool,
    },
}

/// What a new rule says, as the owner gave it on the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRule {
    pub host: String,
    pub port: u16,
    pub methods: Vec<String>,
    pub path_prefix: String,
    pub roles: Vec<String>,
    pub header: String,
    pub format: String,
    pub secret: String,
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List => list(ctx),
        Cmd::Add {
            host,
            port,
            methods,
            path_prefix,
            roles,
            header,
            format,
            secret,
        } => add(
            ctx,
            &NewRule {
                host,
                port,
                methods,
                path_prefix,
                roles,
                header,
                format,
                secret,
            },
        ),
        Cmd::Enable { id } => set_enabled(ctx, &id, true),
        Cmd::Disable { id } => set_enabled(ctx, &id, false),
        Cmd::Rm { id, yes } => remove(ctx, &id, yes),
    }
}

/// One rule's path on the hub.
fn rule_path(id: &str) -> String {
    format!("{RULES}/{}", seg(id))
}

/// The request body for a new rule. Only what the owner gave is sent; the hub fills in the rest.
pub fn rule_body(rule: &NewRule) -> Value {
    json!({
        "roles": rule.roles,
        "host": rule.host,
        "port": rule.port,
        "methods": rule.methods,
        "path_prefix": rule.path_prefix,
        "header": rule.header,
        "format": rule.format,
        "secret": rule.secret,
    })
}

/// The body that turns a rule on or off.
pub fn enabled_body(enabled: bool) -> Value {
    json!({ "enabled": enabled })
}

/// A list of names from a field, joined with commas; `empty` when the list is empty.
fn names(v: &Value, key: &str, empty: &str) -> String {
    let items: Vec<&str> = v
        .get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if items.is_empty() {
        empty.to_string()
    } else {
        items.join(",")
    }
}

/// One rule as a table row, under the header `id roles host port methods path header format secret enabled`.
pub fn rule_row(r: &Value) -> Vec<String> {
    vec![
        ctx::text(r, "id"),
        names(r, "roles", "every role"),
        ctx::text(r, "host"),
        ctx::text(r, "port"),
        names(r, "methods", "any"),
        ctx::text(r, "path_prefix"),
        ctx::text(r, "header"),
        ctx::text(r, "format"),
        ctx::text(r, "secret"),
        if r.get("enabled").and_then(Value::as_bool) == Some(true) {
            "yes".to_string()
        } else {
            "no".to_string()
        },
    ]
}

fn list(ctx: &Ctx) -> Result<(), String> {
    let v = ctx.client.get(RULES)?;
    ctx.out(&v, |v| {
        let rows: Vec<Vec<String>> = v["rules"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(rule_row)
            .collect();
        ctx::table(
            &[
                "id", "roles", "host", "port", "methods", "path", "header", "format", "secret",
                "enabled",
            ],
            &rows,
        );
    });
    Ok(())
}

fn add(ctx: &Ctx, rule: &NewRule) -> Result<(), String> {
    let v = ctx.client.post(RULES, &rule_body(rule))?;
    ctx.out(&v, |r| {
        println!(
            "added egress rule {} for {}:{} (secret {})",
            ctx::text(r, "id"),
            ctx::text(r, "host"),
            ctx::text(r, "port"),
            ctx::text(r, "secret")
        )
    });
    Ok(())
}

fn set_enabled(ctx: &Ctx, id: &str, enabled: bool) -> Result<(), String> {
    let v = ctx.client.patch(&rule_path(id), &enabled_body(enabled))?;
    ctx.out(&v, |_| {
        println!(
            "{} egress rule {id}",
            if enabled { "enabled" } else { "disabled" }
        )
    });
    Ok(())
}

fn remove(ctx: &Ctx, id: &str, yes: bool) -> Result<(), String> {
    if !confirm(
        yes,
        &format!("Remove egress rule {id}? Requests it covered go out without its credential."),
    )? {
        println!("cancelled");
        return Ok(());
    }
    let v = ctx.client.delete(&rule_path(id))?;
    ctx.out(&v, |_| println!("removed egress rule {id}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// The `egress` command alone, so its subcommands parse the way the CLI parses them.
    #[derive(Parser, Debug)]
    struct Probe {
        #[command(subcommand)]
        cmd: Cmd,
    }

    /// `egress ARGS`, parsed.
    fn parse(args: &[&str]) -> Result<Cmd, clap::Error> {
        let mut argv = vec!["egress"];
        argv.extend_from_slice(args);
        Probe::try_parse_from(argv).map(|p| p.cmd)
    }

    #[test]
    fn add_takes_the_host_and_the_secret_and_defaults_the_rest() {
        match parse(&["add", "--host", "api.example.com", "--secret", "GH_TOKEN"]).unwrap() {
            Cmd::Add {
                host,
                port,
                methods,
                path_prefix,
                roles,
                header,
                format,
                secret,
            } => {
                assert_eq!(host, "api.example.com");
                assert_eq!(port, 443);
                assert!(methods.is_empty() && roles.is_empty());
                assert_eq!(path_prefix, "/");
                assert_eq!(header, "Authorization");
                assert_eq!(format, "Bearer ${SECRET}");
                assert_eq!(secret, "GH_TOKEN");
            }
            other => panic!("not an add: {other:?}"),
        }
    }

    #[test]
    fn add_repeats_methods_and_roles() {
        match parse(&[
            "add",
            "--host",
            "*.example.com",
            "--port",
            "8443",
            "--method",
            "GET",
            "--method",
            "post",
            "--role",
            "builder",
            "--role",
            "ops",
            "--path-prefix",
            "/v1",
            "--header",
            "X-Key",
            "--format",
            "${SECRET}",
            "--secret",
            "S",
        ])
        .unwrap()
        {
            Cmd::Add {
                port,
                methods,
                roles,
                path_prefix,
                header,
                format,
                ..
            } => {
                assert_eq!(port, 8443);
                assert_eq!(methods, ["GET", "post"]);
                assert_eq!(roles, ["builder", "ops"]);
                assert_eq!(
                    (path_prefix.as_str(), header.as_str(), format.as_str()),
                    ("/v1", "X-Key", "${SECRET}")
                );
            }
            other => panic!("not an add: {other:?}"),
        }
    }

    #[test]
    fn add_needs_a_secret_and_a_host() {
        assert!(parse(&["add", "--host", "api.example.com"]).is_err());
        assert!(parse(&["add", "--secret", "S"]).is_err());
    }

    #[test]
    fn enable_disable_and_rm_take_the_rule_id() {
        match parse(&["enable", "egr_1"]).unwrap() {
            Cmd::Enable { id } => assert_eq!(id, "egr_1"),
            other => panic!("not an enable: {other:?}"),
        }
        match parse(&["disable", "egr_1"]).unwrap() {
            Cmd::Disable { id } => assert_eq!(id, "egr_1"),
            other => panic!("not a disable: {other:?}"),
        }
        match parse(&["rm", "egr_1"]).unwrap() {
            Cmd::Rm { id, yes } => assert_eq!((id.as_str(), yes), ("egr_1", false)),
            other => panic!("not an rm: {other:?}"),
        }
        match parse(&["rm", "egr_1", "--yes"]).unwrap() {
            Cmd::Rm { yes, .. } => assert!(yes),
            other => panic!("not an rm: {other:?}"),
        }
    }

    #[test]
    fn a_rule_body_carries_what_the_owner_gave() {
        let rule = NewRule {
            host: "api.example.com".into(),
            port: 443,
            methods: vec!["GET".into()],
            path_prefix: "/v1".into(),
            roles: vec!["builder".into()],
            header: "Authorization".into(),
            format: "Bearer ${SECRET}".into(),
            secret: "GH_TOKEN".into(),
        };
        assert_eq!(
            rule_body(&rule),
            json!({
                "roles": ["builder"],
                "host": "api.example.com",
                "port": 443,
                "methods": ["GET"],
                "path_prefix": "/v1",
                "header": "Authorization",
                "format": "Bearer ${SECRET}",
                "secret": "GH_TOKEN"
            })
        );
    }

    #[test]
    fn the_toggle_body_and_the_rule_path_encode_the_id() {
        assert_eq!(enabled_body(true), json!({ "enabled": true }));
        assert_eq!(enabled_body(false), json!({ "enabled": false }));
        assert_eq!(rule_path("egr_1"), "/v1/egress/inject/egr_1");
        assert_eq!(rule_path("a/b c"), "/v1/egress/inject/a%2Fb%20c");
    }

    #[test]
    fn a_rule_row_reads_empty_roles_and_methods_as_every_and_any() {
        let open = json!({
            "id": "egr_1", "roles": [], "host": "api.example.com", "port": 443,
            "methods": [], "path_prefix": "/", "header": "Authorization",
            "format": "Bearer ${SECRET}", "secret": "GH_TOKEN", "enabled": true
        });
        assert_eq!(
            rule_row(&open),
            vec![
                "egr_1",
                "every role",
                "api.example.com",
                "443",
                "any",
                "/",
                "Authorization",
                "Bearer ${SECRET}",
                "GH_TOKEN",
                "yes"
            ]
        );
        let narrow = json!({
            "id": "egr_2", "roles": ["builder", "ops"], "host": "h", "port": 8443,
            "methods": ["GET", "POST"], "path_prefix": "/v1", "header": "X-Key",
            "format": "${SECRET}", "secret": "S", "enabled": false
        });
        let row = rule_row(&narrow);
        assert_eq!(row[1], "builder,ops");
        assert_eq!(row[4], "GET,POST");
        assert_eq!(row[9], "no");
    }
}
