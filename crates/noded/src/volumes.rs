//! Shared read-only data volumes on the node: ext4 images that several agent VMs mount read-only
//! at `/data/<name>`.
//!
//! Every volume is two files directly under the store's directory:
//!
//! ```text
//! <dir>/<name>.ext4    the image
//! <dir>/<name>.json    its metadata (a `VolumeView`)
//! ```
//!
//! An image is written under `<name>.ext4.partial` and renamed into place before its metadata is
//! written, so a volume is listed only once its image is complete. The store's directory is 0700.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::snapshots::private_dir;

/// The extension of a volume's image.
const IMAGE_EXT: &str = "ext4";
/// The extension of a volume's metadata.
const META_EXT: &str = "json";
/// The suffix of an image that is still being written.
const PARTIAL_SUFFIX: &str = ".partial";
const MIB: u64 = 1024 * 1024;
/// Room added to a directory's bytes when its image is sized: ext4 metadata and free space.
const DIR_SLACK_BYTES: u64 = 64 * MIB;

/// One volume, as the store reports it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VolumeView {
    pub name: String,
    /// The image's size in bytes.
    pub size_bytes: u64,
    /// When the volume was created, in milliseconds since the Unix epoch.
    pub created_ms: u64,
    /// The source path the volume was made from, as text.
    pub source: String,
}

/// The node's store of data volumes.
#[derive(Debug, Clone)]
pub struct VolumeStore {
    dir: PathBuf,
}

/// Why a volume operation was refused.
#[derive(Debug)]
pub enum VolumeError {
    /// The name or the source path is not usable.
    Invalid(String),
    /// A volume with this name already exists.
    Exists(String),
    /// No volume with this name exists.
    NotFound(String),
    /// A running VM carries the volume.
    InUse(String),
    /// The filesystem or `mkfs.ext4` failed.
    Io(String),
}

impl std::fmt::Display for VolumeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VolumeError::Invalid(why) => write!(f, "invalid volume: {why}"),
            VolumeError::Exists(name) => write!(f, "volume {name} already exists"),
            VolumeError::NotFound(name) => write!(f, "no volume {name}"),
            VolumeError::InUse(name) => write!(f, "volume {name} is in use by a running VM"),
            VolumeError::Io(why) => write!(f, "volume store: {why}"),
        }
    }
}

impl std::error::Error for VolumeError {}

impl VolumeStore {
    /// Opens (creating, mode 0700 on unix) the store at `dir`.
    pub fn open(dir: PathBuf) -> io::Result<VolumeStore> {
        private_dir(&dir)?;
        Ok(VolumeStore { dir })
    }

