//! Artifacts: what an agent shows of its work. An artifact is a title and one or more [`Panel`]s on the same
//! 12-column grid Metrics uses; one panel is a chart or a table, several make a dashboard. The hub validates every
//! spec with [`validate`] on create and on each new version, and refuses with problems the agent can act on.

use serde::{Deserialize, Serialize};

use crate::libs;
use crate::model::{Body, HtmlBody, Panel};

/// What an agent sends to `artifact_create` and `artifact_update`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Spec {
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sub: String,
    pub panels: Vec<Panel>,
}

/// An artifact as the hub stores and serves it (`GET /v1/artifacts/{id}`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    /// 1 for the first version; each update adds one.
    pub version: u32,
    /// The newest version (equal to `version` when this is the newest).
    pub latest: u32,
    pub title: String,
    #[serde(default)]
    pub sub: String,
    pub panels: Vec<Panel>,
    /// [`kind_of`] the panels: `chart`, `table`, `dashboard`, `html` …
    pub kind: String,
    /// The agent that made it (`a12 · Ada`), or `owner`.
    pub author: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub created_ms: u64,
    pub updated_ms: u64,
    /// Bytes across the spec and its blobs.
    #[serde(default)]
    pub bytes: u64,
}

/// One line of `GET /v1/artifacts`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub latest: u32,
    pub author: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub updated_ms: u64,
    pub bytes: u64,
    /// Whether a share link is live for it.
    #[serde(default)]
    pub shared: bool,
}

/// The limits every artifact is held to.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Caps {
    pub panels: usize,
    pub title: usize,
    pub series: usize,
    pub points: usize,
    pub table_rows: usize,
    pub inline_rows: usize,
    pub columns: usize,
    pub nodes: usize,
    pub edges: usize,
    pub text: usize,
    pub html_files: usize,
    pub html_bytes: usize,
    /// The whole spec as JSON.
    pub spec_bytes: usize,
    /// One blob.
    pub blob_bytes: u64,
}

impl Default for Caps {
    fn default() -> Self {
        Caps {
            panels: 24,
            title: 120,
            series: 24,
            points: 5_000,
            table_rows: 1_000,
            inline_rows: 2_000,
            columns: 64,
            nodes: 1_000,
            edges: 4_000,
            text: 400_000,
            html_files: 48,
            html_bytes: 2 * 1024 * 1024,
            spec_bytes: 4 * 1024 * 1024,
            blob_bytes: 256 * 1024 * 1024,
        }
    }
}

/// What kind of thing the panels make, for the Library and the embed's label.
pub fn kind_of(panels: &[Panel]) -> &'static str {
    if panels.len() > 1 {
        return "dashboard";
    }
    match panels.first().map(|p| &p.body) {
        Some(Body::Chart(_) | Body::Heatmap(_) | Body::Treemap { .. } | Body::Bars { .. } | Body::Split { .. }) => {
            "chart"
        }
        Some(Body::Table(_)) => "table",
        Some(Body::Dataset(_)) => "dataset",
        Some(Body::Image(_)) => "image",
        Some(Body::Graph(_)) => "graph",
        Some(Body::Document { .. }) => "document",
        Some(Body::Code(_)) => "code",
        Some(Body::Html(_)) => "html",
        Some(Body::Tiles { .. } | Body::Gauges { .. } | Body::Stats { .. }) => "figures",
        Some(_) => "panel",
        None => "empty",
    }
}

/// Checks a spec against the caps. Each problem names the panel and says what to change.
pub fn validate(spec: &Spec, caps: &Caps) -> Vec<String> {
    let mut out = vec![];
    if spec.title.trim().is_empty() {
        out.push("title is empty: give the artifact a short title".into());
    }
    if spec.title.chars().count() > caps.title {
        out.push(format!("title is longer than {} characters", caps.title));
    }
    if spec.panels.is_empty() {
        out.push("panels is empty: an artifact needs at least one panel".into());
    }
    if spec.panels.len() > caps.panels {
        out.push(format!("{} panels; at most {}", spec.panels.len(), caps.panels));
    }
    if let Ok(json) = serde_json::to_vec(spec)
        && json.len() > caps.spec_bytes
    {
        out.push(format!(
            "the spec is {} bytes; at most {}: move rows or files into data with artifact_attach_data",
            json.len(),
            caps.spec_bytes
        ));
    }
    let mut ids = std::collections::BTreeSet::new();
    for (i, p) in spec.panels.iter().enumerate() {
        let at = if p.id.is_empty() { format!("panel {}", i + 1) } else { format!("panel {:?}", p.id) };
        if !p.id.is_empty() && !ids.insert(p.id.clone()) {
            out.push(format!("{at}: the id is used twice"));
        }
        if ![3u8, 4, 6, 8, 12].contains(&p.span) {
            out.push(format!("{at}: span {} is not 3, 4, 6, 8 or 12", p.span));
        }
        body_problems(&p.body, caps, &at, &mut out);
    }
    out
}

