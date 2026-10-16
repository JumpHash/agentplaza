//! Newline-delimited framing for [`crate::wire`]: one UTF-8 line per message,
//! over any byte stream.
//!
//! Reads are bounded by [`MAX_LINE_BYTES`]. A read that times out (the
//! stream's own read timeout, `WouldBlock`/`TimedOut`) is [`Recv::Idle`] and
//! keeps a partial line buffered, so a session can wake on a schedule without
//! losing bytes. A peer that closes mid-line yields that line, then
//! [`Recv::Closed`].

use std::io::{self, Read, Write};
use std::time::Duration;

pub use crate::wire::MAX_LINE_BYTES;

const READ_CHUNK: usize = 64 << 10;

/// What one [`Lines::recv`] saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recv {
    /// A whole line, without its `\n` (or `\r\n`).
    Line(String),
    /// The stream's read timeout passed with no whole line.
    Idle,
    /// The peer closed.
    Closed,
}

/// A stream whose reads can time out; how a session bounds its waits.
pub trait ReadTimeout {
    /// `None` blocks; `Some` must be non-zero.
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
}

impl ReadTimeout for std::net::TcpStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        std::net::TcpStream::set_read_timeout(self, timeout)
    }
}

#[cfg(unix)]
impl ReadTimeout for std::os::unix::net::UnixStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        std::os::unix::net::UnixStream::set_read_timeout(self, timeout)
    }
}

/// The host side of the guest protocol on Windows: an AF_UNIX stream.
#[cfg(windows)]
impl ReadTimeout for apz_uds::UnixStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        apz_uds::UnixStream::set_read_timeout(self, timeout)
    }
}

/// A line channel over a byte stream.
#[derive(Debug)]
pub struct Lines<S> {
    inner: S,
    buf: Vec<u8>,
    /// How much of `buf` is known to hold no `\n`.
    scanned: usize,
    eof: bool,
}

impl<S> Lines<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            buf: Vec::new(),
            scanned: 0,
            eof: false,
        }
    }

    pub fn get_ref(&self) -> &S {
        &self.inner
    }

    pub fn get_mut(&mut self) -> &mut S {
        &mut self.inner
    }

    /// Whether [`recv`](Self::recv) can return without reading the stream: a whole line is buffered, or the end of
    /// the stream was seen. A caller that waits for its stream to become readable checks this first, so a line
    /// already buffered is never left waiting for more bytes.
    pub fn pending(&self) -> bool {
        self.eof || self.buf.contains(&b'\n')
    }
}

impl<S: Read> Lines<S> {
    /// Reads until a whole line, a timeout, or the end of the stream. A line
    /// over [`MAX_LINE_BYTES`] or not UTF-8 is an `InvalidData` error.
    pub fn recv(&mut self) -> io::Result<Recv> {
        loop {
            if let Some(at) = self.buf[self.scanned..]
                .iter()
                .position(|byte| *byte == b'\n')
            {
                let mut line: Vec<u8> = self.buf.drain(..=self.scanned + at).collect();
                self.scanned = 0;
                line.pop();
                return decode(line).map(Recv::Line);
            }
            self.scanned = self.buf.len();
            if self.buf.len() > MAX_LINE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("line exceeds {MAX_LINE_BYTES} bytes"),
                ));
            }
            if self.eof {
                if self.buf.is_empty() {
                    return Ok(Recv::Closed);
                }
                self.scanned = 0;
                return decode(std::mem::take(&mut self.buf)).map(Recv::Line);
            }
            let start = self.buf.len();
            self.buf.resize(start + READ_CHUNK, 0);
            let read = self.inner.read(&mut self.buf[start..]);
            self.buf.truncate(start + *read.as_ref().unwrap_or(&0));
            match read {
                Ok(0) => self.eof = true,
                Ok(_) => {}
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(Recv::Idle);
                }
                Err(err) => return Err(err),
            }
        }
    }
}

impl<S: Write> Lines<S> {
    /// Writes `line` and its `\n`, and flushes. A line holding a `\n` would
    /// frame as two and is refused.
    pub fn send(&mut self, line: &str) -> io::Result<()> {
        if line.contains('\n') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "line contains a newline",
            ));
        }
        self.inner.write_all(line.as_bytes())?;
        self.inner.write_all(b"\n")?;
        self.inner.flush()
    }
}

/// UTF-8, with a trailing `\r` dropped so a CRLF peer stays readable.
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
    use std::collections::VecDeque;

    /// A stream that plays back reads: bytes, a timed-out read (`None`), then EOF.
    struct Script(VecDeque<Option<Vec<u8>>>);

    impl Read for Script {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.0.pop_front() {
                None => Ok(0),
                Some(None) => Err(io::ErrorKind::WouldBlock.into()),
                Some(Some(mut bytes)) => {
                    let take = bytes.len().min(buf.len());
                    buf[..take].copy_from_slice(&bytes[..take]);
                    if take < bytes.len() {
                        self.0.push_front(Some(bytes.split_off(take)));
                    }
                    Ok(take)
                }
            }
        }
    }

    fn script(steps: &[Option<&[u8]>]) -> Lines<Script> {
        Lines::new(Script(
            steps.iter().map(|step| step.map(<[u8]>::to_vec)).collect(),
        ))
    }

    fn line(text: &str) -> Recv {
        Recv::Line(text.to_string())
    }

    #[test]
    fn a_line_split_across_reads_and_a_timeout_is_reassembled() {
        let mut lines = script(&[Some(b"{\"type\":\"heart"), None, Some(b"beat\"}\r\nlast")]);
        assert_eq!(lines.recv().unwrap(), Recv::Idle);
        assert_eq!(lines.recv().unwrap(), line(r#"{"type":"heartbeat"}"#));
        assert_eq!(lines.recv().unwrap(), line("last"));
        assert_eq!(lines.recv().unwrap(), Recv::Closed);
    }

    #[test]
    fn an_over_long_or_non_utf8_line_is_an_error() {
        let long = vec![b'x'; MAX_LINE_BYTES + 1];
        let err = script(&[Some(&long)]).recv().expect_err("over-long");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let err = script(&[Some(b"\xff\n")]).recv().expect_err("not UTF-8");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn send_appends_a_newline_and_refuses_an_embedded_one() {
        let mut lines = Lines::new(Vec::new());
        lines.send("one").unwrap();
        lines.send("").unwrap();
        assert_eq!(lines.get_ref(), b"one\n\n");
        assert!(lines.send("two\nlines").is_err());
    }

    #[test]
    fn a_fresh_channel_over_an_empty_stream_is_not_pending() {
        let lines = script(&[]);
        assert!(!lines.pending());
    }

    #[test]
    fn a_second_buffered_line_is_pending_after_the_first_is_taken() {
        let mut lines = script(&[Some(b"a\nb\n")]);
        assert_eq!(lines.recv().unwrap(), line("a"));
        assert!(lines.pending());
        assert_eq!(lines.recv().unwrap(), line("b"));
    }

    #[test]
    fn a_channel_that_saw_eof_is_pending() {
        let mut lines = script(&[]);
        assert_eq!(lines.recv().unwrap(), Recv::Closed);
        assert!(lines.pending());
    }
}
