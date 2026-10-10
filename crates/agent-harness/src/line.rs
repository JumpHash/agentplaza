//! Newline-delimited UTF-8 JSON framing: the byte-level half of the frozen
//! guest↔host protocol.
//!
//! One protocol message is one line. A line is `\n`-terminated and every byte
//! before the terminator is UTF-8. Reading is bounded by [`MAX_LINE_BYTES`]:
//! an over-long line is an error rather than unbounded growth of this process,
//! and a peer that closes mid-line yields the partial line and then
//! end-of-stream.
//!
//! A receive may also be given a deadline ([`LineTransport::recv_line_by`]).
//! The keepalive session needs one: it beats on a schedule while it waits, so a
//! wait that returns nothing has to end on time rather than block until the
//! peer speaks. Bytes of an unfinished line are kept, so a deadline that
//! expires mid-line loses nothing.

use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

/// The largest line this side will assemble, in bytes (1 MiB).
pub const MAX_LINE_BYTES: usize = 1 << 20;

/// How many bytes one read takes from the underlying stream.
const READ_CHUNK: usize = 4096;

/// What one bounded receive saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Received {
    /// One line arrived, without its terminator.
    Line(String),
    /// The deadline passed before a complete line arrived. Bytes of an
    /// unfinished line, if any, stay buffered for the next receive.
    TimedOut,
    /// The peer closed. A final line without its terminator is delivered as
    /// [`Received::Line`] first, so no bytes of a closed peer are lost.
    Closed,
}

/// A two-way line channel.
///
/// The protocol runs over [`StreamLines`] in the guest and over an in-process
/// pair in tests, so the session logic never mentions a socket.
pub trait LineTransport {
    /// Sends one line; a trailing newline is appended and the stream flushed.
    ///
    /// A `line` that contains a newline is refused: the peer would read it as
    /// two lines, which is a framing break, not a message.
    fn send_line(&mut self, line: &str) -> io::Result<()>;

    /// Receives one line, waiting at most `timeout`; `None` waits until the
    /// peer speaks or closes.
    fn recv_line_by(&mut self, timeout: Option<Duration>) -> io::Result<Received>;

    /// Receives one line without its terminator; `Ok(None)` means the peer
    /// closed cleanly.
    fn recv_line(&mut self) -> io::Result<Option<String>> {
        match self.recv_line_by(None)? {
            Received::Line(line) => Ok(Some(line)),
            Received::Closed => Ok(None),
            // A transport that times out a wait with no deadline has not
            // honoured the request; say so rather than reporting a close.
            Received::TimedOut => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "the transport timed out a receive that had no deadline",
            )),
        }
    }
}

/// A byte stream whose readability can be awaited with a deadline.
///
/// Framing needs this and nothing else from the transport: it is the one
/// syscall-adjacent question [`StreamLines`] has to ask, and the guest's
/// AF_VSOCK socket answers it with `poll(2)` (see `crate::vsock`).
pub trait WaitReadable {
    /// Waits until the stream may be readable, or `timeout` passes. `Ok(true)`
    /// means a read now will not block (it may still return end-of-stream).
    fn wait_readable(&self, timeout: Duration) -> io::Result<bool>;
}

/// A [`LineTransport`] over any byte stream.
#[derive(Debug)]
pub struct StreamLines<S> {
    inner: S,
    buf: Vec<u8>,
    eof: bool,
}

impl<S> StreamLines<S> {
    /// Wraps `inner`.
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            buf: Vec::new(),
            eof: false,
        }
    }
}

