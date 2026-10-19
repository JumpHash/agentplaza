//! Git's object model, written and read by Weft itself: blobs, trees, commits and tags (SHA-1), loose objects,
//! packfiles (with their deltas) and refs. Git is only ever a mirror: Weft writes it on export and reads it once on
//! import, and needs no git program for either.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use sha1::{Digest, Sha1};

pub type Oid = [u8; 20];

pub fn hex(id: &Oid) -> String {
    crate::hash::hex(id)
}

pub fn parse_oid(s: &str) -> Result<Oid, String> {
    let b = crate::hash::unhex(s.trim()).map_err(|_| format!("not a git id: {s}"))?;
    b.try_into().map_err(|_| format!("not a git id: {s}"))
}

pub const ZERO: Oid = [0; 20];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Commit,
    Tree,
    Blob,
    Tag,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Commit => "commit",
            Kind::Tree => "tree",
            Kind::Blob => "blob",
            Kind::Tag => "tag",
        }
    }

    fn from_name(s: &str) -> Result<Kind, String> {
        Ok(match s {
            "commit" => Kind::Commit,
            "tree" => Kind::Tree,
            "blob" => Kind::Blob,
            "tag" => Kind::Tag,
            other => return Err(format!("unknown object type {other}")),
        })
    }

    fn pack_type(self) -> u8 {
        match self {
            Kind::Commit => 1,
            Kind::Tree => 2,
            Kind::Blob => 3,
            Kind::Tag => 4,
        }
    }

    fn from_pack(t: u8) -> Result<Kind, String> {
        Ok(match t {
            1 => Kind::Commit,
            2 => Kind::Tree,
            3 => Kind::Blob,
            4 => Kind::Tag,
            other => return Err(format!("pack object type {other}")),
        })
    }
}

/// An object's id: SHA-1 of `<type> <len>\0<data>`.
pub fn oid(kind: Kind, data: &[u8]) -> Oid {
    let mut h = Sha1::new();
    h.update(format!("{} {}\0", kind.name(), data.len()).as_bytes());
    h.update(data);
    h.finalize().into()
}

// ── trees, commits, tags ─────────────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntry {
    pub mode: u32,
    pub name: String,
    pub id: Oid,
}

impl TreeEntry {
    pub fn is_tree(&self) -> bool {
        self.mode == 0o40000
    }
}

pub fn parse_tree(data: &[u8]) -> Result<Vec<TreeEntry>, String> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let sp = data[at..]
            .iter()
            .position(|b| *b == b' ')
            .ok_or("bad tree")?
            + at;
        let mode = u32::from_str_radix(
            std::str::from_utf8(&data[at..sp]).map_err(|_| "bad tree mode")?,
            8,
        )
        .map_err(|_| "bad tree mode")?;
        let nul = data[sp..].iter().position(|b| *b == 0).ok_or("bad tree")? + sp;
        let name = String::from_utf8_lossy(&data[sp + 1..nul]).to_string();
        let id: Oid = data
            .get(nul + 1..nul + 21)
            .ok_or("bad tree")?
            .try_into()
            .unwrap();
        out.push(TreeEntry { mode, name, id });
        at = nul + 21;
    }
    Ok(out)
}

