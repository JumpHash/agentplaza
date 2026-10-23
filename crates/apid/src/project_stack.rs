//! A project's stack declaration (PLAN 3.1, 16.1): `.plaza/project.toml`, read from the project's Weft repo at main the
//! way project memory reads `.plaza/memory/` (project_memory.rs). It says what the project's base holds: the languages
//! whose language servers start in it (15.2), the toolchains and their versions, the services its tests need, the pools
//! it runs in (17.2), and the targets its changes are previewed or tested on (19, 21.3).
//!
//! Every field is typed and an unknown key is refused, so a typo fails the base's build instead of being ignored. The
//! base recipe (base_recipe.rs) turns a declaration into the commands that build the base.
//!
//! ```toml
//! languages = ["rust", "typescript"]
//!
//! [toolchains]
//! rust = "1.88.0"
//! node = "24"
//!
//! [build]                       # optional: replaces a toolchain's default commands when present
//! fetch = [["cargo", "fetch"]]
//!
//! [[services]]                  # started in the test base, left running
//! name = "postgres"
//! command = ["postgres", "-D", "/var/lib/pg"]
//!
//! [test]
//! setup = [["make", "seed"]]    # run in the test base once the services are up
//!
//! [[pools]]
//! arch = "aarch64"
//! hypervisor = "kvm"
//!
//! [[targets]]
//! name = "web"
//! kind = "preview"
//! ```

use std::collections::{BTreeMap, HashSet};

use apz_lsp::Language;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::state::App;

/// Where a project's declaration lives in its repo.
pub const FILE: &str = ".plaza/project.toml";

/// A command as its argument vector: the program, then its arguments. Run in the VM, never through a shell.
pub type Argv = Vec<String>;

/// A project's declaration (every field optional: an empty file declares an empty stack).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Stack {
    /// The languages the project uses; their language servers start in the base (15.2).
    #[serde(deserialize_with = "languages")]
    pub languages: Vec<Language>,
    /// Each toolchain's version, by its name.
    pub toolchains: BTreeMap<Toolchain, String>,
    /// The services the test base starts (a database, a queue) and leaves running.
    pub services: Vec<Service>,
    /// The commands of the base, replacing the toolchains' defaults.
    pub build: Build,
    /// What the test base adds to the build base.
    pub test: Test,
    /// The pools the project runs in (17.2): each an architecture and a hypervisor.
    pub pools: Vec<Pool>,
    /// Where the project's changes are previewed or tried by hand (19, 21.3).
    pub targets: Vec<Target>,
}

/// The toolchains a declaration may name, and how a base installs each one (base_recipe.rs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Toolchain {
    Rust,
    Node,
    Python,
}

impl Toolchain {
    /// The name the declaration and the steps' labels use.
    pub fn name(self) -> &'static str {
        match self {
            Toolchain::Rust => "rust",
            Toolchain::Node => "node",
            Toolchain::Python => "python",
        }
    }
}

/// A service a test base starts: its name, and the command that runs it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Service {
    pub name: String,
    pub command: Argv,
}

/// The base's commands. `None` keeps the toolchains' defaults; a list, even an empty one, replaces them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Build {
    pub fetch: Option<Vec<Argv>>,
    pub build: Option<Vec<Argv>>,
}

/// What the test base runs after its services start.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Test {
    pub setup: Vec<Argv>,
}

/// A machine a project runs on: an architecture and a hypervisor (its pool, with the CPU baseline, 17.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pool {
    pub arch: Arch,
    pub hypervisor: Hypervisor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Arch {
    #[serde(rename = "x86_64")]
    X86_64,
    #[serde(rename = "aarch64")]
    Aarch64,
}

