//! Where the vault's document lives when Postgres is not configured: `state_dir/vault.json`.
//! Writes are atomic: a temp file beside it (created exclusively, mode 0600), `fsync`, rename over
//! the target, then `fsync` of the directory. A leftover temp file is never read, and a failed write
//! leaves the previous file in place.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The file's text, or `None` when it does not exist.
pub fn read(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// Replaces `path` with `bytes`, atomically and privately.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = temp_beside(path);
    let _ = fs::remove_file(&tmp);
    let result = (|| {
        let mut file = create_private(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)?;
        sync_dir(dir)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Creates `path` exclusively with mode 0600 (on Unix). Fails when it exists.
pub fn create_private(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

/// `fsync` of a directory, so a rename or a create in it survives a crash.
pub fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(dir)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(())
    }
}

fn temp_beside(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!(".{name}.tmp.{}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apid-vault-disk-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn a_leftover_temp_file_is_ignored() {
        let dir = scratch("leftover");
        let path = dir.join("vault.json");
        fs::write(dir.join(".vault.json.tmp.4242"), b"{ half written").expect("temp");
        assert!(read(&path).expect("read").is_none(), "only vault.json is read");
        write_atomic(&path, b"{\"good\":true}").expect("written");
        assert_eq!(read(&path).expect("read").as_deref(), Some("{\"good\":true}"));
    }

    #[test]
    fn a_half_written_file_never_replaces_a_good_one() {
        let dir = scratch("half");
        let path = dir.join("vault.json");
        write_atomic(&path, b"good").expect("written");
        // Block the temp file of this process: the write fails before anything is renamed.
        fs::create_dir(temp_beside(&path)).expect("blocker");
        assert!(write_atomic(&path, b"new").is_err());
        assert_eq!(read(&path).expect("read").as_deref(), Some("good"));
    }

    #[test]
    #[cfg(unix)]
    fn files_are_written_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("mode");
        let path = dir.join("vault.json");
        write_atomic(&path, b"x").expect("written");
        let mode = fs::metadata(&path).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