/// A tree's bytes, its entries in git's order (names compared as if trees ended in `/`).
pub fn write_tree(entries: &[TreeEntry]) -> Vec<u8> {
    let mut sorted: Vec<&TreeEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| {
        let ka = if a.is_tree() {
            format!("{}/", a.name)
        } else {
            a.name.clone()
        };
        let kb = if b.is_tree() {
            format!("{}/", b.name)
        } else {
            b.name.clone()
        };
        ka.as_bytes().cmp(kb.as_bytes())
    });
    let mut out = Vec::new();
    for e in sorted {
        out.extend_from_slice(format!("{:o} {}\0", e.mode, e.name).as_bytes());
        out.extend_from_slice(&e.id);
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Person {
    pub name: String,
    pub email: String,
    pub time: i64,
    /// `+0200`.
    pub tz: String,
}

impl Person {
    pub fn parse(s: &str) -> Result<Person, String> {
        let lt = s.find('<').ok_or("bad person")?;
        let gt = s.rfind('>').ok_or("bad person")?;
        let name = s[..lt].trim().to_string();
        let email = s[lt + 1..gt].to_string();
        let mut rest = s[gt + 1..].split_whitespace();
        let time = rest.next().and_then(|t| t.parse().ok()).unwrap_or(0);
        let tz = rest.next().unwrap_or("+0000").to_string();
        Ok(Person {
            name,
            email,
            time,
            tz,
        })
    }

    pub fn line(&self) -> String {
        format!("{} <{}> {} {}", self.name, self.email, self.time, self.tz)
    }

    /// Milliseconds since the epoch.
    pub fn ms(&self) -> u64 {
        (self.time.max(0) as u64) * 1000
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub tree: Oid,
    pub parents: Vec<Oid>,
    pub author: Person,
    pub committer: Person,
    /// Other headers (`gpgsig`, `encoding`, `mergetag`, …), kept as they came.
    pub extra: Vec<(String, String)>,
    pub message: String,
}

pub fn parse_commit(data: &[u8]) -> Result<Commit, String> {
    let text = String::from_utf8_lossy(data);
    let (head, message) = text.split_once("\n\n").unwrap_or((&text, ""));
    let mut tree = None;
    let mut parents = Vec::new();
    let mut author = None;
    let mut committer = None;
    let mut extra: Vec<(String, String)> = Vec::new();
    for line in head.lines() {
        if let Some(cont) = line.strip_prefix(' ') {
            if let Some(last) = extra.last_mut() {
                last.1.push('\n');
                last.1.push_str(cont);
            }
            continue;
        }
        let (k, v) = line.split_once(' ').unwrap_or((line, ""));
        match k {
            "tree" => tree = Some(parse_oid(v)?),
            "parent" => parents.push(parse_oid(v)?),
            "author" => author = Some(Person::parse(v)?),
            "committer" => committer = Some(Person::parse(v)?),
            _ => extra.push((k.to_string(), v.to_string())),
        }
    }
    Ok(Commit {
        tree: tree.ok_or("a commit with no tree")?,
        parents,
        author: author.ok_or("a commit with no author")?,
        committer: committer.ok_or("a commit with no committer")?,
        extra,
        message: message.to_string(),
    })
}

/// A commit's bytes. `signature` (an armored SSH signature) goes in a `gpgsig` header.
pub fn write_commit(c: &Commit, signature: Option<&str>) -> Vec<u8> {
    let mut out = format!("tree {}\n", hex(&c.tree));
    for p in &c.parents {
        out.push_str(&format!("parent {}\n", hex(p)));
    }
    out.push_str(&format!(
        "author {}\ncommitter {}\n",
        c.author.line(),
        c.committer.line()
    ));
    for (k, v) in &c.extra {
        out.push_str(&format!("{k} {}\n", v.replace('\n', "\n ")));
    }
    if let Some(sig) = signature {
        out.push_str(&format!("gpgsig {}\n", sig.trim_end().replace('\n', "\n ")));
    }
    out.push('\n');
    out.push_str(&c.message);
    out.into_bytes()
}

/// An annotated tag's bytes; an SSH signature follows the message.
pub fn write_tag(
    object: &Oid,
    kind: Kind,
    name: &str,
    tagger: &Person,
    message: &str,
    signature: Option<&str>,
) -> Vec<u8> {
    let mut out = format!(
        "object {}\ntype {}\ntag {name}\ntagger {}\n\n{message}",
        hex(object),
        kind.name(),
        tagger.line()
    );
    if !out.ends_with('\n') {
        out.push('\n');
    }
    if let Some(sig) = signature {
        out.push_str(sig.trim_end());
        out.push('\n');
    }
    out.into_bytes()
}

// ── a repository on disk ─────────────────────────────────────────────────────────────────────────────────────────

/// A git repository's object store and refs (`.git` or a bare repository).
pub struct Repo {
    pub dir: PathBuf,
    packs: Mutex<Option<Vec<Pack>>>,
}

impl Repo {
    /// Opens `path`: a bare repository, a worktree with `.git/`, or a worktree whose `.git` file names its git dir.
    pub fn open(path: &Path) -> Result<Repo, String> {
        let dot = path.join(".git");
        let dir = if dot.is_dir() {
            dot
        } else if dot.is_file() {
            let text = std::fs::read_to_string(&dot).map_err(|e| e.to_string())?;
            let gitdir = text
                .trim()
                .strip_prefix("gitdir:")
                .ok_or("a .git file without gitdir")?
                .trim();
            let p = PathBuf::from(gitdir);
            if p.is_absolute() { p } else { path.join(p) }
        } else if path.join("objects").is_dir() {
            path.to_path_buf()
        } else {
            return Err(format!("{} is not a git repository", path.display()));
        };
        Ok(Repo {
            dir,
            packs: Mutex::new(None),
        })
    }

    /// A new bare repository (or the one already there).
    pub fn init_bare(path: &Path) -> Result<Repo, String> {
        for d in ["objects/pack", "refs/heads", "refs/tags"] {
            std::fs::create_dir_all(path.join(d))
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        if !path.join("HEAD").exists() {
            std::fs::write(path.join("HEAD"), "ref: refs/heads/main\n")
                .map_err(|e| e.to_string())?;
            std::fs::write(
                path.join("config"),
                "[core]\n\trepositoryformatversion = 0\n\tbare = true\n",
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(Repo {
            dir: path.to_path_buf(),
            packs: Mutex::new(None),
        })
    }

    /// The shared object store, for a worktree's git dir (`commondir`).
    fn common(&self) -> PathBuf {
        match std::fs::read_to_string(self.dir.join("commondir")) {
            Ok(c) => {
                let p = PathBuf::from(c.trim());
                if p.is_absolute() { p } else { self.dir.join(p) }
            }
            Err(_) => self.dir.clone(),
        }
    }

    fn loose_path(&self, id: &Oid) -> PathBuf {
        let h = hex(id);
        self.common().join("objects").join(&h[..2]).join(&h[2..])
    }

    pub fn has(&self, id: &Oid) -> bool {
        self.loose_path(id).exists()
            || self.with_packs(|packs| packs.iter().any(|p| p.find(id).is_some()))
    }

    fn with_packs<T>(&self, f: impl FnOnce(&[Pack]) -> T) -> T {
        let mut guard = self.packs.lock().unwrap_or_else(|p| p.into_inner());
        if guard.is_none() {
            let mut packs = Vec::new();
            let dir = self.common().join("objects").join("pack");
            for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "idx")
                    && let Ok(pack) = Pack::open(&p.with_extension("pack"), Some(&p))
                {
                    packs.push(pack);
                }
            }
            *guard = Some(packs);
        }
        f(guard.as_ref().unwrap())
    }

    pub fn read(&self, id: &Oid) -> Result<(Kind, Vec<u8>), String> {
        if let Ok(z) = std::fs::read(self.loose_path(id)) {
            let mut raw = Vec::new();
            ZlibDecoder::new(&z[..])
                .read_to_end(&mut raw)
                .map_err(|e| format!("object {}: {e}", hex(id)))?;
            let nul = raw.iter().position(|b| *b == 0).ok_or("bad loose object")?;
            let head = String::from_utf8_lossy(&raw[..nul]).to_string();
            let (k, _) = head.split_once(' ').ok_or("bad loose object")?;
            return Ok((Kind::from_name(k)?, raw[nul + 1..].to_vec()));
        }
        let found = self.with_packs(|packs| {
            for p in packs {
                if let Some(off) = p.find(id) {
                    return Some(p.read_at(off, &|base| self.read(base)));
                }
            }
            None
        });
        found.unwrap_or_else(|| Err(format!("no git object {}", hex(id))))
    }

    /// Writes an object (loose); returns its id.
    pub fn write(&self, kind: Kind, data: &[u8]) -> Result<Oid, String> {
        let id = oid(kind, data);
        let p = self.loose_path(&id);
        if p.exists() {
            return Ok(id);
        }
        std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
        let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
        z.write_all(format!("{} {}\0", kind.name(), data.len()).as_bytes())
            .map_err(|e| e.to_string())?;
        z.write_all(data).map_err(|e| e.to_string())?;
        let bytes = z.finish().map_err(|e| e.to_string())?;
        let tmp = p.with_extension(format!("tmp{}", std::process::id()));
        std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &p).map_err(|e| e.to_string())?;
        Ok(id)
    }

    /// Every ref: `refs/heads/main` → id, from loose refs and `packed-refs`.
    pub fn refs(&self) -> Result<BTreeMap<String, Oid>, String> {
        let mut out = BTreeMap::new();
        let common = self.common();
        if let Ok(packed) = std::fs::read_to_string(common.join("packed-refs")) {
            for line in packed
                .lines()
                .filter(|l| !l.starts_with('#') && !l.starts_with('^'))
            {
                if let Some((id, name)) = line.split_once(' ') {
                    out.insert(name.to_string(), parse_oid(id)?);
                }
            }
        }
        let mut stack = vec![common.join("refs")];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if let Ok(text) = std::fs::read_to_string(&p)
                    && let Ok(id) = parse_oid(&text)
                {
                    let name = p
                        .strip_prefix(&common)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.insert(name, id);
                }
            }
        }
        Ok(out)
    }

    /// A branch, tag or id.
    pub fn resolve(&self, name: &str) -> Result<Oid, String> {
        if let Ok(id) = parse_oid(name) {
            return Ok(id);
        }
        let refs = self.refs()?;
        for candidate in [
            name.to_string(),
            format!("refs/heads/{name}"),
            format!("refs/tags/{name}"),
            format!("refs/remotes/origin/{name}"),
        ] {
            if let Some(id) = refs.get(&candidate) {
                return Ok(*id);
            }
        }
        if name == "HEAD" {
            let head = std::fs::read_to_string(self.dir.join("HEAD")).map_err(|e| e.to_string())?;
            if let Some(r) = head.trim().strip_prefix("ref: ") {
                return self.resolve(r);
            }
            return parse_oid(&head);
        }
        Err(format!("no ref {name}"))
    }

    /// Moves a ref from `old` (`None`: it must not exist) to `new`, under a lock.
    pub fn update_ref(&self, name: &str, old: Option<Oid>, new: Oid) -> Result<(), String> {
        let p = self.common().join(name);
        std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
        let lock = p.with_extension("lock");
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
            .map_err(|e| format!("{name} is locked: {e}"))?;
        let current = self.refs()?.get(name).copied();
        if current != old {
            let _ = std::fs::remove_file(&lock);
            return Err(format!(
                "{name} moved: it is at {}, not {}",
                current.map(|c| hex(&c)).unwrap_or("nothing".into()),
                old.map(|c| hex(&c)).unwrap_or("nothing".into())
            ));
        }
        f.write_all(format!("{}\n", hex(&new)).as_bytes())
            .map_err(|e| e.to_string())?;
        drop(f);
        std::fs::rename(&lock, &p).map_err(|e| e.to_string())
    }

    /// The paths of a tree, flattened: path → (mode, blob id).
    pub fn flatten(&self, tree: &Oid) -> Result<BTreeMap<String, (u32, Oid)>, String> {
        let mut out = BTreeMap::new();
        self.walk(tree, "", &mut out)?;
        Ok(out)
    }

    fn walk(
        &self,
        tree: &Oid,
        prefix: &str,
        out: &mut BTreeMap<String, (u32, Oid)>,
    ) -> Result<(), String> {
        let (_, data) = self.read(tree)?;
        for e in parse_tree(&data)? {
            let path = if prefix.is_empty() {
                e.name.clone()
            } else {
                format!("{prefix}/{}", e.name)
            };
            if e.is_tree() {
                self.walk(&e.id, &path, out)?;
            } else if e.mode != 0o160000 {
                out.insert(path, (e.mode, e.id));
            }
        }
        Ok(())
    }

    /// What changed between two trees: path → the new (mode, id), or `None` when it is gone. Only differing
    /// subtrees are read.
    pub fn diff_trees(
        &self,
        old: Option<&Oid>,
        new: &Oid,
    ) -> Result<BTreeMap<String, Option<(u32, Oid)>>, String> {
        let mut out = BTreeMap::new();
        self.diff_into(old, Some(new), "", &mut out)?;
        Ok(out)
    }

    fn diff_into(
        &self,
        old: Option<&Oid>,
        new: Option<&Oid>,
        prefix: &str,
        out: &mut BTreeMap<String, Option<(u32, Oid)>>,
    ) -> Result<(), String> {
        if old == new {
            return Ok(());
        }
        let entries = |t: Option<&Oid>| -> Result<BTreeMap<String, TreeEntry>, String> {
            match t {
                Some(t) => Ok(parse_tree(&self.read(t)?.1)?
                    .into_iter()
                    .map(|e| (e.name.clone(), e))
                    .collect()),
                None => Ok(BTreeMap::new()),
            }
        };
        let (a, b) = (entries(old)?, entries(new)?);
        let names: std::collections::BTreeSet<&String> = a.keys().chain(b.keys()).collect();
        for name in names {
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let (ea, eb) = (a.get(name), b.get(name));
            if ea == eb {
                continue;
            }
            let ta = ea.filter(|e| e.is_tree()).map(|e| &e.id);
            let tb = eb.filter(|e| e.is_tree()).map(|e| &e.id);
            if ta.is_some() || tb.is_some() {
                self.diff_into(ta, tb, &path, out)?;
            }
            let fa = ea.filter(|e| !e.is_tree() && e.mode != 0o160000);
            let fb = eb.filter(|e| !e.is_tree() && e.mode != 0o160000);
            match (fa, fb) {
                (_, Some(f)) if fa != Some(f) => {
                    out.insert(path.clone(), Some((f.mode, f.id)));
                }
                (Some(_), None) => {
                    out.insert(path.clone(), None);
                }
                _ => {}
            }
        }
        Ok(())
    }
}

