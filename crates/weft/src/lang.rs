//! Language and format packs, loaded only when a file needs one.
//!
//! A pack is a tree-sitter grammar compiled to WebAssembly plus a tags query (`@definition.<kind>` with a
//! `@name`, and `@reference.<kind>`). Packs live in Weft (`pack.add` records, their files as blobs). The
//! runtime loads a pack into a WebAssembly sandbox the first time a file matches it, keeps it while it is used,
//! and unloads it when it has been idle. Nothing is loaded up front.

use std::collections::{BTreeMap, HashMap};
#[cfg(feature = "packs")]
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
#[cfg(feature = "packs")]
use std::time::Instant;

use serde::Serialize;

use crate::hash::Hash;
use crate::model::PackAdd;
use crate::state::PackInfo;

/// A definition in a file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Symbol {
    /// `function`, `method`, `class`, `module`, `impl`, `constant`, `section`, `key`, …
    pub kind: String,
    pub name: String,
    /// The path from the outermost definition: `impl Queues › fn assign_reviews`.
    pub qualified: String,
    pub start_byte: usize,
    pub end_byte: usize,
    /// 1-based, inclusive.
    pub start_line: usize,
    pub end_line: usize,
    /// The definition's text with whitespace runs collapsed.
    pub content_hash: Hash,
    /// The same, with the definition's own name taken out: equal for a renamed definition.
    pub shape_hash: Hash,
    pub parent: Option<usize>,
}

/// A name a file refers to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Reference {
    pub kind: String,
    pub name: String,
    pub line: usize,
}

/// What a pack found in one file.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Parsed {
    pub pack: Option<String>,
    pub symbols: Vec<Symbol>,
    pub references: Vec<Reference>,
    pub error: bool,
}

/// The pack a path (and its first line) belongs to.
pub fn pack_for<'a>(
    packs: &'a BTreeMap<String, PackInfo>,
    path: &str,
    first_line: &str,
) -> Option<&'a PackAdd> {
    let file = path.rsplit('/').next().unwrap_or(path);
    let ext = file.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    let by_name = packs
        .values()
        .find(|p| p.pack.filenames.iter().any(|f| f == file));
    let by_ext = || {
        packs.values().find(|p| {
            ext.as_deref()
                .is_some_and(|e| p.pack.extensions.iter().any(|x| x == e))
        })
    };
    let by_shebang = || {
        first_line.strip_prefix("#!").and_then(|line| {
            packs
                .values()
                .find(|p| p.pack.shebangs.iter().any(|s| line.contains(s.as_str())))
        })
    };
    by_name.or_else(by_ext).or_else(by_shebang).map(|p| &p.pack)
}

/// The short label people read for a symbol kind (`fn`, `struct`, `impl`, …).
pub fn kind_label(kind: &str) -> &str {
    match kind {
        "function" | "method" => "fn",
        "class" => "type",
        "interface" => "trait",
        "module" => "mod",
        "constant" => "const",
        other => other,
    }
}

#[cfg(feature = "packs")]
struct Loaded {
    version: u32,
    parser: tree_sitter::Parser,
    query: tree_sitter::Query,
    last_used: Instant,
}

/// The loaded packs.
pub struct Runtime {
    #[cfg(feature = "packs")]
    engine: tree_sitter::wasmtime::Engine,
    #[cfg(feature = "packs")]
    loaded: Mutex<HashMap<String, Arc<Mutex<Loaded>>>>,
    #[cfg(not(feature = "packs"))]
    loaded: Mutex<HashMap<String, ()>>,
    loads: Mutex<u64>,
}

impl Default for Runtime {
    fn default() -> Self {
        Runtime::new()
    }
}

impl Runtime {
    pub fn new() -> Runtime {
        Runtime {
            #[cfg(feature = "packs")]
            engine: tree_sitter::wasmtime::Engine::default(),
            loaded: Mutex::new(HashMap::new()),
            loads: Mutex::new(0),
        }
    }

    /// The packs loaded now.
    pub fn loaded(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .loaded
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .keys()
            .cloned()
            .collect();
        v.sort();
        v
    }

