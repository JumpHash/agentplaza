//! Newline-delimited UTF-8 JSON framing: the byte-level half of the guest↔host
//! protocol.
//!
//! One protocol message is one line. A line is `\n`-terminated and every byte
//! before the terminator is UTF-8. Reading is bounded by [`MAX_LINE_BYTES`]:
//! an over-long line is an error rather than unbounded growth of this process,
//! and a peer that closes mid-line yields the partial line and then
//! end-of-stream.
//!
//! This is the host-side copy of the framing the guest harness also
//! implements; the two are independent binaries with no shared library.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// The largest line this side will assemble, in bytes (1 MiB).
pub const MAX_LINE_BYTES: usize = 1 << 20;

/// How many bytes one read takes from the underlying stream.
const READ_CHUNK: usize = 4096;

/// What one read of a line produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inbound {
    /// A whole line, without its terminator.
    Line(String),
    /// The peer closed — cleanly, or mid-line, in which case the partial line
    /// was returned as a [`Inbound::Line`] first.
    Closed,
    /// Nothing arrived within the transport's read slice. A partial line, if
    /// one was already read, stays buffered and the next read continues it.
    Idle,
}

/// A stream whose individual reads can be bounded by a slice of time.
///
/// A keepalive session has to wake up regularly to check its idle deadline and
/// its outbound work, so its socket is given a short read slice and a read that
/// finds nothing reports [`Inbound::Idle`] rather than blocking the worker
/// thread forever.
pub trait ReadSlice {
    /// Sets how long one read waits before reporting idle.
    fn set_read_slice(&mut self, slice: Duration) -> io::Result<()>;
}

impl ReadSlice for UnixStream {
    fn set_read_slice(&mut self, slice: Duration) -> io::Result<()> {
        self.set_read_timeout(Some(slice))
    }
}

/// A two-way line channel.
///
/// The protocol runs over [`StreamLines`] in production and over an in-process
/// pair in tests, so the session logic never mentions a socket.
pub trait LineTransport {
    /// Sends one line; a trailing newline is appended and the stream flushed.
    ///
    /// A `line` that contains a newline is refused: the peer would read it as
    /// two lines, which is a framing break, not a message.
    fn send_line(&mut self, line: &str) -> io::Result<()>;

    /// Receives one line without its terminator; `Ok(None)` means the peer
    /// closed cleanly.
    ///
    /// A last line without a terminator is returned before the
    /// end-of-stream, so no bytes of a closed peer are lost.
    fn recv_line(&mut self) -> io::Result<Option<String>>;

    /// Receives one line, or reports that the peer sent nothing within this
    /// transport's read slice ([`Inbound::Idle`]). The default never reports
    /// idle — it blocks for a line, the behaviour of a transport with no read
    /// slice.
    fn recv_line_idle(&mut self) -> io::Result<Inbound> {
        Ok(match self.recv_line()? {
            Some(line) => Inbound::Line(line),
            None => Inbound::Closed,
        })
    }

