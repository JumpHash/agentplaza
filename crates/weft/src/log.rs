//! The log: append-only segment files, the truth Weft keeps.
//!
//! `log/000001.wlog`, `log/000002.wlog`, … Each frame is `len u32 | check u32 | record`, where `check` is the
//! first four bytes of the record's BLAKE3 hash. A record is written and fsynced before `append` returns. On
//! open, frames are read in order; a short or corrupt frame at the very end of the last segment (a crash in
//! the middle of a write) is cut away; anywhere else it is an error, because the log has been damaged. The
//! records' sequence numbers and `prev` hashes must form one unbroken chain.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::hash::Hash;
use crate::record::{LogRecord, Signed};

/// A segment rolls over past this size.
const SEGMENT_BYTES: u64 = 64 << 20;
/// The most one record may be.
pub const MAX_RECORD: usize = 16 << 20;

pub struct Log {
    dir: PathBuf,
    current: File,
    current_no: u32,
    current_len: u64,
    next_seq: u64,
    last: Hash,
}

/// What `open` found.
pub struct Opened {
    pub log: Log,
    pub records: Vec<LogRecord>,
    /// Bytes cut from a torn tail, if any.
    pub cut: u64,
}

impl Log {
    /// Opens (or creates) the log in `dir`, returning every record in order.
    pub fn open(dir: &Path) -> Result<Opened, String> {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut segments: Vec<(u32, PathBuf)> = fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                let no = name.strip_suffix(".wlog")?.parse::<u32>().ok()?;
                Some((no, e.path()))
            })
            .collect();
        segments.sort();
        let mut records = Vec::new();
        let mut cut = 0;
        let mut last = Hash::ZERO;
        let mut next_seq = 1;
        let count = segments.len();
        for (i, (_, path)) in segments.iter().enumerate() {
            let is_last = i + 1 == count;
            let mut bytes = Vec::new();
            File::open(path)
                .and_then(|mut f| f.read_to_end(&mut bytes))
                .map_err(|e| format!("{}: {e}", path.display()))?;
            let mut at = 0usize;
            while at < bytes.len() {
                match frame_at(&bytes, at) {
                    Ok((rec, len)) => {
                        if rec.seq != next_seq {
                            return Err(format!(
                                "{}: record {} where {} was expected",
                                path.display(),
                                rec.seq,
                                next_seq
                            ));
                        }
                        if rec.prev != last {
                            return Err(format!(
                                "{}: record {} does not follow the one before it",
                                path.display(),
                                rec.seq
                            ));
                        }
                        last = rec.hash();
                        next_seq += 1;
                        records.push(rec);
                        at += len;
                    }
                    Err(why) if is_last && reaches_end(&bytes, at) => {
                        cut = (bytes.len() - at) as u64;
                        let file = OpenOptions::new()
                            .write(true)
                            .open(path)
                            .map_err(|e| format!("{}: {e}", path.display()))?;
                        file.set_len(at as u64)
                            .map_err(|e| format!("{}: {e}", path.display()))?;
                        file.sync_all().ok();
                        eprintln!(
                            "weft: cut {cut} bytes from the end of {} ({why})",
                            path.display()
                        );
                        break;
                    }
                    Err(why) => {
                        return Err(format!("{}: damaged at byte {at}: {why}", path.display()));
                    }
                }
            }
        }
        let current_no = segments.last().map_or(1, |(no, _)| *no);
        let path = segment_path(dir, current_no);
        let current = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let current_len = current.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Opened {
            log: Log {
                dir: dir.to_path_buf(),
                current,
                current_no,
                current_len,
                next_seq,
                last,
            },
            records,
            cut,
        })
    }

    /// Appends a signed statement as the next record, fsynced, and returns the record.
    pub fn append(&mut self, signed: Signed) -> Result<LogRecord, String> {
        let rec = LogRecord {
            seq: self.next_seq,
            prev: self.last,
            signed,
        };
        let body = rec.encode();
        if body.len() > MAX_RECORD {
            return Err(format!("a record may be at most {MAX_RECORD} bytes"));
        }
        if self.current_len > 0 && self.current_len + body.len() as u64 > SEGMENT_BYTES {
            self.current_no += 1;
            let path = segment_path(&self.dir, self.current_no);
            self.current = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            self.current_len = 0;
        }
        let hash = Hash::of(&body);
        let mut frame = Vec::with_capacity(body.len() + 8);
        frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
        frame.extend_from_slice(&hash.0[..4]);
        frame.extend_from_slice(&body);
        self.current
            .write_all(&frame)
            .map_err(|e| format!("log write: {e}"))?;
        self.current
            .sync_data()
            .map_err(|e| format!("log sync: {e}"))?;
        self.current_len += frame.len() as u64;
        self.next_seq += 1;
        self.last = hash;
        Ok(rec)
    }

    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    pub fn head(&self) -> Hash {
        self.last
    }

    /// Bytes on disk, every segment.
    pub fn size(&self) -> u64 {
        fs::read_dir(&self.dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok()?.metadata().ok())
                    .map(|m| m.len())
                    .sum()
            })
            .unwrap_or(0)
    }

    /// The segment files in order (for backup: new segments are the only thing to copy).
    pub fn segments(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = fs::read_dir(&self.dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x == "wlog"))
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    #[cfg(test)]
    fn current_path(&self) -> PathBuf {
        segment_path(&self.dir, self.current_no)
    }
}

