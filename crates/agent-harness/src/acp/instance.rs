//! The engine a `code_engine` call runs: a catalogue engine, or an instance the owner added through the
//! hub (`apz_proto::wire::EngineSpec`). The hub checked the spec when it was saved; this module checks it
//! again where the harness acts on it: the pins it installs, the names it writes to disk, the environment
//! and arguments it passes on. A call without a spec runs the catalogue exactly as before.

use std::path::{Component, Path, PathBuf};

use apz_proto::wire::{EngineInstall, EngineSpec};

use super::catalog::{self, Install as Catalogue, ModelVia};
use super::run::engine_dir;
use crate::engines::{self, NPM_PREFIX};
use crate::files::Workspace;

/// The modes an engine the owner added refuses, beyond the global list: its bypass, yolo and danger modes.
const ADDED_REFUSED_MODES: &[&str] = &["bypass", "yolo", "danger", "full-access"];
const NO_MODES: &[&str] = &[];
/// Variables an added engine may not set: the harness's own identity, routing and proxy settings.
const RESERVED_ENV: &[&str] = &[
    "HOME",
    "PATH",
    "XDG_CONFIG_HOME",
    "NODE_OPTIONS",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
];
/// Variable names ending like the gateway's base URLs and keys, which the harness sets for each engine.
const RESERVED_ENV_SUFFIXES: &[&str] = &["_BASE_URL", "_API_KEY", "_AUTH_TOKEN"];

/// How the VM installs an engine: the exact version and the check its bytes must pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pin {
    /// An npm package at an exact version, checked against its `dist.integrity`.
    Npm {
        package: String,
        version: String,
        integrity: String,
    },
    /// A PyPI wheel at an exact version, checked against its sha256.
    Pip {
        package: String,
        version: String,
        file: String,
        sha256: String,
    },
    /// The release archive for this VM's architecture, checked against its sha256; `cmd` is the binary in it.
    Archive {
        version: String,
        url: String,
        sha256: String,
        cmd: String,
    },
    /// A command already in the VM's image or workspace: nothing is installed.
    Command { command: String },
}

/// One engine to start for a call.
#[derive(Debug, Clone)]
pub struct Instance {
    /// The instance id the call names: a built-in engine's id, or the owner's id for an added instance.
    /// It names the sessions a call can continue.
    pub id: String,
    /// The engine's home folder name under the engines home (`acp::layout::engine_home`).
    pub home: String,
    /// The catalogue id, `acp:<registry id>` or `acp-local`: which launch the engine gets.
    pub engine: String,
    /// True when the install is the catalogue's own pin, installed into the npm prefix as before.
    pub catalogue_pin: bool,
    pub pin: Pin,
    /// The command's name in the install's `bin` folder. Empty until known: an npm or PyPI package names
    /// its command in its own metadata, which is read after the install.
    pub binary: String,
    /// The folder whose `bin` holds the install's command.
    pub bin_root: PathBuf,
    /// The arguments the catalogue entry (and a release asset) gives, before the call's own.
    pub base_args: Vec<String>,
    /// The owner's launch arguments, after the call's own.
    pub args: Vec<String>,
    /// The owner's environment. Never a name the harness sets (see [`Instance::extra_env`]).
    pub env: Vec<(String, String)>,
    /// The model `<provider>/<model>` a call gets when it names none.
    pub route: Option<String>,
    pub model_via: ModelVia,
    refused_modes: &'static [&'static str],
    refused_exact_modes: &'static [&'static str],
}

impl Instance {
    /// The instance a call names. Without a spec it is the catalogue engine `id`; with one, the spec's
    /// engine, install, arguments, environment, home and route, all checked here.
    pub fn resolve(
        workspace: &Workspace,
        id: &str,
        spec: Option<&EngineSpec>,
    ) -> Result<Self, String> {
        match spec {
            None => Self::from_catalogue(id),
            Some(spec) => Self::from_spec(workspace, id, spec),
        }
    }

