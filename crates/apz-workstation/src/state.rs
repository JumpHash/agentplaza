//! The daemon's state directory: the enrolment record, the settings, and the status the `status` command reads.
//!
//! Every file is private to the owner's account (0600 on Unix; on Windows the profile's ACL, see
//! `apz_os::private`). The record holds the station token, so nothing in this module prints it.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use apz_station::{AccountMode, StationSettings};
use serde::{Deserialize, Serialize};

use crate::facts::host_os;

/// The enrolment record: where the hub is, which station this is, and its token.
pub const RECORD_FILE: &str = "station.json";
/// The hub-side settings last pushed to the daemon.
pub const SETTINGS_FILE: &str = "settings.json";
/// What the running daemon last reported about its link.
pub const STATUS_FILE: &str = "status.json";
/// The log.
pub const LOG_FILE: &str = "apz-workstation.log";

/// What `join` stored: the hub, its certificate pin (when one was given), the station's id and token.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub hub: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<String>,
    pub station: String,
    pub token: String,
    pub name: String,
    pub account: AccountMode,
}

impl fmt::Debug for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Record")
            .field("hub", &self.hub)
            .field("pin", &self.pin.as_ref().map(|_| "set"))
            .field("station", &self.station)
            .field("token", &"<redacted>")
            .field("name", &self.name)
            .field("account", &self.account)
            .finish()
    }
}

impl Record {
    /// The pin as the 32 bytes the TLS check takes; `None` when the record has no pin.
    pub fn pin_bytes(&self) -> Result<Option<[u8; 32]>, String> {
        self.pin.as_deref().map(parse_pin).transpose()
    }
}

/// A certificate pin: base64url of the SHA-256 of the hub's certificate.
pub fn parse_pin(text: &str) -> Result<[u8; 32], String> {
    let raw = apz_trust::unb64(text).map_err(|e| format!("pin: {e}"))?;
    raw.try_into()
        .map_err(|v: Vec<u8>| format!("pin: {} bytes, want 32", v.len()))
}

/// What the daemon reports about its link, for the `status` command. Holds no secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub pid: u32,
    pub version: String,
    pub backend: String,
    pub connected: bool,
    /// Unix time the link last changed state.
    pub since: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// The state directory for this machine, by its OS's convention:
/// `$XDG_STATE_HOME/agentplaza-workstation` on Linux, `~/Library/Application Support/AgentPlaza Workstation`
/// on macOS, `%ProgramData%\AgentPlaza\Workstation` on Windows.
pub fn default_state_dir() -> Result<PathBuf, String> {
    let home = apz_os::dirs::home();
    let get = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    default_state_dir_with(host_os(), get, home)
}

/// [`default_state_dir`] with the environment and home passed in, so each OS's rule is testable.
pub fn default_state_dir_with(
    os: apz_station::Os,
    get: impl Fn(&str) -> Option<String>,
    home: Option<PathBuf>,
) -> Result<PathBuf, String> {
    match os {
        apz_station::Os::Linux => match get("XDG_STATE_HOME") {
            Some(dir) => Ok(PathBuf::from(dir).join("agentplaza-workstation")),
            None => home
                .map(|h| h.join(".local/state/agentplaza-workstation"))
                .ok_or_else(|| "no home directory: pass --state-dir".to_string()),
        },
        apz_station::Os::Macos => home
            .map(|h| h.join("Library/Application Support/AgentPlaza Workstation"))
            .ok_or_else(|| "no home directory: pass --state-dir".to_string()),
        apz_station::Os::Windows => {
            let programdata = get("ProgramData").unwrap_or_else(|| "C:\\ProgramData".to_string());
            Ok(PathBuf::from(programdata)
                .join("AgentPlaza")
                .join("Workstation"))
        }
    }
}

/// The enrolment record, or `None` when the machine has not joined.
pub fn load_record(dir: &Path) -> Result<Option<Record>, String> {
    read_json(&dir.join(RECORD_FILE))
}

/// Writes the enrolment record privately: mode 0600 on Unix, and on Windows an owner-only ACL (the record holds the
/// station token).
pub fn save_record(dir: &Path, record: &Record) -> Result<(), String> {
    write_json(dir, RECORD_FILE, record)?;
    crate::acl::restrict_to_owner(&dir.join(RECORD_FILE))
}

