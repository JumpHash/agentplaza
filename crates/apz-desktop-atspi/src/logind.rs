//! The owner's idle time from logind, for telling when someone is using the machine.
//!
//! logind keeps `IdleHint` and `IdleSinceHint` per session: the hint is set when the session has seen no input
//! for its idle timeout, and the timestamp is when the session went idle (CLOCK_REALTIME, microseconds).

use apz_desktop::{DesktopError, Result};
use zbus::zvariant::OwnedObjectPath;

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Manager {
    fn get_session(&self, id: &str) -> zbus::Result<OwnedObjectPath>;
    fn get_session_by_pid(&self, pid: u32) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "org.freedesktop.login1.Session",
    default_service = "org.freedesktop.login1"
)]
trait Session {
    #[zbus(property)]
    fn idle_hint(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn idle_since_hint(&self) -> zbus::Result<u64>;
}

/// Milliseconds the session has been idle, or 0 when it is not idle or logind gives no time for it.
///
/// The session is the one `XDG_SESSION_ID` names, else the session this process runs in. It is read from the
/// system bus, which logind serves; the owner's session bus is never used.
pub fn idle_ms() -> Result<u64> {
    async_io::block_on(async {
        let system = zbus::Connection::system().await.map_err(platform)?;
        let manager = ManagerProxy::new(&system).await.map_err(platform)?;
        let session_path = match std::env::var("XDG_SESSION_ID") {
            Ok(id) if !id.is_empty() => manager.get_session(&id).await,
            _ => manager.get_session_by_pid(std::process::id()).await,
        }
        .map_err(platform)?;
        let session = SessionProxy::builder(&system)
            .path(session_path)
            .map_err(platform)?
            .build()
            .await
            .map_err(platform)?;
        let idle = session.idle_hint().await.map_err(platform)?;
        let since = session.idle_since_hint().await.map_err(platform)?;
        Ok(idle_ms_from(idle, since, now_usec()))
    })
}

/// The idle milliseconds for a hint, its timestamp (microseconds since the epoch) and the current time.
pub(crate) fn idle_ms_from(idle: bool, since_usec: u64, now_usec: u64) -> u64 {
    if !idle || since_usec == 0 {
        return 0;
    }
    now_usec.saturating_sub(since_usec) / 1_000
}

fn now_usec() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn platform(error: impl std::fmt::Display) -> DesktopError {
    DesktopError::Platform(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::idle_ms_from;

    #[test]
    fn a_session_that_is_not_idle_has_no_idle_time() {
        assert_eq!(idle_ms_from(false, 1_000_000, 9_000_000), 0);
    }

    #[test]
    fn an_idle_session_counts_from_its_idle_timestamp() {
        // Idle since 1 s after the epoch, now 4.5 s: 3.5 s idle.
        assert_eq!(idle_ms_from(true, 1_000_000, 4_500_000), 3_500);
    }

    #[test]
    fn a_missing_timestamp_or_a_clock_behind_it_is_zero() {
        assert_eq!(idle_ms_from(true, 0, 4_500_000), 0);
        assert_eq!(idle_ms_from(true, 9_000_000, 4_500_000), 0);
    }
}
