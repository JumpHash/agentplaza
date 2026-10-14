//! The work-folder sync's shared rules and replies (`Op::Sync` carries the requests; see [`crate::SyncOp`]).
//!
//! The daemon and the hub both check names and sizes with these, so a path one side accepts the other accepts.
//! Files are content-addressed: a file is named by the SHA-256 of its bytes, in lower-case hex.

use serde::{Deserialize, Serialize};

/// The largest file the sync moves (512 MiB). A larger file in the work folder is left out of the sync.
pub const FILE_CAP: u64 = 512 << 20;
/// The most content one `Pull` reply or one `Push` request carries, before base64 (8 MiB). A WebSocket
/// message is at most 32 MiB, so a chunk and its encoding always fit.
pub const CHUNK: usize = 8 << 20;
/// The longest path the sync names, in bytes.
pub const PATH_CAP: usize = 1024;
/// The prefix of a partial upload's file, which lives beside the file it will become.
pub const PART_PREFIX: &str = ".apz-part-";

/// Whether a file of this name is never synced: editors' lock and swap files, macOS's metadata, and the
/// daemon's own partial uploads.
pub fn ignored_name(name: &str) -> bool {
    name.starts_with("~$")
        || name.starts_with(".#")
        || name.ends_with(".swp")
        || name == ".DS_Store"
        || name.starts_with(PART_PREFIX)
}

/// Whether `path` names a place under the work folder: a relative, forward-slash path with no empty, `.` or `..`
/// segment, no backslash, no NUL, no ignored name, and no segment or path longer than the caps.
pub fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= PATH_CAP
        && !path.starts_with('/')
        && !path.contains(['\\', '\0'])
        && path.split('/').all(|seg| {
            !seg.is_empty() && seg != "." && seg != ".." && seg.len() <= 255 && !ignored_name(seg)
        })
}

/// One file of a manifest: its path, its content hash and its size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncEntry {
    pub path: String,
    pub hash: String,
    pub size: u64,
}

/// Whether the work folder could be read when the daemon looked. A folder that is `Missing` (it does not exist, or
/// cannot be listed: an unmounted drive, a renamed folder) has no entries that mean anything; the hub must not read
/// its files as deleted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FolderState {
    #[default]
    Present,
    Missing,
}

/// The reply to `SyncOp::Manifest`: every synced file of the work folder, and whether the folder was there to list.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SyncManifest {
    pub entries: Vec<SyncEntry>,
    /// `Missing` with no entries when the folder could not be read. A daemon older than this field sends none, which
    /// reads as `Present`.
    #[serde(default)]
    pub state: FolderState,
}

/// The reply to `SyncOp::Pull`: one chunk of the file, from `offset`. `eof` is set on the last chunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncPull {
    pub path: String,
    pub hash: String,
    pub size: u64,
    pub offset: u64,
    pub data_b64: String,
    pub eof: bool,
}

/// The reply to `SyncOp::Push`: `received` bytes of the file are on the machine. `hash` is set once the file is
/// complete and in place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncPushed {
    pub path: String,
    pub received: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

/// The reply to `SyncOp::Status`: the files the daemon knows, and the changes it has not yet reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncStatus {
    pub files: u64,
    pub pending: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_wire;

    #[test]
    fn the_manifest_and_its_entries_are_wire_objects() {
        assert_wire(
            &SyncManifest {
                entries: vec![SyncEntry {
                    path: "reports/q3.csv".into(),
                    hash: "ab".into(),
                    size: 12,
                }],
                state: FolderState::Present,
            },
            r#"{"entries":[{"path":"reports/q3.csv","hash":"ab","size":12}],"state":"present"}"#,
        );
    }

    #[test]
    fn a_missing_folder_is_a_manifest_with_no_entries_and_says_so() {
        assert_wire(
            &SyncManifest {
                entries: vec![],
                state: FolderState::Missing,
            },
            r#"{"entries":[],"state":"missing"}"#,
        );
        let older: SyncManifest = serde_json::from_str(r#"{"entries":[]}"#).unwrap();
        assert_eq!(
            older.state,
            FolderState::Present,
            "a daemon without the field reads as present"
        );
    }

    #[test]
    fn a_pull_chunk_and_a_push_reply_are_wire_objects() {
        assert_wire(
            &SyncPull {
                path: "a.txt".into(),
                hash: "cc".into(),
                size: 4,
                offset: 2,
                data_b64: "eHk=".into(),
                eof: true,
            },
            r#"{"path":"a.txt","hash":"cc","size":4,"offset":2,"data_b64":"eHk=","eof":true}"#,
        );
        assert_wire(
            &SyncPushed {
                path: "a.txt".into(),
                received: 2,
                hash: None,
            },
            r#"{"path":"a.txt","received":2}"#,
        );
        assert_wire(
            &SyncPushed {
                path: "a.txt".into(),
                received: 4,
                hash: Some("cc".into()),
            },
            r#"{"path":"a.txt","received":4,"hash":"cc"}"#,
        );
    }

    #[test]
    fn the_status_counts_files_and_pending_changes() {
        assert_wire(
            &SyncStatus {
                files: 3,
                pending: 1,
            },
            r#"{"files":3,"pending":1}"#,
        );
    }

    #[test]
    fn temporary_and_lock_names_are_never_synced() {
        for name in [
            "~$report.docx",
            ".#notes.md",
            "draft.md.swp",
            ".DS_Store",
            ".apz-part-0123",
        ] {
            assert!(ignored_name(name), "{name}");
        }
        for name in ["report.docx", ".env", "~notes", "swap.swpx"] {
            assert!(!ignored_name(name), "{name}");
        }
    }

    #[test]
    fn a_path_stays_under_the_work_folder() {
        assert!(valid_path("reports/q3.csv"));
        assert!(valid_path(".config/app.toml"));
        for bad in [
            "",
            "/etc/passwd",
            "a//b",
            "a/../b",
            "./a",
            "a\\b",
            "a\0b",
            "a/~$x.docx",
            "a/.apz-part-1",
        ] {
            assert!(!valid_path(bad), "{bad:?}");
        }
        assert!(!valid_path(&"x/".repeat(PATH_CAP)));
    }
}
