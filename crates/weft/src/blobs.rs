//! File contents and artifacts, by BLAKE3 hash.
//!
//! Blobs live in append-only packs, `blobs/pack-000001.wpk`, … Each entry is
//! `stored_len u32 | hash 32 | flags u8 | raw_len u64 | stored bytes`. Flags: 1 = zstd, 2 = chunk manifest. A
//! file over [`CHUNK_OVER`] bytes is cut into content-defined chunks (FastCDC), each stored as its own blob, and
//! the file's hash names a manifest listing them, so a large file that changes a little shares most chunks with
//! its last version. The index is rebuilt by scanning the packs' entry headers on open.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::hash::Hash;

/// Files larger than this are stored as chunks.
pub const CHUNK_OVER: usize = 4 << 20;
const PACK_BYTES: u64 = 1 << 30;
const FLAG_ZSTD: u8 = 1;
const FLAG_MANIFEST: u8 = 2;
const HEAD: usize = 4 + 32 + 1 + 8;

#[derive(Clone, Copy, Debug)]
struct Loc {
    pack: u32,
    offset: u64,
    stored: u32,
    flags: u8,
    raw: u64,
}

pub struct Blobs {
    dir: PathBuf,
    inner: Mutex<Inner>,
}

struct Inner {
    index: HashMap<Hash, Loc>,
    current: File,
    current_no: u32,
    current_len: u64,
    stored_bytes: u64,
}

