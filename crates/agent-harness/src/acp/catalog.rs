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

/// Every engine the harness knows, in the order they are listed to the model. Gemini CLI is ACP
/// only (`gemini --acp`): it has no headless run, and the owner enables it in `engines_enabled`.
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
}