fn body_problems(body: &Body, caps: &Caps, at: &str, out: &mut Vec<String>) {
    match body {
        Body::Chart(c) => {
            if c.series.len() > caps.series {
                out.push(format!("{at}: {} series; at most {}", c.series.len(), caps.series));
            }
            for s in &c.series {
                if s.values.len() > caps.points {
                    out.push(format!(
                        "{at}: series {:?} has {} points; at most {} (downsample it)",
                        s.name,
                        s.values.len(),
                        caps.points
                    ));
                }
            }
        }
        Body::Table(t) if t.rows.len() > caps.table_rows => {
            out.push(format!(
                "{at}: {} table rows; at most {} (use a dataset body for more)",
                t.rows.len(),
                caps.table_rows
            ));
        }
        Body::Dataset(d) => {
            if d.columns.is_empty() {
                out.push(format!("{at}: a dataset needs columns"));
            }
            if d.columns.len() > caps.columns {
                out.push(format!("{at}: {} columns; at most {}", d.columns.len(), caps.columns));
            }
            if d.rows.len() > caps.inline_rows {
                out.push(format!(
                    "{at}: {} inline rows; at most {}: attach the rows as CSV with artifact_attach_data and set blob",
                    d.rows.len(),
                    caps.inline_rows
                ));
            }
            if let Some(bad) = d.rows.iter().position(|r| r.len() != d.columns.len()) {
                out.push(format!(
                    "{at}: row {} has {} values for {} columns",
                    bad + 1,
                    d.rows[bad].len(),
                    d.columns.len()
                ));
            }
            if d.rows.is_empty() && d.blob.is_none() {
                out.push(format!("{at}: a dataset needs rows or a blob"));
            }
        }
        Body::Image(img) => {
            if img.alt.trim().is_empty() {
                out.push(format!("{at}: an image needs alt text"));
            }
            if !img.blob.content_type.is_empty() && !img.blob.content_type.starts_with("image/") {
                out.push(format!("{at}: the image blob is {}, not an image", img.blob.content_type));
            }
            if img.blob.id.is_empty() {
                out.push(format!("{at}: the image has no blob: attach it with artifact_attach_data first"));
            }
        }
        Body::Graph(g) => {
            if g.nodes.len() > caps.nodes {
                out.push(format!("{at}: {} nodes; at most {}", g.nodes.len(), caps.nodes));
            }
            if g.edges.len() > caps.edges {
                out.push(format!("{at}: {} edges; at most {}", g.edges.len(), caps.edges));
            }
            let ids: std::collections::BTreeSet<&str> = g.nodes.iter().map(|n| n.id.as_str()).collect();
            if ids.len() != g.nodes.len() {
                out.push(format!("{at}: node ids must be unique"));
            }
            for e in &g.edges {
                for end in [&e.from, &e.to] {
                    if !ids.contains(end.as_str()) {
                        out.push(format!("{at}: an edge names node {end:?}, which is not in nodes"));
                    }
                }
            }
        }
        Body::Document { markdown } if markdown.len() > caps.text => {
            out.push(format!("{at}: the document is {} bytes; at most {}", markdown.len(), caps.text));
        }
        Body::Code(c) if c.text.len() > caps.text => {
            out.push(format!("{at}: the code is {} bytes; at most {}", c.text.len(), caps.text));
        }
        Body::Html(h) => out.extend(html_problems(h, caps).into_iter().map(|p| format!("{at}: {p}"))),
        _ => {}
    }
}

/// Whether a path inside an HTML artifact is safe: relative, no `..`, no backslashes, no leading `/` or `_lib`.
pub fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 200
        && !path.starts_with('/')
        && !path.starts_with("_lib")
        && !path.contains('\\')
        && !path.contains("//")
        && path.split('/').all(|seg| !seg.is_empty() && seg != "." && seg != "..")
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '/'))
}

/// The extensions an HTML artifact's code files use.
pub fn is_script(path: &str) -> bool {
    [".js", ".mjs", ".jsx", ".ts", ".tsx"].iter().any(|e| path.ends_with(e))
}

