//! Which packages a project has, and which of them an advisory's ranges reach (PLAN 24.3): the components of `main`'s
//! lock files (10.8's and 9.3's readers: `releases::sbom`) and of every release's SBOM (the CycloneDX document the
//! release's artifacts carry).

use serde_json::Value as Json;

use super::advisory::Advisory;
use crate::releases::sbom;
use crate::upgrades::Tree;
use crate::upgrades::ecosystem::Ecosystem;

/// A package a project has: its ecosystem, name and the version it resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub ecosystem: Ecosystem,
    pub name: String,
    pub version: String,
}

/// The ecosystem a component's type names (`cargo`, `npm`, `pypi`, `golang`).
fn ecosystem_of_kind(kind: &str) -> Option<Ecosystem> {
    match kind {
        "cargo" => Some(Ecosystem::Cargo),
        "npm" => Some(Ecosystem::Npm),
        "pypi" => Some(Ecosystem::Pip),
        "golang" => Some(Ecosystem::Go),
        _ => None,
    }
}

/// The packages of `main`'s lock files, with the notes of the lock files that did not read.
pub fn main_packages(tree: &Tree) -> (Vec<Package>, Vec<String>) {
    let root: Vec<String> = tree.keys().filter(|p| !p.contains('/')).cloned().collect();
    let mut out = Vec::new();
    let mut notes = Vec::new();
    for eco in ["cargo", "npm", "python", "go"] {
        let Some((file, format)) = sbom::source(eco, &root) else {
            continue;
        };
        let text = String::from_utf8_lossy(&tree[&file]).into_owned();
        match sbom::components(format, &text) {
            Ok(components) => out.extend(components.into_iter().filter_map(|c| {
                Some(Package {
                    ecosystem: ecosystem_of_kind(c.kind)?,
                    name: c.name,
                    version: c.version,
                })
            })),
            Err(e) => notes.push(e),
        }
    }
    (out, notes)
}

/// The packages of a release's SBOM (CycloneDX 1.5: its `components`, each with a name, a version and a purl type).
pub fn sbom_packages(text: &str) -> Result<Vec<Package>, String> {
    let doc: Json =
        serde_json::from_str(text).map_err(|e| format!("the SBOM does not parse: {e}"))?;
    let mut out = Vec::new();
    for comp in doc["components"].as_array().into_iter().flatten() {
        let purl = comp["purl"].as_str().unwrap_or_default();
        let kind = purl
            .strip_prefix("pkg:")
            .and_then(|p| p.split('/').next())
            .unwrap_or("");
        let (Some(ecosystem), Some(name), Some(version)) = (
            ecosystem_of_kind(kind),
            comp["name"].as_str(),
            comp["version"].as_str(),
        ) else {
            continue;
        };
        out.push(Package {
            ecosystem,
            name: name.to_string(),
            version: version.to_string(),
        });
    }
    Ok(out)
}

/// The packages an advisory reaches at a version of the package it names: the advisory's ecosystem and name, and a
/// version it affects. An audit's observation (`Observed`) reaches the version it was seen at, which the caller says.
pub fn reached<'a>(advisory: &Advisory, packages: &'a [Package]) -> Vec<&'a Package> {
    packages
        .iter()
        .filter(|p| p.ecosystem == advisory.ecosystem && p.name == advisory.package)
        .filter(|p| advisory.affected(&p.version))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cyclonedx_sbom_names_its_packages_by_purl_type() {
        let sbom = r#"{"bomFormat":"CycloneDX","components":[
            {"name":"h2","version":"0.3.9","purl":"pkg:cargo/h2@0.3.9"},
            {"name":"@types/node","version":"20.1.0","purl":"pkg:npm/%40types/node@20.1.0"},
            {"name":"requests","version":"2.19.0","purl":"pkg:pypi/requests@2.19.0"}]}"#;
        let packages = sbom_packages(sbom).unwrap();
        assert_eq!(packages.len(), 3);
        assert_eq!(packages[0].ecosystem, Ecosystem::Cargo);
        assert_eq!(packages[1].name, "@types/node");
        assert_eq!(packages[2].ecosystem, Ecosystem::Pip);
    }
}
