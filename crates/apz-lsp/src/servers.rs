//! The W40 languages, the server each one uses (PLAN 15.2's table), and how a path picks its language.
//!
//! A server runs with its working directory at the workspace root. TypeScript and JavaScript share one server.

use std::path::{Path, PathBuf};

use crate::limits::Limits;

/// A language server: the command that runs it, its arguments, the options its `initialize` carries, and its caps.
#[derive(Clone, Copy, Debug)]
pub struct ServerSpec {
    /// Names the server in the pool; languages that share a server share this.
    pub id: &'static str,
    pub command: &'static str,
    pub args: &'static [&'static str],
    /// The `initializationOptions` as JSON text.
    pub init_options: &'static str,
    pub limits: Limits,
}

const GIB: u64 = 1 << 30;
/// Most servers: 4 GiB of address space and two hours of CPU time for the process's life.
const DEFAULT_LIMITS: Limits = Limits {
    address_space: 4 * GIB,
    cpu_seconds: 2 * 3600,
};
/// The JVM servers reserve more address space than the others use.
const JVM_LIMITS: Limits = Limits {
    address_space: 8 * GIB,
    cpu_seconds: 2 * 3600,
};

/// `checkOnSave` off: diagnostics come from the server's own analysis; `cargo check` runs only where a change asks.
pub const RUST_ANALYZER: ServerSpec = ServerSpec {
    id: "rust-analyzer",
    command: "rust-analyzer",
    args: &[],
    init_options: r#"{"checkOnSave": false}"#,
    limits: DEFAULT_LIMITS,
};
pub const GOPLS: ServerSpec = ServerSpec {
    id: "gopls",
    command: "gopls",
    args: &[],
    init_options: "{}",
    limits: DEFAULT_LIMITS,
};
pub const PYRIGHT: ServerSpec = ServerSpec {
    id: "pyright",
    command: "pyright-langserver",
    args: &["--stdio"],
    init_options: "{}",
    limits: DEFAULT_LIMITS,
};
pub const TYPESCRIPT: ServerSpec = ServerSpec {
    id: "typescript-language-server",
    command: "typescript-language-server",
    args: &["--stdio"],
    init_options: "{}",
    limits: DEFAULT_LIMITS,
};
pub const SOURCEKIT_LSP: ServerSpec = ServerSpec {
    id: "sourcekit-lsp",
    command: "sourcekit-lsp",
    args: &[],
    // References come from its index: with background indexing it builds one itself (Swift 6.0 on), with no prior
    // `swift build`. An older server ignores the option.
    init_options: r#"{"backgroundIndexing": true}"#,
    limits: DEFAULT_LIMITS,
};
pub const KOTLIN_LANGUAGE_SERVER: ServerSpec = ServerSpec {
    id: "kotlin-language-server",
    command: "kotlin-language-server",
    args: &[],
    init_options: "{}",
    limits: JVM_LIMITS,
};
pub const JDTLS: ServerSpec = ServerSpec {
    id: "jdtls",
    command: "jdtls",
    args: &[],
    init_options: "{}",
    limits: JVM_LIMITS,
};
/// The plan names the Roslyn language server; `csharp-ls` is the one this table starts until an image ships Roslyn's.
pub const CSHARP: ServerSpec = ServerSpec {
    id: "csharp-ls",
    command: "csharp-ls",
    args: &[],
    init_options: "{}",
    limits: DEFAULT_LIMITS,
};
pub const VSCODE_HTML: ServerSpec = ServerSpec {
    id: "vscode-html-language-server",
    command: "vscode-html-language-server",
    args: &["--stdio"],
    init_options: "{}",
    limits: DEFAULT_LIMITS,
};
pub const VSCODE_CSS: ServerSpec = ServerSpec {
    id: "vscode-css-language-server",
    command: "vscode-css-language-server",
    args: &["--stdio"],
    init_options: "{}",
    limits: DEFAULT_LIMITS,
};
pub const BASH_LANGUAGE_SERVER: ServerSpec = ServerSpec {
    id: "bash-language-server",
    command: "bash-language-server",
    args: &["start"],
    init_options: "{}",
    limits: DEFAULT_LIMITS,
};
/// PowerShell Editor Services is started by its own script; this is the launcher an image provides for it.
pub const POWERSHELL_EDITOR_SERVICES: ServerSpec = ServerSpec {
    id: "powershell-editor-services",
    command: "pses",
    args: &["--stdio"],
    init_options: "{}",
    limits: DEFAULT_LIMITS,
};
pub const YAML_LANGUAGE_SERVER: ServerSpec = ServerSpec {
    id: "yaml-language-server",
    command: "yaml-language-server",
    args: &["--stdio"],
    init_options: "{}",
    limits: DEFAULT_LIMITS,
};
pub const TAPLO: ServerSpec = ServerSpec {
    id: "taplo",
    command: "taplo",
    args: &["lsp", "stdio"],
    init_options: "{}",
    limits: DEFAULT_LIMITS,
};
pub const VSCODE_JSON: ServerSpec = ServerSpec {
    id: "vscode-json-language-server",
    command: "vscode-json-language-server",
    args: &["--stdio"],
    init_options: "{}",
    limits: DEFAULT_LIMITS,
};

