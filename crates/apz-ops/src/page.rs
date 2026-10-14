//! The reply budget and the chunk boundaries paged replies use.
//!
//! A paged reply carries at most [`CHUNK`] bytes of text, cut on a character boundary, and short
//! enough that the whole reply fits [`OUTPUT_BUDGET`]. The caller says in the reply whether more
//! follows, so nothing is cut silently.

use apz_proto::wire::MAX_TOOL_OUTPUT;

/// Most text one chunk carries.
pub const CHUNK: usize = 48 * 1024;

/// A reply's size cap: the output cap a tool result has, less room for its envelope.
pub const OUTPUT_BUDGET: usize = MAX_TOOL_OUTPUT - 1024;

/// The end of the chunk of `text` that starts at `start`: at most [`CHUNK`] bytes, on a character
/// boundary, and short enough that `fits` (the reply around the chunk) is within the budget.
pub fn chunk_end(text: &str, start: usize, fits: impl Fn(usize) -> bool) -> usize {
    let mut end = floor_boundary(text, (start + CHUNK).min(text.len()));
    while end > start && !fits(end) {
        end = floor_boundary(text, start + (end - start) / 2);
    }
    end
}

/// The largest index at most `at` that starts a character of `text`.
fn floor_boundary(text: &str, mut at: usize) -> usize {
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// The byte-level twin of [`chunk_end`], for output that is not yet known to be text: a cut never
/// splits a UTF-8 character, so the chunk is a whole number of characters.
#[cfg(unix)]
pub(crate) fn byte_chunk_end(bytes: &[u8], fits: impl Fn(usize) -> bool) -> usize {
    let mut end = bytes.len().min(CHUNK);
    loop {
        while end > 0 && end < bytes.len() && is_continuation(bytes[end]) {
            end -= 1;
        }
        if end == 0 || fits(end) {
            return end;
        }
        end /= 2;
    }
}

/// Whether `byte` continues a multi-byte UTF-8 character rather than starting one.
#[cfg(unix)]
pub(crate) fn is_continuation(byte: u8) -> bool {
    byte & 0xC0 == 0x80
}
