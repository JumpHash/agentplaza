//! The MCP presets the console offers (docs/design/mcp.md, "Add server", step 1).
//!
//! A preset names a server's container image (stdio) or endpoint URL (http), the env vars and
//! HTTP headers the owner must fill in, and where the docs are. It never holds a secret value:
//! a `secret: true` spec is filled from the vault (binding `mcp:<id>`) by the console.
//!
//! Every image or URL here was checked against an official source (README or Docker Hub page).
//! A server with no official image is listed with `status: "custom image needed"` and `image: None`.

use serde::Serialize;

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

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Preset {
    /// Matches [a-z][a-z0-9-]{1,31}; becomes the server id and the tool prefix mcp.<id>.<tool>.
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// "stdio" (runs in a container) or "http" (a remote Streamable HTTP endpoint).
    pub transport: &'static str,
    /// For stdio: the container image. None when there is no official image (see `status`).
    pub image: Option<&'static str>,
    /// For stdio: the command passed after the image (may be empty).
    pub command: &'static [&'static str],
    /// For http: the endpoint URL (https).
    pub url: Option<&'static str>,
    pub env: &'static [Spec],
    pub headers: &'static [Spec],
    /// Non-secret settings the console fills in for the owner (name, value).
    pub fixed_env: &'static [(&'static str, &'static str)],
    /// Whether the server keeps storage: a docker volume `apz-mcp-<id>` at /data.
    pub volume: bool,
    /// The vendor's or project's documentation URL.
    pub docs: &'static str,
    /// "verified" when the image or URL was confirmed against an official source; "custom image needed" otherwise.
    pub status: &'static str,
    /// One line of notes for the owner (e.g. "needs a Docker image built from the project's Dockerfile").
    pub note: &'static str,
}

/// Every preset, in the order the console lists them.
pub const PRESETS: &[Preset] = &[
    Preset {
        id: "github",
        name: "GitHub",
        description: "GitHub's official server: repositories, issues, pull requests and code search.",
        transport: "stdio",
        image: Some("ghcr.io/github/github-mcp-server"),
        command: &[],
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
        note: "Image and token name from the github/github-mcp-server README (docker run -e GITHUB_PERSONAL_ACCESS_TOKEN ghcr.io/github/github-mcp-server).",
    },
    Preset {
        id: "fetch",
        name: "Fetch",
        description: "Reference server: fetch a web page and convert it to Markdown for the model.",
        transport: "stdio",
        image: Some("mcp/fetch"),
        command: &[],
        url: None,
        env: &[],
        headers: &[],
        fixed_env: &[],
        volume: false,
        docs: "https://github.com/modelcontextprotocol/servers/tree/main/src/fetch",
        status: "verified",
        note: "Image from Docker Hub (mcp/fetch, Docker Inc. build of the reference server). No settings; requests still pass the hub's egress rules.",
    },
    Preset {
        id: "memory",
        name: "Memory",
        description: "Reference server: a knowledge graph the agent can read and write across runs.",
        transport: "stdio",
        image: Some("mcp/memory"),
        command: &[],
        url: None,
        env: &[Spec {
            name: "MEMORY_FILE_PATH",
            secret: false,
            required: false,
            help: "Where the graph is saved, e.g. /tmp/memory.jsonl. The container is read-only except /tmp.",
            format: None,
        }],
        headers: &[],
        fixed_env: &[("MEMORY_FILE_PATH", "/data/memory.json")],
        volume: true,
        docs: "https://github.com/modelcontextprotocol/servers/tree/main/src/memory",
        status: "verified",
        note: "Image from Docker Hub (mcp/memory). The graph is kept in the server's storage volume at /data (memory.json), so it survives restarts.",
    },
    Preset {
        id: "time",
        name: "Time",
        description: "Reference server: current time and time zone conversion.",
        transport: "stdio",
        image: Some("mcp/time"),
        command: &[],
        url: None,
        env: &[],
        headers: &[],
        fixed_env: &[],
        volume: false,
        docs: "https://github.com/modelcontextprotocol/servers/tree/main/src/time",
        status: "verified",
        note: "Image from Docker Hub (mcp/time, Docker Inc. build of the reference server). No settings needed.",
    },
    Preset {
        id: "brave-search",
        name: "Brave Search",
        description: "Brave's official server: web and local search through the Brave Search API.",
        transport: "stdio",
        image: Some("mcp/brave-search"),
        command: &[],
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
        note: "Image and env name from the brave/brave-search-mcp-server README (docker run -e BRAVE_API_KEY mcp/brave-search). The old modelcontextprotocol Brave server is archived.",
    },
    Preset {
        id: "linear",
        name: "Linear",
        description: "Linear's hosted server: find, create and update issues, projects and comments.",
        transport: "http",
        image: None,
        command: &[],
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
        image: None,
        command: &[],
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
            && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
    }

    fn valid_env_name(name: &str) -> bool {
        let b = name.as_bytes();
        !b.is_empty()
            && (b[0].is_ascii_uppercase() || b[0] == b'_')
            && b.iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == b'_')
    }

    #[test]
    fn ids_are_valid_and_unique() {
        let mut seen = HashSet::new();
        for p in PRESETS {
            assert!(valid_id(p.id), "bad id {}", p.id);
            assert!(seen.insert(p.id), "duplicate id {}", p.id);
        }
    }

    #[test]
    fn statuses_and_transports_are_known() {
        for p in PRESETS {
            assert!(p.status == "verified" || p.status == "custom image needed", "{}", p.id);
            assert!(p.transport == "stdio" || p.transport == "http", "{}", p.id);
            assert!(p.docs.starts_with("https://"), "{} docs", p.id);
            assert!(!p.name.is_empty() && !p.description.is_empty() && !p.note.is_empty(), "{}", p.id);
        }
    }

    #[test]
    fn stdio_presets_have_an_image_or_need_one() {
        for p in PRESETS.iter().filter(|p| p.transport == "stdio") {
            assert!(p.image.is_some() || p.status == "custom image needed", "{}", p.id);
            if p.status == "verified" {
                assert!(p.image.is_some(), "{} verified without image", p.id);
            }
            assert!(p.url.is_none(), "{} stdio with url", p.id);
            assert!(p.headers.is_empty(), "{} stdio with headers", p.id);
        }
    }

    #[test]
    fn http_presets_have_an_https_url() {
        for p in PRESETS.iter().filter(|p| p.transport == "http") {
            let url = p.url.unwrap_or_else(|| panic!("{} http without url", p.id));
            assert!(url.starts_with("https://"), "{} url {}", p.id, url);
            assert!(p.image.is_none(), "{} http with image", p.id);
            assert!(p.env.is_empty(), "{} http with env", p.id);
        }
    }

    #[test]
    fn env_and_header_specs_are_well_formed() {
        for p in PRESETS {
            for s in p.env {
                assert!(valid_env_name(s.name), "{} env {}", p.id, s.name);
            }
            for s in p.headers {
                assert!(!s.name.is_empty() && !s.name.contains(' ') && !s.name.contains(':'), "{} header {}", p.id, s.name);
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
