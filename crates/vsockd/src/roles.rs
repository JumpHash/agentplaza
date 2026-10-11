//! The codex's tool grants, as the hub enforces them.
//!
//! `docs/agents/initial-roles.json` carries, for every role, the tools the
//! codex grants that role (`roles v1.1`, the statutes S-009…S-016).
//! `vsockd --roles <manifest>` reads that file and makes it the ceiling on what
//! the hub may **drive** for an agent: before the admin surface (`POST /tell`)
//! forwards a `tool_call` into a connected agent's session, the tool is checked
//! against the role the agent declared at its handshake, and a call the role
//! does not grant is refused with **403**
//! `{"ok":false,"output":"role <r> does not grant <tool>"}` — decided before
//! anything is dispatched, so the guest never sees the call.
//!
//! The mapping: the codex's vocabulary onto the harness's
//!
//! The manifest's tool names are the codex's; the harness
//! (`crates/agent-harness/src/tools.rs`) serves a subset of them, and
//! [`TOOL_GRANTS`] is the whole of the translation:
//!
//! | harness tool | granted by |
//! |---|---|
//! | `read_file` | `read_file`, or `bash` |
//! | `write_file` | `write_file`, or `bash` |
//! | `edit_file` | `edit_file`, or `bash` |
//! | `append_file` | `write_file`, or `bash` — the same authority, another door |
//! | `list` | `list`, or `bash` |
//! | `search` | `search`, or `bash` |
//! | `echo`, `uptime`, `conversation` | nothing — they are basis tools: `echo` and `uptime` touch nothing, and `conversation` touches only the agent's own `/workspace/messages.log`. A channel every agent must have to be an agent is the plaza's basis, not a role's privilege, so none of the three is gated |
//!
//! `bash` is in every file tool's set because a shell can do what those tools
//! do: a role the codex grants a shell is not then refused a file call it could
//! make through it. (`bash` is a *grant source*, not a driveable tool — the
//! harness serves no shell, so a `/tell` asking for `bash` maps to nothing and
//! is refused.)
//!
//! Any other harness tool — a name this table does not know, or one the harness
//! does not implement — maps to no granted name and is refused: a grant the
//! harness cannot honour (the codex's `read_memory`, `assign_task`,
//! `submit_claim`) simply never matches a driven call, so it neither allows nor
//! breaks anything.
//!
//! The honest consequence, stated plainly: the **librarian**'s declared tools
//! are `read_memory`, `search`, `transcript_search`, `read_artifact` and
//! `submit_memory_change`, of which the harness implements **one** — so this
//! policy grants a librarian `search` and nothing else. It may not `list`, may
//! not read a file and may not write one. The dispatcher's librarian workflow
//! is read-only for exactly that reason (`crates/dispatchd/src/workflow.rs`).
//! What the librarian — and every other role, including a role whose declared
//! tools the harness implements none of — *may* be driven with is the basis:
//! `echo`, `uptime` and `conversation`. A role that can be spoken to is the
//! plaza's basis, so the social channel is not a privilege a role could be
//! denied; the alternative would be a fleet whose agents cannot talk to each
//! other unless the codex happened to list a `conversation` tool, which no role
//! declares.
//!
//! # The manifest
//!
//! The same document `noded --agents` reads, validated the same way: a role is
//! filed under its own name, and must declare a non-empty `tools` array. A
//! manifest the daemon cannot read is a startup error (`vsockd: --roles: …`),
//! never a fleet served under a policy nobody can name. `vsockd` cannot depend
//! on `noded`, so this loader is `noded::fleet`'s sibling and the two stay in
//! step by hand.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The harness tools the hub may drive, and the manifest tool names any one of
/// which grants each one.
///
/// The names here are exactly the harness's tool table (`crates/agent-harness/
/// src/tools.rs`), pinned by a test: a tool the harness does not serve has no
/// row, so a call for it maps to no grant and is refused.
pub const TOOL_GRANTS: &[(&str, &[&str])] = &[
    ("echo", &[]),
    ("uptime", &[]),
    ("conversation", &[]),
    ("read_file", &["read_file", "bash"]),
    ("write_file", &["write_file", "bash"]),
    ("edit_file", &["edit_file", "bash"]),
    ("append_file", &["write_file", "bash"]),
    ("list", &["list", "bash"]),
    ("search", &["search", "bash"]),
];