/// The languages the code-intelligence tools answer for (W40).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    Go,
    Python,
    TypeScript,
    JavaScript,
    Swift,
    Kotlin,
    Java,
    CSharp,
    Html,
    Css,
    Bash,
    PowerShell,
    Yaml,
    Toml,
    Json,
}

impl Language {
    pub const ALL: [Language; 16] = [
        Language::Rust,
        Language::Go,
        Language::Python,
        Language::TypeScript,
        Language::JavaScript,
        Language::Swift,
        Language::Kotlin,
        Language::Java,
        Language::CSharp,
        Language::Html,
        Language::Css,
        Language::Bash,
        Language::PowerShell,
        Language::Yaml,
        Language::Toml,
        Language::Json,
    ];

    /// The name people and tools use for the language.
    pub fn name(self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::Go => "go",
            Language::Python => "python",
            Language::TypeScript => "typescript",
            Language::JavaScript => "javascript",
            Language::Swift => "swift",
            Language::Kotlin => "kotlin",
            Language::Java => "java",
            Language::CSharp => "csharp",
            Language::Html => "html",
            Language::Css => "css",
            Language::Bash => "bash",
            Language::PowerShell => "powershell",
            Language::Yaml => "yaml",
            Language::Toml => "toml",
            Language::Json => "json",
        }
    }

    /// The language of a path, by its extension (`None` for an extension no language uses).
    pub fn from_path(path: &str) -> Option<Language> {
        Some(match extension(path)?.as_str() {
            "rs" => Language::Rust,
            "go" => Language::Go,
            "py" | "pyi" => Language::Python,
            "ts" | "tsx" | "mts" | "cts" => Language::TypeScript,
            "js" | "jsx" | "mjs" | "cjs" => Language::JavaScript,
            "swift" => Language::Swift,
            "kt" | "kts" => Language::Kotlin,
            "java" => Language::Java,
            "cs" => Language::CSharp,
            "html" | "htm" => Language::Html,
            "css" => Language::Css,
            "sh" | "bash" => Language::Bash,
            "ps1" | "psm1" | "psd1" => Language::PowerShell,
            "yaml" | "yml" => Language::Yaml,
            "toml" => Language::Toml,
            "json" => Language::Json,
            _ => return None,
        })
    }

    /// The language identifier the protocol names a document by, given its path.
    pub fn language_id(self, path: &str) -> &'static str {
        let ext = extension(path).unwrap_or_default();
        match self {
            Language::Rust => "rust",
            Language::Go => "go",
            Language::Python => "python",
            Language::TypeScript if ext == "tsx" => "typescriptreact",
            Language::TypeScript => "typescript",
            Language::JavaScript if ext == "jsx" => "javascriptreact",
            Language::JavaScript => "javascript",
            Language::Swift => "swift",
            Language::Kotlin => "kotlin",
            Language::Java => "java",
            Language::CSharp => "csharp",
            Language::Html => "html",
            Language::Css => "css",
            Language::Bash => "shellscript",
            Language::PowerShell => "powershell",
            Language::Yaml => "yaml",
            Language::Toml => "toml",
            Language::Json => "json",
        }
    }

    /// The server for the language.
    pub fn server(self) -> &'static ServerSpec {
        match self {
            Language::Rust => &RUST_ANALYZER,
            Language::Go => &GOPLS,
            Language::Python => &PYRIGHT,
            Language::TypeScript | Language::JavaScript => &TYPESCRIPT,
            Language::Swift => &SOURCEKIT_LSP,
            Language::Kotlin => &KOTLIN_LANGUAGE_SERVER,
            Language::Java => &JDTLS,
            Language::CSharp => &CSHARP,
            Language::Html => &VSCODE_HTML,
            Language::Css => &VSCODE_CSS,
            Language::Bash => &BASH_LANGUAGE_SERVER,
            Language::PowerShell => &POWERSHELL_EDITOR_SERVICES,
            Language::Yaml => &YAML_LANGUAGE_SERVER,
            Language::Toml => &TAPLO,
            Language::Json => &VSCODE_JSON,
        }
    }
}

