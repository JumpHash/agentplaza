//! The serial captures as the stream moves them: the offset-idempotent merge
//! and the per-VM capture bookkeeping it needs — pure, no iced, no sockets.
//!
//! A `serial` event carries the byte offset its text starts at in the VM's
//! capture, which makes the merge a one-rule decision per VM:
//!
//! * `offset == length` — the text continues the capture: append it;
//! * `offset < length` — the text starts inside what is already held: clip the
//!   bytes already known and append the rest. A frame that is entirely known
//!   (a replayed event, a reconnect that re-sends) appends **nothing**, which
//!   is what keeps a transcript free of duplicates;
//! * `offset > length` — bytes went by unseen: the capture is no longer
//!   contiguous, so nothing is invented and the caller re-fetches it.
//!
//! The bookkeeping is one [`Capture`] per (run, vm): the length in bytes, the
//! lines it has ended (the count of `\n`, so a poll's whole capture and a
//! stream's deltas count a capture the same way), and whether those numbers
//! came from a real capture — only then are they a rate the register may
//! infer from, because after a gap the length is known but the middle is not.

use std::collections::HashMap;

use crate::api::Run;

/// What one merge did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Merge {
    /// Every byte the event carries is already in the capture: append
    /// nothing. This is what makes an event idempotent.
    Known,
    /// Append `text[skip..]` to the capture: `bytes` and `lines` are what
    /// that adds, and `skip` counts the bytes the capture already had.
    Append {
        /// How many bytes of the event's text to skip.
        skip: usize,
        /// How many bytes are being appended.
        bytes: u64,
        /// How many lines those bytes end.
        lines: u64,
    },
    /// The event starts past the capture's end: bytes went by unseen, so the
    /// capture has to be read again rather than patched.
    Gap {
        /// What the capture's own length was.
        known: u64,
        /// Where the event starts.
        offset: u64,
    },
}

/// One (run, vm)'s capture, as the stream knows it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capture {
    /// The capture's length in bytes, as far as the last sync or merge knew.
    length: u64,
    /// How many lines the capture has ended, counted in `\n` bytes.
    lines: u64,
    /// Whether `length` and `lines` describe a capture this console has
    /// actually read. After a gap they do not: the length is right, the
    /// lines are unknown, and no rate is inferred until the next read.
    synced: bool,
}

/// Every VM's capture bookkeeping.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Feed {
    captures: HashMap<(String, u32), Capture>,
}

impl Feed {
    /// A feed that has read nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a capture that was read whole, from a poll or a fetch: its
    /// length and its line count are now the truth, and the next event is
    /// merged against them.
    pub fn sync(&mut self, run: &str, vm: u32, capture: &str) {
        let entry = self.captures.entry((run.to_string(), vm)).or_default();
        entry.length = capture.len() as u64;
        entry.lines = newlines(capture);
        entry.synced = true;
    }

    /// Merges one serial event into one VM's bookkeeping.
    pub fn merge(&mut self, run: &str, vm: u32, offset: u64, text: &str) -> Merge {
        let key = (run.to_string(), vm);
        let known = self.captures.get(&key).map_or(0, |capture| capture.length);
        let bytes = text.len() as u64;

        if offset > known {
            return self.gap(key, known, offset, bytes);
        }
        if offset + bytes <= known {
            // The event replays bytes the capture already has: nothing to
            // append, and nothing to re-read.
            return Merge::Known;
        }
        let skip = (known - offset) as usize;
        if !text.is_char_boundary(skip) {
            // The clip would split a character: the daemon's offsets are byte
            // offsets, so this is a capture whose bytes are not ours to
            // splice. Read it again rather than write half a character.
            return self.gap(key, known, offset, bytes);
        }

        let added = &text[skip..];
        let lines = newlines(added);
        let entry = self.captures.entry(key).or_default();
        entry.length += added.len() as u64;
        entry.lines += lines;
        Merge::Append {
            skip,
            bytes: added.len() as u64,
            lines,
        }
    }

    /// Records that a stretch went by unseen: the length moves to where the
    /// event ends, the lines and the sync flag do not survive it, and the
    /// next append is merged against the new length.
    fn gap(&mut self, key: (String, u32), known: u64, offset: u64, bytes: u64) -> Merge {
        let entry = self.captures.entry(key).or_default();
        entry.length = offset + bytes;
        entry.lines = 0;
        entry.synced = false;
        Merge::Gap { known, offset }
    }

