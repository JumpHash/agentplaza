//! Connector manifests (PLAN 14.4): what a connector is, as data the owner approves.
//!
//! A manifest is TOML: `id`, `kind`, `version`, `scopes` (the risk classes of its actions), `egress` (the hosts it
//! may reach, exact names), `secrets` (vault names, never values), `events`, `actions`, `webhook`, and the header a
//! glue component's request carries the secret in (`auth_header`, `auth_format` with `${SECRET}`, as
//! `egress_inject.rs` writes a fork's header). The built-in connectors ship with a manifest each under
//! `connectors/manifests/`; shipping one is the owner's approval, and a test holds each to its catalog entry.
//!
//! Stored in plaza.db (table `connector_manifest`): every version proposed, with its state. A version that widens
//! the scopes, egress, secrets or the injected header needs the owner's card (`connectors::propose_manifest`).

use serde::{Deserialize, Serialize};

/// The built-in manifests, in catalog order.
const BUILTIN: [&str; 22] = [
    include_str!("manifests/firecrawl.toml"),
    include_str!("manifests/elevenlabs.toml"),
    include_str!("manifests/unsplash.toml"),
    include_str!("manifests/pexels.toml"),
    include_str!("manifests/pixabay.toml"),
    include_str!("manifests/replicate.toml"),
    include_str!("manifests/telegram.toml"),
    include_str!("manifests/cloudflare.toml"),
    include_str!("manifests/aws.toml"),
    include_str!("manifests/prometheus.toml"),
    include_str!("manifests/sentry.toml"),
    include_str!("manifests/alertmanager.toml"),
    include_str!("manifests/grafana.toml"),
    include_str!("manifests/cloudwatch.toml"),
    include_str!("manifests/uptime.toml"),
    include_str!("manifests/loki.toml"),
    include_str!("manifests/kubernetes.toml"),
    include_str!("manifests/digitalocean.toml"),
    include_str!("manifests/github.toml"),
    include_str!("manifests/namecheap.toml"),
    include_str!("manifests/higgsfield.toml"),
    include_str!("manifests/magnific.toml"),
];

/// The package registries' publish connectors (9.5), one manifest each. They are not catalog connectors: the release's
/// publish step reads them (`releases::registries`), and the owner's approval of a registry is its vault secret.
const REGISTRIES: [&str; 8] = [
    include_str!("manifests/registries/crates-io.toml"),
    include_str!("manifests/registries/npm.toml"),
    include_str!("manifests/registries/pypi.toml"),
    include_str!("manifests/registries/maven-central.toml"),
    include_str!("manifests/registries/nuget.toml"),
    include_str!("manifests/registries/oci-registry.toml"),
    include_str!("manifests/registries/go-proxy.toml"),
    include_str!("manifests/registries/homebrew-tap.toml"),
];

/// The store submissions' connectors (9.6), one manifest each: App Store Connect and Google Play. Like the registries', they
/// are read by the release's store step (`releases::stores`), not the catalog.
const STORES: [&str; 2] = [
    include_str!("manifests/stores/app-store-connect.toml"),
    include_str!("manifests/stores/google-play.toml"),
];

/// The device farms' connectors (PLAN 19.3, 8.5), one manifest each: BrowserStack, Firebase Test Lab and AWS Device Farm.
/// Like the stores', they are read by the user test's farm step (`user_test::farm`), not the catalog. AWS signs each request
/// (SigV4), which one static header cannot carry: its manifest names no header, and the signing is the farm's own step.
const FARMS: [&str; 3] = [
    include_str!("manifests/farms/browserstack.toml"),
    include_str!("manifests/farms/firebase-test-lab.toml"),
    include_str!("manifests/farms/aws-device-farm.toml"),
];

/// The advisory feeds' manifests (PLAN 24.3), one each: OSV's API and RustSec's advisory-db on GitHub. They are read by the
/// CVE poll (`cves::feeds`), not the catalog; GitHub's advisories go through the catalog's `github` connector.
const FEEDS: [&str; 2] = [
    include_str!("manifests/feeds/osv.toml"),
    include_str!("manifests/feeds/rustsec.toml"),
];