/// Whether the language's server is installed here.
pub fn installed(language: Language) -> bool {
    which(language.server().command).is_some()
}

/// The server of the table with the id `id` (the name the pool and a base's warm server use), when the table has one.
pub fn by_id(id: &str) -> Option<&'static ServerSpec> {
    Language::ALL
        .iter()
        .map(|language| language.server())
        .find(|spec| spec.id == id)
}

/// The path of an installed command: searched on `PATH`, or the command itself when it names a path.
pub fn which(command: &str) -> Option<PathBuf> {
    let direct = Path::new(command);
    if direct.components().count() > 1 {
        return runnable(direct).then(|| direct.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .flat_map(|dir| {
            let plain = dir.join(command);
            let exe = dir.join(format!("{command}.exe"));
            [plain, exe]
        })
        .find(|candidate| runnable(candidate))
}

#[cfg(unix)]
fn runnable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn runnable(path: &Path) -> bool {
    path.is_file()
}

/// The lowercase extension of a path's file name, when it has one.
fn extension(path: &str) -> Option<String> {
    let file = path.rsplit(['/', '\\']).next()?;
    let (stem, ext) = file.rsplit_once('.')?;
    (!stem.is_empty()).then(|| ext.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_w40_extension_maps_to_its_language() {
        let cases = [
            ("src/lib.rs", Language::Rust),
            ("main.go", Language::Go),
            ("tool.pyi", Language::Python),
            ("App.TSX", Language::TypeScript),
            ("app.mjs", Language::JavaScript),
            ("Greet.swift", Language::Swift),
            ("build.gradle.kts", Language::Kotlin),
            ("Main.java", Language::Java),
            ("Program.cs", Language::CSharp),
            ("index.html", Language::Html),
            ("app.css", Language::Css),
            ("build.sh", Language::Bash),
            ("Install.ps1", Language::PowerShell),
            ("ci.yml", Language::Yaml),
            ("Cargo.toml", Language::Toml),
            ("package.json", Language::Json),
        ];
        for (path, want) in cases {
            assert_eq!(Language::from_path(path), Some(want), "{path}");
        }
        assert_eq!(Language::from_path("Makefile"), None);
        assert_eq!(Language::from_path(".bashrc"), None);
        assert_eq!(Language::TypeScript.language_id("a.tsx"), "typescriptreact");
    }

    #[test]
    fn typescript_and_javascript_share_one_server_and_rust_checks_nothing_on_save() {
        assert_eq!(
            Language::TypeScript.server().id,
            Language::JavaScript.server().id
        );
        assert_eq!(
            Language::Rust.server().init_options,
            r#"{"checkOnSave": false}"#
        );
        let parsed: serde_json::Value =
            serde_json::from_str(Language::Rust.server().init_options).unwrap();
        assert_eq!(parsed["checkOnSave"], false);
    }

    #[test]
    fn a_missing_command_is_not_installed() {
        assert_eq!(which("apz-lsp-no-such-command-anywhere"), None);
        assert_eq!(which("/definitely/not/here/server"), None);
    }
}
