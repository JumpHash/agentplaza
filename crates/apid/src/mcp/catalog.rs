//! The MCP presets the console offers (docs/design/mcp.md, "Add server", step 1).
//!
//! A preset names a stdio server's pinned package (npm, PyPI, or a binary by URL and SHA-256) or an http endpoint
//! URL, the env vars and HTTP headers the owner must fill in, and where the docs are. It never holds a secret value:
//! a `secret: true` spec is filled from the vault (binding `mcp:<id>`) by the console.
//!
//! Every version and hash here was read from the publisher's index when it was pinned: npm and PyPI metadata, the
//! GitHub release digests, the Linear and Context7 docs. A binary is pinned per platform; a platform without a pin
//! has no package (`status: "binary needed"`).

use serde::Serialize;
use serde_json::{Value, json};

use super::config::Package;

/// One env var or HTTP header a server needs.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Spec {
    /// The env var name (e.g. GITHUB_PERSONAL_ACCESS_TOKEN) or the header name (e.g. Authorization).
    pub name: &'static str,
    /// True when the value is a secret: it is picked from or stored in the vault.
    pub secret: bool,
    pub required: bool,
    /// One line of help shown in the console.
    pub help: &'static str,
    /// For a header: the value template with `{}` where the secret goes, e.g. "Bearer {}". None means the raw value.
    pub format: Option<&'static str>,
}

/// A binary pinned for one platform: where to download it, its SHA-256, and the file to run inside it.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct BinaryPin {
    /// The platform: `darwin-arm64`, `darwin-x64`, `linux-x64`, `linux-arm64` or `win-x64`.
    pub target: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub entry: &'static str,
}

/// How a stdio preset runs.
#[derive(Debug, Clone, Copy)]
pub enum PresetPackage {
    /// An npm package at an exact version.
    Npm {
        name: &'static str,
        version: &'static str,
    },
    /// A PyPI package at an exact version; its command is the package name.
    Pypi {
        name: &'static str,
        version: &'static str,
    },
    /// A release binary, pinned per platform.
    Binary { pins: &'static [BinaryPin] },
}

#[derive(Debug, Clone, Copy)]
pub struct Preset {
    /// Matches [a-z][a-z0-9-]{1,31}; becomes the server id and the tool prefix mcp__<id>__<tool>.
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// "stdio" (a pinned package run under the sandbox) or "http" (a remote Streamable HTTP endpoint).
    pub transport: &'static str,
    /// For stdio: the package. None for http.
    pub package: Option<PresetPackage>,
    /// For stdio: the arguments after the package's command (may be empty).
    pub args: &'static [&'static str],
    /// For http: the endpoint URL (https).
    pub url: Option<&'static str>,
    pub env: &'static [Spec],
    pub headers: &'static [Spec],
    /// Non-secret settings the console fills in for the owner (name, value). A `{data}` is the server's data
    /// directory, expanded when the server starts.
    pub fixed_env: &'static [(&'static str, &'static str)],
    /// Whether the server keeps its data between runs (its `data/` directory is kept).
    pub volume: bool,
    /// The vendor's or project's documentation URL.
    pub docs: &'static str,
    /// "verified" when the package or URL was confirmed against an official source; "binary needed" when the host's
    /// platform has no pinned binary.
    pub status: &'static str,
    /// One line of notes for the owner.
    pub note: &'static str,
}

impl Preset {
    /// The package this preset runs on `target` (the hub's platform), when it has one there.
    pub fn package_for(&self, target: &str) -> Option<Package> {
        match self.package? {
            PresetPackage::Npm { name, version } => Some(Package::Npm {
                name: name.into(),
                version: version.into(),
            }),
            PresetPackage::Pypi { name, version } => Some(Package::Pypi {
                name: name.into(),
                version: version.into(),
                entry: None,
            }),
            PresetPackage::Binary { pins } => {
                pins.iter()
                    .find(|pin| pin.target == target)
                    .map(|pin| Package::Binary {
                        url: pin.url.into(),
                        sha256: pin.sha256.into(),
                        entry: pin.entry.into(),
                    })
            }
        }
    }

