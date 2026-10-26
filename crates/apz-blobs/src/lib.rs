//! The plaza's object store (PLAN 30.5, W67): blobs by BLAKE3 hash.
//!
//! Blobs live in append-only packs, `pack-000001.wpk`, … Each entry is
//! `stored_len u32 | hash 32 | flags u8 | raw_len u64 | stored bytes`. Flags: 1 = zstd, 2 = chunk manifest. A blob
//! over [`CHUNK_OVER`] bytes is cut into content-defined chunks (FastCDC), each stored as its own blob, and the
//! blob's hash names a manifest listing them, so a large blob that changes a little shares most chunks with its last
//! version, and a range read reads only the chunks it covers.
//!
//! The packs are the truth. Where each blob lives is indexed in SQLite (`index.db`, typed tables): opening reads only
//! the packs' tails appended after the index's checkpoint (`pack.indexed_len`), so it takes the same time at any
//! size. A put is durable when it returns: its pack bytes are synced before its index row is written.
//!
//! Deletion is mark and sweep ([`Blobs::sweep`]): the caller says which blobs are referenced; a blob touched (written
//! or put again) within the grace window is always kept, so a put racing a sweep is safe; packs whose live share
//! falls below a threshold are compacted (their live entries copied forward, the pack removed).

mod hash;

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use apz_store::rusqlite::{OptionalExtension, params};
use apz_store::{Durability, FileSpec, Migration, Store, now_ms};

pub use hash::{Hash, hex, unhex};

/// Blobs larger than this are stored as chunks.
pub const CHUNK_OVER: usize = 4 << 20;
/// The smallest, average and largest chunk (FastCDC).
const CHUNK_MIN: u32 = 256 << 10;
const CHUNK_AVG: u32 = 1 << 20;
const CHUNK_MAX: u32 = 4 << 20;
/// A pack is closed for appends past this size.
const PACK_BYTES: u64 = 1 << 30;
const FLAG_ZSTD: u8 = 1;
const FLAG_MANIFEST: u8 = 2;
const HEAD: usize = 4 + 32 + 1 + 8;

const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "index",
    sql: include_str!("../sql/0001_index.sql"),
}];

const FILES: &[FileSpec] = &[FileSpec {
    name: "index",
    file: "index.db",
    // The packs are the truth and the tail scan rebuilds what the index missed: a crash costs nothing.
    durability: Durability::Normal,
    migrations: MIGRATIONS,
}];

/// The index of a test's throwaway store ([`Blobs::scratch`]).
const SCRATCH_FILES: &[FileSpec] = &[FileSpec {
    durability: Durability::Scratch,
    ..FILES[0]
}];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Loc {
    pack: u32,
    offset: u64,
    stored: u32,
    flags: u8,
    raw: u64,
}

/// What a [`Blobs::fetch_missing`] did: the chunks it fetched, the chunks it already held, and the bytes fetched.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Fetched {
    pub fetched: usize,
    pub reused: usize,
    pub bytes: u64,
}

/// What a sweep did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SweepReport {
    /// Blobs kept: referenced, chunks of referenced manifests, or touched within the grace window.
    pub live: usize,
    /// Blobs dropped from the index.
    pub dead: usize,
    /// Pack bytes given back to the disk (packs compacted or removed).
    pub freed_bytes: u64,
    pub packs_compacted: usize,
    pub packs_removed: usize,
}

/// How a sweep runs.
#[derive(Debug, Clone, Copy)]
pub struct SweepOptions {
    /// A blob written or put again this recently is kept whatever the roots say.
    pub grace: Duration,
    /// A pack whose live bytes fall below this share of its size is compacted.
    pub compact_below: f64,
}

impl Default for SweepOptions {
    fn default() -> Self {
        SweepOptions {
            grace: Duration::from_secs(3600),
            compact_below: 0.5,
        }
    }
}

struct Current {
    file: File,
    no: u32,
    len: u64,
}