/// The manifest tool names any one of which grants harness tool `tool`.
///
/// `Some(&[])` for a basis tool — `echo`, `uptime`, `conversation`: it needs no
/// grant, so no manifest tool name is its source — and `None` for a tool the
/// harness does not serve: nothing grants it, so under a loaded manifest a call
/// for it is refused.
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
    /// basis tool): the call may be driven.
    Allow,
    /// The agent declared no role at its handshake, and a loaded manifest
    /// admits only roled agents.
    NoRole,
    /// The role the agent declared is not in the manifest.
    UnknownRole,
    /// The role is in the manifest and grants neither the tool nor its source.
    Ungranted,
}

/// The roles manifest's shape: `{"source": "...", "roles": {"<role>": {...}}}`.
///
/// Unknown fields are ignored — the manifest carries provenance (a title, the
/// statutes, a budget) the hub has no use for — but the two fields the policy
/// is made of are read in full: a role that does not name itself, or declares
/// no tools, is a manifest to refuse rather than one to guess at.
#[derive(Debug, Deserialize)]
struct Doc {
    roles: BTreeMap<String, Entry>,
}

/// One role, as far as the grants are concerned.
#[derive(Debug, Deserialize)]
struct Entry {
    /// The role's name; it must agree with the key it is filed under.
    #[serde(default)]
    role: Option<String>,
    /// The tools the codex grants the role. `None` is a role that declares
    /// none at all, which the policy refuses (see [`Manifest::parse`]).
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
            Self::BadRole { role, reason } => write!(formatter, "role {role:?}: {reason}"),
            Self::BadTools { role, reason } => {
                write!(formatter, "role {role:?}: {reason}")
            }
        }
    }
}

impl std::error::Error for ManifestError {}