/// Problems with an HTML body: paths, sizes, the entry, network loads and imports outside the vendored set.
pub fn html_problems(h: &HtmlBody, caps: &Caps) -> Vec<String> {
    let mut out = vec![];
    if h.files.is_empty() {
        out.push("an html body needs files".into());
    }
    if h.files.len() > caps.html_files {
        out.push(format!("{} files; at most {}", h.files.len(), caps.html_files));
    }
    let total: usize = h.files.iter().map(|f| f.text.as_ref().map_or(0, String::len)).sum();
    if total > caps.html_bytes {
        out.push(format!("the files are {total} bytes; at most {}", caps.html_bytes));
    }
    if !h.files.iter().any(|f| f.path == h.entry) {
        out.push(format!("the entry {:?} is not one of the files", h.entry));
    }
    if !(h.entry.ends_with(".html") || [".jsx", ".tsx"].iter().any(|e| h.entry.ends_with(e))) {
        out.push("the entry must be an .html page or a .jsx/.tsx component".into());
    }
    let paths: std::collections::BTreeSet<&str> = h.files.iter().map(|f| f.path.as_str()).collect();
    for f in &h.files {
        if !safe_path(&f.path) {
            out.push(format!("{:?} is not a plain relative path", f.path));
        }
        if f.text.is_none() && f.blob.is_none() {
            out.push(format!("{:?} has neither text nor a blob", f.path));
        }
        if let Some(text) = &f.text {
            for p in network_loads(text) {
                out.push(format!("{:?} loads {p} from the network: artifacts run offline; use /_lib/ or a file of the artifact", f.path));
            }
            if is_script(&f.path) {
                for spec in imports(text) {
                    if spec.starts_with("./") || spec.starts_with("../") {
                        let target = normalize(&f.path, &spec);
                        let found = target.as_deref().is_some_and(|t| {
                            paths.contains(t) || [".js", ".jsx", ".ts", ".tsx"].iter().any(|e| paths.contains(format!("{t}{e}").as_str()))
                        });
                        if !found {
                            out.push(format!("{:?} imports {spec:?}, which is not one of the files", f.path));
                        }
                    } else if libs::lib(&spec).is_none() {
                        out.push(format!(
                            "{:?} imports {spec:?}, which is not vendored (available: {})",
                            f.path,
                            libs::LIBS.iter().filter(|l| l.name != "prop-types").map(|l| l.name).collect::<Vec<_>>().join(", ")
                        ));
                    }
                }
            }
        }
    }
    out
}

/// Resolves `./x` or `../x` against the directory of `from`.
pub fn normalize(from: &str, spec: &str) -> Option<String> {
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for seg in spec.split('/') {
        match seg {
            "." | "" => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

/// The module specifiers a script imports: `import … from "x"`, `import "x"`, `export … from "x"` and
/// `import("x")`. A scanner, not a parser: it skips comments and template text it can see, which is enough to
/// refuse network imports early; the hub's compile step is the real check.
pub fn imports(src: &str) -> Vec<String> {
    let mut out = vec![];
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        let word_start = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] == b'$' || b[i - 1] == b'.');
        if word_start && (src[i..].starts_with("import") || src[i..].starts_with("export")) {
            let kw_end = i + 6;
            if b.get(kw_end).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_') {
                i += 1;
                continue;
            }
            // Find the statement's string: right after `import`/`import(`, or after `from`.
            let mut j = kw_end;
            let stop = src[j..].find(';').map_or(b.len(), |k| j + k).min(j + 2000);
            let seg = &src[j..stop.min(src.len())];
            let lead = seg.trim_start();
            let spec = if lead.starts_with('"') || lead.starts_with('\'') || lead.starts_with('(') {
                let lead = lead.trim_start_matches('(').trim_start();
                quoted(lead)
            } else if let Some(k) = find_word(seg, "from") {
                quoted(seg[k + 4..].trim_start())
            } else {
                None
            };
            if let Some(s) = spec {
                out.push(s);
            }
            j = kw_end;
            i = j;
            continue;
        }
        i += 1;
    }
    out
}

fn find_word(s: &str, w: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut from = 0;
    while let Some(k) = s[from..].find(w) {
        let at = from + k;
        let before = at == 0 || !(b[at - 1].is_ascii_alphanumeric() || b[at - 1] == b'_');
        let after = b.get(at + w.len()).is_none_or(|c| !(c.is_ascii_alphanumeric() || *c == b'_'));
        if before && after {
            return Some(at);
        }
        from = at + w.len();
    }
    None
}

fn quoted(s: &str) -> Option<String> {
    let q = s.chars().next()?;
    if q != '"' && q != '\'' && q != '`' {
        return None;
    }
    let rest = &s[1..];
    let end = rest.find(q)?;
    Some(rest[..end].to_string())
}