    /// The preset as the console reads it, with the package for `target` (or none).
    pub fn view(&self, target: &str) -> Value {
        let package = self.package_for(target);
        let status = match (self.package, &package) {
            (Some(PresetPackage::Binary { .. }), None) => "binary needed",
            _ => self.status,
        };
        json!({
            "id": self.id,
            "name": self.name,
            "description": self.description,
            "transport": self.transport,
            "package": package,
            "args": self.args,
            "url": self.url,
            "env": self.env,
            "headers": self.headers,
            "fixed_env": self.fixed_env,
            "volume": self.volume,
            "docs": self.docs,
            "status": status,
            "note": self.note,
        })
    }
}

const GITHUB_PINS: &[BinaryPin] = &[
    BinaryPin {
        target: "darwin-arm64",
        url: "https://github.com/github/github-mcp-server/releases/download/v2.0.3/github-mcp-server_Darwin_arm64.tar.gz",
        sha256: "5c95c01f75a7ec2370607e989b2968d314b02a54d78af8a9fdf38f3dfe6135f4",
        entry: "github-mcp-server",
    },
    BinaryPin {
        target: "darwin-x64",
        url: "https://github.com/github/github-mcp-server/releases/download/v2.0.3/github-mcp-server_Darwin_x86_64.tar.gz",
        sha256: "115350858c3287843274efbf45e821d98f15d3304cb2c074740461ed32b45598",
        entry: "github-mcp-server",
    },
    BinaryPin {
        target: "linux-x64",
        url: "https://github.com/github/github-mcp-server/releases/download/v2.0.3/github-mcp-server_Linux_x86_64.tar.gz",
        sha256: "f797c20fe02a755865d0e2d46184ab47e31bf1619b0855b8d89c531e45a651c2",
        entry: "github-mcp-server",
    },
    BinaryPin {
        target: "linux-arm64",
        url: "https://github.com/github/github-mcp-server/releases/download/v2.0.3/github-mcp-server_Linux_arm64.tar.gz",
        sha256: "5d2ddc71273fec8917e8f42cf19f26a6bd7acbb679b3a57de2917018b2eadf82",
        entry: "github-mcp-server",
    },
    BinaryPin {
        target: "win-x64",
        url: "https://github.com/github/github-mcp-server/releases/download/v2.0.3/github-mcp-server_Windows_x86_64.zip",
        sha256: "f0e1b9880a1b88730d1250d1c25cfee39617ec3e800a7a252f2279cd0c8394e0",
        entry: "github-mcp-server.exe",
    },
];