/// The saved settings, or the defaults when none were saved yet.
pub fn load_settings(dir: &Path) -> Result<StationSettings, String> {
    Ok(read_json(&dir.join(SETTINGS_FILE))?.unwrap_or_default())
}

/// Writes the settings privately.
pub fn save_settings(dir: &Path, settings: &StationSettings) -> Result<(), String> {
    write_json(dir, SETTINGS_FILE, settings)
}

/// The last status the daemon wrote, or `None`.
pub fn load_status(dir: &Path) -> Result<Option<Status>, String> {
    read_json(&dir.join(STATUS_FILE))
}

/// Writes the status privately.
pub fn save_status(dir: &Path, status: &Status) -> Result<(), String> {
    write_json(dir, STATUS_FILE, status)
}

/// Forgets this machine's enrolment: the record, the settings and the status. The log stays.
pub fn forget(dir: &Path) -> Result<(), String> {
    for name in [RECORD_FILE, SETTINGS_FILE, STATUS_FILE] {
        match fs::remove_file(dir.join(name)) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(format!("{name}: {err}")),
        }
    }
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, String> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("{}: {err}", path.display())),
    }
}

fn write_json<T: Serialize>(dir: &Path, name: &str, value: &T) -> Result<(), String> {
    apz_os::private::create_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| format!("{name}: {e}"))?;
    apz_os::private::write(&dir.join(name), &bytes).map_err(|e| format!("{name}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_station::Os;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apz-ws-state-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn record() -> Record {
        Record {
            hub: "https://hub.example:7443".into(),
            pin: Some(apz_trust::b64(&[7u8; 32])),
            station: "ws-7".into(),
            token: "secret-token".into(),
            name: "studio".into(),
            account: AccountMode::AgentUser {
                name: "agentws".into(),
            },
        }
    }

    #[test]
    fn the_default_directory_follows_each_os() {
        let none = |_: &str| None;
        let home = Some(PathBuf::from("/home/u"));
        assert_eq!(
            default_state_dir_with(Os::Linux, none, home.clone()).unwrap(),
            PathBuf::from("/home/u/.local/state/agentplaza-workstation")
        );
        let xdg = |name: &str| (name == "XDG_STATE_HOME").then(|| "/state".to_string());
        assert_eq!(
            default_state_dir_with(Os::Linux, xdg, home.clone()).unwrap(),
            PathBuf::from("/state/agentplaza-workstation")
        );
        assert_eq!(
            default_state_dir_with(Os::Macos, none, Some(PathBuf::from("/Users/u"))).unwrap(),
            PathBuf::from("/Users/u/Library/Application Support/AgentPlaza Workstation")
        );
        let pd = |name: &str| (name == "ProgramData").then(|| "D:\\PD".to_string());
        assert_eq!(
            default_state_dir_with(Os::Windows, pd, None).unwrap(),
            PathBuf::from("D:\\PD")
                .join("AgentPlaza")
                .join("Workstation")
        );
        assert!(default_state_dir_with(Os::Linux, none, None).is_err());
    }

    #[test]
    fn the_record_round_trips_privately_and_never_prints_its_token() {
        let dir = scratch("record");
        assert_eq!(load_record(&dir).unwrap(), None);
        save_record(&dir, &record()).unwrap();
        assert_eq!(load_record(&dir).unwrap(), Some(record()));
        assert!(!format!("{:?}", record()).contains("secret-token"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.join(RECORD_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "the record is private");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_default_until_saved_and_forget_removes_the_enrolment() {
        let dir = scratch("forget");
        assert_eq!(load_settings(&dir).unwrap(), StationSettings::default());
        save_record(&dir, &record()).unwrap();
        save_settings(
            &dir,
            &StationSettings {
                record: false,
                ..StationSettings::default()
            },
        )
        .unwrap();
        forget(&dir).unwrap();
        assert_eq!(load_record(&dir).unwrap(), None);
        assert_eq!(load_settings(&dir).unwrap(), StationSettings::default());
        forget(&dir).expect("forgetting twice is fine");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pin_is_thirty_two_bytes() {
        assert!(parse_pin(&apz_trust::b64(&[1u8; 32])).is_ok());
        assert!(parse_pin(&apz_trust::b64(&[1u8; 16])).is_err());
        assert!(parse_pin("!!").is_err());
    }
}
