//! The engines `code_engine` can run over ACP: which npm package provides each one's ACP adapter,
//! at which pinned version, how it starts, and how it is told its model.
//!
//! Versions are pinned (never `latest`) with the registry's sha512 integrity, and installed in the
//! VM on first use under `/workspace/.npm-global`, like the headless engines. A version bump is a
//! code change, re-checked for which of the adapter's tools ask permission before they act.

/// How an engine learns the model the hub allowed for the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelVia {
    /// The ACP session config option `model` (`session/set_config_option`), after `session/new`.
    ConfigOption,
}

/// One engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// The name the model uses in `code_engine{engine}`.
    pub id: &'static str,
    /// The npm package of the ACP adapter.
    pub package: &'static str,
    /// The pinned version of that package.
    pub version: &'static str,
    /// The registry's `dist.integrity` for that version, checked after the install.
    pub integrity: &'static str,
    /// The command the package installs.
    pub binary: &'static str,
    /// The arguments that start the adapter as an ACP agent on its stdio.
    pub args: &'static [&'static str],
    pub model: ModelVia,
    /// Session modes the harness never selects, whatever the engine offers (its bypass modes).
    pub refused_modes: &'static [&'static str],
}

/// Every engine the harness knows, in the order they are listed to the model. Gemini CLI, Qwen Code,
/// Kilo Code and Cline are ACP only (`gemini --acp`, `qwen --acp`, `kilo acp`, `cline --acp`): they
/// have no headless run, and the owner enables them in `engines_enabled`.
pub const ENGINES: &[Entry] = &[
    Entry {
        id: "claude",
        package: "@agentclientprotocol/claude-agent-acp",
        version: "0.84.0",
        integrity: "sha512-Zhjyxvm7USDB/BAFx2L6U6rA3spJ6qwEDFbLgByzpmQMeZGuAlZPXhMcAc+Xsndj9kDh+OFJNjETRwGJCR1eTQ==",
        binary: "claude-agent-acp",
        args: &[],
        model: ModelVia::ConfigOption,
        refused_modes: &["bypassPermissions"],
    },
    Entry {
        id: "codex",
        package: "@agentclientprotocol/codex-acp",
        version: "2.0.1",
        integrity: "sha512-P144H9s2hUVvVDDgnL1+8u7VJu1OIBxHlb81VCxIBQM/M2D0JAepMufEW7jTKK4AML0TOHL+G1t5RqEt/t1fFA==",
        binary: "codex-acp",
        args: &[],
        model: ModelVia::ConfigOption,
        refused_modes: &["full-access", "danger-full-access"],
    },
    Entry {
        id: "opencode",
        package: "opencode-ai",
        version: "1.18.34",
        integrity: "sha512-9WUS2T0t4HHDVzXvuwTHF0nvhXvZ9mQ0r+ozCvKdJu0LVoQpOzqAW4qWTC3ygNc5Oedcc7j+Oct24JYlQYnySA==",
        binary: "opencode",
        args: &["acp"],
        model: ModelVia::ConfigOption,
        refused_modes: &[],
    },
    Entry {
        id: "gemini",
        package: "@google/gemini-cli",
        version: "0.62.0",
        integrity: "sha512-A1rw0Tf2sHLpGncfYdaq5WaJIufKAP8il4BmHD5Yw4ewmB/Wo0vRQb2bEvx7OqyaPFPZCh0hVhcMKsICZyIBww==",
        binary: "gemini",
        args: &["--acp"],
        // Gemini CLI takes its model from the environment (`GEMINI_MODEL`), not the session, so the
        // launch names no model for it (`engines::acp_launch`).
        model: ModelVia::ConfigOption,
        refused_modes: &["yolo"],
    },
    Entry {
        id: "qwen",
        package: "@qwen-code/qwen-code",
        version: "0.24.7",
        integrity: "sha512-NE+Ps9juMJS7DaHEVENIAJWCrzTiO/2bNq2TidmxmFc0ZbSMVKCfX3tlu2RY+CoP8xHFfUcy3MfQDQNszBJpJQ==",
        binary: "qwen",
        args: &["--acp"],
        // Qwen Code takes its model from the environment (`OPENAI_MODEL`), not the session, so the
        // launch names no model for it (`engines::acp_launch`).
        model: ModelVia::ConfigOption,
        refused_modes: &["yolo"],
    },
    Entry {
        id: "kilo",
        package: "@kilocode/cli",
        version: "7.8.1",
        integrity: "sha512-3dNWXhFhMxLkTXt8lMJ8fH1ZK8Ws7D6jEipMCmxejkaMNp9hd7FPnjCGUjK6zknnSJZokkA4+guFxoJQ+njYYw==",
        binary: "kilo",
        args: &["acp"],
        model: ModelVia::ConfigOption,
        refused_modes: &[],
    },
    Entry {
        id: "cline",
        package: "cline",
        version: "3.0.67",
        integrity: "sha512-4aMoB/jww42vDt/9iXqdMbmA7QiVNP5dPLDrz4mt4veKp9Y2DL041RL67PCrEH0TeWsLuAo4nIjEiwsa93TL3A==",
        binary: "cline",
        args: &["--acp"],
        // Cline takes its model from its saved provider settings, which `engines::setup_args` writes
        // before each call, so the session is given none.
        model: ModelVia::ConfigOption,
        refused_modes: &[],
    },
];

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
                    .version
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit()),
                "{} is pinned to a version, not a tag",
                engine.id
            );
            assert!(engine.integrity.starts_with("sha512-"), "{}", engine.id);
            assert_eq!(entry(engine.id), Some(engine));
        }
        assert_eq!(entry("bash"), None);
    }

    #[test]
    fn gemini_is_the_cli_in_acp_mode_with_yolo_refused() {
        let gemini = entry("gemini").expect("gemini");
        assert_eq!(gemini.package, "@google/gemini-cli");
        assert_eq!(gemini.version, "0.62.0");
        assert_eq!(gemini.binary, "gemini");
        assert_eq!(gemini.args, ["--acp"]);
        assert_eq!(gemini.refused_modes, ["yolo"]);
        assert_eq!(
            gemini.integrity,
            "sha512-A1rw0Tf2sHLpGncfYdaq5WaJIufKAP8il4BmHD5Yw4ewmB/Wo0vRQb2bEvx7OqyaPFPZCh0hVhcMKsICZyIBww=="
        );
    }

    #[test]
    fn qwen_is_the_cli_in_acp_mode_with_yolo_refused() {
        let qwen = entry("qwen").expect("qwen");
        assert_eq!(qwen.package, "@qwen-code/qwen-code");
        assert_eq!(qwen.version, "0.24.7");
        assert_eq!(qwen.binary, "qwen");
        assert_eq!(qwen.args, ["--acp"]);
        assert_eq!(qwen.refused_modes, ["yolo"]);
        assert_eq!(
            qwen.integrity,
            "sha512-NE+Ps9juMJS7DaHEVENIAJWCrzTiO/2bNq2TidmxmFc0ZbSMVKCfX3tlu2RY+CoP8xHFfUcy3MfQDQNszBJpJQ=="
        );
    }

    #[test]
    fn cline_is_the_cli_in_acp_mode_with_no_mode_refused() {
        let cline = entry("cline").expect("cline");
        assert_eq!(cline.package, "cline");
        assert_eq!(cline.version, "3.0.67");
        assert_eq!(cline.binary, "cline");
        assert_eq!(cline.args, ["--acp"]);
        assert!(cline.refused_modes.is_empty());
        assert_eq!(
            cline.integrity,
            "sha512-4aMoB/jww42vDt/9iXqdMbmA7QiVNP5dPLDrz4mt4veKp9Y2DL041RL67PCrEH0TeWsLuAo4nIjEiwsa93TL3A=="
        );
    }

    #[test]
    fn kilo_is_the_cli_in_acp_mode_with_no_mode_refused() {
        let kilo = entry("kilo").expect("kilo");
        assert_eq!(kilo.package, "@kilocode/cli");
        assert_eq!(kilo.version, "7.8.1");
        assert_eq!(kilo.binary, "kilo");
        assert_eq!(kilo.args, ["acp"]);
        assert!(kilo.refused_modes.is_empty());
        assert_eq!(
            kilo.integrity,
            "sha512-3dNWXhFhMxLkTXt8lMJ8fH1ZK8Ws7D6jEipMCmxejkaMNp9hd7FPnjCGUjK6zknnSJZokkA4+guFxoJQ+njYYw=="
        );
    }
}