/// Every preset, in the order the console lists them.
pub const PRESETS: &[Preset] = &[
    Preset {
        id: "github",
        name: "GitHub",
        description: "GitHub's official server: repositories, issues, pull requests and code search.",
        transport: "stdio",
        package: Some(PresetPackage::Binary { pins: GITHUB_PINS }),
        args: &["stdio"],
        url: None,
        env: &[
            Spec {
                name: "GITHUB_PERSONAL_ACCESS_TOKEN",
                secret: true,
                required: true,
                help: "A GitHub token; its scopes limit what the agent can do.",
                format: None,
            },
            Spec {
                name: "GITHUB_HOST",
                secret: false,
                required: false,
                help: "Only for GitHub Enterprise Server or ghe.com, e.g. https://ghe.example.com. Leave empty for github.com.",
                format: None,
            },
        ],
        headers: &[],
        fixed_env: &[],
        volume: false,
        docs: "https://github.com/github/github-mcp-server",
        status: "verified",
        note: "The release binary of github/github-mcp-server v2.0.3 for this platform, pinned by its SHA-256 from the release digests, run as `github-mcp-server stdio`. The file name inside the archive and the stdio argument come from the project's README and GoReleaser config; they were not run offline.",
    },
    Preset {
        id: "fetch",
        name: "Fetch",
        description: "Reference server: fetch a web page and convert it to Markdown for the model.",
        transport: "stdio",
        package: Some(PresetPackage::Pypi {
            name: "mcp-server-fetch",
            version: "2026.10.19",
        }),
        args: &[],
        url: None,
        env: &[],
        headers: &[],
        fixed_env: &[],
        volume: false,
        docs: "https://github.com/modelcontextprotocol/servers/tree/main/src/fetch",
        status: "verified",
        note: "PyPI mcp-server-fetch 2026.10.19, run with uv. No settings; requests still pass the hub's egress rules.",
    },
    Preset {
        id: "memory",
        name: "Memory",
        description: "Reference server: a knowledge graph the agent can read and write across runs.",
        transport: "stdio",
        package: Some(PresetPackage::Npm {
            name: "@modelcontextprotocol/server-memory",
            version: "2.0.0",
        }),
        args: &[],
        url: None,
        env: &[Spec {
            name: "MEMORY_FILE_PATH",
            secret: false,
            required: false,
            help: "Where the graph is saved. The default is the server's own directory, which the sandbox keeps read-only, so keep the default path here.",
            format: None,
        }],
        headers: &[],
        fixed_env: &[("MEMORY_FILE_PATH", "{data}/memory.jsonl")],
        volume: true,
        docs: "https://github.com/modelcontextprotocol/servers/tree/main/src/memory",
        status: "verified",
        note: "npm @modelcontextprotocol/server-memory 2.0.0. The graph is kept in the server's data directory (memory.jsonl), so it survives restarts.",
    },
    Preset {
        id: "time",
        name: "Time",
        description: "Reference server: current time and time zone conversion.",
        transport: "stdio",
        package: Some(PresetPackage::Pypi {
            name: "mcp-server-time",
            version: "2026.10.19",
        }),
        args: &[],
        url: None,
        env: &[],
        headers: &[],
        fixed_env: &[],
        volume: false,
        docs: "https://github.com/modelcontextprotocol/servers/tree/main/src/time",
        status: "verified",
        note: "PyPI mcp-server-time 2026.10.19, run with uv. No settings needed.",
    },
    Preset {
        id: "brave-search",
        name: "Brave Search",
        description: "Brave's official server: web and local search through the Brave Search API.",
        transport: "stdio",
        package: Some(PresetPackage::Npm {
            name: "@brave/brave-search-mcp-server",
            version: "2.1.4",
        }),
        args: &[],
        url: None,
        env: &[Spec {
            name: "BRAVE_API_KEY",
            secret: true,
            required: true,
            help: "Brave Search API key from brave.com/search/api.",
            format: None,
        }],
        headers: &[],
        fixed_env: &[],
        volume: false,
        docs: "https://github.com/brave/brave-search-mcp-server",
        status: "verified",
        note: "npm @brave/brave-search-mcp-server 2.1.4 (its README: stdio is the default transport, BRAVE_API_KEY is the key). The old modelcontextprotocol Brave server is archived.",
    },
    Preset {
        id: "linear",
        name: "Linear",
        description: "Linear's hosted server: find, create and update issues, projects and comments.",
        transport: "http",
        package: None,
        args: &[],
        url: Some("https://mcp.linear.app/mcp"),
        env: &[],
        headers: &[Spec {
            name: "Authorization",
            secret: true,
            required: true,
            help: "A Linear API key (Settings, Account, Security & Access) or an OAuth access token.",
            format: Some("Bearer {}"),
        }],
        fixed_env: &[],
        volume: false,
        docs: "https://linear.app/docs/mcp",
        status: "verified",
        note: "Linear's default flow is interactive OAuth, which the hub cannot run; the Authorization header takes an API key instead. For read-only access use a key with only the Read permission.",
    },
    Preset {
        id: "context7",
        name: "Context7",
        description: "Upstash's hosted server: current library documentation and code examples for prompts.",
        transport: "http",
        package: None,
        args: &[],
        url: Some("https://mcp.context7.com/mcp"),
        env: &[],
        headers: &[Spec {
            name: "Authorization",
            secret: true,
            required: false,
            help: "Optional Context7 API key from context7.com/dashboard; a key raises the rate limits.",
            format: Some("Bearer {}"),
        }],
        fixed_env: &[],
        volume: false,
        docs: "https://github.com/upstash/context7",
        status: "verified",
        note: "Endpoint and Authorization Bearer header from the upstash/context7 README. The key is optional.",
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn valid_id(id: &str) -> bool {
        let b = id.as_bytes();
        (2..=32).contains(&b.len())
            && b[0].is_ascii_lowercase()
            && b.iter()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
    }

    fn valid_env_name(name: &str) -> bool {
        let b = name.as_bytes();
        !b.is_empty()
            && (b[0].is_ascii_uppercase() || b[0] == b'_')
            && b.iter()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b'_')
    }

    fn sha256_hex(text: &str) -> bool {
        text.len() == 64
            && text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }

    /// Presets have valid unique ids, known statuses and transports, docs links and copy; the transport
    /// fields match the transport; and env and header specs are well formed.
    #[test]
    fn preset_catalog_is_well_formed() {
        // Ids are valid and unique, statuses and transports are known, docs link to https, and the copy is filled in.
        {
            let mut seen = HashSet::new();
            for p in PRESETS {
                assert!(valid_id(p.id), "bad id {}", p.id);
                assert!(seen.insert(p.id), "duplicate id {}", p.id);
                assert!(p.status == "verified", "{}", p.id);
                assert!(p.transport == "stdio" || p.transport == "http", "{}", p.id);
                assert!(p.docs.starts_with("https://"), "{} docs", p.id);
                assert!(
                    !p.name.is_empty() && !p.description.is_empty() && !p.note.is_empty(),
                    "{}",
                    p.id
                );
            }
        }
        // stdio presets have a package and no url or headers; http presets have an https url and no package or env.
        {
            for p in PRESETS {
                if p.transport == "stdio" {
                    assert!(p.package.is_some(), "{} stdio without a package", p.id);
                    assert!(p.url.is_none(), "{} stdio with url", p.id);
                    assert!(p.headers.is_empty(), "{} stdio with headers", p.id);
                } else {
                    let url = p.url.unwrap_or_else(|| panic!("{} http without url", p.id));
                    assert!(url.starts_with("https://"), "{} url {}", p.id, url);
                    assert!(p.package.is_none(), "{} http with a package", p.id);
                    assert!(p.env.is_empty(), "{} http with env", p.id);
                }
            }
        }
        // Every pinned version is exact, every binary hash is SHA-256 hex, and a binary has a pin for each platform.
        {
            for p in PRESETS {
                match p.package {
                    Some(
                        PresetPackage::Npm { version, .. } | PresetPackage::Pypi { version, .. },
                    ) => {
                        assert!(
                            version.starts_with(|c: char| c.is_ascii_digit()),
                            "{}",
                            p.id
                        );
                        assert!(!version.contains(['^', '~', '>', '<', '*']), "{}", p.id);
                    }
                    Some(PresetPackage::Binary { pins }) => {
                        for pin in pins {
                            assert!(pin.url.starts_with("https://"), "{} {}", p.id, pin.target);
                            assert!(sha256_hex(pin.sha256), "{} {}", p.id, pin.target);
                        }
                        for target in [
                            "darwin-arm64",
                            "darwin-x64",
                            "linux-x64",
                            "linux-arm64",
                            "win-x64",
                        ] {
                            assert!(
                                pins.iter().any(|pin| pin.target == target),
                                "{} {target}",
                                p.id
                            );
                        }
                    }
                    None => {}
                }
            }
        }
        // Env names are valid, header names are plain, every spec has help, and formats hold a placeholder.
        {
            for p in PRESETS {
                for s in p.env {
                    assert!(valid_env_name(s.name), "{} env {}", p.id, s.name);
                }
                for s in p.headers {
                    assert!(
                        !s.name.is_empty() && !s.name.contains(' ') && !s.name.contains(':'),
                        "{} header {}",
                        p.id,
                        s.name
                    );
                }
                for s in p.env.iter().chain(p.headers.iter()) {
                    assert!(!s.help.is_empty(), "{} spec {} has no help", p.id, s.name);
                    if let Some(f) = s.format {
                        assert!(f.contains("{}"), "{} format {}", p.id, f);
                    }
                }
            }
        }
    }

    #[test]
    fn a_binary_preset_has_a_package_only_on_its_pinned_platforms() {
        let github = PRESETS.iter().find(|p| p.id == "github").unwrap();
        assert!(github.package_for("linux-x64").is_some());
        assert!(github.package_for("freebsd-x64").is_none());
        let view = github.view("freebsd-x64");
        assert_eq!(view["status"], "binary needed");
        let memory = PRESETS.iter().find(|p| p.id == "memory").unwrap();
        assert_eq!(memory.view("linux-x64")["package"]["kind"], "npm");
    }
}