/// The kinds a manifest may name: what the connector does for the plaza.
const KINDS: [&str; 4] = ["tool", "intake", "notify", "publish"];
/// The scopes a manifest may name: the risk classes of connector actions.
const SCOPES: [&str; 3] = ["read", "write", "irreversible"];

/// One manifest version. `state` and `checks` are the hub's (a TOML file's values for them are ignored).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, apz_store::Table)]
#[serde(default)]
pub struct Manifest {
    pub id: String,
    pub version: String,
    #[table(list)]
    pub kind: Vec<String>,
    #[table(list)]
    pub scopes: Vec<String>,
    #[table(list)]
    pub egress: Vec<String>,
    #[table(list)]
    pub secrets: Vec<String>,
    #[table(list)]
    pub events: Vec<String>,
    #[table(list)]
    pub actions: Vec<String>,
    pub webhook: bool,
    pub auth_header: String,
    pub auth_format: String,
    /// `approved`, `pending` or `rejected`.
    #[serde(skip_deserializing)]
    pub state: String,
    /// What failed the checks; a pending version with checks cannot be approved.
    #[table(list)]
    #[serde(skip_deserializing)]
    pub checks: Vec<String>,
    #[serde(skip_deserializing)]
    pub created_ms: u64,
}

/// The package registries' manifests, approved as shipped (9.5). Each file parses and passes [`check`] (a test holds that).
pub fn registries() -> Vec<Manifest> {
    REGISTRIES
        .iter()
        .map(|text| {
            let mut m = parse(text).expect("a shipped registry manifest parses");
            m.state = "approved".into();
            m
        })
        .collect()
}

/// The store connectors' manifests, approved as shipped (9.6). Each file parses and passes [`check`] (a test holds that).
pub fn stores() -> Vec<Manifest> {
    STORES
        .iter()
        .map(|text| {
            let mut m = parse(text).expect("a shipped store manifest parses");
            m.state = "approved".into();
            m
        })
        .collect()
}

/// The advisory feeds' manifests, approved as shipped (24.3). Each file parses and passes [`check`] (a test holds that).
pub fn feeds() -> Vec<Manifest> {
    FEEDS
        .iter()
        .map(|text| {
            let mut m = parse(text).expect("a shipped feed manifest parses");
            m.state = "approved".into();
            m
        })
        .collect()
}

/// The device farms' manifests, approved as shipped (8.5). Each file parses and passes [`check`] (a test holds that).
pub fn farms() -> Vec<Manifest> {
    FARMS
        .iter()
        .map(|text| {
            let mut m = parse(text).expect("a shipped farm manifest parses");
            m.state = "approved".into();
            m
        })
        .collect()
}

/// Parses a manifest's TOML. The state is the hub's to set, so it comes out empty.
pub fn parse(text: &str) -> Result<Manifest, String> {
    toml::from_str(text).map_err(|e| format!("not a connector manifest: {e}"))
}

/// The built-in manifests, approved as shipped. Each file parses (a test holds that).
pub fn builtin() -> Vec<Manifest> {
    BUILTIN
        .iter()
        .map(|text| {
            let mut m = parse(text).expect("a shipped connector manifest parses");
            m.state = "approved".into();
            m
        })
        .collect()
}