    /// Bounds this transport's individual reads to `slice`; a transport with
    /// no read slice ignores it.
    fn set_read_slice(&mut self, _slice: Duration) -> io::Result<()> {
        Ok(())
    }
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

/// Whether a read error means "the read slice elapsed with nothing to read".
fn is_idle(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

impl<S: Read + Write + ReadSlice> LineTransport for StreamLines<S> {
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

    fn recv_line(&mut self) -> io::Result<Option<String>> {
        loop {
            if let Some(newline) = self.buf.iter().position(|byte| *byte == b'\n') {
                let mut line: Vec<u8> = self.buf.drain(..=newline).collect();
                line.pop();
                return decode(line).map(Some);
            }
            if self.eof {
                if self.buf.is_empty() {
                    return Ok(None);
                }
                return decode(std::mem::take(&mut self.buf)).map(Some);
            }
            if self.buf.len() > MAX_LINE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("line exceeds {MAX_LINE_BYTES} bytes"),
                ));
            }
            let mut chunk = [0u8; READ_CHUNK];
            match self.inner.read(&mut chunk) {
                Ok(0) => self.eof = true,
                Ok(read) => self.buf.extend_from_slice(&chunk[..read]),
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                // A read slice elapsing is reported to the caller, which
                // decides whether the session may continue waiting. Any
                // partial line stays in `buf` for the next read.
                Err(err) if is_idle(&err) => return Err(err),
                Err(err) => return Err(err),
            }
        }
    }

    fn recv_line_idle(&mut self) -> io::Result<Inbound> {
        match self.recv_line() {
            Ok(Some(line)) => Ok(Inbound::Line(line)),
            Ok(None) => Ok(Inbound::Closed),
            Err(err) if is_idle(&err) => Ok(Inbound::Idle),
            Err(err) => Err(err),
        }
    }

    fn set_read_slice(&mut self, slice: Duration) -> io::Result<()> {
        self.inner.set_read_slice(slice)
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

    /// A stream that yields `chunks` and then EOF; everything written to it
    /// lands in a buffer the test keeps a handle on.
    #[derive(Debug)]
    struct Stub {
        chunks: Vec<Vec<u8>>,
        written: Rc<RefCell<Vec<u8>>>,
    }

    impl Stub {
        /// A stream over `chunks`, plus a handle on what gets written.
        fn new(chunks: Vec<Vec<u8>>) -> (Self, Rc<RefCell<Vec<u8>>>) {
            let written = Rc::new(RefCell::new(Vec::new()));
            (
                Self {
                    chunks,
                    written: Rc::clone(&written),
                },
                written,
            )
        }

        /// A stream that reads nothing.
        fn sink() -> (Self, Rc<RefCell<Vec<u8>>>) {
            Self::new(Vec::new())
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

    impl ReadSlice for Stub {
        fn set_read_slice(&mut self, _slice: Duration) -> io::Result<()> {
            Ok(())
        }
    }

    /// A stream that yields a script of bytes, idles, and bytes, then EOF —
    /// what a socket with a read slice looks like to the framing layer.
    #[derive(Debug)]
    struct Scripted {
        steps: Vec<Step>,
    }

    #[derive(Debug)]
    enum Step {
        Bytes(Vec<u8>),
        /// The read slice elapses with nothing to read.
        Idle,
        Eof,
    }

    impl Read for Scripted {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.steps.first_mut() {
                None | Some(Step::Eof) => Ok(0),
                Some(Step::Idle) => {
                    self.steps.remove(0);
                    Err(io::Error::from(io::ErrorKind::WouldBlock))
                }
                Some(Step::Bytes(bytes)) => {
                    let take = bytes.len().min(buf.len());
                    buf[..take].copy_from_slice(&bytes[..take]);
                    bytes.copy_within(take.., 0);
                    bytes.truncate(bytes.len() - take);
                    let drained = bytes.is_empty();
                    if drained {
                        self.steps.remove(0);
                    }
                    Ok(take)
                }
            }
        }
    }

    impl Write for Scripted {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl ReadSlice for Scripted {
        fn set_read_slice(&mut self, _slice: Duration) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_line_split_across_reads_is_reassembled() {
        let (stub, _written) = Stub::new(vec![
            b"{\"type\":\"hel".to_vec(),
            b"lo\"}".to_vec(),
            b"\n".to_vec(),
        ]);
        let mut lines = StreamLines::new(stub);
        assert_eq!(
            lines.recv_line().expect("read succeeds"),
            Some(r#"{"type":"hello"}"#.to_string())
        );
        assert_eq!(lines.recv_line().expect("read succeeds"), None);
    }

    #[test]
    fn send_line_appends_a_newline_and_refuses_an_embedded_one() {
        let (stub, written) = Stub::sink();
        let mut lines = StreamLines::new(stub);
        lines.send_line("result").expect("send succeeds");
        lines.send_line("").expect("send succeeds");
        assert_eq!(*written.borrow(), b"result\n\n");

        let (stub, _written) = Stub::sink();
        let mut lines = StreamLines::new(stub);
        assert!(lines.send_line("two\nlines").is_err());
    }

    #[test]
    fn an_over_long_line_is_an_error_not_a_growth() {
        let (stub, _written) = Stub::new(vec![vec![b'x'; MAX_LINE_BYTES + READ_CHUNK + 1]]);
        let mut lines = StreamLines::new(stub);
        let err = lines.recv_line().expect_err("over-long line is refused");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    /// A read slice elapsing mid-line is `Idle`, not a lost line: the bytes
    /// already read stay buffered and the next read finishes the line.
    #[test]
    fn an_elapsed_slice_reports_idle_and_keeps_the_partial_line() {
        let scripted = Scripted {
            steps: vec![
                Step::Bytes(b"{\"type\":\"heart".to_vec()),
                Step::Idle,
                Step::Bytes(b"beat\",\"seq\":1}\n".to_vec()),
            ],
        };
        let mut lines = StreamLines::new(scripted);
        assert_eq!(lines.recv_line_idle().expect("read succeeds"), Inbound::Idle);
        assert_eq!(
            lines.recv_line_idle().expect("read succeeds"),
            Inbound::Line(r#"{"type":"heartbeat","seq":1}"#.to_string())
        );
    }

    #[test]
    fn a_closed_peer_is_reported_as_closed() {
        let scripted = Scripted {
            steps: vec![Step::Eof],
        };
        let mut lines = StreamLines::new(scripted);
        assert_eq!(
            lines.recv_line_idle().expect("read succeeds"),
            Inbound::Closed
        );
    }
}