impl<S: Read + Write + WaitReadable> LineTransport for StreamLines<S> {
    fn send_line(&mut self, line: &str) -> io::Result<()> {
        if line.as_bytes().contains(&b'\n') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "line contains a newline",
            ));
        }
        self.inner.write_all(line.as_bytes())?;
        self.inner.write_all(b"\n")?;
        self.inner.flush()
    }

    fn recv_line_by(&mut self, timeout: Option<Duration>) -> io::Result<Received> {
        let deadline = timeout.map(|wait| Instant::now() + wait);
        loop {
            if let Some(newline) = self.buf.iter().position(|byte| *byte == b'\n') {
                let mut line: Vec<u8> = self.buf.drain(..=newline).collect();
                line.pop();
                return decode(line).map(Received::Line);
            }
            if self.eof {
                if self.buf.is_empty() {
                    return Ok(Received::Closed);
                }
                return decode(std::mem::take(&mut self.buf)).map(Received::Line);
            }
            if self.buf.len() > MAX_LINE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("line exceeds {MAX_LINE_BYTES} bytes"),
                ));
            }
            if let Some(deadline) = deadline {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() || !self.inner.wait_readable(left)? {
                    return Ok(Received::TimedOut);
                }
            }
            let mut chunk = [0u8; READ_CHUNK];
            match self.inner.read(&mut chunk) {
                Ok(0) => self.eof = true,
                Ok(read) => self.buf.extend_from_slice(&chunk[..read]),
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => return Err(err),
            }
        }
    }
}