// ── packfiles ────────────────────────────────────────────────────────────────────────────────────────────────────

/// A packfile, indexed: by its `.idx` when it has one, else by reading it through once.
pub struct Pack {
    bytes: Vec<u8>,
    index: HashMap<Oid, usize>,
    cache: Mutex<HashMap<usize, (Kind, std::sync::Arc<Vec<u8>>)>>,
}

impl Pack {
    pub fn open(pack: &Path, idx: Option<&Path>) -> Result<Pack, String> {
        let bytes = std::fs::read(pack).map_err(|e| format!("{}: {e}", pack.display()))?;
        match idx.and_then(|i| std::fs::read(i).ok()) {
            Some(i) => Ok(Pack {
                index: read_idx(&i)?,
                bytes,
                cache: Mutex::new(HashMap::new()),
            }),
            None => Pack::from_bytes(bytes),
        }
    }

    /// A pack with no index (one fetched over the wire): every object is read once to learn its id.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Pack, String> {
        if !bytes.starts_with(b"PACK") || bytes.len() < 32 {
            return Err("not a packfile".into());
        }
        let count = u32::from_be_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let mut pack = Pack {
            bytes,
            index: HashMap::new(),
            cache: Mutex::new(HashMap::new()),
        };
        let mut at = 12;
        let mut offsets = Vec::with_capacity(count);
        for _ in 0..count {
            offsets.push(at);
            at = pack.skip(at)?;
        }
        let mut pending: Vec<usize> = Vec::new();
        for off in offsets {
            // REF_DELTA bases may come later in the pack: retry those at the end.
            match pack.read_at(off, &|_| Err("later".into())) {
                Ok((k, data)) => {
                    pack.index.insert(oid(k, &data), off);
                }
                Err(_) => pending.push(off),
            }
        }
        for _ in 0..4 {
            let mut still = Vec::new();
            for off in pending {
                let idx = pack.index.clone();
                let r = pack.read_at(off, &|base| match idx.get(base) {
                    Some(o) => pack.read_at(*o, &|_| Err("deep".into())),
                    None => Err("missing base".into()),
                });
                match r {
                    Ok((k, data)) => {
                        pack.index.insert(oid(k, &data), off);
                    }
                    Err(_) => still.push(off),
                }
            }
            pending = still;
            if pending.is_empty() {
                break;
            }
        }
        if !pending.is_empty() {
            return Err(format!(
                "{} objects in the pack refer to bases it lacks",
                pending.len()
            ));
        }
        Ok(pack)
    }

    pub fn ids(&self) -> Vec<Oid> {
        self.index.keys().copied().collect()
    }

    pub fn find(&self, id: &Oid) -> Option<usize> {
        self.index.get(id).copied()
    }

    pub fn get(&self, id: &Oid) -> Result<(Kind, Vec<u8>), String> {
        let off = self
            .find(id)
            .ok_or_else(|| format!("no git object {}", hex(id)))?;
        self.read_at(off, &|base| self.get(base))
    }

    fn header(&self, at: usize) -> Result<(u8, usize, usize), String> {
        let b = &self.bytes;
        let mut i = at;
        let c = *b.get(i).ok_or("pack cut short")?;
        let t = (c >> 4) & 7;
        let mut size = (c & 15) as usize;
        let mut shift = 4;
        let mut c = c;
        while c & 0x80 != 0 {
            i += 1;
            c = *b.get(i).ok_or("pack cut short")?;
            size |= ((c & 0x7f) as usize) << shift;
            shift += 7;
        }
        Ok((t, size, i + 1))
    }

    fn ofs(&self, mut i: usize) -> Result<(usize, usize), String> {
        let b = &self.bytes;
        let mut c = *b.get(i).ok_or("pack cut short")?;
        let mut n = (c & 0x7f) as usize;
        while c & 0x80 != 0 {
            i += 1;
            c = *b.get(i).ok_or("pack cut short")?;
            n = ((n + 1) << 7) | (c & 0x7f) as usize;
        }
        Ok((n, i + 1))
    }

    fn inflate(&self, at: usize, size: usize) -> Result<(Vec<u8>, usize), String> {
        let mut d = ZlibDecoder::new(&self.bytes[at..]);
        let mut out = Vec::with_capacity(size);
        d.read_to_end(&mut out)
            .map_err(|e| format!("pack inflate: {e}"))?;
        Ok((out, at + d.total_in() as usize))
    }

    fn skip(&self, at: usize) -> Result<usize, String> {
        let (t, size, mut i) = self.header(at)?;
        if t == 6 {
            i = self.ofs(i)?.1;
        } else if t == 7 {
            i += 20;
        }
        Ok(self.inflate(i, size)?.1)
    }

    /// The object at `at`, deltas applied. `by_id` reads a REF_DELTA's base.
    pub fn read_at(
        &self,
        at: usize,
        by_id: &dyn Fn(&Oid) -> Result<(Kind, Vec<u8>), String>,
    ) -> Result<(Kind, Vec<u8>), String> {
        if let Some((k, d)) = self
            .cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&at)
        {
            return Ok((*k, d.to_vec()));
        }
        let (t, size, i) = self.header(at)?;
        let (kind, data) = match t {
            1..=4 => (Kind::from_pack(t)?, self.inflate(i, size)?.0),
            6 => {
                let (back, j) = self.ofs(i)?;
                let base_at = at.checked_sub(back).ok_or("bad delta offset")?;
                let (bk, base) = self.read_at(base_at, by_id)?;
                (bk, apply_delta(&base, &self.inflate(j, size)?.0)?)
            }
            7 => {
                let base_id: Oid = self
                    .bytes
                    .get(i..i + 20)
                    .ok_or("pack cut short")?
                    .try_into()
                    .unwrap();
                let (bk, base) = match self.find(&base_id) {
                    Some(o) => self.read_at(o, by_id)?,
                    None => by_id(&base_id)?,
                };
                (bk, apply_delta(&base, &self.inflate(i + 20, size)?.0)?)
            }
            other => return Err(format!("pack entry type {other}")),
        };
        // Keep bases that deltas are likely to need.
        let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        if cache.len() > 2048 {
            cache.clear();
        }
        if matches!(kind, Kind::Tree | Kind::Commit) || data.len() < 64 << 10 {
            cache.insert(at, (kind, std::sync::Arc::new(data.clone())));
        }
        Ok((kind, data))
    }
}

