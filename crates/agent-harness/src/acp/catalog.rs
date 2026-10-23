//! The engines `code_engine` can run over ACP: how each one is installed, which command starts its
//! ACP adapter, and how it is told its model.
//!
//! Versions are pinned (never `latest`). An npm install is checked against the registry's sha512
//! integrity; a release archive is checked against its sha256 per architecture. Installs happen in
//! the VM on first use. A version bump is a code change, re-checked for which of the adapter's tools
//! ask permission before they act.

/// How an engine learns the model the hub allowed for the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelVia {
    /// The ACP session config option `model` (`session/set_config_option`), after `session/new`.
    ConfigOption,
}

/// How an engine is installed in the VM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Install {
    /// An npm package, installed globally into the engines' npm prefix. `npm pack` fetches the exact
    /// version, and its tarball must match `integrity` before it is installed.
    Npm {
        /// The npm package of the ACP adapter.
        package: &'static str,
        /// The pinned version of that package.
        version: &'static str,
        /// The registry's `dist.integrity` for that version, checked before the install.
        integrity: &'static str,
    },
    /// A release archive published by the engine's project. The archive for the VM's architecture is
    /// downloaded, checked against its sha256, and extracted; its binary is then linked into the npm
    /// prefix's `bin`, where the other engines are.
    Release {
        /// The pinned release version, as the release names it.
        version: &'static str,
        /// One archive per architecture the engine ships for.
        assets: &'static [Asset],
        /// The path of the binary inside the archive. A binary found elsewhere in the extracted tree
        /// under the same file name is used when the archive keeps it in a folder.
        binary_path: &'static str,
    },
    /// A Python package from PyPI, installed into a virtualenv of its own under the engine's folder.
    /// `pip download` fetches exactly `file` without its dependencies, and its sha256 must match before
    /// anything is installed; then pip installs that file and resolves its dependencies, as npm does
    /// for an npm package.
    Pip {
        /// The PyPI project name.
        package: &'static str,
        /// The pinned version of that project.
        version: &'static str,
        /// The distribution file pip must fetch, by name (a wheel for a pure-Python project).
        file: &'static str,
        /// That file's sha256 as lowercase hex.
        sha256: &'static str,
    },
}

impl Install {
    /// The pinned version, whichever way the engine is installed.
    #[cfg(test)]
    pub fn version(&self) -> &'static str {
        match self {
            Install::Npm { version, .. }
            | Install::Release { version, .. }
            | Install::Pip { version, .. } => version,
        }
    }
}

/// One release archive of a [`Install::Release`] engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asset {
    /// The architecture the archive is for: `std::env::consts::ARCH`, `x86_64` or `aarch64`.
    pub arch: &'static str,
    /// The https URL of the archive on the release page.
    pub url: &'static str,
    /// The archive's sha256 as lowercase hex.
    pub sha256: &'static str,
}

/// One engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// The name the model uses in `code_engine{engine}`.
    pub id: &'static str,
    pub install: Install,
    /// The command the install provides, in the npm prefix's `bin`.
    pub binary: &'static str,
    /// The arguments that start the adapter as an ACP agent on its stdio.
    pub args: &'static [&'static str],
    pub model: ModelVia,
    /// Session modes the harness never selects, whatever the engine offers (its bypass modes). A mode
    /// is refused when its id contains one of these words, ignoring case.
    pub refused_modes: &'static [&'static str],
    /// Session modes refused only when their id is exactly one of these words, ignoring case. For words
    /// too short to match as substrings: `auto` would also refuse a harmless `autonomous`.
    pub refused_exact_modes: &'static [&'static str],
}

