//! The codex's tool grants, as the dispatcher's own plan reads them.
//!
//! `docs/agents/initial-roles.json` carries, for every role, the tools the
//! codex grants that role (`roles v1.1`, the statutes S-009…S-016), and
//! `dispatchd --roles <manifest>` reads it so the dispatcher does not
//! *knowingly* attempt a call the hub will refuse: before a task of a given
//! role is claimed, [`crate::workflow::plan_for`] checks the tools that role's
//! script drives against these grants ([`Manifest::grants`]) and picks a script
//! the role can actually run.
//!
//! The hub is the enforcement point — `vsockd --roles` answers a call a role
//! does not grant with `403` and never forwards it to the agent (see
//! `crates/vsockd/src/roles.rs`, and `Tell::Refused` in [`crate::views`] for
//! what the dispatcher does if it ever sees one). This module is the
//! dispatcher's *cooperation* with that policy: same manifest, same mapping,
//! read here so a task is left open rather than driven into a refusal.
//!
//! # The mapping
//!
//! The manifest's names are the codex's; the harness
//! (`crates/agent-harness/src/tools.rs`) serves a subset, and [`TOOL_GRANTS`]
//! is the translation — `read_file`/`write_file`/`edit_file`/`list`/`search` by
//! their own names, `append_file` by the `write_file` grant, and every file
//! tool by a `bash` grant, because a shell can do what those tools do. `echo`
//! and `uptime` are diagnostics: they touch nothing and need no grant. A tool
//! this table does not know — one the harness does not serve, or a codex grant
//! it cannot honour — maps to no granted name.
//!
//! `dispatchd` cannot depend on `vsockd` (neither is a library), so this is a
//! deliberate sibling of `crates/vsockd/src/roles.rs` and the two stay in step
//! by hand, exactly as `crates/noded/src/http.rs` and `crates/vsockd/src/
//! status.rs` do for the loopback HTTP the plaza's daemons speak.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The harness tools the dispatcher may drive, and the manifest tool names any
/// one of which grants each one.
///
/// A diagnostic (`echo`, `uptime`) carries no source: it needs no grant.
pub const TOOL_GRANTS: &[(&str, &[&str])] = &[
    ("echo", &[]),
    ("uptime", &[]),
    ("read_file", &["read_file", "bash"]),
    ("write_file", &["write_file", "bash"]),
    ("edit_file", &["edit_file", "bash"]),
    ("append_file", &["write_file", "bash"]),
    ("list", &["list", "bash"]),
    ("search", &["search", "bash"]),
];

/// The manifest tool names any one of which grants harness tool `tool`:
/// `Some(&[])` for a diagnostic, `None` for a tool the harness does not serve.
pub fn grant_sources(tool: &str) -> Option<&'static [&'static str]> {
    TOOL_GRANTS
        .iter()
        .find(|(name, _)| *name == tool)
        .map(|(_, sources)| *sources)
}

/// What the manifest makes of one agent's tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// The role is in the manifest and the tool is granted (or the tool is a
    /// diagnostic).
    Allow,
    /// The agent declared no role.
    NoRole,
    /// The role the agent declared is not in the manifest.
    UnknownRole,
    /// The role is in the manifest and grants neither the tool nor its source.
    Ungranted,
}

/// The roles manifest's shape: `{"source": "...", "roles": {"<role>": {...}}}`.
///
/// Read exactly as `vsockd --roles` and `noded --agents` read it: unknown
/// fields are ignored, and a role that does not name itself or declares no
/// tools is a manifest to refuse rather than one to guess at.
#[derive(Debug, Deserialize)]
struct Doc {
    roles: BTreeMap<String, Entry>,
}

/// One role, as far as the grants are concerned.
#[derive(Debug, Deserialize)]
struct Entry {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    tools: Option<Vec<String>>,
}

/// Why a manifest could not be turned into a policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    /// The manifest could not be read: {path}: {reason}
    Read {
        /// The path that was tried.
        path: PathBuf,
        /// Why reading it failed.
        reason: String,
    },
    /// The manifest is not the roles document: {reason}
    Json {
        /// What the parser said.
        reason: String,
    },
    /// The manifest lists no roles
    Empty,
    /// A role does not name itself the way it is filed: {role}: {reason}
    BadRole {
        /// The key the role is filed under.
        role: String,
        /// What is wrong with the entry's own name.
        reason: String,
    },
    /// A role's tools are not what the policy is made of: {role}: {reason}
    BadTools {
        /// The role the tools were declared for.
        role: String,
        /// What is wrong with the declaration.
        reason: String,
    },
}

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, reason } => {
                write!(formatter, "cannot read {}: {reason}", path.display())
            }
            Self::Json { reason } => {
                write!(formatter, "the roles manifest is not valid: {reason}")
            }
            Self::Empty => write!(formatter, "the roles manifest lists no roles"),
            Self::BadRole { role, reason } | Self::BadTools { role, reason } => {
                write!(formatter, "role {role:?}: {reason}")
            }
        }
    }
}