fn read_idx(i: &[u8]) -> Result<HashMap<Oid, usize>, String> {
    if !i.starts_with(&[0xff, b't', b'O', b'c']) {
        return Err("only pack index version 2 is read".into());
    }
    let n = u32::from_be_bytes(i[8 + 255 * 4..8 + 256 * 4].try_into().unwrap()) as usize;
    let ids = 8 + 256 * 4;
    let offs = ids + n * 20 + n * 4;
    let large = offs + n * 4;
    let mut out = HashMap::with_capacity(n);
    for k in 0..n {
        let id: Oid = i[ids + k * 20..ids + k * 20 + 20].try_into().unwrap();
        let o = u32::from_be_bytes(i[offs + k * 4..offs + k * 4 + 4].try_into().unwrap());
        let off = if o & 0x8000_0000 != 0 {
            let j = (o & 0x7fff_ffff) as usize;
            u64::from_be_bytes(i[large + j * 8..large + j * 8 + 8].try_into().unwrap()) as usize
        } else {
            o as usize
        };
        out.insert(id, off);
    }
    Ok(out)
}

pub fn apply_delta(base: &[u8], delta: &[u8]) -> Result<Vec<u8>, String> {
    let mut i = 0;
    let mut varint = || -> Result<usize, String> {
        let mut n = 0usize;
        let mut shift = 0;
        loop {
            let c = *delta.get(i).ok_or("delta cut short")?;
            i += 1;
            n |= ((c & 0x7f) as usize) << shift;
            shift += 7;
            if c & 0x80 == 0 {
                return Ok(n);
            }
        }
    };
    let src = varint()?;
    let dst = varint()?;
    if src != base.len() {
        return Err("the delta's base has the wrong size".into());
    }
    let mut out = Vec::with_capacity(dst);
    while i < delta.len() {
        let op = delta[i];
        i += 1;
        if op & 0x80 != 0 {
            let mut off = 0usize;
            let mut len = 0usize;
            for k in 0..4 {
                if op & (1 << k) != 0 {
                    off |= (*delta.get(i).ok_or("delta cut short")? as usize) << (8 * k);
                    i += 1;
                }
            }
            for k in 0..3 {
                if op & (0x10 << k) != 0 {
                    len |= (*delta.get(i).ok_or("delta cut short")? as usize) << (8 * k);
                    i += 1;
                }
            }
            if len == 0 {
                len = 0x10000;
            }
            out.extend_from_slice(
                base.get(off..off + len)
                    .ok_or("delta copies past its base")?,
            );
        } else if op != 0 {
            let n = op as usize;
            out.extend_from_slice(delta.get(i..i + n).ok_or("delta cut short")?);
            i += n;
        } else {
            return Err("delta op 0".into());
        }
    }
    if out.len() != dst {
        return Err("the delta made the wrong size".into());
    }
    Ok(out)
}