/// Every engine the harness knows, in the order they are listed to the model. Gemini CLI, Qwen Code,
/// Kilo Code, Cline, GitHub Copilot CLI, Goose, Kimi Code, Auggie and Mistral Vibe are ACP only
/// (`gemini --acp`, `qwen --acp`, `kilo acp`, `cline --acp`, `copilot --acp --stdio`, `goose acp`,
/// `kimi acp`, `auggie --acp`, `vibe-acp`); the owner enables them in `engines_enabled`.
pub const ENGINES: &[Entry] = &[
    Entry {
        id: "claude",
        install: Install::Npm {
            package: "@agentclientprotocol/claude-agent-acp",
            version: "0.84.0",
            integrity: "sha512-Zhjyxvm7USDB/BAFx2L6U6rA3spJ6qwEDFbLgByzpmQMeZGuAlZPXhMcAc+Xsndj9kDh+OFJNjETRwGJCR1eTQ==",
        },
        binary: "claude-agent-acp",
        args: &[],
        model: ModelVia::ConfigOption,
        refused_exact_modes: &[],
        refused_modes: &["bypassPermissions"],
    },
    Entry {
        id: "codex",
        install: Install::Npm {
            package: "@agentclientprotocol/codex-acp",
            version: "2.0.1",
            integrity: "sha512-P144H9s2hUVvVDDgnL1+8u7VJu1OIBxHlb81VCxIBQM/M2D0JAepMufEW7jTKK4AML0TOHL+G1t5RqEt/t1fFA==",
        },
        binary: "codex-acp",
        args: &[],
        model: ModelVia::ConfigOption,
        refused_exact_modes: &[],
        refused_modes: &["full-access", "danger-full-access"],
    },
    Entry {
        id: "opencode",
        install: Install::Npm {
            package: "opencode-ai",
            version: "1.18.34",
            integrity: "sha512-9WUS2T0t4HHDVzXvuwTHF0nvhXvZ9mQ0r+ozCvKdJu0LVoQpOzqAW4qWTC3ygNc5Oedcc7j+Oct24JYlQYnySA==",
        },
        binary: "opencode",
        args: &["acp"],
        model: ModelVia::ConfigOption,
        refused_exact_modes: &[],
        refused_modes: &[],
    },
    Entry {
        id: "gemini",
        install: Install::Npm {
            package: "@google/gemini-cli",
            version: "0.62.0",
            integrity: "sha512-A1rw0Tf2sHLpGncfYdaq5WaJIufKAP8il4BmHD5Yw4ewmB/Wo0vRQb2bEvx7OqyaPFPZCh0hVhcMKsICZyIBww==",
        },
        binary: "gemini",
        args: &["--acp"],
        // Gemini CLI takes its model from the environment (`GEMINI_MODEL`), not the session, so the
        // launch names no model for it (`engines::acp_launch`).
        model: ModelVia::ConfigOption,
        refused_exact_modes: &[],
        refused_modes: &["yolo"],
    },
    Entry {
        id: "qwen",
        install: Install::Npm {
            package: "@qwen-code/qwen-code",
            version: "0.24.7",
            integrity: "sha512-NE+Ps9juMJS7DaHEVENIAJWCrzTiO/2bNq2TidmxmFc0ZbSMVKCfX3tlu2RY+CoP8xHFfUcy3MfQDQNszBJpJQ==",
        },
        binary: "qwen",
        args: &["--acp"],
        // Qwen Code takes its model from the environment (`OPENAI_MODEL`), not the session, so the
        // launch names no model for it (`engines::acp_launch`).
        model: ModelVia::ConfigOption,
        refused_exact_modes: &[],
        refused_modes: &["yolo"],
    },
    Entry {
        id: "kilo",
        install: Install::Npm {
            package: "@kilocode/cli",
            version: "7.8.1",
            integrity: "sha512-3dNWXhFhMxLkTXt8lMJ8fH1ZK8Ws7D6jEipMCmxejkaMNp9hd7FPnjCGUjK6zknnSJZokkA4+guFxoJQ+njYYw==",
        },
        binary: "kilo",
        args: &["acp"],
        model: ModelVia::ConfigOption,
        refused_exact_modes: &[],
        refused_modes: &[],
    },
    Entry {
        id: "cline",
        install: Install::Npm {
            package: "cline",
            version: "3.0.67",
            integrity: "sha512-4aMoB/jww42vDt/9iXqdMbmA7QiVNP5dPLDrz4mt4veKp9Y2DL041RL67PCrEH0TeWsLuAo4nIjEiwsa93TL3A==",
        },
        binary: "cline",
        args: &["--acp"],
        // Cline takes its model from its saved provider settings, which `engines::setup_args` writes
        // before each call, so the session is given none.
        model: ModelVia::ConfigOption,
        refused_exact_modes: &[],
        refused_modes: &[],
    },
    Entry {
        id: "copilot",
        install: Install::Npm {
            package: "@github/copilot",
            version: "1.0.90",
            integrity: "sha512-XNhA7NscW1D/H537wK+NRuUyWNCqK5JIZZ4aknhibZMiOvRLLbasR4t1QxzSEfr+ZOC6AIY9e6PkGMjgOAN5aQ==",
        },
        binary: "copilot",
        args: &["--acp", "--stdio"],
        // Copilot runs in its bring-your-own-key mode (`engines::acp_launch`): the model goes in the
        // environment (`COPILOT_MODEL`), so the session is given none. Its platform binary is an npm
        // optional dependency, so the install must not omit optional packages.
        model: ModelVia::ConfigOption,
        // Its bypass modes: allow-all and autopilot approve every action without asking.
        refused_exact_modes: &[],
        refused_modes: &["allow-all", "allow_all", "yolo", "autopilot"],
    },
    Entry {
        id: "goose",
        install: Install::Release {
            version: "1.52.0",
            assets: &[
                Asset {
                    arch: "x86_64",
                    url: "https://github.com/aaif-goose/goose/releases/download/v1.52.0/goose-x86_64-unknown-linux-musl.tar.gz",
                    sha256: "fdc86653285a89f7dcad6e76736af1688ab2cde44661b089328b2fc40bb79f9f",
                },
                Asset {
                    arch: "aarch64",
                    url: "https://github.com/aaif-goose/goose/releases/download/v1.52.0/goose-aarch64-unknown-linux-musl.tar.gz",
                    sha256: "4584ba8840fc77c2f3e48758f45543c18f1c1293d704941a8eeb5220852ce653",
                },
            ],
            binary_path: "goose",
        },
        binary: "goose",
        args: &["acp"],
        // Goose takes its model and provider from the environment (`engines::acp_launch`), so the
        // session is given none.
        model: ModelVia::ConfigOption,
        // Its auto mode approves what the model asks for without a prompt.
        refused_exact_modes: &[],
        refused_modes: &["auto"],
    },
    Entry {
        id: "kimi",
        install: Install::Npm {
            package: "@moonshot-ai/kimi-code",
            version: "2.1.1",
            integrity: "sha512-xClqcnTQUKgKDbeOGPU78qcwxGzL8wA2rqWuOX3CLpyQ1/NhSX7jAh6hZo/JFwThSX8obkgef3uw/li5JaJNyw==",
        },
        binary: "kimi",
        args: &["acp"],
        // Kimi Code takes its model and provider from the environment (`engines::acp_launch`), so the
        // session is given none. It needs Node 22.19 or later, checked before its install.
        model: ModelVia::ConfigOption,
        // Its auto mode approves what the model asks for. Matched exactly: `autonomous` is not auto.
        refused_exact_modes: &["auto"],
        // Its yolo mode approves every action without asking.
        refused_modes: &["yolo"],
    },
    Entry {
        id: "auggie",
        install: Install::Npm {
            package: "@augmentcode/auggie",
            version: "0.36.0",
            integrity: "sha512-jb4kq97pGzG0i5UyISc//LjTVlFzIywwD+VfPemiSIl58KjLfxry3KDgxWYnr1vmV8yUkySVzjLnj4ewUEbT/Q==",
        },
        binary: "auggie",
        args: &["--acp"],
        // Auggie's account never enters the VM: the session token is a placeholder that the hub's vendor
        // relay answers for (`engines::acp_launch`). A call's model goes on the command line instead
        // (`engines::acp_args`), so the session is given none.
        model: ModelVia::ConfigOption,
        refused_exact_modes: &[],
        // Its launch-process allow rule runs every command the model asks for without a prompt.
        refused_modes: &["launch-process:allow"],
    },
    Entry {
        id: "vibe",
        install: Install::Pip {
            package: "mistral-vibe",
            version: "2.25.0",
            file: "mistral_vibe-2.25.0-py3-none-any.whl",
            sha256: "cdedb722d5b8ae6770b56496f6b17677cc2d02705b6e6d5205c0c09167e652fa",
        },
        binary: "vibe-acp",
        args: &[],
        // Vibe reads its model from the config file the launch writes first (`engines::write_vibe_config`),
        // so the session is given none.
        model: ModelVia::ConfigOption,
        // Its auto-approve mode runs every tool without asking, and bypass turns the approvals off.
        refused_exact_modes: &[],
        refused_modes: &["auto-approve", "bypass"],
    },
    // The six engines of 5.6 run through their thin adapters (`acp/shim.rs`): each entry's binary is the engine's
    // own command, which the adapter runs per turn. The adapter is not yet launched by the VM, so these do not start
    // until that wiring lands; their flags are unverified (see `shim::SHIMS`).
    Entry {
        id: "amp",
        install: Install::Npm {
            package: "@sourcegraph/amp",
            version: "0.0.1792756861-g6187a2",
            integrity: "sha512-NM7rGueC/eUiffTtS2A56NFYyOryOTocKJ1CWBuPzwuP9eA1U9CeqxB2c1P5EP9JS5g7IiNKnRN8OHZZ+8S6bA==",
        },
        binary: "amp",
        args: &["acp-shim", "amp"],
        model: ModelVia::ConfigOption,
        refused_exact_modes: &[],
        refused_modes: &[],
    },
    Entry {
        id: "crush",
        install: Install::Npm {
            package: "@charmland/crush",
            version: "0.98.1",
            integrity: "sha512-TWjk5My+NLi+iNidiYNde4s05g85sOC97IiBs6aXywuFP2pZ9C+wjih985StTvmY3lJgPd9oKrtgbPRGT2SQ8g==",
        },
        binary: "crush",
        args: &["acp-shim", "crush"],
        model: ModelVia::ConfigOption,
        refused_exact_modes: &[],
        refused_modes: &[],
    },
    Entry {
        id: "junie",
        install: Install::Npm {
            package: "@jetbrains/junie-cli",
            version: "1468.30.0",
            integrity: "sha512-qWMbKcebGprv2d4pBzbgPdbsipkqNhOFAM2FA0RjBqGUrsARsESAvz7nSxptcnavr/krCMCe4XUQd7hV/5TAgA==",
        },
        binary: "junie",
        args: &["acp-shim", "junie"],
        model: ModelVia::ConfigOption,
        refused_exact_modes: &[],
        refused_modes: &[],
    },
    Entry {
        id: "droid",
        install: Install::Npm {
            package: "@factory/cli",
            version: "0.248.0",
            integrity: "sha512-eKv+gmgQJXKln3AJo9A8BSW2FA1ddhdYnFS0Q9z9Eju1vk5JZkMrIA20//hpvGYLdzAgH7vAeWl+xMu+SOAsrQ==",
        },
        binary: "droid",
        args: &["acp-shim", "droid"],
        model: ModelVia::ConfigOption,
        refused_exact_modes: &[],
        refused_modes: &[],
    },
    Entry {
        id: "aider",
        install: Install::Pip {
            package: "aider-chat",
            version: "0.86.2",
            file: "aider_chat-0.86.2-py3-none-any.whl",
            sha256: "64f6a0c66c9f4633ad9f479bca3e64ebcba02b9da03c6b604b74a44736b2416e",
        },
        binary: "aider",
        args: &["acp-shim", "aider"],
        model: ModelVia::ConfigOption,
        refused_exact_modes: &[],
        refused_modes: &[],
    },
    Entry {
        id: "openhands",
        install: Install::Pip {
            package: "openhands-ai",
            version: "1.11.0",
            file: "openhands_ai-1.11.0-py3-none-any.whl",
            sha256: "833de097150d498ffc7e175869df4723aec4a6b3c0be27545ca37089e6452c8f",
        },
        binary: "openhands",
        args: &["acp-shim", "openhands"],
        model: ModelVia::ConfigOption,
        refused_exact_modes: &[],
        refused_modes: &[],
    },
];

