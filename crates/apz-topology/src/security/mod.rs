//! The security stages of a lab run (PLAN 22.8, 22.10 step 8): the dependency audits and the image scan where their tools
//! are installed, the secrets scan of the release's tree, the DAST scan of the topology's routes, and the fuzzers the
//! topology declares (`[security]`, see [`crate::spec::Security`]).
//!
//! Each stage yields an [`Outcome`]: it ran and found [`Hit`]s (possibly none), or it did not run and says why. A tool that
//! is not on the `PATH` is a skip, never a pass. apid judges the outcomes (its `lab_security`): a hit is a failing step and a
//! `security` finding on the run's change, with the evidence the stage recorded.

pub mod audit;
pub mod dast;
pub mod exec;
pub mod fuzz;
pub mod secrets;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::spec::{Fuzz, Security};

/// One stage's result: `ran` with its hits and the evidence it kept, or not run (`reason` says why).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub stage: String,
    pub ran: bool,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub hits: Vec<Hit>,
    #[serde(default)]
    pub evidence: String,
}

impl Outcome {
    /// A stage that ran, with the hits it found.
    pub fn ran(stage: &str, hits: Vec<Hit>, evidence: String) -> Outcome {
        Outcome {
            stage: stage.into(),
            ran: true,
            reason: String::new(),
            hits,
            evidence,
        }
    }

    /// A stage that did not run, and why.
    pub fn skipped(stage: &str, reason: impl Into<String>) -> Outcome {
        Outcome {
            stage: stage.into(),
            ran: false,
            reason: reason.into(),
            hits: Vec::new(),
            evidence: String::new(),
        }
    }
}

/// One thing a stage found: an advisory, a rule, a secret's place, a route's weakness, a crash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hit {
    /// The advisory, rule, class or shape the hit is of (`RUSTSEC-2024-0001`, `sql-injection`, `secret`).
    pub id: String,
    /// `critical`, `high`, `medium`, `low` or `unknown`, as the stage reads it.
    pub severity: String,
    /// What it is in: a package, a file and line, a route and parameter.
    pub subject: String,
    /// What the stage saw: the advisory's title, the payload's effect, the shape of a secret (never its value).
    pub detail: String,
}

/// What a run of the stages is given: the release's tree (for the audits, the secrets scan and the fuzzers), the base URL
/// of each service the DAST scan and schemathesis reach, and the `PATH` the tools are found on.
pub struct Context<'a> {
    pub tree: Option<&'a Path>,
    pub services: &'a BTreeMap<String, String>,
    pub path_var: &'a str,
}

/// What a stage says when its tool ran past its time limit.
pub const TIMED_OUT: &str = "did not finish within its time limit";

/// The time a stage's tool may run for (a fuzzer's own budget is its `seconds`).
pub const TOOL_LIMIT: Duration = Duration::from_secs(900);

/// Runs every stage the topology declares, in order: the audits, the image scan, the secrets scan, the DAST scan and the
/// fuzzers. An audit, scan or fuzz that is not declared is not run; the audits, when none are named, are every known one.
pub fn run(security: &Security, cx: &Context<'_>) -> Vec<Outcome> {
    let mut out = Vec::new();
    let audits: Vec<String> = match &security.audit {
        Some(names) => names.clone(),
        None => audit::AUDITS.iter().map(|s| (*s).to_string()).collect(),
    };
    for name in &audits {
        out.push(match cx.tree {
            Some(tree) => audit::run_audit(name, tree, cx.path_var),
            None => Outcome::skipped(
                &format!("audit:{name}"),
                "no release tree was given to audit",
            ),
        });
    }
    if let Some(image) = &security.image {
        out.push(audit::run_image(image, cx.path_var));
    }
    if security.secrets {
        out.push(match cx.tree {
            Some(tree) => secrets::stage(tree),
            None => Outcome::skipped("secrets", "no release tree was given to scan"),
        });
    }
    if !security.dast.is_empty() {
        out.extend(dast::stages(&security.dast, cx.services));
    }
    for fuzz in &security.fuzz {
        out.push(run_fuzz(fuzz, cx));
    }
    out
}

