//! `Blobs` for skill files: `FsBlobs` under `<state_dir>/skills/blobs/<sha256>`, and `LibraryBlobs` in Garage through
//! the Library's store (`deliverables::Store`), under the key `skills/<sha256>`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::{sha256_hex, Blobs};

/// A sha256 as the stores name blobs: 64 lower-case hex characters.
fn valid_sha(sha256: &str) -> bool {
    sha256.len() == 64 && sha256.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn checked(sha256: &str, bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    if sha256_hex(&bytes) == sha256 {
        Ok(bytes)
    } else {
        Err(format!("the bytes under {sha256} do not hash to it"))
    }
}

pub struct FsBlobs {
    dir: PathBuf,
}

impl FsBlobs {
    pub fn open(state_dir: &Path) -> FsBlobs {
        FsBlobs { dir: state_dir.join("skills").join("blobs") }
    }

    fn path(&self, sha256: &str) -> Result<PathBuf, String> {
        if !valid_sha(sha256) {
            return Err(format!("{sha256} is not a sha256"));
        }
        Ok(self.dir.join(sha256))
    }
}

impl Blobs for FsBlobs {
    fn put(&self, bytes: &[u8]) -> Result<String, String> {
        let sha256 = sha256_hex(bytes);
        let path = self.path(&sha256)?;
        if path.is_file() {
            return Ok(sha256);
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("creating {}: {e}", self.dir.display()))?;
        // A temporary name no other write uses, so two writers of the same bytes never share one file.
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let tmp = self.dir.join(format!(".{sha256}.{}.{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let written = std::fs::File::create(&tmp)
            .and_then(|mut f| {
                use std::io::Write;
                f.write_all(bytes)?;
                f.sync_all()
            })
            .and_then(|()| std::fs::rename(&tmp, &path));
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("writing {}: {e}", path.display()));
        }
        Ok(sha256)
    }

    fn get(&self, sha256: &str) -> Result<Vec<u8>, String> {
        let path = self.path(sha256)?;
        let bytes = std::fs::read(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        checked(sha256, bytes)
    }
}

/// Skill files in the Library's object store (Garage when it is configured, else its local folder).
pub struct LibraryBlobs {
    library: Arc<crate::deliverables::Store>,
}

impl LibraryBlobs {
    pub fn new(library: Arc<crate::deliverables::Store>) -> LibraryBlobs {
        LibraryBlobs { library }
    }
}

fn library_key(sha256: &str) -> Result<String, String> {
    if !valid_sha(sha256) {
        return Err(format!("{sha256} is not a sha256"));
    }
    Ok(format!("skills/{sha256}"))
}

impl Blobs for LibraryBlobs {
    fn put(&self, bytes: &[u8]) -> Result<String, String> {
        let sha256 = sha256_hex(bytes);
        self.library.put_object(&library_key(&sha256)?, bytes)?;
        Ok(sha256)
    }

    fn get(&self, sha256: &str) -> Result<Vec<u8>, String> {
        let bytes = self.library.get_object(&library_key(sha256)?)?;
        checked(sha256, bytes)
    }
}