/// The object store. Shared by the threads of its process (`Arc<Blobs>`); one process per folder (its lock).
pub struct Blobs {
    dir: PathBuf,
    index: Store,
    /// The pack being appended to. Held for a whole put, so a blob is written once.
    current: Mutex<Current>,
    /// One sweep at a time.
    sweeping: Mutex<()>,
    /// Entries read from the packs (a range read reads only the chunks it covers).
    entry_reads: AtomicU64,
    /// Whether a put syncs its pack before it answers (not in a test's scratch store).
    sync: bool,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn pack_path(dir: &Path, no: u32) -> PathBuf {
    dir.join(format!("pack-{no:06}.wpk"))
}

/// Where a large blob is cut: `(offset, length)` of each content-defined chunk. Uploaders cut the same way, so they
/// send only the chunks the store lacks.
pub fn chunk_plan(bytes: &[u8]) -> Vec<(usize, usize)> {
    fastcdc::v2020::FastCDC::new(bytes, CHUNK_MIN, CHUNK_AVG, CHUNK_MAX)
        .map(|c| (c.offset, c.length))
        .collect()
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn i64_of(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

impl Blobs {
    /// Opens the store in `dir` (created if needed): opens its index, indexes what the packs hold past it (a crash
    /// between a pack write and its index row), and cuts a torn entry off the last pack.
    pub fn open(dir: &Path) -> Result<Blobs, String> {
        Blobs::open_with(dir, FILES, true)
    }

    /// [`Blobs::open`] for a test's throwaway store: neither the packs nor the index are synced, so what a put wrote
    /// survives the process's crash, not the machine's. A store tests write frame by frame waited on a sync apiece.
    pub fn scratch(dir: &Path) -> Result<Blobs, String> {
        Blobs::open_with(dir, SCRATCH_FILES, false)
    }

    fn open_with(dir: &Path, files: &[FileSpec], sync: bool) -> Result<Blobs, String> {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let index = Store::open(dir, files).map_err(err)?;
        let mut packs: Vec<u32> = fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(Result::ok)
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.strip_prefix("pack-")?
                    .strip_suffix(".wpk")?
                    .parse::<u32>()
                    .ok()
            })
            .collect();
        packs.sort_unstable();
        for no in &packs {
            index_tail(index.db("index"), dir, *no)?;
        }
        let no = packs.last().copied().unwrap_or(1);
        let path = pack_path(dir, no);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let len = file.metadata().map_err(err)?.len();
        Ok(Blobs {
            dir: dir.to_path_buf(),
            current: Mutex::new(Current { file, no, len }),
            index,
            sweeping: Mutex::new(()),
            entry_reads: AtomicU64::new(0),
            sync,
        })
    }

    fn db(&self) -> &apz_store::Db {
        self.index.db("index")
    }

    fn loc(&self, hash: &Hash) -> Option<Loc> {
        let key = hash.0;
        self.db()
            .read(move |conn| {
                Ok(conn
                    .prepare_cached(
                        "SELECT pack, \"offset\", stored, flags, raw FROM blob WHERE hash = ?1",
                    )?
                    .query_row([key], |r| {
                        Ok(Loc {
                            pack: r.get(0)?,
                            offset: u64::try_from(r.get::<_, i64>(1)?).unwrap_or(0),
                            stored: r.get(2)?,
                            flags: r.get(3)?,
                            raw: u64::try_from(r.get::<_, i64>(4)?).unwrap_or(0),
                        })
                    })
                    .optional()?)
            })
            .ok()
            .flatten()
    }

    pub fn has(&self, hash: &Hash) -> bool {
        self.loc(hash).is_some()
    }

    /// The hashes of `hashes` the store does not hold.
    pub fn missing(&self, hashes: &[Hash]) -> Vec<Hash> {
        hashes.iter().filter(|h| !self.has(h)).copied().collect()
    }

    /// Marks a held blob touched now (so a sweep running now spares it). False when the store does not hold it.
    fn touch(&self, hash: &Hash) -> Result<bool, String> {
        let key = hash.0;
        let now = now_ms();
        let w = self
            .db()
            .write(move |conn| {
                Ok(conn
                    .prepare_cached("UPDATE blob SET touched_ms = ?2 WHERE hash = ?1")?
                    .execute(params![key, now])?)
            })
            .map_err(err)?;
        Ok(w.value == 1)
    }

    /// Stores `bytes` and returns their hash. Storing what the store holds is a no-op (it only touches it).
    pub fn put(&self, bytes: &[u8]) -> Result<Hash, String> {
        let hash = Hash::of(bytes);
        if self.touch(&hash)? {
            return Ok(hash);
        }
        if bytes.len() > CHUNK_OVER {
            let mut manifest = Vec::new();
            for (offset, len) in chunk_plan(bytes) {
                let piece = &bytes[offset..offset + len];
                let h = Hash::of(piece);
                if !self.touch(&h)? {
                    self.write_entry(h, piece, 0)?;
                }
                manifest.extend_from_slice(&h.0);
                manifest.extend_from_slice(&(len as u64).to_le_bytes());
            }
            self.write_entry_raw(hash, &manifest, FLAG_MANIFEST, bytes.len() as u64)?;
        } else {
            self.write_entry(hash, bytes, 0)?;
        }
        Ok(hash)
    }

    /// Stores what `reader` yields without holding it all in memory, and returns its hash and size. A stream over
    /// [`CHUNK_OVER`] is stored as chunks as it arrives (the same chunks [`chunk_plan`] would cut).
    pub fn put_reader(&self, mut reader: impl Read) -> Result<(Hash, u64), String> {
        let mut head = Vec::with_capacity(CHUNK_OVER + 1);
        (&mut reader)
            .take(CHUNK_OVER as u64 + 1)
            .read_to_end(&mut head)
            .map_err(err)?;
        if head.len() <= CHUNK_OVER {
            let hash = self.put(&head)?;
            return Ok((hash, head.len() as u64));
        }
        let mut hasher = blake3::Hasher::new();
        let mut manifest = Vec::new();
        let mut total = 0u64;
        let stream = std::io::Cursor::new(head).chain(reader);
        for chunk in fastcdc::v2020::StreamCDC::new(stream, CHUNK_MIN, CHUNK_AVG, CHUNK_MAX) {
            let chunk = chunk.map_err(err)?;
            hasher.update(&chunk.data);
            total += chunk.length as u64;
            let h = Hash::of(&chunk.data);
            if !self.touch(&h)? {
                self.write_entry(h, &chunk.data, 0)?;
            }
            manifest.extend_from_slice(&h.0);
            manifest.extend_from_slice(&(chunk.length as u64).to_le_bytes());
        }
        let hash = Hash(*hasher.finalize().as_bytes());
        if !self.touch(&hash)? {
            self.write_entry_raw(hash, &manifest, FLAG_MANIFEST, total)?;
        }
        Ok((hash, total))
    }

    /// Stores bytes the caller says hash to `hash` (an upload); refuses them if they do not.
    pub fn put_checked(&self, hash: &Hash, bytes: &[u8]) -> Result<(), String> {
        let got = Hash::of(bytes);
        if got != *hash {
            return Err(format!(
                "the bytes hash to {}, not {}",
                got.short(),
                hash.short()
            ));
        }
        self.put(bytes).map(|_| ())
    }

    /// The stored bytes of an entry, decompressed (a manifest's raw list, or a blob's content).
    fn read_entry(&self, hash: &Hash) -> Result<(Loc, Vec<u8>), String> {
        // A sweep may move an entry between the index lookup and the read: look it up again once.
        let mut last = String::new();
        for _ in 0..2 {
            let loc = self
                .loc(hash)
                .ok_or_else(|| format!("no blob {}", hash.short()))?;
            match self.read_stored(loc) {
                Ok(stored) => {
                    let raw = if loc.flags & FLAG_ZSTD != 0 {
                        zstd::decode_all(&stored[..])
                            .map_err(|e| format!("blob {}: {e}", hash.short()))?
                    } else {
                        stored
                    };
                    return Ok((loc, raw));
                }
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    /// A blob's content.
    pub fn get(&self, hash: &Hash) -> Result<Vec<u8>, String> {
        let (loc, raw) = self.read_entry(hash)?;
        if loc.flags & FLAG_MANIFEST != 0 {
            let mut out = Vec::with_capacity(usize::try_from(loc.raw).unwrap_or(0));
            for (h, _) in parse_manifest(&raw)? {
                out.extend_from_slice(&self.get(&h)?);
            }
            if Hash::of(&out) != *hash {
                return Err(format!("blob {} does not match its hash", hash.short()));
            }
            return Ok(out);
        }
        Ok(raw)
    }

    /// A reader over a blob's content that holds one chunk at a time.
    pub fn reader(&self, hash: &Hash) -> Result<BlobReader<'_>, String> {
        let chunks = match self.chunks(hash)? {
            Some(chunks) => chunks,
            None => vec![(*hash, self.size(hash).unwrap_or(0))],
        };
        Ok(BlobReader {
            blobs: self,
            chunks: chunks
                .into_iter()
                .map(|(h, _)| h)
                .collect::<Vec<_>>()
                .into_iter(),
            buf: Vec::new(),
            at: 0,
        })
    }

    /// `len` bytes of a blob's content from `offset` (fewer at its end). A chunked blob reads only the chunks the
    /// range covers.
    pub fn read_range(&self, hash: &Hash, offset: u64, len: u64) -> Result<Vec<u8>, String> {
        let end = offset.saturating_add(len);
        match self.chunks(hash)? {
            None => {
                let all = self.get(hash)?;
                let from = usize::try_from(offset).unwrap_or(usize::MAX).min(all.len());
                let to = usize::try_from(end).unwrap_or(usize::MAX).min(all.len());
                Ok(all[from..to].to_vec())
            }
            Some(chunks) => {
                let mut out = Vec::new();
                let mut start = 0u64;
                for (h, size) in chunks {
                    let stop = start + size;
                    if stop > offset && start < end {
                        let bytes = self.get(&h)?;
                        let from = usize::try_from(offset.saturating_sub(start)).unwrap_or(0);
                        let to = usize::try_from(end.min(stop) - start)
                            .unwrap_or(bytes.len())
                            .min(bytes.len());
                        out.extend_from_slice(&bytes[from..to]);
                    }
                    if stop >= end {
                        break;
                    }
                    start = stop;
                }
                Ok(out)
            }
        }
    }

    /// The chunks of a chunked blob, `None` for a blob stored whole.
    pub fn chunks(&self, hash: &Hash) -> Result<Option<Vec<(Hash, u64)>>, String> {
        let loc = self
            .loc(hash)
            .ok_or_else(|| format!("no blob {}", hash.short()))?;
        if loc.flags & FLAG_MANIFEST == 0 {
            return Ok(None);
        }
        let (_, raw) = self.read_entry(hash)?;
        parse_manifest(&raw).map(Some)
    }

    /// Records a chunked blob whose chunks were uploaded one by one; refuses it unless they add up to `hash`.
    pub fn put_manifest(&self, hash: &Hash, chunks: &[(Hash, u64)]) -> Result<(), String> {
        if self.touch(hash)? {
            return Ok(());
        }
        let mut hasher = blake3::Hasher::new();
        let mut manifest = Vec::with_capacity(chunks.len() * 40);
        let mut total = 0u64;
        for (h, len) in chunks {
            let bytes = self.get(h)?;
            if bytes.len() as u64 != *len {
                return Err(format!(
                    "chunk {} is {} bytes, not {len}",
                    h.short(),
                    bytes.len()
                ));
            }
            hasher.update(&bytes);
            manifest.extend_from_slice(&h.0);
            manifest.extend_from_slice(&len.to_le_bytes());
            total += len;
        }
        if Hash(*hasher.finalize().as_bytes()) != *hash {
            return Err(format!("the chunks do not add up to {}", hash.short()));
        }
        self.write_entry_raw(*hash, &manifest, FLAG_MANIFEST, total)
    }

    /// Records a chunked blob another store published (`root`, listed as `chunks`), fetching only the chunks this
    /// store lacks: `fetch` gives the bytes of a missing chunk (the publisher's copy), which must have the listed
    /// length and hash. A blob this store already holds is not fetched at all.
    pub fn fetch_missing(
        &self,
        root: &Hash,
        chunks: &[(Hash, u64)],
        mut fetch: impl FnMut(&Hash) -> Result<Vec<u8>, String>,
    ) -> Result<Fetched, String> {
        if self.has(root) {
            return Ok(Fetched {
                fetched: 0,
                reused: chunks.len(),
                bytes: 0,
            });
        }
        let mut out = Fetched::default();
        for (h, len) in chunks {
            if self.has(h) {
                out.reused += 1;
                continue;
            }
            let bytes = fetch(h)?;
            if bytes.len() as u64 != *len {
                return Err(format!(
                    "chunk {} is {} bytes, not {len}",
                    h.short(),
                    bytes.len()
                ));
            }
            self.put_checked(h, &bytes)?;
            out.fetched += 1;
            out.bytes += len;
        }
        self.put_manifest(root, chunks)?;
        Ok(out)
    }

    /// The size of a blob's content.
    pub fn size(&self, hash: &Hash) -> Option<u64> {
        self.loc(hash).map(|l| l.raw)
    }

    /// Blobs stored and their bytes on disk.
    pub fn stats(&self) -> (usize, u64) {
        self.db()
            .read(|conn| {
                Ok(conn.query_row(
                    "SELECT count(*), coalesce(sum(stored), 0) FROM blob",
                    [],
                    |r| {
                        Ok((
                            usize::try_from(r.get::<_, i64>(0)?).unwrap_or(0),
                            u64::try_from(r.get::<_, i64>(1)?).unwrap_or(0),
                        ))
                    },
                )?)
            })
            .unwrap_or((0, 0))
    }

    fn write_entry(&self, hash: Hash, bytes: &[u8], flags: u8) -> Result<(), String> {
        let compressed = zstd::encode_all(bytes, 3).map_err(err)?;
        if compressed.len() + compressed.len() / 10 < bytes.len() {
            self.write_entry_raw(hash, &compressed, flags | FLAG_ZSTD, bytes.len() as u64)
        } else {
            self.write_entry_raw(hash, bytes, flags, bytes.len() as u64)
        }
    }

    /// Appends an entry to the current pack, syncs it, then indexes it. Held under the pack lock throughout, so two
    /// puts of one blob write it once.
    fn write_entry_raw(
        &self,
        hash: Hash,
        stored: &[u8],
        flags: u8,
        raw: u64,
    ) -> Result<(), String> {
        let mut current = lock(&self.current);
        if self.loc(&hash).is_some() {
            return Ok(());
        }
        let loc = self.append(&mut current, hash, stored, flags, raw)?;
        self.index_row(hash, loc, current.no, current.len)
    }

    fn append(
        &self,
        current: &mut Current,
        hash: Hash,
        stored: &[u8],
        flags: u8,
        raw: u64,
    ) -> Result<Loc, String> {
        if current.len > 0 && current.len + stored.len() as u64 > PACK_BYTES {
            let no = current.no + 1;
            let path = pack_path(&self.dir, no);
            current.file = OpenOptions::new()
                .create(true)
                .read(true)
                .append(true)
                .open(&path)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            current.no = no;
            current.len = 0;
        }
        let stored_len =
            u32::try_from(stored.len()).map_err(|_| "an entry is at most 4 GiB".to_string())?;
        let mut head = [0u8; HEAD];
        head[..4].copy_from_slice(&stored_len.to_le_bytes());
        head[4..36].copy_from_slice(&hash.0);
        head[36] = flags;
        head[37..].copy_from_slice(&raw.to_le_bytes());
        current
            .file
            .write_all(&head)
            .map_err(|e| format!("blob write: {e}"))?;
        current
            .file
            .write_all(stored)
            .map_err(|e| format!("blob write: {e}"))?;
        if self.sync {
            current
                .file
                .sync_data()
                .map_err(|e| format!("blob sync: {e}"))?;
        }
        let loc = Loc {
            pack: current.no,
            offset: current.len + HEAD as u64,
            stored: stored_len,
            flags,
            raw,
        };
        current.len += (head.len() + stored.len()) as u64;
        Ok(loc)
    }

    fn index_row(&self, hash: Hash, loc: Loc, pack: u32, indexed_len: u64) -> Result<(), String> {
        let touched = now_ms();
        self.db()
            .write(move |conn| {
                conn.prepare_cached(
                    "INSERT INTO blob (hash, pack, \"offset\", stored, flags, raw, touched_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT (hash) DO UPDATE SET pack = excluded.pack, \"offset\" = excluded.\"offset\",
                         stored = excluded.stored, flags = excluded.flags, touched_ms = excluded.touched_ms",
                )?
                .execute(params![
                    hash.0,
                    loc.pack,
                    i64_of(loc.offset),
                    loc.stored,
                    loc.flags,
                    i64_of(loc.raw),
                    touched
                ])?;
                conn.prepare_cached(
                    "INSERT INTO pack (no, indexed_len) VALUES (?1, ?2)
                     ON CONFLICT (no) DO UPDATE SET indexed_len = max(indexed_len, excluded.indexed_len)",
                )?
                .execute(params![pack, i64_of(indexed_len)])?;
                Ok(())
            })
            .map_err(err)?;
        Ok(())
    }

    /// How many entries were read from the packs so far.
    pub fn entry_reads(&self) -> u64 {
        self.entry_reads.load(Ordering::Relaxed)
    }

    fn read_stored(&self, loc: Loc) -> Result<Vec<u8>, String> {
        use std::io::{Seek, SeekFrom};
        self.entry_reads.fetch_add(1, Ordering::Relaxed);
        let path = pack_path(&self.dir, loc.pack);
        let mut f = File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        f.seek(SeekFrom::Start(loc.offset)).map_err(err)?;
        let mut buf = vec![0u8; loc.stored as usize];
        f.read_exact(&mut buf)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(buf)
    }

    /// Mark and sweep. `referenced` says whether a blob is referenced (by the hub's databases, Weft's log, the bases'
    /// manifests); a referenced manifest keeps its chunks; a blob touched within `options.grace` is kept whatever
    /// `referenced` says. Unreferenced blobs leave the index; packs whose live share falls below
    /// `options.compact_below` are compacted (their live entries appended to the current pack) and removed.
    pub fn sweep(
        &self,
        referenced: &dyn Fn(&Hash) -> bool,
        options: SweepOptions,
    ) -> Result<SweepReport, String> {
        let _one = lock(&self.sweeping);
        let cutoff = now_ms() - i64::try_from(options.grace.as_millis()).unwrap_or(i64::MAX);
        // Mark.
        type Row = (Hash, u32, u32, u8, i64);
        let rows: Vec<Row> = self
            .db()
            .read(|conn| {
                let mut stmt =
                    conn.prepare("SELECT hash, pack, stored, flags, touched_ms FROM blob")?;
                let rows = stmt.query_map([], |r| {
                    let h: Vec<u8> = r.get(0)?;
                    Ok((
                        Hash(h.try_into().unwrap_or([0; 32])),
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                    ))
                })?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .map_err(err)?;
        let mut live: HashSet<Hash> = HashSet::new();
        for (hash, _, _, flags, touched) in &rows {
            if *touched > cutoff || referenced(hash) {
                live.insert(*hash);
                if flags & FLAG_MANIFEST != 0
                    && let Ok(Some(chunks)) = self.chunks(hash)
                {
                    live.extend(chunks.into_iter().map(|(h, _)| h));
                }
            }
        }
        // Sweep: drop the dead from the index, unless touched since the mark (a put racing the sweep).
        let dead: Vec<Vec<u8>> = rows
            .iter()
            .filter(|r| !live.contains(&r.0))
            .map(|r| r.0.0.to_vec())
            .collect();
        let dropped = self
            .db()
            .write(move |conn| {
                let mut n = 0usize;
                for key in &dead {
                    n += conn
                        .prepare_cached("DELETE FROM blob WHERE hash = ?1 AND touched_ms <= ?2")?
                        .execute(params![key, cutoff])?;
                }
                Ok(n)
            })
            .map_err(err)?
            .value;
        let mut report = SweepReport {
            live: live.len().min(rows.len()),
            dead: dropped,
            ..SweepReport::default()
        };
        // Compact: every pack but the current one whose live share is low.
        let current_no = lock(&self.current).no;
        let mut packs: Vec<u32> = rows.iter().map(|r| r.1).collect();
        packs.sort_unstable();
        packs.dedup();
        let on_disk: Vec<u32> = fs::read_dir(&self.dir)
            .map_err(err)?
            .filter_map(Result::ok)
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.strip_prefix("pack-")?
                    .strip_suffix(".wpk")?
                    .parse::<u32>()
                    .ok()
            })
            .collect();
        for no in on_disk {
            if no == current_no {
                continue;
            }
            let path = pack_path(&self.dir, no);
            let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let entries: Vec<(Hash, Loc)> = self
                .db()
                .read(move |conn| {
                    let mut stmt = conn.prepare(
                        "SELECT hash, \"offset\", stored, flags, raw FROM blob WHERE pack = ?1",
                    )?;
                    let rows = stmt.query_map([no], |r| {
                        let h: Vec<u8> = r.get(0)?;
                        Ok((
                            Hash(h.try_into().unwrap_or([0; 32])),
                            Loc {
                                pack: no,
                                offset: u64::try_from(r.get::<_, i64>(1)?).unwrap_or(0),
                                stored: r.get(2)?,
                                flags: r.get(3)?,
                                raw: u64::try_from(r.get::<_, i64>(4)?).unwrap_or(0),
                            },
                        ))
                    })?;
                    Ok(rows.collect::<Result<Vec<_>, _>>()?)
                })
                .map_err(err)?;
            let live_bytes: u64 = entries
                .iter()
                .map(|(_, l)| u64::from(l.stored) + HEAD as u64)
                .sum();
            if size > 0 && (live_bytes as f64) / (size as f64) >= options.compact_below {
                continue;
            }
            for (hash, loc) in &entries {
                let stored = self.read_stored(*loc)?;
                let mut current = lock(&self.current);
                let new = self.append(&mut current, *hash, &stored, loc.flags, loc.raw)?;
                let (cur_no, cur_len) = (current.no, current.len);
                // Moves the row only if it still points at the old place (a put may have rewritten it).
                let (key, old) = (hash.0.to_vec(), *loc);
                self.db()
                    .write(move |conn| {
                        conn.prepare_cached(
                            "UPDATE blob SET pack = ?2, \"offset\" = ?3 WHERE hash = ?1 AND pack = ?4 AND \"offset\" = ?5",
                        )?
                        .execute(params![key, new.pack, i64_of(new.offset), old.pack, i64_of(old.offset)])?;
                        conn.prepare_cached(
                            "INSERT INTO pack (no, indexed_len) VALUES (?1, ?2)
                             ON CONFLICT (no) DO UPDATE SET indexed_len = max(indexed_len, excluded.indexed_len)",
                        )?
                        .execute(params![cur_no, i64_of(cur_len)])?;
                        Ok(())
                    })
                    .map_err(err)?;
            }
            let still: i64 = self
                .db()
                .read(move |conn| {
                    Ok(
                        conn.query_row("SELECT count(*) FROM blob WHERE pack = ?1", [no], |r| {
                            r.get(0)
                        })?,
                    )
                })
                .map_err(err)?;
            if still == 0 {
                self.db()
                    .write(move |conn| {
                        conn.execute("DELETE FROM pack WHERE no = ?1", [no])?;
                        Ok(())
                    })
                    .map_err(err)?;
                fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                report.freed_bytes += size.saturating_sub(live_bytes);
                if entries.is_empty() {
                    report.packs_removed += 1;
                } else {
                    report.packs_compacted += 1;
                }
            }
        }
        Ok(report)
    }

    /// Waits until every index write so far is committed.
    pub fn flush(&self) -> Result<(), String> {
        self.db().flush().map(|_| ()).map_err(err)
    }

    /// The folder the store lives in.
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// Indexes the entries of pack `no` past its checkpoint; a torn entry at its end is cut off.
fn index_tail(db: &apz_store::Db, dir: &Path, no: u32) -> Result<(), String> {
    let from: u64 = db
        .read(move |conn| {
            Ok(conn
                .query_row("SELECT indexed_len FROM pack WHERE no = ?1", [no], |r| {
                    r.get::<_, i64>(0)
                })
                .optional()?
                .map_or(0, |v| u64::try_from(v).unwrap_or(0)))
        })
        .map_err(err)?;
    let path = pack_path(dir, no);
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let len = file.metadata().map_err(err)?.len();
    if from >= len {
        return Ok(());
    }
    let mut rows = Vec::new();
    let mut at = from;
    let mut good = from;
    let mut head = [0u8; HEAD];
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(at)).map_err(err)?;
    while at + HEAD as u64 <= len {
        file.read_exact(&mut head).map_err(err)?;
        let stored = u32::from_le_bytes(head[..4].try_into().expect("4"));
        let end = at + HEAD as u64 + u64::from(stored);
        if end > len {
            break;
        }
        let hash = Hash(head[4..36].try_into().expect("32"));
        let flags = head[36];
        let raw = u64::from_le_bytes(head[37..45].try_into().expect("8"));
        rows.push((
            hash,
            Loc {
                pack: no,
                offset: at + HEAD as u64,
                stored,
                flags,
                raw,
            },
        ));
        file.seek(SeekFrom::Start(end)).map_err(err)?;
        at = end;
        good = end;
    }
    if good < len {
        eprintln!(
            "blobs: {} ends in a torn entry; cutting it off at {good}",
            path.display()
        );
        file.set_len(good).map_err(err)?;
        file.sync_all().map_err(err)?;
    }
    let touched = now_ms();
    db.write(move |conn| {
        for (hash, loc) in &rows {
            conn.prepare_cached(
                "INSERT INTO blob (hash, pack, \"offset\", stored, flags, raw, touched_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT (hash) DO NOTHING",
            )?
            .execute(params![
                hash.0.to_vec(),
                loc.pack,
                i64_of(loc.offset),
                loc.stored,
                loc.flags,
                i64_of(loc.raw),
                touched
            ])?;
        }
        conn.execute(
            "INSERT INTO pack (no, indexed_len) VALUES (?1, ?2)
                 ON CONFLICT (no) DO UPDATE SET indexed_len = excluded.indexed_len",
            params![no, i64_of(good)],
        )?;
        Ok(())
    })
    .map_err(err)?;
    Ok(())
}

fn parse_manifest(raw: &[u8]) -> Result<Vec<(Hash, u64)>, String> {
    if !raw.len().is_multiple_of(40) {
        return Err("bad manifest".into());
    }
    raw.chunks(40)
        .map(|entry| {
            let h = Hash(entry[..32].try_into().map_err(|_| "bad manifest")?);
            let len = u64::from_le_bytes(entry[32..40].try_into().map_err(|_| "bad manifest")?);
            Ok((h, len))
        })
        .collect()
}

/// A streaming reader over one blob: one chunk in memory at a time.
pub struct BlobReader<'a> {
    blobs: &'a Blobs,
    chunks: std::vec::IntoIter<Hash>,
    buf: Vec<u8>,
    at: usize,
}

impl Read for BlobReader<'_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.at >= self.buf.len() {
            let Some(next) = self.chunks.next() else {
                return Ok(0);
            };
            self.buf = self.blobs.get(&next).map_err(std::io::Error::other)?;
            self.at = 0;
        }
        let n = out.len().min(self.buf.len() - self.at);
        out[..n].copy_from_slice(&self.buf[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// A shared store.
pub type Shared = Arc<Blobs>;

#[cfg(test)]
mod tests;