/// What a manifest must pass before the owner can approve it. Each failure is one sentence.
pub fn check(m: &Manifest) -> Vec<String> {
    let mut out = Vec::new();
    if !shaped_id(&m.id) {
        out.push(format!(
            "the id `{}` must be lowercase letters, digits or hyphens, starting with a letter",
            m.id
        ));
    }
    if !is_version(&m.version) {
        out.push(format!("the version `{}` must be x.y.z", m.version));
    }
    for k in &m.kind {
        if !KINDS.contains(&k.as_str()) {
            out.push(format!("kind `{k}` is not tool, intake or notify"));
        }
    }
    for s in &m.scopes {
        if !SCOPES.contains(&s.as_str()) {
            out.push(format!("scope `{s}` is not read, write or irreversible"));
        }
    }
    for host in &m.egress {
        if let Some(why) = egress_problem(host) {
            out.push(why);
        }
    }
    for name in &m.secrets {
        if !is_env_name(name) {
            out.push(format!(
                "secret `{name}` must be an environment-style name (upper case, digits, _)"
            ));
        }
    }
    for event in &m.events {
        if !is_event(event) {
            out.push(format!(
                "event `{event}` must be a lower-case name such as issue.created"
            ));
        }
    }
    for action in &m.actions {
        if !shaped_name(action) {
            out.push(format!(
                "action `{action}` must be lower-case letters, digits or underscores"
            ));
        }
    }
    if !m.actions.is_empty() && m.egress.is_empty() {
        out.push("a connector with actions needs at least one egress host".into());
    }
    if m.auth_header.is_empty() {
        if !m.auth_format.is_empty() {
            out.push("auth_format is set without auth_header".into());
        }
    } else {
        if !m
            .auth_header
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            out.push(format!(
                "auth_header `{}` must be letters, digits or -",
                m.auth_header
            ));
        }
        if !m.auth_format.contains("${SECRET}") {
            out.push("auth_format must contain ${SECRET}".into());
        }
        if m.secrets.is_empty() {
            out.push("an auth_header needs a secret to put in it".into());
        }
    }
    out
}

/// What `new` grants that `old` does not: a scope, a host, a secret, the webhook, or a changed injected header. One
/// sentence each, for the owner's card.
pub fn additions(old: &Manifest, new: &Manifest) -> Vec<String> {
    let mut out = Vec::new();
    for (kind, new_items, old_items) in [
        ("scope", &new.scopes, &old.scopes),
        ("host", &new.egress, &old.egress),
        ("secret", &new.secrets, &old.secrets),
    ] {
        for item in new_items.iter().filter(|x| !old_items.contains(x)) {
            out.push(format!("{kind} {item}"));
        }
    }
    if new.webhook && !old.webhook {
        out.push("a webhook".to_string());
    }
    if new.auth_header != old.auth_header || new.auth_format != old.auth_format {
        out.push(format!("a changed header {}", new.auth_header));
    }
    out
}

/// Whether `new` widens what `old` lets the connector do. Such a version needs the owner's approval as a card.
pub fn widens(old: &Manifest, new: &Manifest) -> bool {
    !additions(old, new).is_empty()
}

/// The same manifest, whatever its version record: the hub's state, checks and time left out.
pub fn same_content(a: &Manifest, b: &Manifest) -> bool {
    let (mut a, mut b) = (a.clone(), b.clone());
    for m in [&mut a, &mut b] {
        m.state.clear();
        m.checks.clear();
        m.created_ms = 0;
    }
    a == b
}

/// The manifest a connector proposal becomes when the owner approves it: its one host, its key fields, the risk
/// classes of its actions.
pub fn from_spec(spec: &super::Spec) -> Manifest {
    let mut scopes: Vec<String> = Vec::new();
    for a in &spec.actions {
        if !scopes.contains(&a.risk) {
            scopes.push(a.risk.clone());
        }
    }
    let host = super::host_of(spec.base_url.strip_prefix("https://").unwrap_or(""));
    let (auth_header, auth_format) = match spec.auth.kind.as_str() {
        "header" => (
            spec.auth.name.clone().unwrap_or_default(),
            "${SECRET}".to_string(),
        ),
        "bearer" => ("Authorization".to_string(), "Bearer ${SECRET}".to_string()),
        _ => (String::new(), String::new()),
    };
    Manifest {
        id: spec.id.clone(),
        version: "1.0.0".into(),
        kind: vec!["tool".into()],
        scopes,
        egress: if host.is_empty() {
            Vec::new()
        } else {
            vec![host.to_ascii_lowercase()]
        },
        secrets: spec
            .fields
            .iter()
            .filter(|f| f.secret)
            .map(|f| f.env.clone())
            .collect(),
        events: Vec::new(),
        actions: spec.actions.iter().map(|a| a.name.clone()).collect(),
        webhook: false,
        auth_header,
        auth_format,
        state: "approved".into(),
        checks: Vec::new(),
        created_ms: 0,
    }
}