/// A packfile of whole objects (no deltas).
pub fn write_pack(objects: &[(Kind, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"PACK");
    out.extend_from_slice(&2u32.to_be_bytes());
    out.extend_from_slice(&(objects.len() as u32).to_be_bytes());
    for (k, data) in objects {
        let mut size = data.len();
        let mut c = (k.pack_type() << 4) | (size & 15) as u8;
        size >>= 4;
        while size > 0 {
            out.push(c | 0x80);
            c = (size & 0x7f) as u8;
            size >>= 7;
        }
        out.push(c);
        let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
        z.write_all(data).unwrap();
        out.extend_from_slice(&z.finish().unwrap());
    }
    let sum: [u8; 20] = Sha1::digest(&out).into();
    out.extend_from_slice(&sum);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_match_git() {
        // `printf 'hello\n' | git hash-object --stdin`
        assert_eq!(
            hex(&oid(Kind::Blob, b"hello\n")),
            "ce013625030ba8dba906f756967f9e9ca394464a"
        );
        // The empty tree.
        assert_eq!(
            hex(&oid(Kind::Tree, b"")),
            "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
        );
    }

    #[test]
    fn packs_round_trip() {
        let objs = vec![
            (Kind::Blob, b"one\n".to_vec()),
            (Kind::Blob, vec![7u8; 5000]),
        ];
        let pack = Pack::from_bytes(write_pack(&objs)).unwrap();
        for (k, d) in &objs {
            assert_eq!(pack.get(&oid(*k, d)).unwrap(), (*k, d.clone()));
        }
    }

    #[test]
    fn deltas_apply() {
        let base = b"hello world".to_vec();
        // size 11 → 16: copy 0..5, insert " there", copy 5..10.
        let mut d = vec![11, 16];
        d.extend_from_slice(&[0x80 | 0x10, 5]);
        d.push(6);
        d.extend_from_slice(b" there");
        d.extend_from_slice(&[0x80 | 0x01 | 0x10, 5, 5]);
        assert_eq!(
            apply_delta(&base, &d).unwrap(),
            b"hello there world".to_vec()[..16].to_vec()
        );
    }

    #[test]
    fn commits_round_trip() {
        let c = Commit {
            tree: oid(Kind::Tree, b""),
            parents: vec![],
            author: Person {
                name: "Irfan".into(),
                email: "owner@example.com".into(),
                time: 1_700_000_000,
                tz: "+0200".into(),
            },
            committer: Person {
                name: "Irfan".into(),
                email: "owner@example.com".into(),
                time: 1_700_000_000,
                tz: "+0200".into(),
            },
            extra: vec![],
            message: "Nodes screen works on Mac and Windows hubs\n".into(),
        };
        let bytes = write_commit(&c, None);
        assert_eq!(parse_commit(&bytes).unwrap(), c);
        let signed = write_commit(
            &c,
            Some("-----BEGIN SSH SIGNATURE-----\nabc\n-----END SSH SIGNATURE-----\n"),
        );
        let back = parse_commit(&signed).unwrap();
        assert_eq!(back.extra[0].0, "gpgsig");
        assert!(back.extra[0].1.contains("END SSH SIGNATURE"));
    }
}