    /// The catalogue engine `id`, with the catalogue's pin, launch arguments and model handling.
    pub fn from_catalogue(id: &str) -> Result<Self, String> {
        let entry = catalog::entry(id).ok_or_else(|| {
            format!(
                "invalid input: \"engine\" must be one of {}",
                catalog::ids()
            )
        })?;
        let pin = catalogue_pin(entry)?;
        Ok(Self {
            id: id.to_string(),
            home: id.to_string(),
            engine: id.to_string(),
            catalogue_pin: true,
            binary: catalogue_binary(entry, &pin),
            pin,
            bin_root: PathBuf::from(NPM_PREFIX),
            base_args: entry.args.iter().map(|arg| arg.to_string()).collect(),
            args: Vec::new(),
            env: Vec::new(),
            route: None,
            model_via: entry.model,
            refused_modes: entry.refused_modes,
            refused_exact_modes: entry.refused_exact_modes,
        })
    }

    fn from_spec(workspace: &Workspace, id: &str, spec: &EngineSpec) -> Result<Self, String> {
        check_name(id, "engine")?;
        let entry = catalog::entry(&spec.engine);
        if entry.is_none() {
            check_registry_engine(&spec.engine)?;
        }
        let home = match spec.home.as_deref() {
            Some(home) => {
                check_name(home, "home")?;
                home.to_string()
            }
            None => id.to_string(),
        };
        let (pin, asset_args, catalogue_pin) = match (&spec.install, entry) {
            (Some(install), _) => {
                let (pin, asset_args) = pin_of(workspace, id, install)?;
                (pin, asset_args, false)
            }
            (None, Some(entry)) => (catalogue_pin(entry)?, Vec::new(), true),
            (None, None) => {
                return Err(format!(
                    "invalid input: {} needs an install: its package, archive or command",
                    spec.engine
                ));
            }
        };
        let mut base_args: Vec<String> = entry
            .map(|entry| entry.args.iter().map(|arg| arg.to_string()).collect())
            .unwrap_or_default();
        base_args.extend(asset_args);
        if spec.args.iter().any(|arg| arg.contains('\0')) {
            return Err("invalid input: an engine argument has a NUL byte".to_string());
        }
        let env = spec
            .env
            .iter()
            .map(|(key, value)| {
                check_env(key, value)?;
                Ok((key.clone(), value.clone()))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let route = match spec.route.as_deref() {
            Some(route) => {
                engines::split_model(route)?;
                Some(route.to_string())
            }
            None => None,
        };
        let (model_via, refused_modes, refused_exact_modes) = match entry {
            Some(entry) => (entry.model, entry.refused_modes, entry.refused_exact_modes),
            None => (ModelVia::ConfigOption, ADDED_REFUSED_MODES, NO_MODES),
        };
        let bin_root = if catalogue_pin {
            PathBuf::from(NPM_PREFIX)
        } else {
            engine_dir(&home, pin.version())
        };
        let binary = match (catalogue_pin, entry, &pin) {
            (true, Some(entry), _) => catalogue_binary(entry, &pin),
            (_, _, Pin::Archive { cmd, .. }) => file_name(cmd),
            _ => String::new(),
        };
        Ok(Self {
            id: id.to_string(),
            home,
            engine: spec.engine.clone(),
            catalogue_pin,
            pin,
            binary,
            bin_root,
            base_args,
            args: spec.args.clone(),
            env,
            route,
            model_via,
            refused_modes,
            refused_exact_modes,
        })
    }

    /// The version the install's folders are named by (`command` for a command, which has none).
    pub fn version(&self) -> &str {
        self.pin.version()
    }

    /// The executable the launch runs: the command itself, or the install's command in its `bin` folder.
    pub fn program(&self) -> String {
        match &self.pin {
            Pin::Command { command } => command.clone(),
            _ => self
                .bin_root
                .join("bin")
                .join(&self.binary)
                .display()
                .to_string(),
        }
    }

    /// The launch arguments: the catalogue's and the asset's, then the call's `extra`, then the owner's.
    pub fn launch_args(&self, extra: &[String]) -> Vec<String> {
        self.base_args
            .iter()
            .chain(extra)
            .chain(&self.args)
            .cloned()
            .collect()
    }

    /// The owner's environment as `KEY=VALUE` pairs to add to `pairs` (the harness's own). A name the
    /// harness already sets, or a reserved one, is left out: the harness's value wins.
    pub fn extra_env(&self, pairs: &[String]) -> Vec<String> {
        self.env
            .iter()
            .filter(|(key, _)| !reserved_env(key))
            .filter(|(key, _)| {
                !pairs
                    .iter()
                    .any(|pair| pair.split_once('=').is_some_and(|(name, _)| name == key))
            })
            .map(|(key, value)| format!("{key}={value}"))
            .collect()
    }

    /// True when the harness must never select `mode` (see [`catalog::refused_mode`]).
    pub fn refuses(&self, mode: &str) -> bool {
        catalog::refused_mode(mode, self.refused_exact_modes, self.refused_modes)
    }
}

impl Pin {
    /// The pinned version; a command has none.
    pub fn version(&self) -> &str {
        match self {
            Pin::Npm { version, .. } | Pin::Pip { version, .. } | Pin::Archive { version, .. } => {
                version
            }
            Pin::Command { .. } => "command",
        }
    }
}

/// The catalogue entry's own install as a pin. A release takes the archive for this VM's architecture.
fn catalogue_pin(entry: &catalog::Entry) -> Result<Pin, String> {
    Ok(match entry.install {
        Catalogue::Npm {
            package,
            version,
            integrity,
        } => Pin::Npm {
            package: package.to_string(),
            version: version.to_string(),
            integrity: integrity.to_string(),
        },
        Catalogue::Pip {
            package,
            version,
            file,
            sha256,
        } => Pin::Pip {
            package: package.to_string(),
            version: version.to_string(),
            file: file.to_string(),
            sha256: sha256.to_string(),
        },
        Catalogue::Release {
            version,
            assets,
            binary_path,
        } => {
            let arch = std::env::consts::ARCH;
            let asset = assets
                .iter()
                .find(|asset| asset.arch == arch)
                .ok_or_else(|| {
                    format!("cannot install {} {version}: no build for {arch}", entry.id)
                })?;
            Pin::Archive {
                version: version.to_string(),
                url: asset.url.to_string(),
                sha256: asset.sha256.to_string(),
                cmd: binary_path.to_string(),
            }
        }
    })
}

/// The command a catalogue entry starts, when its install is the catalogue's own (the binary's name is
/// known in advance).
fn catalogue_binary(entry: &catalog::Entry, pin: &Pin) -> String {
    match pin {
        Pin::Command { .. } => String::new(),
        _ => entry.binary.to_string(),
    }
}

/// The owner's install as a pin, checked: an exact version, a plain file name, a sha256 in lowercase hex,
/// an https archive and a relative command. Returns an archive asset's own arguments.
fn pin_of(
    workspace: &Workspace,
    id: &str,
    install: &EngineInstall,
) -> Result<(Pin, Vec<String>), String> {
    match install {
        EngineInstall::Npm {
            package,
            version,
            integrity,
        } => {
            check_package(package)?;
            check_version(version)?;
            if integrity.trim().is_empty() {
                return Err(format!("invalid input: {package} has no integrity pin"));
            }
            let pin = Pin::Npm {
                package: package.clone(),
                version: version.clone(),
                integrity: integrity.clone(),
            };
            Ok((pin, Vec::new()))
        }
        EngineInstall::Pip {
            package,
            version,
            file,
            sha256,
        } => {
            check_package(package)?;
            check_version(version)?;
            check_file_name(file)?;
            check_sha256(sha256)?;
            let pin = Pin::Pip {
                package: package.clone(),
                version: version.clone(),
                file: file.clone(),
                sha256: sha256.clone(),
            };
            Ok((pin, Vec::new()))
        }
        EngineInstall::Archive { version, assets } => {
            check_version(version)?;
            let arch = std::env::consts::ARCH;
            let asset = assets
                .get(&format!("linux-{arch}"))
                .ok_or_else(|| format!("cannot install {id} {version}: no build for {arch}"))?;
            check_url(&asset.url)?;
            check_sha256(&asset.sha256)?;
            check_relative_path(&asset.cmd)?;
            if asset.args.iter().any(|arg| arg.contains('\0')) {
                return Err("invalid input: an archive argument has a NUL byte".to_string());
            }
            let pin = Pin::Archive {
                version: version.clone(),
                url: asset.url.clone(),
                sha256: asset.sha256.clone(),
                cmd: asset.cmd.clone(),
            };
            Ok((pin, asset.args.clone()))
        }
        EngineInstall::Command { command } => {
            if command.is_empty() || command.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return Err(format!(
                    "invalid input: the command {command:?} is not one word"
                ));
            }
            // A relative path names a file in the workspace; a bare name is looked up on the VM's PATH.
            let command = if command.contains('/') && !Path::new(command).is_absolute() {
                workspace.dir().join(command).display().to_string()
            } else {
                command.clone()
            };
            Ok((Pin::Command { command }, Vec::new()))
        }
    }
}

/// An added instance may name only a registry engine (`acp:<id>`) or the local command (`acp-local`).
fn check_registry_engine(engine: &str) -> Result<(), String> {
    match engine.strip_prefix("acp:") {
        Some(registry) => check_name(registry, "registry engine"),
        None if engine == "acp-local" => Ok(()),
        None => Err(format!(
            "invalid input: {engine:?} is not an engine the harness knows"
        )),
    }
}

/// A plain name for an id or a folder: letters, digits, `-`, `_` and `.`, starting with a letter or digit.
fn check_name(value: &str, what: &str) -> Result<(), String> {
    let starts = value.starts_with(|c: char| c.is_ascii_alphanumeric());
    let plain = value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if starts && plain && value.len() <= 64 {
        Ok(())
    } else {
        Err(format!(
            "invalid input: the {what} {value:?} must be a plain name of letters, digits, '-', '_' or '.'"
        ))
    }
}

/// An npm or PyPI package name, as the installer takes it on its command line.
fn check_package(package: &str) -> Result<(), String> {
    let plain = !package.is_empty()
        && !package.starts_with(['-', '.'])
        && !package.contains("..")
        && package
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '/' | '.' | '-' | '_'));
    if plain {
        Ok(())
    } else {
        Err(format!("invalid input: {package:?} is not a package name"))
    }
}

