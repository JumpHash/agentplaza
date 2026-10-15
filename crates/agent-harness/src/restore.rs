//! Catching a restored guest up with the host: the clock and the kernel's random state.
//!
//! A guest woken, forked or started from a warm template resumes with the clock and the random
//! pool it had when the snapshot was taken. Two restored guests would then share a random stream,
//! and each would believe it is still the moment it was frozen. The hub's welcome carries the
//! host's clock and 32 fresh bytes of its CSPRNG (see `HostFacts`); [`apply`] sets the clock when
//! it has drifted and mixes the bytes into the kernel's pool, before the session does anything
//! else.
//!
//! Nothing here fails the session. The harness may not be root (the tests are not), and a guest
//! that cannot set its clock still has a session to serve, so every failure is printed and the
//! caller goes on.

// Syscall boundary: the clock and the random pool are reached through `clock_settime` and the
// `RNDADDENTROPY`/`RNDRESEEDCRNG` ioctls, which `std` does not expose.
#![allow(unsafe_code)]

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::time::{SystemTime, UNIX_EPOCH};

use apz_proto::wire::HostFacts;

use crate::session::note;

/// How far the guest's clock may run from the host's before it is set, in milliseconds.
const CLOCK_TOLERANCE_MS: u64 = 1_000;

/// The bytes of host entropy a welcome carries: 64 hex digits.
const ENTROPY_BYTES: usize = 32;

/// `RNDADDENTROPY = _IOW('R', 0x03, int[2])` (`linux/random.h`): mixes a `rand_pool_info` into
/// the pool and credits its entropy count.
const RNDADDENTROPY: libc::c_ulong = 0x4008_5203;

/// `RNDRESEEDCRNG = _IO('R', 0x07)`: reseeds the CRNG from the pool at once.
const RNDRESEEDCRNG: libc::c_ulong = 0x5207;

/// `struct rand_pool_info` from `linux/random.h` for a [`ENTROPY_BYTES`]-byte buffer.
#[repr(C)]
struct RandPoolInfo {
    entropy_count: libc::c_int,
    buf_size: libc::c_int,
    buf: [u8; ENTROPY_BYTES],
}

/// Mixes host entropy (64 hex digits) into the kernel pool: the sandbox daemon's `/init` path, where
/// the host's clock arrives separately as E2B's `timestamp`.
pub fn mix_hex_entropy(hex: &str) -> Result<(), String> {
    let bytes = decode_entropy(hex).ok_or("the entropy is not 64 hex digits")?;
    mix_entropy(&bytes).map_err(|err| err.to_string())
}

/// Brings the guest up to the host's clock and mixes the host's entropy into the kernel's pool.
///
/// The clock is set only when it is off by more than [`CLOCK_TOLERANCE_MS`]: a guest that is
/// already right (a fresh boot, a guest the hub just started) is left alone. The entropy is always
/// mixed when it decodes. Every outcome is logged on `log`.
pub fn apply(facts: &HostFacts, log: &mut impl Write) {
    if let Some(offset) = clock_offset(now_ms(), facts.time_ms) {
        match set_realtime_clock(facts.time_ms) {
            Ok(()) => note(
                log,
                &format!("agent-harness: clock set (was off by {offset} ms)"),
            ),
            Err(err) => note(
                log,
                &format!("agent-harness: clock not set (off by {offset} ms): {err}"),
            ),
        }
    }
    match decode_entropy(&facts.entropy) {
        Some(bytes) => match mix_entropy(&bytes) {
            Ok(()) => note(
                log,
                "agent-harness: host entropy mixed into the kernel pool",
            ),
            Err(err) => note(
                log,
                &format!("agent-harness: host entropy not mixed: {err}"),
            ),
        },
        None => note(
            log,
            "agent-harness: host entropy not mixed: it is not 64 hex digits",
        ),
    }
}

/// The guest's realtime clock, in milliseconds since the Unix epoch.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// How far the guest's clock is from the host's, in milliseconds, when it is more than
/// [`CLOCK_TOLERANCE_MS`] off; `None` when it is close enough to leave alone.
fn clock_offset(guest_ms: u64, host_ms: u64) -> Option<u64> {
    let offset = guest_ms.abs_diff(host_ms);
    (offset > CLOCK_TOLERANCE_MS).then_some(offset)
}

/// Decodes the welcome's entropy: exactly 64 hex digits, which are 32 bytes. Upper- and lower-case
/// digits are both accepted; anything else (an empty string from a host whose urandom failed, a
/// wrong length, a stray character) is `None`, and nothing is mixed.
fn decode_entropy(hex: &str) -> Option<[u8; ENTROPY_BYTES]> {
    let digits = hex.as_bytes();
    if digits.len() != 2 * ENTROPY_BYTES {
        return None;
    }
    let mut bytes = [0u8; ENTROPY_BYTES];
    for (byte, pair) in bytes.iter_mut().zip(digits.chunks_exact(2)) {
        *byte = (hex_value(pair[0])? << 4) | hex_value(pair[1])?;
    }
    Some(bytes)
}