    /// The capture's line count, when it is one this console read: the rate
    /// the register infers between polls. `None` after a gap.
    pub fn lines(&self, run: &str, vm: u32) -> Option<u64> {
        self.captures
            .get(&(run.to_string(), vm))
            .filter(|capture| capture.synced)
            .map(|capture| capture.lines)
    }

    /// The capture's length in bytes, as far as it is known; 0 for a VM the
    /// stream has never named.
    pub fn length(&self, run: &str, vm: u32) -> u64 {
        self.captures
            .get(&(run.to_string(), vm))
            .map_or(0, |capture| capture.length)
    }

    /// Forgets every run that is no longer in the register.
    pub fn retain(&mut self, runs: &[Run]) {
        self.captures
            .retain(|(run, _), _| runs.iter().any(|known| known.id == *run));
    }

    /// Forgets one run's captures: a run id the daemon has begun again (a
    /// restart makes its ids start over) is a different run, and the lengths
    /// the old one left behind are no longer about it.
    pub fn forget(&mut self, run: &str) {
        self.captures.retain(|(id, _), _| id != run);
    }

    /// How many captures are being tracked: the stream's feed size.
    pub fn len(&self) -> usize {
        self.captures.len()
    }

    /// Whether nothing is being tracked.
    pub fn is_empty(&self) -> bool {
        self.captures.is_empty()
    }
}

