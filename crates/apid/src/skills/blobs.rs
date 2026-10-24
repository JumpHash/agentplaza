//! `Blobs` for skill files: `StoreBlobs` keeps them in the hub's object store (`apz_blobs::Blobs`, in `<state_dir>/blobs`)
//! under their BLAKE3 hash, next to every other blob the hub holds. Nothing deletes bytes here: a row that goes drops its
//! reference, and the hub's sweep frees the bytes no row names (`Skills::hashes` lists the skill files' roots).

use std::sync::Arc;

use super::Blobs;

/// Skill files in the hub's object store.
pub struct StoreBlobs {
    blobs: Arc<apz_blobs::Blobs>,
}

impl StoreBlobs {
    pub fn new(blobs: Arc<apz_blobs::Blobs>) -> StoreBlobs {
        StoreBlobs { blobs }
    }
}

/// A hash as the rows hold it: 64 lower-case hex digits.
fn parse_hash(text: &str) -> Result<apz_blobs::Hash, String> {
    match text.parse::<apz_blobs::Hash>() {
        Ok(hash) if hash.hex() == text => Ok(hash),
        _ => Err(format!("{text} is not a hash (64 lower-case hex digits)")),
    }
}

impl Blobs for StoreBlobs {
    fn put(&self, bytes: &[u8]) -> Result<String, String> {
        Ok(self.blobs.put(bytes)?.hex())
    }

    fn get(&self, hash: &str) -> Result<Vec<u8>, String> {
        let want = parse_hash(hash)?;
        let bytes = self.blobs.get(&want)?;
        // The store checks only the large (chunked) blobs it reads, so every read is checked here.
        if apz_blobs::Hash::of(&bytes) == want {
            Ok(bytes)
        } else {
            Err(format!("the bytes under {hash} do not hash to it"))
        }
    }
}