impl Arch {
    /// The name a node reports its architecture under (`GET /v1/machine`'s `arch`).
    pub fn name(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Hypervisor {
    Kvm,
    Hvf,
    Whp,
}

impl Hypervisor {
    /// The name a node reports its VMM backend under (`GET /v1/machine`'s `backend`).
    pub fn name(self) -> &'static str {
        match self {
            Hypervisor::Kvm => "kvm",
            Hypervisor::Hvf => "hvf",
            Hypervisor::Whp => "whp",
        }
    }
}

/// A target: a place a change is previewed or tried by hand, and the kind of base that serves it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub name: String,
    pub kind: TargetKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetKind {
    Preview,
    Gui,
    Device,
}

/// `languages` as names (the W40 names apz-lsp gives), each one a language the server table knows.
fn languages<'de, D: Deserializer<'de>>(de: D) -> Result<Vec<Language>, D::Error> {
    let names = Vec::<String>::deserialize(de)?;
    names
        .iter()
        .map(|name| {
            Language::ALL
                .into_iter()
                .find(|language| language.name() == name.as_str())
                .ok_or_else(|| {
                    <D::Error as serde::de::Error>::custom(format!("unknown language {name}"))
                })
        })
        .collect()
}

/// The declaration in `text`, checked: a toolchain has a version, a service has a name and a command, no command is
/// empty, and the service names are unique.
pub fn parse(text: &str) -> Result<Stack, String> {
    let stack: Stack = toml::from_str(text).map_err(|e| format!("{FILE}: {e}"))?;
    stack.check()?;
    Ok(stack)
}

impl Stack {
    fn check(&self) -> Result<(), String> {
        for (toolchain, version) in &self.toolchains {
            if version.trim().is_empty() {
                return Err(format!(
                    "{FILE}: toolchains.{} has no version",
                    toolchain.name()
                ));
            }
        }
        let mut names = HashSet::new();
        for service in &self.services {
            if service.name.trim().is_empty() || !names.insert(service.name.as_str()) {
                return Err(format!(
                    "{FILE}: service {:?} is unnamed or repeated",
                    service.name
                ));
            }
            if service.command.is_empty() {
                return Err(format!("{FILE}: service {} has no command", service.name));
            }
        }
        let commands = self
            .build
            .fetch
            .iter()
            .chain(self.build.build.iter())
            .flatten()
            .chain(self.test.setup.iter());
        if commands.into_iter().any(Vec::is_empty) {
            return Err(format!("{FILE}: a command is empty"));
        }
        Ok(())
    }
}

/// The declaration on `project`'s repo at main. `None` when the file is not there; an error when the project has no repo
/// yet, or the file does not parse.
pub fn read(app: &App, project: &str) -> Result<Option<Stack>, String> {
    let repo = app
        .projects
        .get(project)
        .and_then(|p| p.repo)
        .ok_or_else(|| format!("{project} has no repo yet"))?;
    let detail = app.weft.get(&format!("/repos/{repo}"))?;
    let rev = detail["refs"]["main"]
        .as_str()
        .ok_or_else(|| format!("{repo} has no main"))?;
    let listing = app.weft.get(&format!("/revisions/{rev}/files"))?;
    let client = app.weft.client("hub")?;
    in_listing(&listing, |path| {
        client.get_bytes(&format!("/revisions/{rev}/file?path={path}"))
    })
}

/// The declaration in a revision's file listing (Weft's `files`), read with `fetch` only when the listing has it.
pub fn in_listing(
    listing: &Value,
    fetch: impl FnOnce(&str) -> Result<Vec<u8>, String>,
) -> Result<Option<Stack>, String> {
    let present = listing["files"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|file| file["path"].as_str() == Some(FILE));
    if !present {
        return Ok(None);
    }
    let bytes = fetch(FILE)?;
    let text = String::from_utf8(bytes).map_err(|_| format!("{FILE} is not UTF-8"))?;
    parse(&text).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SAMPLE: &str = r#"
languages = ["rust", "typescript"]

[toolchains]
rust = "1.88.0"
node = "24"

[[services]]
name = "postgres"
command = ["postgres", "-D", "/var/lib/pg"]

[test]
setup = [["make", "seed"]]

[[pools]]
arch = "aarch64"
hypervisor = "kvm"

[[targets]]
name = "web"
kind = "preview"
"#;

    #[test]
    fn a_declaration_parses_into_its_typed_sections() {
        let stack = parse(SAMPLE).unwrap();
        assert_eq!(stack.languages, vec![Language::Rust, Language::TypeScript]);
        assert_eq!(
            stack.toolchains,
            BTreeMap::from([
                (Toolchain::Rust, "1.88.0".to_string()),
                (Toolchain::Node, "24".to_string()),
            ])
        );
        assert_eq!(stack.services[0].name, "postgres");
        assert_eq!(stack.test.setup, vec![vec!["make", "seed"]]);
        assert_eq!(
            stack.pools,
            vec![Pool {
                arch: Arch::Aarch64,
                hypervisor: Hypervisor::Kvm
            }]
        );
        assert_eq!(stack.targets[0].kind, TargetKind::Preview);
        assert_eq!(stack.build, Build::default());
    }

    #[test]
    fn an_empty_file_declares_an_empty_stack() {
        assert_eq!(parse("").unwrap(), Stack::default());
    }

    #[test]
    fn an_empty_build_list_replaces_the_defaults_and_an_absent_one_keeps_them() {
        let stack = parse("[build]\nbuild = []\n").unwrap();
        assert_eq!(stack.build.build, Some(Vec::new()));
        assert_eq!(stack.build.fetch, None);
    }

    #[test]
    fn unknown_keys_and_values_are_refused() {
        let unknown_key = parse("langauges = [\"rust\"]\n").unwrap_err();
        assert!(unknown_key.contains("langauges"), "{unknown_key}");
        assert!(
            parse("languages = [\"cobol\"]\n")
                .unwrap_err()
                .contains("cobol")
        );
        assert!(parse("[toolchains]\nruby = \"3\"\n").is_err());
        assert!(parse("[[pools]]\narch = \"sparc\"\nhypervisor = \"kvm\"\n").is_err());
        assert!(parse("[[targets]]\nname = \"w\"\nkind = \"sandbox\"\n").is_err());
    }

    #[test]
    fn a_toolchain_without_a_version_and_a_repeated_service_are_refused() {
        assert!(
            parse("[toolchains]\nrust = \" \"\n")
                .unwrap_err()
                .contains("rust")
        );
        let twice = "[[services]]\nname = \"db\"\ncommand = [\"db\"]\n[[services]]\nname = \"db\"\ncommand = [\"db\"]\n";
        assert!(parse(twice).unwrap_err().contains("repeated"));
        assert!(
            parse("[build]\nfetch = [[]]\n")
                .unwrap_err()
                .contains("empty")
        );
    }

    #[test]
    fn the_declaration_is_read_from_main_only_when_the_file_is_listed() {
        let listed = json!({ "files": [{ "path": "README.md" }, { "path": FILE }] });
        let stack = in_listing(&listed, |path| {
            assert_eq!(path, FILE);
            Ok(SAMPLE.as_bytes().to_vec())
        })
        .unwrap();
        assert_eq!(stack, Some(parse(SAMPLE).unwrap()));

        let unlisted = json!({ "files": [{ "path": "README.md" }] });
        let none = in_listing(&unlisted, |_| panic!("not listed: never fetched")).unwrap();
        assert_eq!(none, None);
    }

    #[test]
    fn a_listed_file_that_does_not_parse_or_cannot_be_fetched_is_an_error() {
        let listed = json!({ "files": [{ "path": FILE }] });
        let fetch_failed = in_listing(&listed, |_| Err("weft is down".into()));
        assert_eq!(fetch_failed.unwrap_err(), "weft is down");
        let not_utf8 = in_listing(&listed, |_| Ok(vec![0xff, 0xfe]));
        assert!(not_utf8.unwrap_err().contains("UTF-8"));
        let broken = in_listing(&listed, |_| Ok(b"[toolchains".to_vec()));
        assert!(broken.unwrap_err().starts_with(FILE));
    }
}