/// The value of one hex digit.
fn hex_value(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

/// Sets `CLOCK_REALTIME` to `host_ms`. Needs `CAP_SYS_TIME`.
fn set_realtime_clock(host_ms: u64) -> io::Result<()> {
    let seconds = libc::time_t::try_from(host_ms / 1_000)
        .map_err(|_| io::Error::other("the host's clock is out of range"))?;
    let timespec = libc::timespec {
        tv_sec: seconds,
        // Below one second, so it always fits a `c_long`.
        tv_nsec: libc::c_long::try_from((host_ms % 1_000) * 1_000_000).unwrap_or(0),
    };
    // SAFETY: `timespec` is a fully initialised `timespec` that lives across the call, and
    // `CLOCK_REALTIME` is a valid clock id; the result is checked below.
    let rc = unsafe { libc::clock_settime(libc::CLOCK_REALTIME, &raw const timespec) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Mixes `bytes` into the kernel's pool, crediting their full 256 bits, then reseeds the CRNG so
/// the next random number already reflects them. Needs `CAP_SYS_ADMIN`.
fn mix_entropy(bytes: &[u8; ENTROPY_BYTES]) -> io::Result<()> {
    let urandom = OpenOptions::new().write(true).open("/dev/urandom")?;
    let fd = urandom.as_raw_fd();
    let info = RandPoolInfo {
        entropy_count: 8 * ENTROPY_BYTES as libc::c_int,
        buf_size: ENTROPY_BYTES as libc::c_int,
        buf: *bytes,
    };
    // SAFETY: `info` is a `#[repr(C)]` `rand_pool_info` with a fully initialised buffer of
    // `buf_size` bytes, alive for the call; `fd` is the open `/dev/urandom` handle.
    let rc = unsafe { libc::ioctl(fd, RNDADDENTROPY, &raw const info) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `RNDRESEEDCRNG` takes no argument, and `fd` is the open `/dev/urandom` handle.
    let rc = unsafe { libc::ioctl(fd, RNDRESEEDCRNG) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entropy_decodes_from_64_hex_digits_in_either_case() {
        let hex = "00ff7f80abcdefABCDEF0123456789000102030405060708090a0b0c0d0e0f10";
        let bytes = decode_entropy(hex).expect("64 hex digits");
        assert_eq!(bytes[0], 0x00);
        assert_eq!(bytes[1], 0xff);
        assert_eq!(bytes[2], 0x7f);
        assert_eq!(bytes[3], 0x80);
        assert_eq!(bytes[4], 0xab);
        assert_eq!(bytes[31], 0x10);
        assert_eq!(
            decode_entropy(&"ab".repeat(32)),
            Some([0xab; ENTROPY_BYTES])
        );
    }

    #[test]
    fn a_malformed_entropy_string_decodes_to_nothing() {
        assert_eq!(decode_entropy(""), None);
        assert_eq!(decode_entropy(&"a".repeat(63)), None);
        assert_eq!(decode_entropy(&"a".repeat(66)), None);
        assert_eq!(decode_entropy(&format!("{}g", "a".repeat(63))), None);
        assert_eq!(decode_entropy(&format!("{} ", "a".repeat(63))), None);
    }

    #[test]
    fn the_clock_is_set_only_past_the_tolerance() {
        assert_eq!(clock_offset(5_000, 5_000), None);
        assert_eq!(clock_offset(5_000, 5_999), None);
        assert_eq!(clock_offset(5_999, 5_000), None);
        assert_eq!(clock_offset(5_000, 6_000), None);
        assert_eq!(clock_offset(5_000, 6_001), Some(1_001));
        assert_eq!(clock_offset(9_000, 1_000), Some(8_000));
        assert_eq!(clock_offset(0, u64::MAX), Some(u64::MAX));
    }

    #[test]
    fn a_guest_already_on_the_host_clock_logs_no_clock_change() {
        // The clock is within tolerance and the entropy is empty, so nothing outside this
        // process is touched.
        let facts = HostFacts {
            time_ms: now_ms(),
            entropy: String::new(),
        };
        let mut log = Vec::new();
        apply(&facts, &mut log);
        let log = String::from_utf8(log).expect("UTF-8");
        assert!(!log.contains("clock"), "{log}");
        assert!(
            log.contains("agent-harness: host entropy not mixed: it is not 64 hex digits"),
            "{log}"
        );
    }
}
