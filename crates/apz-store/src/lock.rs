//! One hub per state folder: an exclusive lock on a file in it, held for the store's life.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::StoreError;

/// The lock file's name in the folder.
pub const LOCK_FILE: &str = "store.lock";

/// An exclusive lock on a state folder. Released when dropped (or when the process ends, however it ends: the OS
/// drops the lock with the file handle, so a crashed hub never leaves a stale lock).
#[derive(Debug)]
pub struct FolderLock {
    _file: File,
    dir: PathBuf,
}

impl FolderLock {
    /// Takes the folder's lock, creating the folder if needed. Refuses with [`StoreError::Locked`] when another
    /// process holds it, naming that process as it recorded itself.
    pub fn acquire(dir: &Path) -> Result<Self, StoreError> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(LOCK_FILE);
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                let mut holder = String::new();
                // On Windows the locked range cannot be read by another process: the holder stays unknown.
                let _ = file.read_to_string(&mut holder);
                let holder = holder.trim();
                return Err(StoreError::Locked {
                    dir: dir.to_path_buf(),
                    holder: if holder.is_empty() {
                        "a process that holds its lock".to_string()
                    } else {
                        holder.to_string()
                    },
                });
            }
            Err(TryLockError::Error(e)) => return Err(e.into()),
        }
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        write!(file, "pid {}", std::process::id())?;
        file.flush()?;
        Ok(FolderLock {
            _file: file,
            dir: dir.to_path_buf(),
        })
    }

    /// The folder this lock holds.
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_lock_on_the_same_folder_is_refused_until_the_first_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let first = FolderLock::acquire(dir.path()).unwrap();
        match FolderLock::acquire(dir.path()) {
            Err(StoreError::Locked { holder, .. }) => {
                assert!(
                    holder.contains("pid") || holder.contains("lock"),
                    "{holder}"
                )
            }
            other => panic!("expected Locked, got {other:?}"),
        }
        drop(first);
        FolderLock::acquire(dir.path()).expect("free again once dropped");
    }
}