    /// How many times a pack has been loaded since start.
    pub fn loads(&self) -> u64 {
        *self.loads.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Unloads packs unused for `idle`; returns how many.
    pub fn unload_idle(&self, idle: Duration) -> usize {
        #[cfg(feature = "packs")]
        {
            let mut map = self.loaded.lock().unwrap_or_else(|p| p.into_inner());
            let before = map.len();
            map.retain(|_, l| {
                l.lock()
                    .map(|l| l.last_used.elapsed() < idle)
                    .unwrap_or(false)
            });
            before - map.len()
        }
        #[cfg(not(feature = "packs"))]
        {
            let _ = idle;
            0
        }
    }

    /// Parses `source` with `pack`, loading the pack if needed. `blob` reads a pack file by hash.
    pub fn parse(
        &self,
        pack: &PackAdd,
        blob: &dyn Fn(&Hash) -> Result<Vec<u8>, String>,
        source: &[u8],
    ) -> Result<Parsed, String> {
        #[cfg(feature = "packs")]
        {
            let loaded = self.load(pack, blob)?;
            let mut l = loaded.lock().unwrap_or_else(|p| p.into_inner());
            l.last_used = Instant::now();
            let Loaded { parser, query, .. } = &mut *l;
            let tree = parser.parse(source, None).ok_or("the parser gave up")?;
            let mut parsed = extract(query, &tree, source);
            parsed.pack = Some(pack.name.clone());
            Ok(parsed)
        }
        #[cfg(not(feature = "packs"))]
        {
            let _ = (pack, blob, source);
            Ok(Parsed::default())
        }
    }

    #[cfg(feature = "packs")]
    fn load(
        &self,
        pack: &PackAdd,
        blob: &dyn Fn(&Hash) -> Result<Vec<u8>, String>,
    ) -> Result<Arc<Mutex<Loaded>>, String> {
        if let Some(l) = self
            .loaded
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&pack.name)
            && l.lock().map(|l| l.version == pack.version).unwrap_or(false)
        {
            return Ok(l.clone());
        }
        let wasm = blob(&pack.wasm)?;
        let tags = String::from_utf8(blob(&pack.tags)?)
            .map_err(|_| format!("pack {}: its query is not text", pack.name))?;
        let mut store = tree_sitter::WasmStore::new(&self.engine)
            .map_err(|e| format!("pack {}: {e}", pack.name))?;
        let language = store
            .load_language(&pack.language, &wasm)
            .map_err(|e| format!("pack {}: {e}", pack.name))?;
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_wasm_store(store)
            .map_err(|e| format!("pack {}: {e}", pack.name))?;
        parser
            .set_language(&language)
            .map_err(|e| format!("pack {}: {e}", pack.name))?;
        let query = tree_sitter::Query::new(&language, &tags)
            .map_err(|e| format!("pack {}: its query: {e}", pack.name))?;
        let loaded = Arc::new(Mutex::new(Loaded {
            version: pack.version,
            parser,
            query,
            last_used: Instant::now(),
        }));
        self.loaded
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(pack.name.clone(), loaded.clone());
        *self.loads.lock().unwrap_or_else(|p| p.into_inner()) += 1;
        Ok(loaded)
    }
}

#[cfg(feature = "packs")]
fn extract(query: &tree_sitter::Query, tree: &tree_sitter::Tree, source: &[u8]) -> Parsed {
    use tree_sitter::StreamingIterator;
    let names = query.capture_names();
    let mut cursor = tree_sitter::QueryCursor::new();
    let mut defs: Vec<(usize, usize, usize, usize, String, String)> = Vec::new();
    let mut refs = Vec::new();
    let mut matches = cursor.matches(query, tree.root_node(), source);
    while let Some(m) = matches.next() {
        let mut name_text = None;
        let mut def = None;
        let mut reference = None;
        for c in m.captures() {
            let cap = names[c.index as usize];
            if cap == "name" {
                name_text = c.node.utf8_text(source).ok().map(short_name);
            } else if let Some(kind) = cap.strip_prefix("definition.") {
                def = Some((c.node, kind.to_string()));
            } else if let Some(kind) = cap.strip_prefix("reference.") {
                reference = Some((c.node, kind.to_string()));
            }
        }
        let Some(name) = name_text else { continue };
        if let Some((node, kind)) = def {
            defs.push((
                node.start_byte(),
                node.end_byte(),
                node.start_position().row + 1,
                node.end_position().row + 1,
                kind,
                name,
            ));
        } else if let Some((node, kind)) = reference {
            refs.push(Reference {
                kind,
                name,
                line: node.start_position().row + 1,
            });
        }
    }
    // Outer definitions first; drop exact duplicates (two query patterns matching one node).
    defs.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    defs.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1 && a.5 == b.5);
    let mut symbols: Vec<Symbol> = Vec::with_capacity(defs.len());
    let mut stack: Vec<usize> = Vec::new();
    for (start, end, sl, el, kind, name) in defs {
        while let Some(&top) = stack.last() {
            if symbols[top].end_byte >= end && symbols[top].start_byte <= start {
                break;
            }
            stack.pop();
        }
        let parent = stack.last().copied();
        let label = format!("{} {}", kind_label(&kind), name);
        let qualified = match parent {
            Some(p) => format!("{} › {}", symbols[p].qualified, label),
            None => label,
        };
        let text = String::from_utf8_lossy(&source[start..end]);
        let normal = collapse(&text);
        let shape = collapse(&text.replacen(&name, "\u{1}", 1));
        symbols.push(Symbol {
            kind,
            name,
            qualified,
            start_byte: start,
            end_byte: end,
            start_line: sl,
            end_line: el,
            content_hash: Hash::of(normal.as_bytes()),
            shape_hash: Hash::of(shape.as_bytes()),
            parent,
        });
        stack.push(symbols.len() - 1);
    }
    Parsed {
        pack: None,
        symbols,
        references: refs,
        error: tree.root_node().has_error(),
    }
}

/// A name as people read it: one line, at most 80 characters.
#[cfg(feature = "packs")]
fn short_name(text: &str) -> String {
    let one = collapse(text);
    match one.char_indices().nth(80) {
        Some((at, _)) => format!("{}…", &one[..at]),
        None => one,
    }
}

#[cfg(feature = "packs")]
fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            space = true;
        } else {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(ch);
        }
    }
    out
}

/// The innermost symbol covering 1-based `line`.
pub fn symbol_at(symbols: &[Symbol], line: usize) -> Option<usize> {
    symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| s.start_line <= line && line <= s.end_line)
        .max_by_key(|(_, s)| s.start_line)
        .map(|(i, _)| i)
}