impl std::error::Error for ManifestError {}

/// The grants of every role a dispatcher may drive.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// Each role's declared tools, in the manifest's key order.
    roles: BTreeMap<String, Vec<String>>,
}

impl Manifest {
    /// Loads and validates the roles manifest at `path`.
    pub fn load(path: &Path) -> Result<Self, ManifestError> {
        let text = std::fs::read_to_string(path).map_err(|err| ManifestError::Read {
            path: path.to_path_buf(),
            reason: err.to_string(),
        })?;
        Self::parse(&text)
    }

    /// Builds the policy from a manifest document.
    pub fn parse(text: &str) -> Result<Self, ManifestError> {
        let doc: Doc = serde_json::from_str(text).map_err(|err| ManifestError::Json {
            reason: err.to_string(),
        })?;
        let mut roles = BTreeMap::new();
        for (key, entry) in doc.roles {
            match entry.role.as_deref() {
                None => {
                    return Err(ManifestError::BadRole {
                        role: key,
                        reason: "the entry does not name a role".to_string(),
                    });
                }
                Some(name) if name != key => {
                    return Err(ManifestError::BadRole {
                        role: key.clone(),
                        reason: format!("the entry calls itself {name:?}"),
                    });
                }
                Some(_) => {}
            }
            let tools = match entry.tools {
                None => {
                    return Err(ManifestError::BadTools {
                        role: key,
                        reason: "the entry declares no tools".to_string(),
                    });
                }
                Some(tools) if tools.is_empty() => {
                    return Err(ManifestError::BadTools {
                        role: key,
                        reason: "the entry's tools array is empty".to_string(),
                    });
                }
                Some(tools) => tools,
            };
            roles.insert(key, tools);
        }
        if roles.is_empty() {
            return Err(ManifestError::Empty);
        }
        let manifest = Self { roles };
        debug_assert!(!manifest.is_empty(), "just checked");
        Ok(manifest)
    }

    /// How many roles the manifest carries.
    pub fn len(&self) -> usize {
        self.roles.len()
    }

    /// Whether the manifest carries no roles. A manifest that parses is never
    /// empty ([`Manifest::parse`] refuses one that is), but a policy that
    /// admits nothing is worth being able to ask about rather than assume.
    pub fn is_empty(&self) -> bool {
        self.roles.is_empty()
    }

    /// Whether the manifest carries `role`. The empty role is never known: an
    /// agent whose handshake accepted no role is not one the policy can name.
    pub fn knows(&self, role: &str) -> bool {
        self.roles.contains_key(role)
    }

    /// Whether the manifest grants `role` harness tool `tool`.
    ///
    /// A role the manifest does not carry grants nothing: an unknown agent is
    /// never driven on this dispatcher's account (see [`crate::workflow::
    /// plan_for`]).
    pub fn grants(&self, role: &str, tool: &str) -> bool {
        self.decide(role, tool) == Decision::Allow
    }