impl Blobs {
    pub fn open(dir: &Path) -> Result<Blobs, String> {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut packs: Vec<(u32, PathBuf)> = fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                let no = name
                    .strip_prefix("pack-")?
                    .strip_suffix(".wpk")?
                    .parse::<u32>()
                    .ok()?;
                Some((no, e.path()))
            })
            .collect();
        packs.sort();
        let mut index = HashMap::new();
        let mut stored_bytes = 0;
        let count = packs.len();
        for (i, (no, path)) in packs.iter().enumerate() {
            let mut f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            let mut at = 0u64;
            let mut head = [0u8; HEAD];
            while at < len {
                f.seek(SeekFrom::Start(at)).map_err(|e| e.to_string())?;
                let ok = f.read_exact(&mut head).is_ok();
                let stored = u32::from_le_bytes(head[..4].try_into().unwrap());
                if !ok || at + HEAD as u64 + stored as u64 > len {
                    if i + 1 == count {
                        // A write cut short by a crash: drop it.
                        OpenOptions::new()
                            .write(true)
                            .open(path)
                            .and_then(|f| f.set_len(at))
                            .map_err(|e| e.to_string())?;
                        break;
                    }
                    return Err(format!("{}: damaged at byte {at}", path.display()));
                }
                let hash = Hash(head[4..36].try_into().unwrap());
                let flags = head[36];
                let raw = u64::from_le_bytes(head[37..45].try_into().unwrap());
                index.insert(
                    hash,
                    Loc {
                        pack: *no,
                        offset: at + HEAD as u64,
                        stored,
                        flags,
                        raw,
                    },
                );
                stored_bytes += stored as u64;
                at += HEAD as u64 + stored as u64;
            }
        }
        let current_no = packs.last().map_or(1, |(no, _)| *no);
        let path = pack_path(dir, current_no);
        let current = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let current_len = current.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Blobs {
            dir: dir.to_path_buf(),
            inner: Mutex::new(Inner {
                index,
                current,
                current_no,
                current_len,
                stored_bytes,
            }),
        })
    }

    pub fn has(&self, hash: &Hash) -> bool {
        self.lock().index.contains_key(hash)
    }

    /// Which of `hashes` are missing.
    pub fn missing(&self, hashes: &[Hash]) -> Vec<Hash> {
        let inner = self.lock();
        hashes
            .iter()
            .filter(|h| !inner.index.contains_key(h))
            .copied()
            .collect()
    }

    /// Stores `bytes` and returns their hash (the hash of the whole content, chunked or not).
    pub fn put(&self, bytes: &[u8]) -> Result<Hash, String> {
        let hash = Hash::of(bytes);
        if self.has(&hash) {
            return Ok(hash);
        }
        if bytes.len() > CHUNK_OVER {
            let mut manifest = Vec::new();
            for (offset, len) in chunk_plan(bytes) {
                let piece = &bytes[offset..offset + len];
                let h = Hash::of(piece);
                if !self.has(&h) {
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

    pub fn get(&self, hash: &Hash) -> Result<Vec<u8>, String> {
        let loc = self
            .lock()
            .index
            .get(hash)
            .copied()
            .ok_or_else(|| format!("no blob {}", hash.short()))?;
        let stored = self.read_stored(loc)?;
        let raw = if loc.flags & FLAG_ZSTD != 0 {
            zstd::decode_all(&stored[..]).map_err(|e| format!("blob {}: {e}", hash.short()))?
        } else {
            stored
        };
        if loc.flags & FLAG_MANIFEST != 0 {
            let mut out = Vec::with_capacity(loc.raw as usize);
            for entry in raw.chunks(40) {
                let h = Hash(entry[..32].try_into().map_err(|_| "bad manifest")?);
                out.extend_from_slice(&self.get(&h)?);
            }
            if Hash::of(&out) != *hash {
                return Err(format!("blob {} does not match its hash", hash.short()));
            }
            return Ok(out);
        }
        Ok(raw)
    }

    /// The chunks of a chunked blob, `None` for a blob stored whole.
    pub fn chunks(&self, hash: &Hash) -> Result<Option<Vec<(Hash, u64)>>, String> {
        let loc = self
            .lock()
            .index
            .get(hash)
            .copied()
            .ok_or_else(|| format!("no blob {}", hash.short()))?;
        if loc.flags & FLAG_MANIFEST == 0 {
            return Ok(None);
        }
        let stored = self.read_stored(loc)?;
        let raw = if loc.flags & FLAG_ZSTD != 0 {
            zstd::decode_all(&stored[..]).map_err(|e| e.to_string())?
        } else {
            stored
        };
        let mut out = Vec::new();
        for entry in raw.chunks(40) {
            let h = Hash(entry[..32].try_into().map_err(|_| "bad manifest")?);
            let len = u64::from_le_bytes(entry[32..40].try_into().map_err(|_| "bad manifest")?);
            out.push((h, len));
        }
        Ok(Some(out))
    }

    /// Records a chunked blob whose chunks were uploaded one by one; refuses it unless they add up to `hash`.
    pub fn put_manifest(&self, hash: &Hash, chunks: &[(Hash, u64)]) -> Result<(), String> {
        if self.has(hash) {
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

    /// The size of a blob's content.
    pub fn size(&self, hash: &Hash) -> Option<u64> {
        self.lock().index.get(hash).map(|l| l.raw)
    }

    /// Blobs stored and bytes on disk.
    pub fn stats(&self) -> (usize, u64) {
        let inner = self.lock();
        (inner.index.len(), inner.stored_bytes)
    }

    fn write_entry(&self, hash: Hash, bytes: &[u8], flags: u8) -> Result<(), String> {
        let compressed = zstd::encode_all(bytes, 3).map_err(|e| e.to_string())?;
        if compressed.len() + compressed.len() / 10 < bytes.len() {
            self.write_entry_raw(hash, &compressed, flags | FLAG_ZSTD, bytes.len() as u64)
        } else {
            self.write_entry_raw(hash, bytes, flags, bytes.len() as u64)
        }
    }

    fn write_entry_raw(
        &self,
        hash: Hash,
        stored: &[u8],
        flags: u8,
        raw: u64,
    ) -> Result<(), String> {
        let mut inner = self.lock();
        if inner.index.contains_key(&hash) {
            return Ok(());
        }
        if inner.current_len > 0 && inner.current_len + stored.len() as u64 > PACK_BYTES {
            inner.current_no += 1;
            let path = pack_path(&self.dir, inner.current_no);
            inner.current = OpenOptions::new()
                .create(true)
                .read(true)
                .append(true)
                .open(&path)
                .map_err(|e| e.to_string())?;
            inner.current_len = 0;
        }
        let mut entry = Vec::with_capacity(HEAD + stored.len());
        entry.extend_from_slice(&(stored.len() as u32).to_le_bytes());
        entry.extend_from_slice(&hash.0);
        entry.push(flags);
        entry.extend_from_slice(&raw.to_le_bytes());
        entry.extend_from_slice(stored);
        inner
            .current
            .write_all(&entry)
            .map_err(|e| format!("blob write: {e}"))?;
        inner
            .current
            .sync_data()
            .map_err(|e| format!("blob sync: {e}"))?;
        let loc = Loc {
            pack: inner.current_no,
            offset: inner.current_len + HEAD as u64,
            stored: stored.len() as u32,
            flags,
            raw,
        };
        inner.current_len += entry.len() as u64;
        inner.stored_bytes += stored.len() as u64;
        inner.index.insert(hash, loc);
        Ok(())
    }

    fn read_stored(&self, loc: Loc) -> Result<Vec<u8>, String> {
        let path = pack_path(&self.dir, loc.pack);
        let mut f = File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        f.seek(SeekFrom::Start(loc.offset))
            .map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; loc.stored as usize];
        f.read_exact(&mut buf)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(buf)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Where a large file is cut: `(offset, length)` of each content-defined chunk. Uploaders cut the same way, so
/// they send only the chunks the store lacks.
pub fn chunk_plan(bytes: &[u8]) -> Vec<(usize, usize)> {
    fastcdc::v2020::FastCDC::new(bytes, 256 << 10, 1 << 20, 4 << 20)
        .map(|c| (c.offset, c.length))
        .collect()
}

fn pack_path(dir: &Path, no: u32) -> PathBuf {
    dir.join(format!("pack-{no:06}.wpk"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("weft-blobs-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn blobs_round_trip_and_survive_a_reopen() {
        let dir = tmp("rt");
        let text = b"fn memory_mib() -> Value {\n    Value::Null\n}\n".repeat(50);
        let h = {
            let blobs = Blobs::open(&dir).unwrap();
            let h = blobs.put(&text).unwrap();
            assert_eq!(blobs.put(&text).unwrap(), h, "a second put is a no-op");
            h
        };
        let blobs = Blobs::open(&dir).unwrap();
        assert_eq!(blobs.get(&h).unwrap(), text);
        assert!(blobs.missing(&[h, Hash::of(b"nope")]).len() == 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_large_file_is_chunked_and_a_small_edit_shares_chunks() {
        let dir = tmp("chunk");
        let blobs = Blobs::open(&dir).unwrap();
        let mut big: Vec<u8> = (0..12_000_000u32)
            .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
            .collect();
        let h1 = blobs.put(&big).unwrap();
        let (_, after_first) = blobs.stats();
        big[6_000_000] ^= 0xff;
        let h2 = blobs.put(&big).unwrap();
        let (_, after_second) = blobs.stats();
        assert_ne!(h1, h2);
        assert_eq!(blobs.get(&h2).unwrap(), big);
        assert!(
            after_second - after_first < 5 << 20,
            "an edit stores a few chunks, not the whole file again"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_upload_that_does_not_match_its_hash_is_refused() {
        let dir = tmp("check");
        let blobs = Blobs::open(&dir).unwrap();
        assert!(blobs.put_checked(&Hash::of(b"a"), b"b").is_err());
        assert!(blobs.put_checked(&Hash::of(b"a"), b"a").is_ok());
        fs::remove_dir_all(&dir).ok();
    }
}
