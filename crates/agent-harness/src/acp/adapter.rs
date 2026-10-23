//! The adapters of the pinned engines (PLAN 12.2): what each engine reads from its folder, and what the
//! plaza renders for it. The session's prompt parts (PLAN 13.3) become the engine's instruction file in
//! its working folder before the engine starts, and that file is removed when the session ends. What an
//! engine can do is declared here for the version the catalogue pins; the plaza relies on what the table
//! declares and on nothing else.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// What the plaza relies on in one pinned engine. A capability the table does not declare is not used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// The engine's id (see [`catalog::ENGINES`]).
    pub engine: &'static str,
    /// The version the catalogue pins; the table was written against it.
    pub version: &'static str,
    /// The instruction file the plaza writes for the engine, which the engine reads from its working folder.
    pub instructions: Option<&'static str>,
    /// Whether the engine reads skills from its home's skills folder (a folder per skill, with SKILL.md). The
    /// plaza serves skills over MCP to every engine; it renders no native folder yet (no skill files reach here).
    pub native_skills: bool,
    /// Whether the engine runs hooks from its settings (PLAN 14.3). Not rendered yet: hooks (PLAN 1.7) are not built.
    pub hooks: bool,
}

/// The capabilities of every engine the catalogue pins. Claude Code reads CLAUDE.md, Gemini CLI GEMINI.md,
/// Codex and OpenCode AGENTS.md; the others get no instruction file from the plaza.
const TABLE: &[Capabilities] = &[
    Capabilities {
        engine: "claude",
        version: "0.84.0",
        instructions: Some("CLAUDE.md"),
        native_skills: true,
        hooks: true,
    },
    Capabilities {
        engine: "codex",
        version: "2.0.1",
        instructions: Some("AGENTS.md"),
        native_skills: false,
        hooks: false,
    },
    Capabilities {
        engine: "opencode",
        version: "1.18.34",
        instructions: Some("AGENTS.md"),
        native_skills: false,
        hooks: false,
    },
    Capabilities {
        engine: "gemini",
        version: "0.62.0",
        instructions: Some("GEMINI.md"),
        native_skills: false,
        hooks: false,
    },
    Capabilities {
        engine: "qwen",
        version: "0.24.7",
        instructions: None,
        native_skills: false,
        hooks: false,
    },
    Capabilities {
        engine: "kilo",
        version: "7.8.1",
        instructions: None,
        native_skills: false,
        hooks: false,
    },
    Capabilities {
        engine: "cline",
        version: "3.0.67",
        instructions: None,
        native_skills: false,
        hooks: false,
    },
    Capabilities {
        engine: "copilot",
        version: "1.0.90",
        instructions: None,
        native_skills: false,
        hooks: false,
    },
    Capabilities {
        engine: "goose",
        version: "1.52.0",
        instructions: None,
        native_skills: false,
        hooks: false,
    },
    Capabilities {
        engine: "kimi",
        version: "2.1.1",
        instructions: None,
        native_skills: false,
        hooks: false,
    },
    Capabilities {
        engine: "auggie",
        version: "0.36.0",
        instructions: None,
        native_skills: false,
        hooks: false,
    },
    Capabilities {
        engine: "vibe",
        version: "2.25.0",
        instructions: None,
        native_skills: false,
        hooks: false,
    },
];

/// The capabilities of engine `id`, or `None` for an engine the table does not declare.
pub fn capabilities(id: &str) -> Option<&'static Capabilities> {
    TABLE.iter().find(|caps| caps.engine == id)
}

/// The prompt parts as an instruction file's text: each part trimmed, the empty ones dropped, the rest
/// in order and separated by a blank line, with a final newline. Empty when there is no text.
pub fn render(parts: &[String]) -> String {
    let mut text = String::new();
    for part in parts
        .iter()
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
    {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(part);
    }
    if !text.is_empty() {
        text.push('\n');
    }
    text
}

/// An instruction file the plaza wrote into an engine's working folder. It is removed when this is dropped,
/// so it lasts as long as the session. A file the folder already has is never replaced: then nothing is
/// written and there is nothing to remove.
#[derive(Debug)]
pub struct Instructions {
    path: PathBuf,
}

