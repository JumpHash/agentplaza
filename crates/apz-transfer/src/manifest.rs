//! A manifest: the files of a move or a base, as blobs in a node's store. Its id is the hash of its JSON, so two nodes
//! with the same files agree on it, and a file's chunks are what a fetch compares.

use std::io::Read;

use apz_blobs::{Blobs, Hash};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub files: Vec<File>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    pub name: String,
    pub len: u64,
    /// The hash of the whole file.
    pub root: Hash,
    /// Its content-defined chunks in order, each with its length. Empty when the file is one blob (at most 4 MiB).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chunks: Vec<(Hash, u64)>,
}

impl Manifest {
    /// Stores one file's bytes in `blobs` and describes the file.
    pub fn store(blobs: &Blobs, name: &str, reader: impl Read) -> Result<File, String> {
        let (root, len) = blobs.put_reader(reader)?;
        let chunks = blobs.chunks(&root)?.unwrap_or_default();
        Ok(File {
            name: name.to_string(),
            len,
            root,
            chunks,
        })
    }

    /// The manifest's id: the hash of its JSON.
    pub fn id(&self) -> String {
        Hash::of(&serde_json::to_vec(self).unwrap_or_default()).hex()
    }

    /// The blobs a node must hold to offer this manifest: each file's chunks, or the file itself when it is one blob.
    pub fn leaves(&self) -> Vec<Hash> {
        self.files
            .iter()
            .flat_map(|file| {
                if file.chunks.is_empty() {
                    vec![file.root]
                } else {
                    file.chunks.iter().map(|(hash, _)| *hash).collect()
                }
            })
            .collect()
    }

    /// The bytes of the manifest's files.
    pub fn size(&self) -> u64 {
        self.files.iter().map(|file| file.len).sum()
    }
}