    /// The store's directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Every volume, by name.
    pub fn list(&self) -> Vec<VolumeView> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut views: Vec<VolumeView> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some(META_EXT))
            .filter_map(|path| {
                let name = path.file_stem()?.to_str()?.to_owned();
                self.get(&name)
            })
            .collect();
        views.sort_by(|a, b| a.name.cmp(&b.name));
        views
    }

    /// The volume called `name`, if it exists.
    pub fn get(&self, name: &str) -> Option<VolumeView> {
        if vmm::spawner::check_volume_name(name).is_err() || !self.image_path(name).is_file() {
            return None;
        }
        let text = fs::read(self.meta_path(name)).ok()?;
        serde_json::from_slice(&text).ok()
    }

    /// The image of volume `name`, if it exists.
    pub fn image(&self, name: &str) -> Option<PathBuf> {
        self.get(name).map(|_| self.image_path(name))
    }

    /// Makes volume `name` from `source`: an ext4 image file is cloned (reflink when the filesystem
    /// can); a directory is packed with `mkfs.ext4 -d`, into an image sized to its contents plus
    /// room to spare. Refuses an existing name, a bad name, a relative source path, or a source
    /// that is neither a file nor a directory.
    pub fn create(&self, name: &str, source: &Path) -> Result<VolumeView, VolumeError> {
        vmm::spawner::check_volume_name(name).map_err(VolumeError::Invalid)?;
        if !source.is_absolute() {
            return Err(VolumeError::Invalid(format!(
                "source path must be absolute: {}",
                source.display()
            )));
        }
        if self.image_path(name).exists() || self.meta_path(name).exists() {
            return Err(VolumeError::Exists(name.to_owned()));
        }

        let image = self.image_path(name);
        let partial = self.dir.join(format!("{name}.{IMAGE_EXT}{PARTIAL_SUFFIX}"));
        let _ = fs::remove_file(&partial);
        if source.is_file() {
            vmm::clonefile::clone_file(source, &partial)
                .map_err(|err| VolumeError::Io(format!("cloning {}: {err}", source.display())))?;
        } else if source.is_dir() {
            if let Err(err) = self.pack_dir(name, source, &partial) {
                let _ = fs::remove_file(&partial);
                return Err(err);
            }
        } else {
            return Err(VolumeError::Invalid(format!(
                "source is neither a file nor a directory: {}",
                source.display()
            )));
        }

        fs::rename(&partial, &image)
            .map_err(|err| VolumeError::Io(format!("placing the image of {name}: {err}")))?;
        let size_bytes = fs::metadata(&image)
            .map_err(|err| VolumeError::Io(format!("reading the image of {name}: {err}")))?
            .len();
        let view = VolumeView {
            name: name.to_owned(),
            size_bytes,
            created_ms: now_ms(),
            source: source.display().to_string(),
        };
        let written = serde_json::to_vec_pretty(&view)
            .map_err(|err| VolumeError::Io(format!("encoding {name}: {err}")))
            .and_then(|json| {
                fs::write(self.meta_path(name), json).map_err(|err| {
                    VolumeError::Io(format!("writing the metadata of {name}: {err}"))
                })
            });
        if let Err(err) = written {
            let _ = fs::remove_file(&image);
            return Err(err);
        }
        Ok(view)
    }

    /// Removes volume `name` (its image and metadata). `in_use` is true when a live VM carries it:
    /// then the volume is kept.
    pub fn remove(&self, name: &str, in_use: bool) -> Result<(), VolumeError> {
        vmm::spawner::check_volume_name(name).map_err(VolumeError::Invalid)?;
        if !self.image_path(name).exists() && !self.meta_path(name).exists() {
            return Err(VolumeError::NotFound(name.to_owned()));
        }
        if in_use {
            return Err(VolumeError::InUse(name.to_owned()));
        }
        remove_if_present(&self.image_path(name))?;
        remove_if_present(&self.meta_path(name))
    }

    fn image_path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.{IMAGE_EXT}"))
    }

    fn meta_path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.{META_EXT}"))
    }

    /// Writes the ext4 image of directory `source` to `partial`, sized to its contents.
    fn pack_dir(&self, name: &str, source: &Path, partial: &Path) -> Result<(), VolumeError> {
        let bytes = dir_bytes(source)
            .map_err(|err| VolumeError::Io(format!("measuring {}: {err}", source.display())))?;
        let image_mib = (bytes.saturating_mul(5) / 4 + DIR_SLACK_BYTES).div_ceil(MIB);
        // mkfs.ext4 takes a size in MiB with the `M` suffix.
        let status = Command::new("mkfs.ext4")
            .args(["-q", "-F", "-L", name, "-d"])
            .arg(source)
            .arg(partial)
            .arg(format!("{image_mib}M"))
            .status()
            .map_err(|err| {
                if err.kind() == io::ErrorKind::NotFound {
                    VolumeError::Io(format!("mkfs.ext4 is not installed: {err}"))
                } else {
                    VolumeError::Io(format!("running mkfs.ext4: {err}"))
                }
            })?;
        if !status.success() {
            return Err(VolumeError::Io(format!("mkfs.ext4 exited with {status}")));
        }
        Ok(())
    }
}

/// Removes `path`; a missing file is not an error.
fn remove_if_present(path: &Path) -> Result<(), VolumeError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(VolumeError::Io(format!(
            "removing {}: {err}",
            path.display()
        ))),
    }
}