/// Words every engine refuses in a mode or profile id, on top of its own lists. A bypass, yolo or
/// allow-all mode approves every action, so none of them is selected whichever engine offers it; an
/// engine's own list is not trusted to name every such mode.
pub const REFUSED_MODES_EVERYWHERE: &[&str] = &[
    "bypass",
    "yolo",
    "allow-all",
    "allow_all",
    "full-access",
    "full_access",
    "danger",
    "autopilot",
];

/// True when the harness must never select `mode`: its id contains one of the refused words (`words`,
/// with the global ones), or is exactly one of `exact`, ignoring case, so `Allow-All` and
/// `allow_all_tools` are refused like `allow-all`.
pub fn refused_mode(mode: &str, exact: &[&str], words: &[&str]) -> bool {
    let mode = mode.to_ascii_lowercase();
    exact.iter().any(|word| mode == word.to_ascii_lowercase())
        || REFUSED_MODES_EVERYWHERE
            .iter()
            .chain(words)
            .any(|word| mode.contains(&word.to_ascii_lowercase()))
}

#[cfg(test)]
impl Entry {
    /// True when the harness must never select `mode` (see [`refused_mode`]). Tests check the catalogue's
    /// lists with it; the launch asks its [`super::instance::Instance`].
    pub fn refuses(&self, mode: &str) -> bool {
        refused_mode(mode, self.refused_exact_modes, self.refused_modes)
    }
}

