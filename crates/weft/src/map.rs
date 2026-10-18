//! The project map: where everything goes and what is what.
//!
//! Each repo keeps a `map.toml` at its root, versioned like any file:
//!
//! ```toml
//! description = "the hub, the consoles, the CLI"
//!
//! [[component]]
//! name = "web-console"
//! title = "the web console"
//! paths = ["apps/web/**"]
//! languages = ["typescript", "css"]
//! roles = ["builder"]
//!
//! [[release]]
//! name = "agentplaza"
//! targets = ["all"]
//! channel = "github release, signed tag v*"
//! ```
//!
//! When a repo has none, [`draft`] proposes one from its tree (folders, build files, READMEs, languages), which an
//! indexing task refines and the owner approves. [`answer`] tells an agent where something lives.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::derived::{Derived, glob};
use crate::lang;
use crate::model::Tree;
use crate::store::Store;

pub const FILE: &str = "map.toml";

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Map {
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "component")]
    pub components: Vec<Component>,
    #[serde(default, rename = "release")]
    pub releases: Vec<ReleaseDef>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Component {
    pub name: String,
    #[serde(default)]
    pub title: String,
    pub paths: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub roles: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ReleaseDef {
    pub name: String,
    /// `all` (every target the build files declare) or named targets.
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default)]
    pub channel: String,
}