/// How many lines a capture has ended: its `\n` bytes.
///
/// `str::lines` counts a trailing partial line as a line, which a delta
/// cannot see — the guest may still be typing it. Counting the `\n` bytes is
/// the one measure a whole capture and a stream of deltas agree on.
fn newlines(text: &str) -> u64 {
    text.bytes().filter(|byte| *byte == b'\n').count() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: &str) -> Run {
        Run {
            id: id.to_string(),
            state: crate::api::RunState::Running,
            vcpus: 1,
            memory_mib: 128,
            beats: None,
            created_at: 0,
            ended_at: None,
            vms: Vec::new(),
        }
    }

    #[test]
    fn the_first_event_of_a_capture_appends_from_the_start() {
        let mut feed = Feed::new();
        let merge = feed.merge("r1", 0, 0, "login: plaza\n");
        assert_eq!(
            merge,
            Merge::Append {
                skip: 0,
                bytes: 13,
                lines: 1
            }
        );
        assert_eq!(feed.length("r1", 0), 13);
        assert_eq!(feed.lines("r1", 0), None, "nothing was read whole yet");
    }

    #[test]
    fn an_offset_at_the_end_appends_and_an_offset_inside_clips() {
        let mut feed = Feed::new();
        feed.sync("r1", 0, "login: plaza\n$ uname\n");
        assert_eq!(feed.length("r1", 0), 21);
        assert_eq!(feed.lines("r1", 0), Some(2));

        // The next event starts where the capture ended.
        assert_eq!(
            feed.merge("r1", 0, 21, "Linux plaza\n"),
            Merge::Append {
                skip: 0,
                bytes: 12,
                lines: 1
            }
        );
        assert_eq!(feed.length("r1", 0), 33);
        assert_eq!(feed.lines("r1", 0), Some(3), "the line count moves with it");

        // An event that starts inside the capture makes the caller clip.
        assert_eq!(
            feed.merge("r1", 0, 27, "plaza\nmore\n"),
            Merge::Append {
                skip: 6,
                bytes: 5,
                lines: 1
            },
            "the first six bytes are already held"
        );
        assert_eq!(feed.length("r1", 0), 38);
    }

    #[test]
    fn an_event_that_is_entirely_known_appends_nothing() {
        let mut feed = Feed::new();
        feed.sync("r1", 0, "login: plaza\n$ uname\n");
        // A replay: the same event twice, and a replayed stretch inside what
        // is held. Neither may duplicate a byte.
        assert_eq!(feed.merge("r1", 0, 0, "login: plaza\n"), Merge::Known);
        assert_eq!(feed.merge("r1", 0, 6, "plaza\n"), Merge::Known);
        assert_eq!(feed.length("r1", 0), 21, "the capture did not grow");
        assert_eq!(feed.lines("r1", 0), Some(2), "and neither did its lines");
    }

    #[test]
    fn an_offset_past_the_capture_is_a_gap() {
        let mut feed = Feed::new();
        feed.sync("r1", 0, "login: plaza\n");
        assert_eq!(
            feed.merge("r1", 0, 100, "much later\n"),
            Merge::Gap {
                known: 13,
                offset: 100
            }
        );
        assert_eq!(feed.length("r1", 0), 111, "the length moves to the event's end");
        assert_eq!(feed.lines("r1", 0), None, "a gap is not a rate");
    }

    #[test]
    fn the_first_event_of_an_unread_capture_is_a_gap_not_a_guess() {
        let mut feed = Feed::new();
        assert_eq!(
            feed.merge("r1", 0, 16_647, "…"),
            Merge::Gap {
                known: 0,
                offset: 16_647
            }
        );
        assert_eq!(feed.length("r1", 0), 16_647 + "…".len() as u64);
        assert_eq!(feed.lines("r1", 0), None);
    }

    #[test]
    fn after_a_gap_an_append_continues_from_the_events_own_end() {
        let mut feed = Feed::new();
        feed.sync("r1", 0, "abc\n");
        assert!(matches!(feed.merge("r1", 0, 40, "def\n"), Merge::Gap { .. }));
        // The next event, at the gap's end, appends cleanly — the splice
        // point is where the stream was, even though the middle is missing.
        assert_eq!(
            feed.merge("r1", 0, 44, "ghi\n"),
            Merge::Append {
                skip: 0,
                bytes: 4,
                lines: 1
            }
        );
        assert_eq!(feed.lines("r1", 0), None, "still no rate: a sync has to happen first");

        feed.sync("r1", 0, "abc\ndef\nghi\n");
        assert_eq!(feed.lines("r1", 0), Some(3), "a real read restores the rate");
    }

    #[test]
    fn a_clip_that_would_split_a_character_is_a_gap() {
        let mut feed = Feed::new();
        let capture = "é is two bytes\n";
        feed.sync("r1", 0, capture);
        // The capture is 16 bytes; an offset one byte inside it lands in the
        // middle of the first character.
        let offset = 15;
        assert_eq!(
            feed.merge("r1", 0, offset, "…\n"),
            Merge::Gap {
                known: 16,
                offset
            },
            "half a character is not appended"
        );
    }

    #[test]
    fn a_vms_capture_is_tracked_on_its_own() {
        let mut feed = Feed::new();
        feed.sync("r1", 0, "one\n");
        feed.sync("r1", 1, "two\n");
        assert_eq!(feed.lines("r1", 0), Some(1));
        assert_eq!(feed.lines("r1", 1), Some(1));
        assert_eq!(
            feed.merge("r1", 1, 4, "three\n"),
            Merge::Append {
                skip: 0,
                bytes: 6,
                lines: 1
            }
        );
        assert_eq!(feed.length("r1", 0), 4, "vm 0's capture did not move");
        assert_eq!(feed.length("r1", 1), 10);
    }

    #[test]
    fn a_run_that_left_the_register_takes_its_captures_with_it() {
        let mut feed = Feed::new();
        feed.sync("r1", 0, "one\n");
        feed.sync("r2", 0, "two\n");
        assert_eq!(feed.len(), 2);

        feed.retain(&[run("r2")]);
        assert_eq!(feed.len(), 1);
        assert_eq!(feed.lines("r2", 0), Some(1));
        assert_eq!(feed.lines("r1", 0), None);

        feed.retain(&[]);
        assert!(feed.is_empty(), "every run gone, every capture gone");
    }

    #[test]
    fn a_captures_lines_are_counted_the_same_way_a_poll_counts_them() {
        // A trailing partial line is not a line yet: the guest is still
        // typing it, and a delta cannot know it is finished.
        assert_eq!(newlines(""), 0);
        assert_eq!(newlines("no newline"), 0);
        assert_eq!(newlines("one\n"), 1);
        assert_eq!(newlines("one\ntwo\r\n"), 2);
        assert_eq!(newlines("one\n\n\n"), 3);
    }
}