/// The total length of the regular files under `dir`. Symbolic links are not followed.
fn dir_bytes(dir: &Path) -> io::Result<u64> {
    let mut total = 0;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_dir() {
            total += dir_bytes(&entry.path())?;
        } else if meta.is_file() {
            total += meta.len();
        }
    }
    Ok(total)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch root unique to this process and test.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("apz-noded-volumes-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// A 64 KiB file standing in for an ext4 image.
    fn write_image(path: &Path) {
        fs::write(path, vec![0u8; 64 * 1024]).expect("a source image");
    }

    fn have_mkfs() -> bool {
        Command::new("mkfs.ext4").arg("-V").output().is_ok()
    }

    #[test]
    fn create_from_image_then_list_get_image() {
        let root = scratch("image");
        let src = root.join("src.ext4");
        write_image(&src);
        let store = VolumeStore::open(root.join("store")).expect("a store");

        let view = store.create("data", &src).expect("a volume");
        assert_eq!(view.name, "data");
        assert_eq!(view.size_bytes, 64 * 1024);
        assert_eq!(view.source, src.display().to_string());

        assert_eq!(store.list(), vec![view.clone()]);
        assert_eq!(store.get("data"), Some(view));
        let image = store.image("data").expect("an image path");
        assert_eq!(fs::metadata(&image).expect("the image").len(), 64 * 1024);
        assert!(store.get("missing").is_none());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn volume_create_refuses_duplicate_bad_and_relative_inputs() {
        // duplicate_name_is_refused: a second volume with the same name is refused.
        {
            let root = scratch("duplicate");
            let src = root.join("src.ext4");
            write_image(&src);
            let store = VolumeStore::open(root.join("store")).expect("a store");
            store.create("data", &src).expect("a volume");

            assert!(matches!(
                store.create("data", &src),
                Err(VolumeError::Exists(_))
            ));

            let _ = fs::remove_dir_all(&root);
        }
        // bad_name_is_refused: a name with a path part or an empty name is refused, and nothing is listed.
        {
            let root = scratch("bad-name");
            let src = root.join("src.ext4");
            write_image(&src);
            let store = VolumeStore::open(root.join("store")).expect("a store");

            assert!(matches!(
                store.create("../escape", &src),
                Err(VolumeError::Invalid(_))
            ));
            assert!(matches!(
                store.create("", &src),
                Err(VolumeError::Invalid(_))
            ));
            assert!(store.list().is_empty());

            let _ = fs::remove_dir_all(&root);
        }
        // relative_source_is_refused: a relative source image path is refused.
        {
            let root = scratch("relative");
            let store = VolumeStore::open(root.join("store")).expect("a store");

            assert!(matches!(
                store.create("data", Path::new("src.ext4")),
                Err(VolumeError::Invalid(_))
            ));

            let _ = fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn remove_is_refused_while_in_use_then_done() {
        let root = scratch("remove");
        let src = root.join("src.ext4");
        write_image(&src);
        let store = VolumeStore::open(root.join("store")).expect("a store");
        store.create("data", &src).expect("a volume");

        assert!(matches!(
            store.remove("data", true),
            Err(VolumeError::InUse(_))
        ));
        assert!(store.image("data").is_some());

        store.remove("data", false).expect("the volume is removed");
        assert!(store.get("data").is_none());
        assert!(store.list().is_empty());
        assert!(matches!(
            store.remove("data", false),
            Err(VolumeError::NotFound(_))
        ));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn create_from_directory_packs_an_ext4_image() {
        if !have_mkfs() {
            return;
        }
        let root = scratch("directory");
        let src = root.join("tree");
        fs::create_dir_all(src.join("sub")).expect("a source tree");
        fs::write(src.join("hello.txt"), "hi").expect("a file");
        fs::write(src.join("sub/more.txt"), "there").expect("a nested file");
        let store = VolumeStore::open(root.join("store")).expect("a store");

        let view = store.create("tree", &src).expect("a volume");
        assert_eq!(view.size_bytes % MIB, 0);
        assert!(view.size_bytes >= DIR_SLACK_BYTES);
        assert!(store.image("tree").is_some());

        let _ = fs::remove_dir_all(&root);
    }
}