impl Map {
    pub fn parse(text: &str) -> Result<Map, String> {
        toml::from_str(text).map_err(|e| format!("{FILE}: {e}"))
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// The component a path belongs to: the one whose glob is most specific.
    pub fn component_of(&self, path: &str) -> Option<&Component> {
        self.components
            .iter()
            .filter(|c| c.paths.iter().any(|g| glob(g, path)))
            .max_by_key(|c| {
                c.paths
                    .iter()
                    .filter(|g| glob(g, path))
                    .map(|g| g.len())
                    .max()
                    .unwrap_or(0)
            })
    }
}

/// A repo's map at a revision (its head when `rev` is `None`); empty when it has none.
pub fn load(store: &Store, repo: &str, rev: Option<&str>) -> Result<Option<Map>, String> {
    let rev = match rev {
        Some(r) => r.to_string(),
        None => store.state().repo(repo)?.head("main"),
    };
    match store.read(&rev, FILE)? {
        Some(b) => Map::parse(&String::from_utf8_lossy(&b)).map(Some),
        None => Ok(None),
    }
}

/// A component's globs, by name or title (empty when there is no such component).
pub fn component_globs(store: &Store, repo: &str, name: &str) -> Vec<String> {
    let Ok(Some(map)) = load(store, repo, None) else {
        return vec![];
    };
    map.components
        .into_iter()
        .filter(|c| c.name == name || c.title == name)
        .flat_map(|c| c.paths)
        .collect()
}

/// A first map, drafted from a tree: one component per top-level folder (and per member of `crates/`, `apps/`,
/// `packages/`, `services/`), titled from its README or build file, with the languages its files are in.
pub fn draft(store: &Store, tree: &Tree) -> Map {
    let st = store.state();
    let grouped = ["crates", "apps", "packages", "services", "libs", "cmd"];
    let mut groups: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for path in tree.keys() {
        let parts: Vec<&str> = path.split('/').collect();
        let key = match parts.as_slice() {
            [one] => {
                let _ = one;
                continue;
            }
            [top, member, _, ..] if grouped.contains(top) => format!("{top}/{member}"),
            [top, ..] => top.to_string(),
            _ => continue,
        };
        groups.entry(key).or_default().push(path);
    }
    let mut components = Vec::new();
    for (dir, files) in groups {
        let mut langs: BTreeMap<String, usize> = BTreeMap::new();
        for f in &files {
            if let Some(p) = lang::pack_for(&st.packs, f, "") {
                *langs.entry(p.name.clone()).or_default() += 1;
            }
        }
        let mut languages: Vec<(String, usize)> = langs.into_iter().collect();
        languages.sort_by_key(|l| std::cmp::Reverse(l.1));
        let title = describe(store, tree, &dir).unwrap_or_default();
        components.push(Component {
            name: dir.rsplit('/').next().unwrap_or(&dir).to_string(),
            title,
            paths: vec![format!("{dir}/**")],
            languages: languages.into_iter().take(3).map(|l| l.0).collect(),
            roles: vec![],
        });
    }
    let description = describe(store, tree, "").unwrap_or_default();
    Map {
        description,
        components,
        releases: vec![],
    }
}

/// A folder's one-line description: its README's first paragraph line, or its build file's description.
fn describe(store: &Store, tree: &Tree, dir: &str) -> Option<String> {
    let at = |name: &str| {
        if dir.is_empty() {
            name.to_string()
        } else {
            format!("{dir}/{name}")
        }
    };
    let read = |p: &str| {
        tree.get(p)
            .and_then(|e| store.blobs.get(&e.hash).ok())
            .map(|b| String::from_utf8_lossy(&b).to_string())
    };
    for (file, key) in [
        ("Cargo.toml", "description"),
        ("package.json", "description"),
        ("pyproject.toml", "description"),
    ] {
        if let Some(text) = read(&at(file)) {
            for line in text.lines() {
                let l = line.trim();
                if let Some(rest) = l
                    .strip_prefix(&format!("\"{key}\""))
                    .or_else(|| l.strip_prefix(key))
                {
                    let v = rest
                        .trim_start_matches([' ', ':', '='])
                        .trim()
                        .trim_end_matches(',')
                        .trim_matches('"');
                    if !v.is_empty() && !v.contains("workspace") {
                        return Some(v.to_string());
                    }
                }
            }
        }
    }
    let readme = read(&at("README.md"))?;
    readme
        .lines()
        .map(str::trim)
        .find(|l| {
            !l.is_empty()
                && !l.starts_with('#')
                && !l.starts_with('!')
                && !l.starts_with('[')
                && !l.starts_with('<')
        })
        .map(|l| l.chars().take(140).collect())
}

/// One place an answer points to.
#[derive(Clone, Debug, Serialize)]
pub struct Hit {
    pub path: String,
    pub symbol: Option<String>,
    pub component: Option<String>,
    pub why: String,
    pub score: u32,
}

/// Where something lives: paths, definitions and components whose names match the question's words.
pub fn answer(
    store: &Store,
    derived: &Derived,
    repo: &str,
    question: &str,
) -> Result<Vec<Hit>, String> {
    let head = store.state().repo(repo)?.head("main");
    let tree = store.tree(&head)?;
    let map = load(store, repo, Some(&head))?.unwrap_or_default();
    let stop: BTreeSet<&str> = [
        "the", "a", "an", "is", "where", "what", "which", "of", "in", "for", "to", "and", "does",
        "do", "how", "screen", "file", "code", "lives", "live",
    ]
    .into_iter()
    .collect();
    let words: Vec<String> = question
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| w.len() > 1 && !stop.contains(w))
        .map(|w| w.trim_end_matches('s').to_string())
        .collect();
    if words.is_empty() {
        return Ok(vec![]);
    }
    let score_text = |text: &str| -> u32 {
        let t = text.to_lowercase();
        words.iter().filter(|w| t.contains(w.as_str())).count() as u32
    };
    let mut hits: Vec<Hit> = Vec::new();
    let comp_name = |p: &str| {
        map.component_of(p).map(|c| {
            if c.title.is_empty() {
                c.name.clone()
            } else {
                format!("{} ({})", c.title, c.name)
            }
        })
    };
    for path in tree.keys() {
        let s = score_text(path);
        if s > 0 {
            hits.push(Hit {
                path: path.clone(),
                symbol: None,
                component: comp_name(path),
                why: "its path".into(),
                score: s * 3,
            });
        }
    }
    if let Ok(names) = derived.names(store, &head) {
        for (name, defs) in &names.defs {
            let s = score_text(name);
            if s == 0 {
                continue;
            }
            for (path, q) in defs.iter().take(3) {
                hits.push(Hit {
                    path: path.clone(),
                    symbol: Some(q.clone()),
                    component: comp_name(path),
                    why: format!("defines {name}"),
                    score: s * 4,
                });
            }
        }
    }
    for c in &map.components {
        let s = score_text(&format!("{} {}", c.name, c.title));
        if s > 0 {
            hits.push(Hit {
                path: c.paths.join(", "),
                symbol: None,
                component: Some(c.title.clone()),
                why: format!("component {}", c.name),
                score: s * 5,
            });
        }
    }
    hits.sort_by(|a, b| b.score.cmp(&a.score).then(a.path.len().cmp(&b.path.len())));
    let mut seen = BTreeSet::new();
    hits.retain(|h| seen.insert((h.path.clone(), h.symbol.clone())));
    hits.truncate(12);
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_round_trip_and_the_most_specific_wins() {
        let map = Map {
            description: "the hub".into(),
            components: vec![
                Component {
                    name: "apid".into(),
                    title: "the hub daemon".into(),
                    paths: vec!["crates/apid/**".into()],
                    languages: vec!["rust".into()],
                    roles: vec!["builder".into()],
                },
                Component {
                    name: "scheduler".into(),
                    title: "the scheduler".into(),
                    paths: vec!["crates/apid/src/scheduler/**".into()],
                    languages: vec![],
                    roles: vec![],
                },
            ],
            releases: vec![ReleaseDef {
                name: "agentplaza".into(),
                targets: vec!["all".into()],
                channel: "github".into(),
            }],
        };
        let back = Map::parse(&map.to_toml()).unwrap();
        assert_eq!(back, map);
        assert_eq!(
            map.component_of("crates/apid/src/scheduler/review.rs")
                .unwrap()
                .name,
            "scheduler"
        );
        assert_eq!(
            map.component_of("crates/apid/src/main.rs").unwrap().name,
            "apid"
        );
        assert!(map.component_of("README.md").is_none());
    }
}