    /// What the manifest makes of agent `role` calling harness tool `tool`.
    pub fn decide(&self, role: &str, tool: &str) -> Decision {
        if role.is_empty() {
            return Decision::NoRole;
        }
        let Some(declared) = self.roles.get(role) else {
            return Decision::UnknownRole;
        };
        let Some(sources) = grant_sources(tool) else {
            return Decision::Ungranted;
        };
        if sources.is_empty()
            || sources
                .iter()
                .any(|source| declared.iter().any(|name| name == source))
        {
            Decision::Allow
        } else {
            Decision::Ungranted
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A policy built from a role → tools table.
    fn manifest(roles: &[(&str, &[&str])]) -> Manifest {
        Manifest {
            roles: roles
                .iter()
                .map(|(role, tools)| {
                    (
                        (*role).to_string(),
                        tools.iter().map(|tool| (*tool).to_string()).collect(),
                    )
                })
                .collect(),
        }
    }

    /// The codex's own declarations, as
    /// `docs/agents/initial-roles.json` carries them.
    const BUILDER: &[&str] = &[
        "bash",
        "read_file",
        "write_file",
        "edit_file",
        "list",
        "search",
    ];
    const LIBRARIAN: &[&str] = &["read_memory", "search", "transcript_search"];

    /// The table is the harness's tool table, which is what makes `grants` a
    /// real ceiling rather than a list of codex names the dispatcher never
    /// drives. (`crates/agent-harness/src/tools.rs` is what this mirrors.)
    #[test]
    fn the_grant_table_is_the_harnesses_tools() {
        let harness = [
            "echo",
            "uptime",
            "read_file",
            "write_file",
            "edit_file",
            "append_file",
            "list",
            "search",
        ];
        let table: Vec<&str> = TOOL_GRANTS.iter().map(|(name, _)| *name).collect();
        assert_eq!(table, harness);
        assert_eq!(
            grant_sources("bash"),
            None,
            "a shell is a source, not a tool"
        );
    }

    /// The grants the dispatcher's plans are checked against: a builder may
    /// write, a librarian may only search — and neither may drive a codex tool
    /// the harness does not serve.
    #[test]
    fn the_grants_are_read_per_role_and_per_tool() {
        let roles = manifest(&[("builder", BUILDER), ("librarian", LIBRARIAN)]);
        assert!(roles.knows("builder") && roles.knows("librarian"));
        assert!(!roles.knows("Builder") && !roles.knows("") && !roles.knows("architect"));

        for tool in [
            "read_file",
            "write_file",
            "edit_file",
            "append_file",
            "list",
            "search",
        ] {
            assert!(roles.grants("builder", tool), "builder/{tool}");
        }
        assert!(roles.grants("librarian", "search"));
        for tool in [
            "read_file",
            "write_file",
            "edit_file",
            "append_file",
            "list",
        ] {
            assert!(!roles.grants("librarian", tool), "librarian/{tool}");
        }
        for tool in [
            "read_memory",
            "transcript_search",
            "submit_memory_change",
            "bash",
        ] {
            assert!(!roles.grants("librarian", tool), "librarian/{tool}");
        }
        // A diagnostic needs no grant, and an unknown role has none.
        assert!(roles.grants("librarian", "uptime"));
        assert!(!roles.grants("intruder", "read_file"));
        assert!(!roles.grants("", "read_file"));
    }

    /// A manifest the dispatcher cannot read is an error, never a policy with
    /// roles missing from it.
    #[test]
    fn a_manifest_outside_the_contract_is_refused() {
        for text in [
            "not json",
            "{}",
            r#"{"roles": {}}"#,
            r#"{"roles": {"builder": {"role": "builder"}}}"#,
            r#"{"roles": {"builder": {"role": "builder", "tools": []}}}"#,
            r#"{"roles": {"builder": {"tools": ["bash"]}}}"#,
            r#"{"roles": {"builder": {"role": "builer", "tools": ["bash"]}}}"#,
        ] {
            let err = Manifest::parse(text).expect_err("outside the contract");
            assert!(!err.to_string().is_empty(), "{text}");
        }
        // The messages name what to fix.
        assert!(
            Manifest::parse(r#"{"roles": {"builder": {"role": "builder"}}}"#)
                .expect_err("no tools")
                .to_string()
                .contains("declares no tools")
        );
        assert!(
            Manifest::parse(r#"{"roles": {}}"#)
                .expect_err("no roles")
                .to_string()
                .contains("lists no roles")
        );
        assert!(
            Manifest::load(Path::new("/no/such/dispatchd-roles.json"))
                .expect_err("not there")
                .to_string()
                .contains("/no/such/dispatchd-roles.json")
        );
    }

    /// A manifest with every role read in full answers for each of them.
    #[test]
    fn a_well_formed_manifest_reads() {
        let text = r#"{
            "source": "docs/codex-draft/roles/build/roles_src.py",
            "roles": {
                "builder": {
                    "role": "builder",
                    "title": "Builder",
                    "permits": ["S-001"],
                    "toolsets": ["repo"],
                    "budget": 2000,
                    "variant": null,
                    "tools": ["bash", "read_file", "write_file"]
                },
                "librarian": { "role": "librarian", "title": "Librarian", "tools": ["search"] }
            }
        }"#;
        let roles = Manifest::parse(text).expect("the manifest reads");
        assert_eq!(roles.len(), 2);
        assert_eq!(roles.decide("builder", "write_file"), Decision::Allow);
        assert_eq!(roles.decide("librarian", "write_file"), Decision::Ungranted);
        assert_eq!(roles.decide("librarian", "search"), Decision::Allow);
    }
}