/// The grants of every role a daemon may drive, and the table that reads a
/// harness tool against them.
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
        // `BTreeMap` iterates in key order, so a manifest's roles come back in
        // the same order every run and every error names the first offender
        // alphabetically rather than whichever serde happened to see.
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
                    // A role that grants nothing could never be driven at all:
                    // a manifest that says so is a mistake to report, not a
                    // policy to serve.
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

    /// What the manifest makes of agent `role` calling harness tool `tool`.
    ///
    /// The empty role is [`Decision::NoRole`]: a guest that declared no role is
    /// an agent this manifest cannot name, and an unnamed agent is not one a
    /// policy can grant anything to. A role the manifest does not carry is
    /// [`Decision::UnknownRole`] — a connected agent the fleet was not meant to
    /// have, which is reported rather than silently granted.
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
        // A diagnostic carries no source: it needs no grant.
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

    /// The basis tools, read off the table itself: the rows that carry no
    /// grant. Deriving them (rather than duplicating a list) is what keeps
    /// [`TOOL_GRANTS`] the single source of truth for the mapping.
    ///
    /// They are the **basis**, not "diagnostics": `echo` and `uptime` touch
    /// nothing, and `conversation` touches only the agent's own workspace log.
    /// A channel every agent needs in order to be an agent at all is the
    /// plaza's basis — it is not a privilege a role can be denied, which is
    /// exactly why the table gives it no grant source.
    fn basis_tools() -> Vec<&'static str> {
        TOOL_GRANTS
            .iter()
            .filter(|(_, sources)| sources.is_empty())
            .map(|(name, _)| *name)
            .collect()
    }

    /// A policy built from a role → tools table, without going through JSON.
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

    /// The tools the codex's own roles declare, as `docs/agents/initial-roles.json`
    /// carries them (roles v1.1).
    const BUILDER: &[&str] = &[
        "bash",
        "read_file",
        "write_file",
        "edit_file",
        "list",
        "search",
        "fetch_url",
        "transcript_search",
        "submit_claim",
    ];
    const LIBRARIAN: &[&str] = &[
        "read_memory",
        "search",
        "transcript_search",
        "read_artifact",
        "submit_memory_change",
    ];
    const MAYOR: &[&str] = &[
        "read_file",
        "list",
        "search",
        "transcript_search",
        "assign_task",
        "spawn_agent",
        "hibernate_agent",
        "approve_plan",
        "approve_merge",
        "escalate",
        "send_message",
    ];
    /// A role whose declared tools the harness implements none of.
    const JUDGE: &[&str] = &["read_case_record", "search_precedent", "issue_ruling"];

    fn decide(roles: &[(&str, &[&str])], role: &str, tool: &str) -> Decision {
        manifest(roles).decide(role, tool)
    }

    /// A well-formed document reads, in key order, and answers for the roles it
    /// carries.
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
                    "tools": ["bash", "read_file", "write_file", "edit_file", "list", "search"]
                },
                "librarian": {
                    "role": "librarian",
                    "title": "Librarian",
                    "tools": ["read_memory", "search", "transcript_search"]
                }
            }
        }"#;
        let manifest = Manifest::parse(text).expect("the manifest reads");
        assert_eq!(manifest.len(), 2);
        assert_eq!(manifest.decide("builder", "write_file"), Decision::Allow);
        assert_eq!(
            manifest.decide("librarian", "write_file"),
            Decision::Ungranted
        );
        assert_eq!(manifest.decide("librarian", "search"), Decision::Allow);
    }

    /// A document that is not the roles shape — and one that lists no roles —
    /// is refused, never read as an empty policy.
    #[test]
    fn a_document_that_is_not_the_roles_manifest_is_refused() {
        for text in [
            "[]",
            "{}",
            r#"{"roles": 3}"#,
            r#"{"roles": {"builder": 3}}"#,
            r#"{"roles": {"builder": {"role": "builder", "tools": "bash"}}}"#,
            "not json at all",
        ] {
            let err = Manifest::parse(text).expect_err("outside the contract");
            assert!(matches!(err, ManifestError::Json { .. }), "{text}: {err:?}");
            assert!(err.to_string().contains("not valid"), "{text}: {err}");
        }
        assert_eq!(
            Manifest::parse(r#"{"roles": {}}"#).expect_err("no roles"),
            ManifestError::Empty
        );
    }

    /// `--roles` is a policy or it is nothing: a role that declares no tools,
    /// or an empty array, refuses the whole manifest rather than granting the
    /// role nothing at all.
    #[test]
    fn a_role_whose_tools_are_missing_or_empty_is_refused() {
        for (text, reason) in [
            (
                r#"{"roles": {"builder": {"role": "builder"}}}"#,
                "the entry declares no tools",
            ),
            (
                r#"{"roles": {"builder": {"role": "builder", "tools": []}}}"#,
                "the entry's tools array is empty",
            ),
        ] {
            let err = Manifest::parse(text).expect_err("outside the contract");
            assert_eq!(
                err,
                ManifestError::BadTools {
                    role: "builder".to_string(),
                    reason: reason.to_string(),
                }
            );
            assert!(err.to_string().contains("role \"builder\""), "{err}");
        }
    }

    /// A role that does not call itself by the key it is filed under makes the
    /// policy ambiguous: the manifest is refused rather than one of the two
    /// names being believed.
    #[test]
    fn a_role_that_disagrees_with_its_own_key_is_refused() {
        let err =
            Manifest::parse(r#"{"roles": {"builder": {"role": "builer", "tools": ["bash"]}}}"#)
                .expect_err("the names disagree");
        assert_eq!(
            err,
            ManifestError::BadRole {
                role: "builder".to_string(),
                reason: "the entry calls itself \"builer\"".to_string(),
            }
        );

        let err = Manifest::parse(r#"{"roles": {"builder": {"tools": ["bash"]}}}"#)
            .expect_err("no name at all");
        assert_eq!(
            err,
            ManifestError::BadRole {
                role: "builder".to_string(),
                reason: "the entry does not name a role".to_string(),
            }
        );
    }

    /// A manifest that is not on disk says which path it tried.
    #[test]
    fn a_manifest_that_is_not_there_names_the_path() {
        let err = Manifest::load(Path::new("/no/such/plaza/roles.json"))
            .expect_err("nothing is there")
            .to_string();
        assert!(err.contains("/no/such/plaza/roles.json"), "{err}");
        assert!(err.starts_with("cannot read"), "{err}");
    }

    /// The table is the harness's tool table, tool for tool: a tool the harness
    /// serves has a row, and a tool it does not serve has none (so it maps to no
    /// grant and is refused). `crates/agent-harness/src/tools.rs` is the list
    /// this mirrors — the two are kept in step by hand, and this is the test
    /// that says so.
    #[test]
    fn the_grant_table_is_exactly_the_harnesses_tools() {
        let harness = [
            "echo",
            "uptime",
            "conversation",
            "read_file",
            "write_file",
            "edit_file",
            "append_file",
            "list",
            "search",
        ];
        let table: Vec<&str> = TOOL_GRANTS.iter().map(|(name, _)| *name).collect();
        assert_eq!(table, harness, "the harness's tools, in its own order");

        // The basis tools are the rows that carry no grant, and no other row
        // does: the basis is the only thing granted without a grant. What is
        // ungated is asserted here explicitly — a row added to the table
        // without a source joins this list, and a file tool that lost its
        // sources would have to be spelled out again.
        assert_eq!(basis_tools(), vec!["echo", "uptime", "conversation"]);
        for tool in basis_tools() {
            assert_eq!(grant_sources(tool), Some(&[][..]), "{tool}");
        }
        // A tool the harness does not serve grants nothing.
        for tool in ["bash", "fetch_url", "read_memory", "assign_task", ""] {
            assert_eq!(grant_sources(tool), None, "{tool}");
        }
    }

    /// A builder — a shell and the file tools — may drive every file tool the
    /// harness serves, through either the tool's own grant or the shell's.
    #[test]
    fn a_builder_may_drive_the_file_tools_it_grants() {
        for tool in [
            "read_file",
            "write_file",
            "edit_file",
            "append_file",
            "list",
            "search",
        ] {
            assert_eq!(
                decide(&[("builder", BUILDER)], "builder", tool),
                Decision::Allow,
                "{tool}"
            );
        }
        // The basis tools too: they need no grant, so no role can be without
        // them.
        for tool in basis_tools() {
            assert_eq!(
                decide(&[("builder", BUILDER)], "builder", tool),
                Decision::Allow,
                "{tool}"
            );
        }
    }

    /// The consequence the policy has to own: the librarian's declared tools
    /// give it `search` among the harness's file tools, and nothing else — not
    /// `list`, not a read, not a write. What it does hold is the basis, the
    /// conversation channel among it: a librarian that could not be spoken to
    /// would not be an agent on the plaza.
    #[test]
    fn the_librarian_grants_only_search_of_the_file_tools() {
        let roles = [("librarian", LIBRARIAN)];
        assert_eq!(decide(&roles, "librarian", "search"), Decision::Allow);
        for tool in [
            "read_file",
            "write_file",
            "edit_file",
            "append_file",
            "list",
        ] {
            assert_eq!(
                decide(&roles, "librarian", tool),
                Decision::Ungranted,
                "{tool}"
            );
        }
        // `conversation` is not a grant: it is the basis, so the role that
        // holds only `search` holds it too.
        assert_eq!(
            decide(&roles, "librarian", "conversation"),
            Decision::Allow,
            "the social channel is the plaza's basis, not a role's privilege"
        );
        // The codex tools the harness does not implement map to nothing, so a
        // call for one of them is refused rather than allowed by its name.
        for tool in ["read_memory", "transcript_search", "submit_memory_change"] {
            assert_eq!(
                decide(&roles, "librarian", tool),
                Decision::Ungranted,
                "{tool}"
            );
        }
    }

    /// The whole point of the basis: **every** role the shipped manifest names
    /// may be driven with `conversation`, the librarian included (it holds only
    /// `search` among the harness's file tools). A role that says nothing more
    /// than "I am an agent" must be speakable — the plaza's social channel is
    /// not something the codex's per-role tool lists can switch off.
    #[test]
    fn every_role_in_the_shipped_manifest_may_be_spoken_to() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/agents/initial-roles.json");
        let manifest = Manifest::load(&path).expect("the shipped manifest");
        assert_eq!(manifest.len(), 21, "the fleet's initial roles");
        for role in manifest.roles.keys() {
            assert_eq!(
                manifest.decide(role, "conversation"),
                Decision::Allow,
                "role {role} must be speakable"
            );
        }
        // The librarian is the role the policy is narrowest for: `search` is
        // its only file tool, and it is still speakable.
        assert_eq!(manifest.decide("librarian", "search"), Decision::Allow);
        assert_eq!(
            manifest.decide("librarian", "write_file"),
            Decision::Ungranted
        );
    }

    /// The mayor reads and lists but never writes, so this vault's file tools
    /// split exactly along the write line — the case that says the policy is
    /// per tool, not per role family.
    #[test]
    fn the_mayor_reads_but_does_not_write() {
        let roles = [("mayor", MAYOR)];
        for tool in ["read_file", "list", "search"] {
            assert_eq!(decide(&roles, "mayor", tool), Decision::Allow, "{tool}");
        }
        for tool in ["write_file", "edit_file", "append_file"] {
            assert_eq!(decide(&roles, "mayor", tool), Decision::Ungranted, "{tool}");
        }
        // `send_message` is a codex tool the harness does not serve.
        assert_eq!(decide(&roles, "mayor", "send_message"), Decision::Ungranted);
    }

    /// A role whose declared tools the harness implements none of may still be
    /// sent the basis — and nothing else, however the call is spelled.
    #[test]
    fn a_role_with_no_harness_tools_gets_the_basis_tools_alone() {
        let roles = [("legal-gov-judge", JUDGE)];
        for tool in basis_tools() {
            assert_eq!(
                decide(&roles, "legal-gov-judge", tool),
                Decision::Allow,
                "{tool}"
            );
        }
        for tool in [
            "search_precedent",
            "read_file",
            "search",
            "bash",
            "write_file",
            "",
        ] {
            assert_eq!(
                decide(&roles, "legal-gov-judge", tool),
                Decision::Ungranted,
                "{tool:?}"
            );
        }
    }

    /// The empty role and the unknown role are distinct refusals: one is an
    /// agent the manifest cannot name, the other an agent the fleet was not
    /// meant to have.
    #[test]
    fn an_empty_role_and_an_unknown_role_are_their_own_refusals() {
        let roles = [("builder", BUILDER)];
        assert_eq!(decide(&roles, "", "read_file"), Decision::NoRole);
        assert_eq!(
            decide(&roles, "builderx", "read_file"),
            Decision::UnknownRole
        );
        assert_eq!(
            decide(&roles, "Builder", "read_file"),
            Decision::UnknownRole
        );
        // However it is spelled, an empty role is never allowed anything: the
        // empty role is not a role the manifest could have granted.
        for tool in ["echo", "read_file", "search"] {
            assert_eq!(decide(&roles, "", tool), Decision::NoRole, "{tool}");
        }
    }

    /// `bash` is a grant source, not a driveable tool: a role granted a shell
    /// still cannot have the shell itself dispatched, because the harness serves
    /// no such tool.
    #[test]
    fn a_shell_grant_is_a_source_and_not_a_tool() {
        let roles = [("builder", BUILDER)];
        assert_eq!(decide(&roles, "builder", "bash"), Decision::Ungranted);
        // The implication, tool by tool: `bash` alone grants the file tools.
        let shell_only = [("builder", &["bash"][..])];
        for tool in [
            "read_file",
            "write_file",
            "edit_file",
            "append_file",
            "list",
            "search",
        ] {
            assert_eq!(
                decide(&shell_only, "builder", tool),
                Decision::Allow,
                "{tool}"
            );
        }
        assert_eq!(decide(&shell_only, "builder", "bash"), Decision::Ungranted);
    }

    /// `append_file` is the `write_file` grant: a role that may write a file may
    /// append to one, and a role that may not, may not.
    #[test]
    fn append_is_the_write_grant_and_not_its_own() {
        let write_only = [("builder", &["write_file"][..])];
        assert_eq!(
            decide(&write_only, "builder", "append_file"),
            Decision::Allow
        );
        let append_only = [("builder", &["append_file"][..])];
        assert_eq!(
            decide(&append_only, "builder", "append_file"),
            Decision::Ungranted,
            "there is no `append_file` grant to hold"
        );
        assert_eq!(
            decide(&append_only, "builder", "write_file"),
            Decision::Ungranted
        );
    }

    /// A manifest's error is a sentence an operator can act on.
    #[test]
    fn the_errors_are_sentences() {
        assert_eq!(
            ManifestError::Empty.to_string(),
            "the roles manifest lists no roles"
        );
        assert_eq!(
            ManifestError::Read {
                path: PathBuf::from("/x/roles.json"),
                reason: "No such file or directory (os error 2)".to_string(),
            }
            .to_string(),
            "cannot read /x/roles.json: No such file or directory (os error 2)"
        );
        assert_eq!(
            ManifestError::BadTools {
                role: "builder".to_string(),
                reason: "the entry's tools array is empty".to_string(),
            }
            .to_string(),
            "role \"builder\": the entry's tools array is empty"
        );
    }
}