/// Decodes a framed line: UTF-8, with a trailing `\r` dropped so a peer that
/// speaks CRLF stays readable.
fn decode(mut line: Vec<u8>) -> io::Result<String> {
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    String::from_utf8(line)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "line is not UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    use std::cell::Cell;

    /// A stream that yields `chunks` and then EOF; everything written to it
    /// lands in a buffer the test keeps a handle on.
    ///
    /// `waits` is what [`WaitReadable::wait_readable`] answers: it is the
    /// number of further waits that report the stream readable, after which
    /// they report it quiet (which is how a test asks for a timeout). It is
    /// shared with the test so quiet stretches can be ended.
    #[derive(Debug)]
    struct Stub {
        chunks: Vec<Vec<u8>>,
        written: Rc<RefCell<Vec<u8>>>,
        waits: Rc<Cell<usize>>,
    }

    impl Stub {
        /// A stream over `chunks` that is always readable, plus a handle on
        /// what gets written.
        fn new(chunks: Vec<Vec<u8>>) -> (Self, Rc<RefCell<Vec<u8>>>) {
            let written = Rc::new(RefCell::new(Vec::new()));
            (
                Self {
                    chunks,
                    written: Rc::clone(&written),
                    waits: Rc::new(Cell::new(usize::MAX)),
                },
                written,
            )
        }

        /// A stream that reads nothing.
        fn sink() -> (Self, Rc<RefCell<Vec<u8>>>) {
            Self::new(Vec::new())
        }

        /// A stream that never becomes readable.
        fn silent() -> Self {
            Self {
                chunks: Vec::new(),
                written: Rc::new(RefCell::new(Vec::new())),
                waits: Rc::new(Cell::new(0)),
            }
        }
    }

    impl Read for Stub {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.chunks.is_empty() {
                return Ok(0);
            }
            let take = self.chunks[0].len().min(buf.len());
            buf[..take].copy_from_slice(&self.chunks[0][..take]);
            self.chunks[0].drain(..take);
            if self.chunks[0].is_empty() {
                self.chunks.remove(0);
            }
            Ok(take)
        }
    }

    impl Write for Stub {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.written.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl WaitReadable for Stub {
        fn wait_readable(&self, _timeout: Duration) -> io::Result<bool> {
            let left = self.waits.get();
            if left == 0 {
                return Ok(false);
            }
            self.waits.set(left.saturating_sub(1));
            Ok(true)
        }
    }

    #[test]
    fn a_line_split_across_reads_is_reassembled() {
        let (stub, _written) = Stub::new(vec![
            b"{\"type\":\"wel".to_vec(),
            b"come\"}".to_vec(),
            b"\n".to_vec(),
        ]);
        let mut lines = StreamLines::new(stub);
        assert_eq!(
            lines.recv_line().expect("read succeeds"),
            Some(r#"{"type":"welcome"}"#.to_string())
        );
        assert_eq!(lines.recv_line().expect("read succeeds"), None);
    }

    #[test]
    fn send_line_appends_a_newline_and_refuses_an_embedded_one() {
        let (stub, written) = Stub::sink();
        let mut lines = StreamLines::new(stub);
        lines.send_line("hello").expect("send succeeds");
        lines.send_line("").expect("send succeeds");
        assert_eq!(*written.borrow(), b"hello\n\n");

        let (stub, _written) = Stub::sink();
        let mut lines = StreamLines::new(stub);
        assert!(lines.send_line("two\nlines").is_err());
    }

    #[test]
    fn what_send_line_writes_is_what_recv_line_reads() {
        let (stub, written) = Stub::sink();
        let mut out = StreamLines::new(stub);
        out.send_line("one").expect("send succeeds");
        out.send_line("two").expect("send succeeds");
        let bytes = written.borrow().clone();
        assert_eq!(bytes, b"one\ntwo\n");

        let (stub, _written) = Stub::new(vec![bytes]);
        let mut input = StreamLines::new(stub);
        assert_eq!(
            input.recv_line().expect("read succeeds"),
            Some("one".to_string())
        );
        assert_eq!(
            input.recv_line().expect("read succeeds"),
            Some("two".to_string())
        );
        assert_eq!(input.recv_line().expect("read succeeds"), None);
    }

    #[test]
    fn an_over_long_line_is_an_error_not_a_growth() {
        let (stub, _written) = Stub::new(vec![vec![b'x'; MAX_LINE_BYTES + READ_CHUNK + 1]]);
        let mut lines = StreamLines::new(stub);
        let err = lines.recv_line().expect_err("over-long line is refused");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_final_line_without_a_newline_is_still_delivered() {
        let (stub, _written) = Stub::new(vec![b"no-newline".to_vec()]);
        let mut lines = StreamLines::new(stub);
        assert_eq!(
            lines.recv_line().expect("read succeeds"),
            Some("no-newline".to_string())
        );
        assert_eq!(lines.recv_line().expect("read succeeds"), None);
    }

    #[test]
    fn carriage_returns_are_stripped_and_non_utf8_is_refused() {
        let (stub, _written) = Stub::new(vec![b"crlf\r\n".to_vec(), vec![0xff, 0xfe, b'\n']]);
        let mut lines = StreamLines::new(stub);
        assert_eq!(
            lines.recv_line().expect("read succeeds"),
            Some("crlf".to_string())
        );
        let err = lines.recv_line().expect_err("non-UTF-8 line is refused");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_deadline_that_expires_reports_a_timeout() {
        let mut lines = StreamLines::new(Stub::silent());
        assert_eq!(
            lines
                .recv_line_by(Some(Duration::from_millis(1)))
                .expect("the wait succeeds"),
            Received::TimedOut
        );
        // The deadline is a wait limit, not a failure: the stream stays usable.
        assert_eq!(
            lines
                .recv_line_by(Some(Duration::from_millis(1)))
                .expect("the wait succeeds"),
            Received::TimedOut
        );
    }

    #[test]
    fn a_line_interrupted_by_a_timeout_is_read_whole() {
        // Half a line, then a wait that expires, then the rest: the bytes of
        // the unfinished line are kept, so a deadline loses nothing.
        let (stub, _written) = Stub::new(vec![
            b"{\"type\":\"heart".to_vec(),
            b"beat\",\"seq\":1}\n".to_vec(),
        ]);
        let waits = Rc::clone(&stub.waits);
        waits.set(1);
        let mut lines = StreamLines::new(stub);
        assert_eq!(
            lines
                .recv_line_by(Some(Duration::from_millis(1)))
                .expect("the wait succeeds"),
            Received::TimedOut
        );
        waits.set(usize::MAX);
        assert_eq!(
            lines
                .recv_line_by(Some(Duration::from_millis(1)))
                .expect("the read succeeds"),
            Received::Line(r#"{"type":"heartbeat","seq":1}"#.to_string())
        );
    }

    #[test]
    fn a_zero_deadline_never_waits_but_does_not_consume_the_peer() {
        let (stub, _written) = Stub::new(vec![b"ready\n".to_vec()]);
        let mut lines = StreamLines::new(stub);
        assert_eq!(
            lines
                .recv_line_by(Some(Duration::ZERO))
                .expect("the wait succeeds"),
            Received::TimedOut
        );
        // Nothing was read, so a receive that does wait still gets the line.
        assert_eq!(
            lines
                .recv_line_by(Some(Duration::from_millis(50)))
                .expect("the read succeeds"),
            Received::Line("ready".to_string())
        );
    }
}
