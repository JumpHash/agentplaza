//! A loaded set of files: packs, imports and name resolution (docs/jhrl.md section 3).
//!
//! Names resolve in this order: the current pack, then imported packs (two imports exporting the
//! same name are an error at the point of use), then the prelude. Every entity and edge type
//! is also a collection named in lowercase plural (`CREDIT_LINE` → `credit_lines`).

use crate::ast::*;
use crate::parser::parse_module;
use crate::resolve::Tier;
use crate::span::{Diag, SourceMap, Span};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DeclRef {
    pub file: u32,
    pub item: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameKind {
    Decl,
    /// The collection of an entity or edge type.
    Collection,
}

#[derive(Clone, Debug)]
pub struct Pack {
    pub name: String,
    pub files: Vec<u32>,
    pub imports: Vec<(String, Span)>,
    pub tier: Tier,
    names: HashMap<String, (NameKind, DeclRef)>,
    imported: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct Program {
    pub sources: SourceMap,
    pub modules: Vec<Module>,
    pub packs: Vec<Pack>,
    pub file_pack: Vec<usize>,
}

/// `Agent` → `agents`, `CREDIT_LINE` → `credit_lines`, `WorkGroup` → `work_groups`.
pub fn collection_name(type_name: &str) -> String {
    let mut snake = String::new();
    let chars: Vec<char> = type_name.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() {
            let prev_lower = i > 0 && chars[i - 1].is_ascii_lowercase();
            if prev_lower {
                snake.push('_');
            }
            snake.push(c.to_ascii_lowercase());
        } else {
            snake.push(*c);
        }
    }
    if snake.ends_with('s')
        || snake.ends_with('x')
        || snake.ends_with("sh")
        || snake.ends_with("ch")
    {
        snake.push_str("es");
    } else if snake.ends_with('y')
        && !snake.ends_with("ay")
        && !snake.ends_with("ey")
        && !snake.ends_with("oy")
    {
        snake.pop();
        snake.push_str("ies");
    } else {
        snake.push('s');
    }
    snake
}

impl Program {
    /// Parses and links the given (path, text) sources. Files that fail to parse are kept as
    /// empty modules so the rest can still be checked; their errors are returned.
    pub fn load(files: Vec<(String, String)>) -> (Program, Vec<Diag>) {
        let mut sources = SourceMap::default();
        let mut modules = Vec::new();
        let mut diags = Vec::new();
        for (path, text) in files {
            let id = sources.add(path, text);
            match parse_module(id, &sources.files[id as usize].text) {
                Ok(m) => modules.push(m),
                Err(d) => {
                    diags.push(d);
                    modules.push(Module {
                        file: id,
                        pack: None,
                        items: vec![],
                    });
                }
            }
        }
        let mut p = Program {
            sources,
            modules,
            packs: vec![],
            file_pack: vec![],
        };
        diags.extend(p.link());
        (p, diags)
    }

    /// Re-parses one file with new text (used by mutants). Tiers are kept.
    pub fn with_source(&self, file: u32, text: String) -> Result<Program, Diag> {
        let mut p = self.clone();
        let path = p.sources.files[file as usize].path.clone();
        p.sources.files[file as usize] = crate::span::Source::new(path, text);
        let m = parse_module(file, &p.sources.files[file as usize].text)?;
        p.modules[file as usize] = m;
        let tiers: HashMap<String, Tier> = self
            .packs
            .iter()
            .map(|k| (k.name.clone(), k.tier))
            .collect();
        // Link problems (an import of a pack that isn't loaded …) were already reported for the
        // original program; only a parse error of the new text is an error here.
        let _ = p.link();
        for k in p.packs.iter_mut() {
            if let Some(t) = tiers.get(&k.name) {
                k.tier = *t;
            }
        }
        Ok(p)
    }

    fn link(&mut self) -> Vec<Diag> {
        let mut diags = Vec::new();
        self.packs.clear();
        self.file_pack.clear();
        let mut by_name: HashMap<String, usize> = HashMap::new();
        for m in &self.modules {
            let name = match &m.pack {
                Some(p) => p.name.clone(),
                None => format!("<{}>", self.sources.files[m.file as usize].path),
            };
            let idx = *by_name.entry(name.clone()).or_insert_with(|| {
                self.packs.push(Pack {
                    name,
                    files: vec![],
                    imports: vec![],
                    tier: Tier::System,
                    names: HashMap::new(),
                    imported: vec![],
                });
                self.packs.len() - 1
            });
            self.packs[idx].files.push(m.file);
            self.file_pack.push(idx);
            for (i, it) in m.items.iter().enumerate() {
                let r = DeclRef {
                    file: m.file,
                    item: i,
                };
                if let Item::Import(n, s) = it {
                    self.packs[idx].imports.push((n.clone(), *s));
                    continue;
                }
                let mut entries: Vec<(String, NameKind, Span)> = Vec::new();
                if let Some(n) = it.name() {
                    entries.push((n.to_string(), NameKind::Decl, it.span()));
                }
                match it {
                    Item::Entity(e) => entries.push((
                        e.collection
                            .clone()
                            .unwrap_or_else(|| collection_name(&e.name)),
                        NameKind::Collection,
                        e.span,
                    )),
                    Item::Edge(e) => entries.push((
                        e.collection
                            .clone()
                            .unwrap_or_else(|| collection_name(&e.name)),
                        NameKind::Collection,
                        e.span,
                    )),
                    _ => {}
                }
                for (n, k, span) in entries {
                    if let Some(prev) = self.packs[idx].names.get(&n) {
                        let ps = self.modules[prev.1.file as usize].items[prev.1.item].span();
                        let at = self.sources.locate(ps);
                        diags.push(Diag::error(
                            span,
                            format!("`{n}` is already declared in this pack (at {at})"),
                        ));
                    } else {
                        self.packs[idx].names.insert(n, (k, r));
                    }
                }
            }
        }
        for i in 0..self.packs.len() {
            let imports = self.packs[i].imports.clone();
            for (n, s) in imports {
                match by_name.get(&n) {
                    Some(&j) if j != i => self.packs[i].imported.push(j),
                    Some(_) => diags.push(Diag::error(s, "a pack can't import itself")),
                    None => diags.push(Diag::error(
                        s,
                        format!("unknown pack `{n}` (no loaded file declares it)"),
                    )),
                }
            }
        }
        diags
    }

    pub fn item(&self, r: DeclRef) -> &Item {
        &self.modules[r.file as usize].items[r.item]
    }

    pub fn pack_of(&self, file: u32) -> usize {
        self.file_pack[file as usize]
    }

    pub fn pack_by_name(&self, name: &str) -> Option<usize> {
        self.packs.iter().position(|p| p.name == name)
    }

    pub fn set_tier(&mut self, pack: &str, tier: Tier) -> bool {
        match self.pack_by_name(pack) {
            Some(i) => {
                self.packs[i].tier = tier;
                true
            }
            None => false,
        }
    }

    /// Resolves a name seen from `pack`: own pack first, then imports. `Err` names an ambiguity.
    pub fn lookup(&self, pack: usize, name: &str) -> Result<Option<(NameKind, DeclRef)>, String> {
        if let Some(x) = self.packs[pack].names.get(name) {
            return Ok(Some(*x));
        }
        let mut found: Vec<(usize, (NameKind, DeclRef))> = Vec::new();
        for &j in &self.packs[pack].imported {
            if let Some(x) = self.packs[j].names.get(name) {
                found.push((j, *x));
            }
        }
        match found.len() {
            0 => Ok(None),
            1 => Ok(Some(found[0].1)),
            _ => Err(format!(
                "`{name}` is exported by several imported packs: {}",
                found
                    .iter()
                    .map(|(j, _)| self.packs[*j].name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    /// Like `lookup` but only declarations (not collections).
    pub fn lookup_decl(&self, pack: usize, name: &str) -> Option<DeclRef> {
        match self.lookup(pack, name) {
            Ok(Some((NameKind::Decl, r))) => Some(r),
            _ => None,
        }
    }

    /// All items with the pack they belong to.
    pub fn items(&self) -> impl Iterator<Item = (usize, DeclRef, &Item)> {
        self.modules.iter().flat_map(move |m| {
            m.items.iter().enumerate().map(move |(i, it)| {
                (
                    self.pack_of(m.file),
                    DeclRef {
                        file: m.file,
                        item: i,
                    },
                    it,
                )
            })
        })
    }

    pub fn machine(&self, pack: usize, name: &str) -> Result<(usize, &MachineDecl), String> {
        match self.lookup_decl(pack, name) {
            Some(r) => match self.item(r) {
                Item::Machine(m) => Ok((self.pack_of(r.file), m)),
                _ => Err(format!("`{name}` is not a machine")),
            },
            None => Err(format!("unknown machine `{name}`")),
        }
    }

    pub fn locate(&self, span: Span) -> String {
        self.sources.locate(span)
    }

    pub fn path(&self, file: u32) -> &str {
        &self.sources.files[file as usize].path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_names() {
        assert_eq!(collection_name("Agent"), "agents");
        assert_eq!(collection_name("CREDIT_LINE"), "credit_lines");
        assert_eq!(collection_name("OPINION"), "opinions");
        assert_eq!(collection_name("WorkGroup"), "work_groups");
        assert_eq!(collection_name("Plaza"), "plazas");
        assert_eq!(collection_name("Run"), "runs");
        assert_eq!(collection_name("SUCCESSOR_OF"), "successor_ofs");
    }

    #[test]
    fn imports_resolve_and_ambiguity_is_reported() {
        let (p, d) = Program::load(vec![
            ("a.jhrl".into(), "pack a\nfn f(x: Int) -> Int = x".into()),
            ("b.jhrl".into(), "pack b\nfn f(x: Int) -> Int = x".into()),
            (
                "c.jhrl".into(),
                "pack c\nimport a\nimport b\nfn g(x: Int) -> Int = x".into(),
            ),
            ("d.jhrl".into(), "pack d\nimport nowhere".into()),
        ]);
        assert_eq!(d.len(), 1, "{d:?}");
        assert!(d[0].msg.contains("unknown pack `nowhere`"));
        let c = p.pack_by_name("c").unwrap();
        assert!(p.lookup(c, "f").is_err());
        assert!(p.lookup(c, "g").unwrap().is_some());
        let a = p.pack_by_name("a").unwrap();
        assert!(p.lookup(a, "g").unwrap().is_none());
    }
}