impl Instructions {
    /// Writes the instruction file of `engine` into `dir` from `parts`. `Ok(None)` when nothing is written:
    /// the engine reads no file the plaza knows, the parts are empty, or the folder has a file of that name.
    pub fn write(dir: &Path, engine: &str, parts: &[String]) -> io::Result<Option<Self>> {
        let Some(name) = capabilities(engine).and_then(|caps| caps.instructions) else {
            return Ok(None);
        };
        let text = render(parts);
        if text.is_empty() {
            return Ok(None);
        }
        let path = dir.join(name);
        // Created new, so a file that appears meanwhile is kept, not overwritten.
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => return Ok(None),
            Err(err) => return Err(err),
        };
        let guard = Self { path };
        file.write_all(text.as_bytes())?;
        Ok(Some(guard))
    }

    /// The file this wrote.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Instructions {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::catalog::{self, ENGINES};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "apz-harness-acp-adapter-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    #[test]
    fn every_pinned_engine_has_capabilities_for_the_version_it_is_pinned_at() {
        for entry in ENGINES {
            let caps = capabilities(entry.id)
                .unwrap_or_else(|| panic!("{} has no capabilities", entry.id));
            assert_eq!(caps.version, entry.install.version(), "{}", entry.id);
        }
        assert_eq!(TABLE.len(), ENGINES.len(), "one row per engine, no more");
        assert_eq!(
            capabilities("claude").and_then(|c| c.instructions),
            Some("CLAUDE.md")
        );
        assert_eq!(
            capabilities("codex").and_then(|c| c.instructions),
            Some("AGENTS.md")
        );
        assert!(capabilities("nope").is_none());
    }

    #[test]
    fn the_parts_render_in_order_trimmed_and_without_the_empty_ones() {
        let parts = [
            "  Be brief.  ".to_string(),
            String::new(),
            "\n".to_string(),
            "Run the tests.".to_string(),
        ];
        assert_eq!(render(&parts), "Be brief.\n\nRun the tests.\n");
        assert_eq!(render(&[" ".to_string()]), "");
    }

    #[test]
    fn an_instruction_file_lasts_as_long_as_its_guard() {
        let dir = scratch("written");
        let parts = ["Be brief.".to_string()];
        let guard = Instructions::write(&dir, "claude", &parts)
            .expect("written")
            .expect("a file");
        let path = guard.path().to_path_buf();
        assert_eq!(path, dir.join("CLAUDE.md"));
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "Be brief.\n");
        drop(guard);
        assert!(!path.exists(), "removed when the session ends");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folders_own_instruction_file_is_kept_and_nothing_is_written() {
        let dir = scratch("kept");
        std::fs::write(dir.join("AGENTS.md"), "the project's own").expect("the file");
        let parts = ["Be brief.".to_string()];
        assert!(
            Instructions::write(&dir, "codex", &parts)
                .expect("no error")
                .is_none()
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("AGENTS.md")).expect("read"),
            "the project's own"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_is_written_for_no_text_or_an_engine_with_no_file_the_plaza_knows() {
        let dir = scratch("none");
        assert!(
            Instructions::write(&dir, "claude", &[])
                .expect("ok")
                .is_none()
        );
        assert!(
            Instructions::write(&dir, "claude", &[" ".to_string()])
                .expect("ok")
                .is_none()
        );
        let parts = ["Be brief.".to_string()];
        assert!(
            Instructions::write(&dir, "vibe", &parts)
                .expect("ok")
                .is_none()
        );
        assert!(
            Instructions::write(&dir, "unknown", &parts)
                .expect("ok")
                .is_none()
        );
        assert_eq!(std::fs::read_dir(&dir).expect("the folder").count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The scripted session of each engine (PLAN 12.5) needs the engine's pinned package installed. Where it is
    /// not, the test says so and skips: the packages are never vendored, nor downloaded from inside a test.
    #[test]
    fn a_scripted_session_per_engine_runs_where_the_engines_package_is_installed() {
        for engine in ["claude", "codex"] {
            let entry = catalog::entry(engine).expect("a pinned engine");
            let installed = Path::new(crate::engines::NPM_PREFIX)
                .join("bin")
                .join(entry.binary)
                .exists();
            if !installed {
                eprintln!(
                    "skipped the {engine} scripted session: {} is not installed here and no npm or network is available to install it",
                    entry.binary
                );
                continue;
            }
            // Installed: the session replays its script through the mock model (`script`, APZ_MOCK_SCRIPT), where
            // the engine calls a Weft tool and a code-intelligence tool, sees an owner MCP tool, reads its
            // instruction file and writes a handover.
            panic!("the {engine} scripted session is not wired yet");
        }
    }
}