fn segment_path(dir: &Path, no: u32) -> PathBuf {
    dir.join(format!("{no:06}.wlog"))
}

/// Whether a bad frame at `at` runs to the end of the file: only then can it be a write cut short by a crash.
/// A bad frame with whole frames after it is damage, never a torn tail.
fn reaches_end(bytes: &[u8], at: usize) -> bool {
    match bytes.get(at..at + 4) {
        None => true,
        Some(head) => {
            let len = u32::from_le_bytes(head.try_into().unwrap()) as usize;
            len > MAX_RECORD || at + 8 + len >= bytes.len()
        }
    }
}

/// The record framed at `at`, and the frame's length.
fn frame_at(bytes: &[u8], at: usize) -> Result<(LogRecord, usize), String> {
    let head = bytes.get(at..at + 8).ok_or("a frame header is cut short")?;
    let len = u32::from_le_bytes(head[..4].try_into().unwrap()) as usize;
    if len > MAX_RECORD {
        return Err(format!("frame length {len} is too large"));
    }
    let body = bytes
        .get(at + 8..at + 8 + len)
        .ok_or("a frame is cut short")?;
    if Hash::of(body).0[..4] != head[4..8] {
        return Err("a frame's check does not match".into());
    }
    Ok((LogRecord::decode(body)?, 8 + len))
}

/// Reads a single segment file without checking the chain (for `verify` and tools).
pub fn read_segment(path: &Path) -> Result<Vec<LogRecord>, String> {
    let mut f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.seek(SeekFrom::Start(0)).ok();
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let (rec, len) = frame_at(&bytes, at)?;
        out.push(rec);
        at += len;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::KeyPair;
    use crate::record::{Actor, Statement};

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("weft-log-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn stmt(key: &KeyPair, n: u64) -> Signed {
        Statement::new("test", Actor::Hub, serde_json::json!({ "n": n })).sign(key)
    }

    #[test]
    fn records_survive_a_reopen_in_order_and_chained() {
        let dir = tmp("reopen");
        let key = KeyPair::generate();
        {
            let mut log = Log::open(&dir).unwrap().log;
            for n in 0..5 {
                log.append(stmt(&key, n)).unwrap();
            }
        }
        let opened = Log::open(&dir).unwrap();
        assert_eq!(opened.records.len(), 5);
        assert_eq!(opened.records[4].seq, 5);
        assert_eq!(opened.log.next_seq(), 6);
        assert_eq!(opened.cut, 0);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_torn_tail_is_cut_and_the_log_goes_on() {
        let dir = tmp("torn");
        let key = KeyPair::generate();
        let path;
        {
            let mut log = Log::open(&dir).unwrap().log;
            for n in 0..3 {
                log.append(stmt(&key, n)).unwrap();
            }
            path = log.current_path();
        }
        // A crash in the middle of the fourth write: half a frame.
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(&[200, 0, 0, 0, 1, 2]).unwrap();
        drop(f);
        let mut opened = Log::open(&dir).unwrap();
        assert_eq!(opened.records.len(), 3);
        assert_eq!(opened.cut, 6);
        opened.log.append(stmt(&key, 9)).unwrap();
        assert_eq!(Log::open(&dir).unwrap().records.len(), 4);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_changed_record_in_the_middle_is_refused() {
        let dir = tmp("tamper");
        let key = KeyPair::generate();
        let path;
        {
            let mut log = Log::open(&dir).unwrap().log;
            for n in 0..3 {
                log.append(stmt(&key, n)).unwrap();
            }
            path = log.current_path();
        }
        let mut bytes = fs::read(&path).unwrap();
        // Flip a byte inside the first record's statement.
        bytes[60] ^= 1;
        fs::write(&path, &bytes).unwrap();
        // The damaged frame is the first of three: damage, never a torn tail, so nothing is cut.
        let why = Log::open(&dir).err().expect("a damaged log must not open");
        assert!(why.contains("damaged"), "{why}");
        assert_eq!(fs::read(&path).unwrap(), bytes, "nothing was cut");
        fs::remove_dir_all(&dir).ok();
    }
}