fn run_fuzz(fuzz: &Fuzz, cx: &Context<'_>) -> Outcome {
    match fuzz.tool.as_str() {
        "cargo-fuzz" => match cx.tree {
            Some(tree) => fuzz::cargo_fuzz(
                tree,
                fuzz.dir.as_deref().unwrap_or("."),
                fuzz.target.as_deref().unwrap_or_default(),
                fuzz.seconds,
                cx.path_var,
            ),
            None => Outcome::skipped("fuzz:cargo-fuzz", "no release tree was given to fuzz"),
        },
        "schemathesis" => {
            let service = fuzz.service.as_deref().unwrap_or_default();
            match (cx.tree, cx.services.get(service)) {
                (Some(tree), Some(base)) => fuzz::schemathesis(
                    tree,
                    fuzz.schema.as_deref().unwrap_or_default(),
                    base,
                    fuzz.seconds,
                    cx.path_var,
                ),
                (None, _) => Outcome::skipped(
                    "fuzz:schemathesis",
                    "no release tree was given to read the schema from",
                ),
                (_, None) => Outcome::skipped(
                    "fuzz:schemathesis",
                    format!("no URL was given for service {service}"),
                ),
            }
        }
        other => Outcome::skipped(&format!("fuzz:{other}"), "not a known fuzzer"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{self, Security};

    #[test]
    fn a_stage_that_did_not_run_says_why_and_has_no_hits() {
        let skipped = Outcome::skipped("audit:cargo-deny", "not on the PATH");
        assert!(!skipped.ran);
        assert!(skipped.hits.is_empty());
        assert_eq!(skipped.reason, "not on the PATH");
    }

    #[test]
    fn with_no_tree_the_tree_stages_are_skipped_and_the_rest_are_not_declared() {
        let security = Security::default();
        let services = BTreeMap::new();
        let cx = Context {
            tree: None,
            services: &services,
            path_var: "",
        };
        let outcomes = run(&security, &cx);
        // Every known audit, and the secrets scan: each skipped for want of a tree.
        assert_eq!(outcomes.len(), audit::AUDITS.len() + 1);
        assert!(outcomes.iter().all(|o| !o.ran && !o.reason.is_empty()));
        assert!(outcomes.iter().any(|o| o.stage == "secrets"));
    }

    #[test]
    fn a_declared_stage_that_names_what_the_topology_does_not_have_is_refused() {
        let base = "name = \"web\"\n[[service]]\nname = \"api\"\nimage = \"api:1\"\n";
        let refused = [
            "[security]\naudit = [\"cargo-nope\"]\n",
            "[[security.dast]]\nservice = \"db\"\npath = \"/users\"\nparams = { name = \"a\" }\n",
            "[[security.dast]]\nservice = \"api\"\npath = \"users\"\nparams = { name = \"a\" }\n",
            "[[security.dast]]\nservice = \"api\"\npath = \"/users\"\nparams = {}\n",
            "[[security.fuzz]]\ntool = \"cargo-fuzz\"\ntarget = \"x y\"\n",
            "[[security.fuzz]]\ntool = \"cargo-fuzz\"\ntarget = \"parse\"\nseconds = 0\n",
            "[[security.fuzz]]\ntool = \"afl\"\n",
        ];
        for extra in refused {
            assert!(
                spec::parse(&format!("{base}{extra}")).is_err(),
                "accepted: {extra}"
            );
        }
        assert!(spec::parse(&format!("{base}[security]\naudit = [\"cargo-audit\"]\n")).is_ok());
    }

    #[test]
    fn a_topology_declares_its_stages_in_its_file() {
        let text = r#"
name = "web"

[[service]]
name = "api"
image = "api:1"
ports = [8080]

[security]
audit = ["npm-audit"]
image = "postgres:17"
secrets = false

[[security.dast]]
service = "api"
path = "/users"
params = { name = "Alice" }

[[security.fuzz]]
tool = "cargo-fuzz"
dir = "crates/parse"
target = "parse_input"
seconds = 30
"#;
        let topology = spec::parse(text).unwrap();
        let security = &topology.security;
        assert_eq!(
            security.audit.as_deref(),
            Some(&["npm-audit".to_string()][..])
        );
        assert_eq!(security.image.as_deref(), Some("postgres:17"));
        assert!(!security.secrets);
        assert_eq!(security.dast[0].params["name"], "Alice");
        assert_eq!(security.fuzz[0].seconds, 30);
        let back = spec::parse(&spec::to_toml(&topology).unwrap()).unwrap();
        assert_eq!(back, topology);
    }
}
