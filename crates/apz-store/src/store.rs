//! A store: one locked folder and its database files, each with its writer and readers.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rusqlite::Connection;

use crate::backup::{self, Manifest, ManifestEntry};
use crate::conn::{self, Role};
use crate::{
    Durability, FileSpec, FolderLock, Readers, StoreError, Writer, Written, migrate, now_ms,
};

/// The folder backups taken before a migration go to, under the store's folder.
pub const BACKUPS: &str = "backups";

/// One database file: its writer thread and its reader pool.
pub struct Db {
    name: &'static str,
    path: PathBuf,
    durability: Durability,
    schema: u32,
    writer: Writer,
    readers: Readers,
}

impl Db {
    /// Opens (creating it if needed) and migrates the file `spec` names under `dir`, and starts its writer.
    fn open(dir: &Path, spec: &FileSpec, readers: usize) -> Result<Db, StoreError> {
        let path = dir.join(spec.file);
        let mut conn = conn::open(&path, spec.durability, Role::Writer)?;
        let applied = migrate::apply(&mut conn, &path, spec.migrations, &dir.join(BACKUPS))?;
        if let Some(backup) = &applied.backup {
            eprintln!(
                "store: {} migrated from schema {} to {} (backup: {})",
                path.display(),
                applied.from,
                applied.to,
                backup.display()
            );
        }
        let writer = Writer::start(conn, path.clone())?;
        Ok(Db {
            name: spec.name,
            readers: Readers::new(path.clone(), spec.durability, readers),
            path,
            durability: spec.durability,
            schema: applied.to,
            writer,
        })
    }

    /// The file's name in its store (`plaza`, `transcripts`, `metrics`).
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The file's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The file's schema version.
    pub fn schema(&self) -> u32 {
        self.schema
    }

    /// The file's writer.
    pub fn writer(&self) -> &Writer {
        &self.writer
    }

    /// The file's readers.
    pub fn readers(&self) -> &Readers {
        &self.readers
    }

    /// Runs `f` on a reader inside one read transaction (one consistent snapshot).
    pub fn read<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.readers.read(f)
    }

    /// Runs `op` in the writer's next transaction and waits for its commit ([`Writer::write`]).
    pub fn write<T, F>(&self, op: F) -> Result<Written<T>, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
    {
        self.writer.write(op)
    }

    /// Queues `op` without waiting ([`Writer::send`]).
    pub fn send<F>(&self, op: F) -> Result<(), StoreError>
    where
        F: FnOnce(&Connection) -> Result<(), StoreError> + Send + 'static,
    {
        self.writer.send(op)
    }

    /// Waits until everything sent so far is committed ([`Writer::flush`]).
    pub fn flush(&self) -> Result<u64, StoreError> {
        self.writer.flush()
    }

    /// Waits until commit `seq` has happened ([`Writer::wait_for`]).
    pub fn wait_for(&self, seq: u64, timeout: Duration) -> bool {
        self.writer.wait_for(seq, timeout)
    }

    /// Copies the file to `dest` as one consistent snapshot, and says what the copy holds.
    fn backup_to(&self, dest: &Path) -> Result<ManifestEntry, StoreError> {
        // Everything already sent is in the file before the copy starts.
        self.writer.flush()?;
        let src = conn::open(&self.path, self.durability, Role::Reader)?;
        backup::copy(&src, dest)?;
        let copy = Connection::open(dest)?;
        let seq = crate::writer::stored_seq(&copy)?;
        Ok(ManifestEntry {
            file: self
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(self.name)
                .to_string(),
            schema: migrate::version(&copy)?,
            seq,
            bytes: std::fs::metadata(dest)?.len(),
        })
    }

    fn close(&self) {
        self.writer.stop();
        self.readers.close_idle();
    }
}

/// A hub's store: its locked folder and its database files.
pub struct Store {
    lock: FolderLock,
    dbs: Vec<Arc<Db>>,
}

impl Store {
    /// Takes `dir`'s lock (refusing when another hub holds it), then opens and migrates every file of `files`.
    pub fn open(dir: &Path, files: &[FileSpec]) -> Result<Store, StoreError> {
        let lock = FolderLock::acquire(dir)?;
        let readers = std::thread::available_parallelism().map_or(4, |n| n.get());
        let mut dbs = Vec::with_capacity(files.len());
        for spec in files {
            dbs.push(Arc::new(Db::open(dir, spec, readers)?));
        }
        Ok(Store { lock, dbs })
    }

    /// The folder the store holds.
    pub fn dir(&self) -> &Path {
        self.lock.dir()
    }

    /// The file named `name`. Panics on a name the store was not opened with: that is a programming error.
    pub fn db(&self, name: &str) -> &Arc<Db> {
        self.dbs
            .iter()
            .find(|db| db.name == name)
            .unwrap_or_else(|| panic!("the store has no database {name}"))
    }

    /// Every file of the store.
    pub fn dbs(&self) -> &[Arc<Db>] {
        &self.dbs
    }

    /// Backs every file up into `dest` (created; files there are replaced), each one consistent, and writes the
    /// manifest of their sequence points. `extra` lets the caller add what it backs up beside (the object store's
    /// index, Weft's log); it runs after the databases are copied.
    pub fn backup_to(&self, dest: &Path) -> Result<Manifest, StoreError> {
        std::fs::create_dir_all(dest)?;
        let mut files = Vec::with_capacity(self.dbs.len());
        for db in &self.dbs {
            let name = db
                .path
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(db.name));
            files.push(db.backup_to(&dest.join(name))?);
        }
        let manifest = Manifest {
            taken_ms: now_ms(),
            files,
        };
        std::fs::write(dest.join("MANIFEST"), manifest.render())?;
        Ok(manifest)
    }

    /// Restores the backup in `from` into the folder `dir`, which no hub may be using (its lock is taken for the
    /// restore). Each file of the manifest replaces the folder's, its WAL and shared-memory files removed first.
    pub fn restore(from: &Path, dir: &Path) -> Result<Manifest, StoreError> {
        let _lock = FolderLock::acquire(dir)?;
        let manifest = Manifest::parse(&std::fs::read_to_string(from.join("MANIFEST"))?)?;
        for entry in &manifest.files {
            let src = from.join(&entry.file);
            let check = Connection::open(&src)?;
            let ok: String = check.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
            if ok != "ok" {
                return Err(StoreError::invalid(format!(
                    "{} fails its integrity check: {ok}",
                    src.display()
                )));
            }
            drop(check);
            let dest = dir.join(&entry.file);
            for suffix in ["-wal", "-shm"] {
                let mut side = dest.as_os_str().to_owned();
                side.push(suffix);
                let _ = std::fs::remove_file(PathBuf::from(side));
            }
            std::fs::copy(&src, &dest)?;
        }
        Ok(manifest)
    }

    /// Commits what is queued, stops the writers (checkpointing and truncating each WAL) and closes the readers.
    /// The folder's lock is released when the store drops.
    pub fn close(&self) {
        for db in &self.dbs {
            db.close();
        }
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        self.close();
    }
}