/// One exact version: every dot-separated part starts with a digit (`1.2.3-rc.1`), and it holds no
/// range, wildcard or tag (`^1`, `1.x`, `latest`).
fn check_version(version: &str) -> Result<(), String> {
    let exact = version
        .split('.')
        .all(|part| part.starts_with(|c: char| c.is_ascii_digit()))
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-' | '_'));
    if exact {
        Ok(())
    } else {
        Err(format!(
            "invalid input: {version:?} is not one exact version"
        ))
    }
}

fn check_file_name(file: &str) -> Result<(), String> {
    let plain = !file.is_empty()
        && !file.starts_with('.')
        && file
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if plain {
        Ok(())
    } else {
        Err(format!(
            "invalid input: {file:?} is not a distribution file name"
        ))
    }
}

/// A sha256 as 64 lowercase hex digits.
fn check_sha256(sha256: &str) -> Result<(), String> {
    if sha256.len() == 64
        && sha256
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err(format!(
            "invalid input: {sha256:?} is not a sha256 in lowercase hex"
        ))
    }
}

fn check_url(url: &str) -> Result<(), String> {
    if url.starts_with("https://") && !url.chars().any(char::is_whitespace) {
        Ok(())
    } else {
        Err(format!("invalid input: {url:?} is not an https URL"))
    }
}

