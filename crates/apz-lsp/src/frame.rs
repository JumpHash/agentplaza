//! JSON-RPC over a byte stream, framed as the protocol has it: `Content-Length` headers, a blank line, then the body.

use std::io::{self, BufRead, Write};

use serde_json::Value;

/// The largest body accepted; a server answering with more than this is refused rather than buffered.
pub const MAX_BODY: usize = 64 << 20;

/// Writes one message and flushes it.
pub fn write(w: &mut (impl Write + ?Sized), msg: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(msg).map_err(io::Error::other)?;
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(&body)?;
    w.flush()
}

/// Reads one message. `Ok(None)` is a clean end of the stream, between messages.
pub fn read(r: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length: Option<usize> = None;
    let mut started = false;
    let mut line = String::new();
    loop {
        line.clear();
        if r.read_line(&mut line)? == 0 {
            return if started {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the stream ended inside a header",
                ))
            } else {
                Ok(None)
            };
        }
        started = true;
        let header = line.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.trim().eq_ignore_ascii_case("Content-Length")
        {
            length = value.trim().parse().ok();
        }
    }
    let length = length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no Content-Length header"))?;
    if length > MAX_BODY {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a {length}-byte message is over the {MAX_BODY}-byte cap"),
        ));
    }
    let mut body = vec![0u8; length];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn messages_round_trip_and_the_stream_ends_cleanly() {
        let mut wire = Vec::new();
        write(&mut wire, &json!({"id": 1, "method": "a"})).unwrap();
        write(&mut wire, &json!({"id": 2, "result": "héllo"})).unwrap();
        let mut r = io::BufReader::new(&wire[..]);
        assert_eq!(read(&mut r).unwrap(), Some(json!({"id": 1, "method": "a"})));
        assert_eq!(
            read(&mut r).unwrap(),
            Some(json!({"id": 2, "result": "héllo"}))
        );
        assert_eq!(read(&mut r).unwrap(), None);
    }

    #[test]
    fn a_header_without_length_or_a_cut_body_is_an_error() {
        let mut r = io::BufReader::new(&b"Content-Type: x\r\n\r\n{}"[..]);
        assert_eq!(read(&mut r).unwrap_err().kind(), io::ErrorKind::InvalidData);
        let mut r = io::BufReader::new(&b"Content-Length: 9\r\n\r\n{}"[..]);
        assert_eq!(
            read(&mut r).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        let mut r = io::BufReader::new(&b"Content-Le"[..]);
        assert_eq!(
            read(&mut r).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
}