/// The risk class a catalog action carries, as a scope's name.
pub fn scope_of(risk: super::Risk) -> &'static str {
    match risk {
        super::Risk::Read => "read",
        super::Risk::Write => "write",
        super::Risk::Irreversible => "irreversible",
    }
}

fn is_version(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

fn shaped_id(text: &str) -> bool {
    let mut chars = text.chars();
    matches!(chars.next(), Some('a'..='z'))
        && (3..=31).contains(&text.len())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn shaped_name(text: &str) -> bool {
    let mut chars = text.chars();
    matches!(chars.next(), Some('a'..='z'))
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn is_env_name(text: &str) -> bool {
    let mut chars = text.chars();
    matches!(chars.next(), Some('A'..='Z'))
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn is_event(text: &str) -> bool {
    let mut chars = text.chars();
    matches!(chars.next(), Some('a'..='z'))
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
}

/// Why an egress entry is not a public host: an exact name, lower-case, with no port, path or wildcard.
fn egress_problem(host: &str) -> Option<String> {
    let plain = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.');
    if !plain {
        return Some(format!(
            "egress `{host}` must be one exact lower-case host name (no port, path or wildcard)"
        ));
    }
    super::host_problem(host).map(|why| format!("egress `{host}`: {why}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::{Action, CATALOG, Field, host_of};
    use std::collections::BTreeSet;

    fn set<'a>(items: impl IntoIterator<Item = &'a str>) -> BTreeSet<&'a str> {
        items.into_iter().collect()
    }

    fn secret_envs(c: &crate::connectors::Connector) -> BTreeSet<&'static str> {
        c.fields
            .iter()
            .filter(|f: &&Field| f.secret)
            .map(|f| f.env)
            .collect()
    }

    fn action_host(a: &Action) -> Option<&str> {
        a.path.strip_prefix("https://").map(host_of)
    }

    #[test]
    fn every_built_in_connector_has_a_manifest_that_matches_its_catalog_entry() {
        let manifests = builtin();
        assert_eq!(manifests.len(), CATALOG.len());
        for c in CATALOG {
            let m = manifests
                .iter()
                .find(|m| m.id == c.id)
                .unwrap_or_else(|| panic!("{} has no manifest", c.id));
            assert_eq!(m.state, "approved");
            assert!(check(m).is_empty(), "{}: {:?}", c.id, check(m));
            assert_eq!(
                set(m.actions.iter().map(String::as_str)),
                set(c.actions.iter().map(|a| a.name)),
                "{} actions",
                c.id
            );
            assert_eq!(
                set(m.secrets.iter().map(String::as_str)),
                secret_envs(c),
                "{} secrets",
                c.id
            );
            let hosts: BTreeSet<&str> = c.actions.iter().filter_map(action_host).collect();
            assert_eq!(
                set(m.egress.iter().map(String::as_str)),
                hosts,
                "{} egress is the hosts its actions reach",
                c.id
            );
            let risks: BTreeSet<&str> = c.actions.iter().map(|a| scope_of(a.risk)).collect();
            assert_eq!(
                set(m.scopes.iter().map(String::as_str)),
                risks,
                "{} scopes",
                c.id
            );
            let (header, format) = match c.auth {
                crate::connectors::Auth::Bearer => {
                    ("Authorization", "Bearer ${SECRET}".to_string())
                }
                crate::connectors::Auth::Header { name, prefix } => {
                    (name, format!("{prefix}${{SECRET}}"))
                }
                _ => ("", String::new()),
            };
            assert_eq!(m.auth_header, header, "{} auth header", c.id);
            assert_eq!(m.auth_format, format, "{} auth format", c.id);
        }
    }

    #[test]
    fn a_manifest_that_widens_its_grant_is_told_apart_from_one_that_does_not() {
        let base = builtin().into_iter().find(|m| m.id == "firecrawl").unwrap();
        let mut same_host_more_scope = base.clone();
        same_host_more_scope.version = "1.0.1".into();
        same_host_more_scope.scopes.push("write".into());
        assert!(widens(&base, &same_host_more_scope));

        let mut other_host = base.clone();
        other_host.egress.push("uploads.firecrawl.dev".into());
        assert!(widens(&base, &other_host));

        let mut narrower = base.clone();
        narrower.actions.pop();
        assert!(!widens(&base, &narrower), "fewer actions widen nothing");

        let mut bad = base.clone();
        bad.egress = vec![
            "http://api.firecrawl.dev/".into(),
            "*.dev".into(),
            "10.0.0.1".into(),
        ];
        bad.secrets = vec!["lower".into()];
        bad.scopes = vec!["admin".into()];
        let problems = check(&bad);
        assert_eq!(problems.len(), 5, "{problems:?}");
    }

    #[test]
    fn a_manifest_file_is_read_as_toml_and_its_hub_fields_are_not_taken_from_it() {
        let text = r#"
            id = "acme-notes"
            kind = ["tool"]
            version = "0.2.0"
            scopes = ["read"]
            egress = ["api.acme-test.example"]
            secrets = ["ACME_TEST_KEY"]
            actions = ["find_note"]
            state = "approved"
            checks = []
            auth_header = "X-Acme-Key"
            auth_format = "${SECRET}"
        "#;
        let m = parse(text).unwrap();
        assert_eq!(m.id, "acme-notes");
        assert_eq!(m.state, "", "the state is the hub's to set");
        assert!(check(&m).is_empty());
        assert!(parse("id = ").is_err());
    }

    #[test]
    fn every_registry_connector_has_a_publish_manifest_that_passes_the_checks() {
        let expected: [(&str, &str, &[&str]); 8] = [
            ("crates-io", "crates.io", &["CRATES_IO_TOKEN"]),
            ("npm", "registry.npmjs.org", &["NPM_TOKEN"]),
            ("pypi", "upload.pypi.org", &["PYPI_TOKEN"]),
            (
                "maven-central",
                "central.sonatype.com",
                &["MAVEN_CREDENTIALS", "MAVEN_REPOSITORY_URL"],
            ),
            ("nuget", "www.nuget.org", &["NUGET_API_KEY"]),
            ("oci-registry", "ghcr.io", &["OCI_TOKEN"]),
            ("go-proxy", "proxy.golang.org", &[]),
            ("homebrew-tap", "api.github.com", &["HOMEBREW_TAP_TOKEN"]),
        ];
        let manifests = registries();
        assert_eq!(manifests.len(), expected.len());
        for (id, host, secrets) in expected {
            let m = manifests
                .iter()
                .find(|m| m.id == id)
                .unwrap_or_else(|| panic!("{id} has no manifest"));
            assert_eq!(m.state, "approved");
            assert!(check(m).is_empty(), "{id}: {:?}", check(m));
            assert_eq!(m.kind, ["publish"], "{id}");
            assert_eq!(m.egress, [host], "{id} reaches one host");
            assert!(m.actions.is_empty(), "{id}: publishing is not an action");
            assert_eq!(
                m.secrets,
                secrets.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                "{id}"
            );
            assert!(!widens(m, m), "{id}");
        }
        // A registry's manifest that names a second host is a widening the owner must approve.
        let mut wider = manifests[0].clone();
        wider.egress.push("index.crates.io".into());
        assert!(widens(&manifests[0], &wider));
    }
}
