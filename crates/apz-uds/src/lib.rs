//! Unix-domain stream sockets with one API on every platform.
//!
//! The VMM and the daemons talk to each other over socket paths of the form
//! `{dir}/vm{cid}_{port}`. On Unix this crate re-exports the standard library's
//! `UnixStream`, `UnixListener` and `SocketAddr`. On Windows 10 1803 and later,
//! WinSock supports `AF_UNIX` natively; the `windows` module implements the same
//! types over `windows-sys`, so callers compile unchanged on both platforms.
//!
//! The only platform-independent code here is [`encode_sun_path`], which checks
//! a socket path against the limits of `sockaddr_un`.

#![deny(unsafe_code)]

use std::io;
use std::path::Path;

#[cfg(unix)]
pub use std::os::unix::net::{SocketAddr, UnixListener, UnixStream};

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;
#[cfg(windows)]
pub use windows::{SocketAddr, UnixListener, UnixStream};

/// Size of `sockaddr_un.sun_path`, including the terminating NUL.
const SUN_PATH_LEN: usize = 108;

/// Encodes `path` as a NUL-terminated `sun_path` buffer.
///
/// The path must be valid UTF-8, non-empty, free of interior NUL bytes, and at
/// most 107 bytes long so that the terminating NUL fits.
pub fn encode_sun_path(path: &Path) -> io::Result<[u8; SUN_PATH_LEN]> {
    let bytes = path
        .to_str()
        .ok_or_else(|| invalid("socket path is not valid UTF-8"))?
        .as_bytes();
    if bytes.is_empty() {
        return Err(invalid("socket path is empty"));
    }
    if bytes.contains(&0) {
        return Err(invalid("socket path contains a NUL byte"));
    }
    if bytes.len() > SUN_PATH_LEN - 1 {
        return Err(invalid("socket path too long"));
    }
    let mut out = [0u8; SUN_PATH_LEN];
    out[..bytes.len()].copy_from_slice(bytes);
    Ok(out)
}

fn invalid(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn encode_sun_path_enforces_path_rules() {
        // Was encodes_path_and_nul_terminates: the path is copied in and the rest is NUL.
        {
            let buf = encode_sun_path(Path::new("/tmp/vm3_1024")).unwrap();
            assert_eq!(&buf[..13], b"/tmp/vm3_1024");
            assert!(buf[13..].iter().all(|&b| b == 0));
        }

        // Was accepts_exactly_107_bytes: 107 bytes fit with the terminating NUL at index 107.
        {
            let p = "a".repeat(107);
            let buf = encode_sun_path(Path::new(&p)).unwrap();
            assert_eq!(buf[106], b'a');
            assert_eq!(buf[107], 0);
        }

        // Was rejects_108_bytes_as_too_long: 108 bytes leave no room for the NUL.
        {
            let p = "a".repeat(108);
            let err = encode_sun_path(Path::new(&p)).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert!(err.to_string().contains("too long"));
        }

        // Was rejects_empty_path: an empty path is invalid.
        {
            let err = encode_sun_path(Path::new("")).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        }

        // Was rejects_interior_nul: a NUL byte inside the path is invalid.
        {
            let err = encode_sun_path(Path::new("a\0b")).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        }

        // Was rejects_non_utf8 (unix only): a path that is not valid UTF-8 is invalid.
        #[cfg(unix)]
        {
            use std::ffi::OsStr;
            use std::os::unix::ffi::OsStrExt;
            let p = Path::new(OsStr::from_bytes(b"/tmp/\xff"));
            let err = encode_sun_path(p).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        }
    }

    #[test]
    fn listener_round_trip() {
        let path = std::env::temp_dir().join(format!("apz-uds-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let mut client = UnixStream::connect(&path).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        client.write_all(b"ping").unwrap();
        let mut buf = [0u8; 4];
        server.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"ping");
        drop((server, client, listener));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn pair_exchanges_bytes_both_ways() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        a.write_all(b"ab").unwrap();
        let mut buf = [0u8; 2];
        b.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"ab");
        b.write_all(b"cd").unwrap();
        a.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"cd");
    }

    /// Clones behave like `dup`ed descriptors: a read timeout set before the
    /// clone bounds a read through it.
    #[test]
    fn a_read_timeout_set_before_cloning_bounds_the_clone() {
        let (a, _b) = UnixStream::pair().unwrap();
        a.set_read_timeout(Some(std::time::Duration::from_millis(50)))
            .unwrap();
        let mut clone = a.try_clone().unwrap();
        let (done, waited) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(clone.read(&mut [0u8; 1]).map_err(|err| err.kind()));
        });
        let result = waited
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the read through the clone timed out");
        assert!(
            matches!(
                result,
                Err(std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)
            ),
            "{result:?}"
        );
    }

    /// A `shutdown` through one handle ends a read blocked on another, as EOF
    /// (how vsockd's egress relays stop each other).
    #[test]
    fn a_shutdown_through_one_clone_ends_a_read_blocked_on_another() {
        let (a, _b) = UnixStream::pair().unwrap();
        let mut reader = a.try_clone().unwrap();
        let (done, waited) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(reader.read(&mut [0u8; 1]).map_err(|err| err.kind()));
        });
        std::thread::sleep(std::time::Duration::from_millis(100));
        a.shutdown(std::net::Shutdown::Both).unwrap();
        let result = waited
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the blocked read was not woken by the shutdown");
        assert_eq!(result, Ok(0));
    }
}
