//! An open file's text, with the hash it was read at. A save sends that hash, so
//! the hub can refuse a write over a newer one.

use serde::{Deserialize, Serialize};

/// An open file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Buffer {
    pub path: String,
    /// The content hash the text was read at. `None` for a file that did not exist.
    pub base_hash: Option<String>,
    pub text: String,
    /// Edited since it was read or last saved. An edit always sets it, even one that
    /// restores the old text: the buffer does not keep the old text to compare with.
    pub dirty: bool,
}

impl Buffer {
    /// A buffer just read from the hub, not dirty.
    pub fn new(
        path: impl Into<String>,
        base_hash: Option<String>,
        text: impl Into<String>,
    ) -> Self {
        Buffer {
            path: path.into(),
            base_hash,
            text: text.into(),
            dirty: false,
        }
    }

    /// Replaces the text and marks the buffer dirty.
    pub fn edit(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.dirty = true;
    }

    /// Records a successful save: the file now has `new_hash`, and the text is what the hub holds.
    pub fn saved(&mut self, new_hash: impl Into<String>) {
        self.base_hash = Some(new_hash.into());
        self.dirty = false;
    }

    /// Whether the file has changed on the hub since this buffer was read or saved.
    /// `server_hash` is the file's hash now (`None` if it does not exist).
    pub fn is_conflict(&self, server_hash: Option<&str>) -> bool {
        self.base_hash.as_deref() != server_hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_buffer_is_clean() {
        let b = Buffer::new("a.txt", Some("h1".into()), "alpha\n");
        assert!(!b.dirty);
        assert_eq!(b.base_hash.as_deref(), Some("h1"));
    }

    #[test]
    fn edit_marks_dirty_and_keeps_the_base() {
        let mut b = Buffer::new("a.txt", Some("h1".into()), "alpha\n");
        b.edit("beta\n");
        assert!(b.dirty);
        assert_eq!(b.text, "beta\n");
        assert_eq!(b.base_hash.as_deref(), Some("h1"));
    }

    #[test]
    fn saved_adopts_the_new_hash_and_clears_dirty() {
        let mut b = Buffer::new("a.txt", Some("h1".into()), "alpha\n");
        b.edit("beta\n");
        b.saved("h2");
        assert!(!b.dirty);
        assert_eq!(b.base_hash.as_deref(), Some("h2"));
        assert!(!b.is_conflict(Some("h2")));
    }

    #[test]
    fn conflict_when_the_server_hash_moved_on() {
        let b = Buffer::new("a.txt", Some("h1".into()), "alpha\n");
        assert!(!b.is_conflict(Some("h1")));
        assert!(b.is_conflict(Some("h2")), "an agent wrote the file");
        assert!(b.is_conflict(None), "the file was deleted");
    }

    #[test]
    fn a_new_file_conflicts_only_if_it_now_exists() {
        let b = Buffer::new("new.txt", None, "");
        assert!(!b.is_conflict(None));
        assert!(b.is_conflict(Some("h1")), "someone created it first");
    }
}