/// The engine named `id`.
pub fn entry(id: &str) -> Option<&'static Entry> {
    ENGINES.iter().find(|engine| engine.id == id)
}

/// The engine ids, comma-separated, for error messages.
pub fn ids() -> String {
    ENGINES
        .iter()
        .map(|engine| engine.id)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_is_pinned_with_its_integrity() {
        for engine in ENGINES {
            assert!(
                engine
                    .install
                    .version()
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit()),
                "{} is pinned to a version, not a tag",
                engine.id
            );
            match engine.install {
                Install::Npm { integrity, .. } => {
                    assert!(integrity.starts_with("sha512-"), "{}", engine.id);
                }
                Install::Release { assets, .. } => {
                    assert!(!assets.is_empty(), "{}", engine.id);
                    for asset in assets {
                        assert!(asset.url.starts_with("https://"), "{}", engine.id);
                        assert_eq!(asset.sha256.len(), 64, "{}", engine.id);
                        assert!(
                            asset.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
                            "{}",
                            engine.id
                        );
                    }
                }
                Install::Pip { file, sha256, .. } => {
                    assert!(file.ends_with(".whl"), "{}", engine.id);
                    assert_eq!(sha256.len(), 64, "{}", engine.id);
                    assert!(
                        sha256.bytes().all(|b| b.is_ascii_hexdigit()),
                        "{}",
                        engine.id
                    );
                }
            }
            assert_eq!(entry(engine.id), Some(engine));
        }
        assert_eq!(entry("bash"), None);
    }

    #[test]
    fn engine_pins_install_the_cli_in_acp_mode_with_bypass_modes_refused() {
        // gemini is the CLI in ACP mode with yolo refused.
        {
            let gemini = entry("gemini").expect("gemini");
            assert_eq!(
                gemini.install,
                Install::Npm {
                    package: "@google/gemini-cli",
                    version: "0.62.0",
                    integrity: "sha512-A1rw0Tf2sHLpGncfYdaq5WaJIufKAP8il4BmHD5Yw4ewmB/Wo0vRQb2bEvx7OqyaPFPZCh0hVhcMKsICZyIBww==",
                }
            );
            assert_eq!(gemini.binary, "gemini");
            assert_eq!(gemini.args, ["--acp"]);
            assert_eq!(gemini.refused_modes, ["yolo"]);
        }
        // qwen is the CLI in ACP mode with yolo refused.
        {
            let qwen = entry("qwen").expect("qwen");
            assert_eq!(
                qwen.install,
                Install::Npm {
                    package: "@qwen-code/qwen-code",
                    version: "0.24.7",
                    integrity: "sha512-NE+Ps9juMJS7DaHEVENIAJWCrzTiO/2bNq2TidmxmFc0ZbSMVKCfX3tlu2RY+CoP8xHFfUcy3MfQDQNszBJpJQ==",
                }
            );
            assert_eq!(qwen.binary, "qwen");
            assert_eq!(qwen.args, ["--acp"]);
            assert_eq!(qwen.refused_modes, ["yolo"]);
        }
        // cline is the CLI in ACP mode with no mode refused.
        {
            let cline = entry("cline").expect("cline");
            assert_eq!(
                cline.install,
                Install::Npm {
                    package: "cline",
                    version: "3.0.67",
                    integrity: "sha512-4aMoB/jww42vDt/9iXqdMbmA7QiVNP5dPLDrz4mt4veKp9Y2DL041RL67PCrEH0TeWsLuAo4nIjEiwsa93TL3A==",
                }
            );
            assert_eq!(cline.binary, "cline");
            assert_eq!(cline.args, ["--acp"]);
            assert!(cline.refused_modes.is_empty());
        }
        // kilo is the CLI in ACP mode with no mode refused.
        {
            let kilo = entry("kilo").expect("kilo");
            assert_eq!(
                kilo.install,
                Install::Npm {
                    package: "@kilocode/cli",
                    version: "7.8.1",
                    integrity: "sha512-3dNWXhFhMxLkTXt8lMJ8fH1ZK8Ws7D6jEipMCmxejkaMNp9hd7FPnjCGUjK6zknnSJZokkA4+guFxoJQ+njYYw==",
                }
            );
            assert_eq!(kilo.binary, "kilo");
            assert_eq!(kilo.args, ["acp"]);
            assert!(kilo.refused_modes.is_empty());
        }
        // copilot is the CLI in ACP mode with its bypass modes refused.
        {
            let copilot = entry("copilot").expect("copilot");
            assert_eq!(
                copilot.install,
                Install::Npm {
                    package: "@github/copilot",
                    version: "1.0.90",
                    integrity: "sha512-XNhA7NscW1D/H537wK+NRuUyWNCqK5JIZZ4aknhibZMiOvRLLbasR4t1QxzSEfr+ZOC6AIY9e6PkGMjgOAN5aQ==",
                }
            );
            assert_eq!(copilot.binary, "copilot");
            assert_eq!(copilot.args, ["--acp", "--stdio"]);
            assert_eq!(
                copilot.refused_modes,
                ["allow-all", "allow_all", "yolo", "autopilot"]
            );
        }
        // vibe is the PyPI install of its CLI in ACP mode with auto-approve and bypass refused.
        {
            let vibe = entry("vibe").expect("vibe");
            assert_eq!(
                vibe.install,
                Install::Pip {
                    package: "mistral-vibe",
                    version: "2.25.0",
                    file: "mistral_vibe-2.25.0-py3-none-any.whl",
                    sha256: "cdedb722d5b8ae6770b56496f6b17677cc2d02705b6e6d5205c0c09167e652fa",
                }
            );
            assert_eq!(vibe.binary, "vibe-acp");
            assert!(vibe.args.is_empty());
            assert_eq!(vibe.refused_modes, ["auto-approve", "bypass"]);
            assert!(vibe.refuses("auto-approve"));
            assert!(vibe.refuses("Bypass_Tool_Permissions"));
            assert!(!vibe.refuses("default"));
        }
    }

    #[test]
    fn refused_modes_match_by_word_in_any_case_and_exactly() {
        // A mode is refused when its id names a refused word in any case.
        {
            let copilot = entry("copilot").expect("copilot");
            assert!(copilot.refuses("allow-all"));
            assert!(copilot.refuses("Allow_All_Tools"));
            assert!(copilot.refuses("autopilot"));
            assert!(!copilot.refuses("ask"));
            let claude = entry("claude").expect("claude");
            assert!(claude.refuses("bypassPermissions"));
            assert!(!claude.refuses("acceptEdits"));
        }
        // kimi is the CLI in ACP mode with yolo refused, and auto matched exactly.
        {
            let kimi = entry("kimi").expect("kimi");
            assert_eq!(
                kimi.install,
                Install::Npm {
                    package: "@moonshot-ai/kimi-code",
                    version: "2.1.1",
                    integrity: "sha512-xClqcnTQUKgKDbeOGPU78qcwxGzL8wA2rqWuOX3CLpyQ1/NhSX7jAh6hZo/JFwThSX8obkgef3uw/li5JaJNyw==",
                }
            );
            assert_eq!(kimi.binary, "kimi");
            assert_eq!(kimi.args, ["acp"]);
            assert!(kimi.refuses("yolo"));
            assert!(kimi.refuses("YOLO"));
            assert!(kimi.refuses("auto"));
            assert!(kimi.refuses("Auto"));
            // The substring of a harmless mode is not the auto mode.
            assert!(!kimi.refuses("autonomous"));
            assert!(!kimi.refuses("auto-edit"));
            assert!(!kimi.refuses("default"));
        }
        // auggie is the CLI in ACP mode with its launch allow rule refused.
        {
            let auggie = entry("auggie").expect("auggie");
            assert_eq!(
                auggie.install,
                Install::Npm {
                    package: "@augmentcode/auggie",
                    version: "0.36.0",
                    integrity: "sha512-jb4kq97pGzG0i5UyISc//LjTVlFzIywwD+VfPemiSIl58KjLfxry3KDgxWYnr1vmV8yUkySVzjLnj4ewUEbT/Q==",
                }
            );
            assert_eq!(auggie.binary, "auggie");
            assert_eq!(auggie.args, ["--acp"]);
            assert!(auggie.refuses("launch-process:allow"));
            assert!(auggie.refuses("Launch-Process:Allow-All"));
            assert!(!auggie.refuses("launch-process:ask"));
        }
    }

    #[test]
    fn every_engine_refuses_the_global_bypass_words_and_keeps_its_ordinary_modes() {
        // The global list applies whatever the engine's own list says: opencode, kilo and cline
        // have none of their own, and still refuse these.
        for engine in ENGINES {
            for mode in [
                "yolo",
                "BypassPermissions",
                "bypass",
                "Full-Access",
                "danger-full-access",
                "autopilot",
                "allow-all",
            ] {
                assert!(engine.refuses(mode), "{} must refuse {mode}", engine.id);
            }
            for mode in ["build", "plan", "default"] {
                assert!(!engine.refuses(mode), "{} must accept {mode}", engine.id);
            }
        }
        for id in ["opencode", "kilo", "cline"] {
            let engine = entry(id).unwrap_or_else(|| panic!("{id} is an engine"));
            assert!(engine.refuses("yolo"), "{id}");
            assert!(engine.refuses("BypassPermissions"), "{id}");
            assert!(!engine.refuses("build"), "{id}");
        }
    }

    #[test]
    fn goose_ships_a_pinned_musl_archive_per_architecture_with_auto_refused() {
        let goose = entry("goose").expect("goose");
        let Install::Release {
            version,
            assets,
            binary_path,
        } = goose.install
        else {
            panic!("goose is a release install");
        };
        assert_eq!(version, "1.52.0");
        assert_eq!(binary_path, "goose");
        assert_eq!(goose.binary, "goose");
        assert_eq!(goose.args, ["acp"]);
        assert_eq!(goose.refused_modes, ["auto"]);
        assert_eq!(
            assets.iter().map(|a| a.arch).collect::<Vec<_>>(),
            ["x86_64", "aarch64"]
        );
        assert_eq!(
            assets[0].sha256,
            "fdc86653285a89f7dcad6e76736af1688ab2cde44661b089328b2fc40bb79f9f"
        );
        assert_eq!(
            assets[1].sha256,
            "4584ba8840fc77c2f3e48758f45543c18f1c1293d704941a8eeb5220852ce653"
        );
    }
}
