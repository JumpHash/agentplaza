//! The default workspace image: the base ext4 volume every agent's `/workspace` is copied from when
//! `noded` runs without `--workspace`. `noded` makes it once, in its persist directory, with the
//! flags `scripts/mk-workspace-image.sh` uses (a sparse 4 GiB file, the pinned label and UUID, lazy
//! inode tables and journal), so the image is the same whichever way it was made.

use std::fs::{self, File};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The image's file name inside the persist directory.
pub const FILE_NAME: &str = "workspace.ext4";

/// The image's size: 4 GiB, sparse, as the script's default `APZ_WORKSPACE_MIB` is.
const SIZE_BYTES: u64 = 4096 << 20;

/// The filesystem label and UUID the script pins.
const LABEL: &str = "plaza-workspace";
const UUID: &str = "0f1a2b3c-4d5e-4f60-8a9b-0c1d2e3f4a5b";

/// Where the default image lives under `persist_dir`.
pub fn default_path(persist_dir: &Path) -> PathBuf {
    persist_dir.join(FILE_NAME)
}

/// Makes `image` (an ext4 filesystem) when it does not exist; an existing file is left alone. The
/// image is built under a temporary name and renamed into place, so an interrupted build never leaves
/// a half-made image behind. Fails with the reason when `mkfs.ext4` is missing or fails.
pub fn ensure(image: &Path) -> Result<(), String> {
    if image.is_file() {
        return Ok(());
    }
    if let Some(dir) = image.parent() {
        fs::create_dir_all(dir).map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
    }
    let mut tmp = image.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let _ = fs::remove_file(&tmp);
    if let Err(err) = make(&tmp) {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    fs::rename(&tmp, image).map_err(|err| {
        let _ = fs::remove_file(&tmp);
        format!("cannot move the image to {}: {err}", image.display())
    })
}

/// Writes a sparse file of the image's size at `path` and formats it.
fn make(path: &Path) -> Result<(), String> {
    let file =
        File::create(path).map_err(|err| format!("cannot create {}: {err}", path.display()))?;
    file.set_len(SIZE_BYTES)
        .map_err(|err| format!("cannot size {}: {err}", path.display()))?;
    drop(file);
    // mke2fs stamps the times unless SOURCE_DATE_EPOCH pins them, as the script does.
    let epoch = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "0".to_string());
    let output = Command::new("mkfs.ext4")
        .args([
            "-q",
            "-F",
            "-E",
            "lazy_itable_init=1,lazy_journal_init=1",
            "-L",
            LABEL,
            "-U",
            UUID,
            "-m",
            "0",
        ])
        .arg(path)
        .env("SOURCE_DATE_EPOCH", epoch)
        .output()
        .map_err(|err| match err.kind() {
            ErrorKind::NotFound => "mkfs.ext4 is not on PATH".to_string(),
            _ => format!("cannot run mkfs.ext4: {err}"),
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "mkfs.ext4 failed ({}): {}",
            output.status,
            stderr.trim()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// A fresh directory under the temp dir, named for the test.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("noded-workspace-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// Whether `mkfs.ext4` can be run at all (a spawn error means it is not installed).
    fn mkfs_present() -> bool {
        Command::new("mkfs.ext4").arg("-V").output().is_ok()
    }

    #[test]
    fn an_existing_image_is_left_alone() {
        let dir = scratch("existing");
        let image = default_path(&dir);
        fs::write(&image, b"kept").expect("a file to stand in for the image");
        ensure(&image).expect("an existing file is accepted");
        assert_eq!(fs::read(&image).expect("the file"), b"kept");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_default_image_is_a_sparse_ext4_filesystem() {
        if !mkfs_present() {
            eprintln!("skipped: mkfs.ext4 is not on PATH");
            return;
        }
        let dir = scratch("default");
        let image = default_path(&dir);
        ensure(&image).expect("the default image is made");
        let meta = fs::metadata(&image).expect("the image");
        assert_eq!(meta.len(), SIZE_BYTES);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            // Sparse: the lazily formatted image holds far less than its length on disk.
            assert!(
                meta.blocks() * 512 < SIZE_BYTES / 4,
                "the image is not sparse"
            );
        }
        // The ext4 superblock sits at byte 1024: magic 0xEF53 at offset 56, the volume label at 120.
        let mut head = vec![0u8; 1024 + 136];
        File::open(&image)
            .and_then(|mut file| file.read_exact(&mut head))
            .expect("the image's first blocks");
        let superblock = &head[1024..];
        assert_eq!(u16::from_le_bytes([superblock[56], superblock[57]]), 0xEF53);
        assert_eq!(&superblock[120..120 + LABEL.len()], LABEL.as_bytes());
        assert!(
            !dir.join("workspace.ext4.tmp").exists(),
            "no temporary file is left"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