/// The network loads in a file: `src`/`href` of scripts, styles, images and frames, CSS `@import`/`url()` and
/// script imports that point at `http:`, `https:` or protocol-relative `//` URLs. Plain `<a href>` links are
/// allowed (opening one goes through the console, which asks the owner).
pub fn network_loads(text: &str) -> Vec<String> {
    let mut out = vec![];
    let lower = text.to_ascii_lowercase();
    for needle in ["src=", "href=", "url(", "@import", "from ", "import(", "fetch("] {
        let mut from = 0;
        while let Some(k) = lower[from..].find(needle) {
            let at = from + k;
            from = at + needle.len();
            if needle == "href=" {
                // An anchor's link is not a load.
                let tag_start = lower[..at].rfind('<').unwrap_or(0);
                let tag = &lower[tag_start..at];
                if tag.starts_with("<a ") || tag.starts_with("<a\n") || tag.starts_with("<a\t") {
                    continue;
                }
            }
            let rest = lower[from..].trim_start_matches([' ', '"', '\'', '(', '`']);
            if rest.starts_with("http://") || rest.starts_with("https://") || rest.starts_with("//") {
                let end = rest.find(['"', '\'', ')', ' ', '`', '>']).unwrap_or(rest.len()).min(120);
                out.push(rest[..end].to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The vendored libraries an HTML body uses: imports in its scripts and `/_lib/` script tags in its pages.
pub fn libs_used(h: &HtmlBody) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = vec![];
    for f in &h.files {
        let Some(text) = &f.text else { continue };
        if is_script(&f.path) {
            for spec in imports(text) {
                if let Some(l) = libs::lib(&spec) {
                    names.push(l.name);
                }
            }
        }
        let mut from = 0;
        while let Some(k) = text[from..].find("/_lib/") {
            let at = from + k + 6;
            let end = text[at..].find(['"', '\'', ' ', '>']).map_or(text.len(), |e| at + e);
            if let Some(l) = libs::by_file(&text[at..end]) {
                names.push(l.name);
            }
            from = end.max(at);
        }
    }
    if [".jsx", ".tsx"].iter().any(|e| h.entry.ends_with(e)) {
        names.push("react");
        names.push("react-dom");
    }
    libs::closure(&names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::HtmlFile;

    fn html(entry: &str, files: &[(&str, &str)]) -> HtmlBody {
        HtmlBody {
            entry: entry.into(),
            files: files
                .iter()
                .map(|(p, t)| HtmlFile {
                    path: (*p).into(),
                    text: Some((*t).into()),
                    blob: None,
                })
                .collect(),
            height: 360,
            libs: vec![],
            still: None,
        }
    }

    #[test]
    fn imports_are_found() {
        let src = "import React, { useState } from 'react';\nimport * as d3 from \"d3\";\n// import x from 'nope'\nimport './style.css';\nexport { a } from './a.js';\nconst m = import('lodash');";
        assert_eq!(imports(src), vec!["react", "d3", "./style.css", "./a.js", "lodash"]);
    }

    #[test]
    fn network_loads_are_refused_but_links_are_not() {
        let page = r#"<script src="https://cdn.example.com/x.js"></script><a href="https://example.com">x</a><img src='//evil/p.png'>"#;
        assert_eq!(network_loads(page), vec!["//evil/p.png", "https://cdn.example.com/x.js"]);
    }

    #[test]
    fn html_checks_imports_against_the_vendored_set() {
        let h = html("App.tsx", &[("App.tsx", "import {LineChart} from 'recharts'; import pad from 'left-pad'; import {x} from './util';")]);
        let p = html_problems(&h, &Caps::default());
        assert!(p.iter().any(|m| m.contains("left-pad")), "{p:?}");
        assert!(p.iter().any(|m| m.contains("./util")), "{p:?}");
        let ok = html("App.tsx", &[("App.tsx", "import {LineChart} from 'recharts'; import {x} from './util';"), ("util.ts", "export const x = 1;")]);
        assert!(html_problems(&ok, &Caps::default()).is_empty());
        assert_eq!(libs_used(&ok), vec!["react", "react-dom", "prop-types", "recharts"]);
    }

    #[test]
    fn paths_must_be_plain() {
        assert!(safe_path("data/points.csv"));
        for bad in ["../x", "/etc/passwd", "a//b", "_lib/react.js", "a\\b", "a/./b", ""] {
            assert!(!safe_path(bad), "{bad}");
        }
    }
}