/// A path inside an archive: relative, with no `..` and no root.
fn check_relative_path(path: &str) -> Result<(), String> {
    let plain = !path.is_empty()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if plain {
        Ok(())
    } else {
        Err(format!("invalid input: {path:?} is not a relative path"))
    }
}

/// An environment name the VM may set, and its value with no NUL byte.
fn check_env(key: &str, value: &str) -> Result<(), String> {
    let mut chars = key.chars();
    let name = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !name {
        return Err(format!("invalid input: {key:?} is not an environment name"));
    }
    if value.contains('\0') {
        return Err(format!("invalid input: the value of {key} has a NUL byte"));
    }
    Ok(())
}

/// True for a variable the harness sets, or one an added engine may never set.
fn reserved_env(key: &str) -> bool {
    RESERVED_ENV.contains(&key)
        || RESERVED_ENV_SUFFIXES
            .iter()
            .any(|suffix| key.ends_with(suffix))
}

/// The last component of a path, as text (`goose` for `bin/goose`).
fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use apz_proto::wire::EngineAsset;

    use super::*;

    fn workspace() -> Workspace {
        Workspace::at(PathBuf::from("/workspace"))
    }

    fn spec(engine: &str, install: Option<EngineInstall>) -> EngineSpec {
        EngineSpec {
            engine: engine.to_string(),
            install,
            ..EngineSpec::default()
        }
    }

    fn npm(version: &str) -> EngineInstall {
        EngineInstall::Npm {
            package: "my-agent".to_string(),
            version: version.to_string(),
            integrity: "sha512-AAAA".to_string(),
        }
    }

    #[test]
    fn a_call_without_a_spec_runs_the_catalogue_engine_as_before() {
        let claude = Instance::resolve(&workspace(), "claude", None).expect("claude");
        assert_eq!(
            (claude.id.as_str(), claude.home.as_str()),
            ("claude", "claude")
        );
        assert_eq!(claude.engine, "claude");
        assert!(claude.catalogue_pin);
        assert_eq!(claude.bin_root, PathBuf::from(NPM_PREFIX));
        assert_eq!(
            claude.program(),
            format!("{NPM_PREFIX}/bin/claude-agent-acp")
        );
        assert!(claude.route.is_none() && claude.env.is_empty() && claude.args.is_empty());
        let opencode = Instance::resolve(&workspace(), "opencode", None).expect("opencode");
        assert_eq!(opencode.launch_args(&["--x".to_string()]), ["acp", "--x"]);
        let err = Instance::resolve(&workspace(), "bash", None).expect_err("not an engine");
        assert_eq!(
            err,
            format!(
                "invalid input: \"engine\" must be one of {}",
                catalog::ids()
            )
        );
    }

    #[test]
    fn an_npm_pin_must_name_one_exact_version() {
        let ok = Instance::resolve(
            &workspace(),
            "pi",
            Some(&spec("acp-local", Some(npm("1.2.3")))),
        );
        assert!(ok.is_ok(), "{ok:?}");
        for version in ["^1.0.0", "latest", "", "1.x", ">=1"] {
            let refused = Instance::resolve(
                &workspace(),
                "pi",
                Some(&spec("acp-local", Some(npm(version)))),
            );
            assert!(refused.is_err(), "{version:?} is a range or tag");
        }
    }

    #[test]
    fn an_added_engine_is_a_registry_or_local_engine_with_an_install() {
        let ws = workspace();
        assert!(Instance::resolve(&ws, "x", Some(&spec("bash", Some(npm("1.0.0"))))).is_err());
        assert!(Instance::resolve(&ws, "x", Some(&spec("acp:demo", None))).is_err());
        let registry = Instance::resolve(&ws, "x", Some(&spec("acp:demo", Some(npm("1.0.0")))));
        assert!(registry.is_ok(), "{registry:?}");
        let home = EngineSpec {
            home: Some("../escape".to_string()),
            ..spec("acp-local", Some(npm("1.0.0")))
        };
        assert!(Instance::resolve(&ws, "x", Some(&home)).is_err());
    }

    #[test]
    fn an_added_instance_has_its_own_home_and_install_folder() {
        let added = EngineSpec {
            home: Some("pi-home".to_string()),
            ..spec("acp-local", Some(npm("1.0.0")))
        };
        let pi = Instance::resolve(&workspace(), "pi", Some(&added)).expect("pi");
        assert_eq!(pi.home, "pi-home");
        assert!(!pi.catalogue_pin);
        assert_eq!(pi.bin_root, engine_dir("pi-home", "1.0.0"));
        let default_home = Instance::resolve(
            &workspace(),
            "pi",
            Some(&spec("acp-local", Some(npm("1.0.0")))),
        )
        .expect("pi");
        assert_eq!(default_home.home, "pi");
    }

    #[test]
    fn spec_args_follow_the_catalogue_and_the_calls_own() {
        let added = EngineSpec {
            args: vec!["--fast".to_string()],
            ..spec("opencode", None)
        };
        let instance = Instance::resolve(&workspace(), "oc_2", Some(&added)).expect("opencode");
        assert_eq!(
            instance.launch_args(&["--call".to_string()]),
            ["acp", "--call", "--fast"]
        );
    }

    #[test]
    fn the_owners_env_never_overrides_what_the_harness_sets() {
        let mut env = BTreeMap::new();
        env.insert("ANTHROPIC_BASE_URL".to_string(), "http://evil".to_string());
        env.insert("HOME".to_string(), "/elsewhere".to_string());
        env.insert("PATH".to_string(), "/evil".to_string());
        env.insert("FOO".to_string(), "1".to_string());
        env.insert("CLAUDE_X".to_string(), "y".to_string());
        let added = EngineSpec {
            env,
            ..spec("acp-local", Some(npm("1.0.0")))
        };
        let instance = Instance::resolve(&workspace(), "pi", Some(&added)).expect("pi");
        let harness = ["HOME=/workspace/.engines/home/pi".to_string()];
        assert_eq!(instance.extra_env(&harness), ["CLAUDE_X=y", "FOO=1"]);
        // A name the harness sets is left out even when the pairs do not carry it.
        assert_eq!(instance.extra_env(&[]), ["CLAUDE_X=y", "FOO=1"]);
    }

    #[test]
    fn a_bypass_mode_is_refused_for_an_added_engine_too() {
        let instance = Instance::resolve(
            &workspace(),
            "pi",
            Some(&spec("acp-local", Some(npm("1.0.0")))),
        )
        .expect("pi");
        for mode in [
            "bypass",
            "YOLO",
            "danger-mode",
            "full-access",
            "bypassPermissions",
        ] {
            assert!(instance.refuses(mode), "{mode} is refused");
        }
        assert!(!instance.refuses("plan"));
    }

    #[test]
    fn a_local_command_is_an_added_plain_acp_agent() {
        let command = EngineSpec {
            args: vec!["--stdio".to_string()],
            route: Some("anthropic/claude-x".to_string()),
            ..spec(
                "acp-local",
                Some(EngineInstall::Command {
                    command: "/usr/bin/my-agent".to_string(),
                }),
            )
        };
        let instance = Instance::resolve(&workspace(), "local", Some(&command)).expect("local");
        assert_eq!(instance.program(), "/usr/bin/my-agent");
        assert_eq!(instance.launch_args(&[]), ["--stdio"]);
        assert_eq!(instance.route.as_deref(), Some("anthropic/claude-x"));
        assert_eq!(instance.model_via, ModelVia::ConfigOption);
        let bad = spec(
            "acp-local",
            Some(EngineInstall::Command {
                command: "my agent".to_string(),
            }),
        );
        assert!(Instance::resolve(&workspace(), "local", Some(&bad)).is_err());
    }

    #[test]
    fn an_archive_asset_is_chosen_for_this_vm_and_keeps_its_arguments() {
        let mut assets = BTreeMap::new();
        assets.insert(
            format!("linux-{}", std::env::consts::ARCH),
            EngineAsset {
                url: "https://example.com/agent.tar.gz".to_string(),
                sha256: "a".repeat(64),
                cmd: "bin/agent".to_string(),
                args: vec!["acp".to_string()],
            },
        );
        let install = EngineInstall::Archive {
            version: "2.0.0".to_string(),
            assets,
        };
        let instance = Instance::resolve(
            &workspace(),
            "arch",
            Some(&spec("acp-local", Some(install))),
        )
        .expect("archive");
        assert_eq!(instance.binary, "agent");
        assert_eq!(instance.launch_args(&[]), ["acp"]);
        // Another architecture's build only: refused.
        let other = EngineInstall::Archive {
            version: "2.0.0".to_string(),
            assets: BTreeMap::new(),
        };
        assert!(
            Instance::resolve(&workspace(), "arch", Some(&spec("acp-local", Some(other)))).is_err()
        );
    }
}
