//! Enrolment for the front ends: the CLI (`agentplaza workstation`) and the desktop setup's **Add as a workstation**.
//!
//! These are the calls the `apz-workstation` binary makes, so every front end enrols a machine the same way: the
//! facts are gathered here, the join goes through [`crate::join`], and the status and leave read the state directory.
//! The hub's certificate pin is taken as hex (the form every other command takes); the record stores base64url.

use std::path::{Path, PathBuf};

use apz_station::{AccountMode, Os};

use crate::account::{self, AccountChoice};
use crate::backend::{self, Built, Probe};
use crate::facts::{self, Extras};
use crate::join::{self, JoinRequest};
use crate::plan::Plan;
use crate::service;
use crate::state::{self, Record, Status};

/// What a front end asks for when it joins this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enrol {
    /// `https://host:port` (or `http://` for a loopback hub).
    pub hub: String,
    /// The join code the hub showed once.
    pub code: String,
    /// The station's name on the hub; the machine's host name when `None`.
    pub name: Option<String>,
    /// The account the agents work in.
    pub account: AccountChoice,
    /// The hub's certificate SHA-256 as 64 hex digits (colons allowed), for a hub with its own certificate.
    pub pin: Option<String>,
}

/// What the state directory says about this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub dir: PathBuf,
    pub record: Option<Record>,
    pub status: Option<Status>,
}

/// The state directory for this machine, by its OS's convention.
pub fn default_dir() -> Result<PathBuf, String> {
    state::default_state_dir()
}

/// Joins this machine with the hub, storing the record in `dir`. Refuses an account name the OS would not take
/// before anything is sent to the hub.
pub fn join_here(dir: &Path, enrol: &Enrol) -> Result<Record, String> {
    let owner = account::current_user();
    enrol.account.validate(&owner)?;
    let account = enrol.account.account_mode(&owner);
    let facts = gather_facts(&account, dir);
    let name = enrol
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if facts.hostname.is_empty() {
                "workstation".to_string()
            } else {
                facts.hostname.clone()
            }
        });
    let pin = enrol.pin.as_deref().map(hex_pin_to_b64).transpose()?;
    let request = JoinRequest {
        hub: enrol.hub.trim_end_matches('/').to_string(),
        code: enrol.code.clone(),
        name,
        account,
        pin,
    };
    join::join(dir, &request, &facts)
}

/// This machine's enrolment and link status, from the state directory.
pub fn status_here(dir: &Path) -> Result<Snapshot, String> {
    Ok(Snapshot {
        dir: dir.to_path_buf(),
        record: state::load_record(dir)?,
        status: state::load_status(dir)?,
    })
}

/// The link as the daemon last reported it, or `unknown` when it has not run here.
pub fn link_line(snapshot: &Snapshot) -> String {
    match &snapshot.status {
        Some(status) if status.connected => format!("up ({} backend)", status.backend),
        Some(status) => match &status.last_error {
            Some(error) => format!("down: {error}"),
            None => "down".to_string(),
        },
        None => "unknown (the daemon has not run here)".to_string(),
    }
}

/// What joining will do on this machine, as plans, without doing it: the agents' account (for a separate user) and
/// the service. Reads the machine (whether the user exists, its id); changes nothing.
pub fn enrolment_plans(
    dir: &Path,
    choice: &AccountChoice,
) -> Result<Vec<(&'static str, Plan)>, String> {
    let owner = account::current_user();
    choice.validate(&owner)?;
    let mut plans = Vec::new();
    if let AccountChoice::Agent { user } = choice {
        plans.push(("account", account::ensure_agent_user(user, true)?));
    }
    plans.push((
        "service",
        service::install_service(dir, &choice.run_as(&owner))?,
    ));
    Ok(plans)
}

/// Whether any of the plans needs administrator rights this process does not have.
pub fn needs_admin(plans: &[(&'static str, Plan)]) -> bool {
    plans.iter().any(|(_, plan)| plan.privileged) && !account::is_privileged()
}

/// Creates the agents' account (for a separate user) and installs the service, after the record is joined. Each
/// step runs only with administrator rights where it needs them; a refusal is an error with the commands to run.
pub fn finish(dir: &Path, choice: &AccountChoice) -> Result<(), String> {
    let owner = account::current_user();
    if let AccountChoice::Agent { user } = choice {
        account::ensure_agent_user(user, false)?;
    }
    service::install_service(dir, &choice.run_as(&owner))?.execute()
}

/// What the platform still asks of the owner once (section 12 of the design), for the result screen.
pub fn permission_notes(os: Os) -> Vec<String> {
    match os {
        Os::Linux => vec![
            "Linux: on an X11 session nothing is needed: each agent gets its own private display.".to_string(),
            "On Wayland, the first screen share asks the desktop's portal once: approve it.".to_string(),
        ],
        Os::Macos => vec![
            "macOS: allow Screen Recording and Accessibility for apz-workstation in System Settings, Privacy & Security.".to_string(),
            "Sign in as the account the agents work in once, so the grants apply to it.".to_string(),
        ],
        Os::Windows => vec![
            "Windows: nothing to grant. The workstation runs as a logon task in the account's session.".to_string(),
            "Sign in as that account once to start it.".to_string(),
        ],
    }
}

/// Forgets this machine's enrolment in `dir` (the record, the settings and the status). The service is not touched:
/// see [`crate::service::remove_service`].
pub fn leave_here(dir: &Path) -> Result<(), String> {
    state::forget(dir)
}

/// The facts this machine reports on join, with the account it will work in.
pub fn gather_facts(account: &AccountMode, dir: &Path) -> apz_station::Facts {
    let probe = Probe::here();
    let built = Built::here();
    let choice = backend::choose(&probe, built);
    let mut desktop = backend::open(choice, &probe, built, dir);
    let apps = backend::apps_count(choice, desktop.as_mut());
    let extras = Extras {
        account: account.clone(),
        apps,
        virtual_displays: backend::virtual_displays(&probe, built),
        ui_tree: backend::ui_tree(&probe, choice, built),
    };
    facts::gather(&extras)
}

/// A hub certificate pin as 64 hex digits (colons and spaces allowed) to the base64url the record stores.
pub fn hex_pin_to_b64(text: &str) -> Result<String, String> {
    let digits: Vec<char> = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ':')
        .collect();
    if digits.len() != 64 || !digits.iter().all(|c| c.is_ascii_hexdigit()) {
        return Err("the pin is the hub certificate's SHA-256: 64 hex digits".to_string());
    }
    let bytes: Vec<u8> = digits
        .chunks(2)
        .map(|pair| {
            let hi = pair[0].to_digit(16).unwrap_or(0) as u8;
            let lo = pair[1].to_digit(16).unwrap_or(0) as u8;
            hi * 16 + lo
        })
        .collect();
    Ok(apz_trust::b64(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_pins_convert_to_the_stored_base64url_and_bad_pins_are_refused() {
        // a_hex_pin_becomes_the_base64url_the_record_stores: a hex pin is stored as base64url.
        {
            let hex = "00".repeat(31) + "ff";
            let text = hex_pin_to_b64(&hex).unwrap();
            assert_eq!(state::parse_pin(&text).unwrap()[31], 0xff);
            assert_eq!(state::parse_pin(&text).unwrap()[0], 0);
        }
        // colons_and_spaces_in_a_pin_are_allowed_and_short_pins_are_refused: colons are allowed,
        // a short pin and a non-hex pin are refused.
        {
            let colons = (0..32).map(|_| "ab").collect::<Vec<_>>().join(":");
            assert!(hex_pin_to_b64(&colons).is_ok());
            assert!(hex_pin_to_b64("abcd").is_err());
            assert!(hex_pin_to_b64(&"zz".repeat(32)).is_err());
        }
    }

    #[test]
    fn an_agent_account_the_os_would_not_take_is_refused_before_joining() {
        let dir = std::env::temp_dir().join(format!("apz-ws-enrol-{}", std::process::id()));
        let enrol = Enrol {
            hub: "http://127.0.0.1:1".into(),
            code: "JOIN-1".into(),
            name: None,
            account: AccountChoice::Agent {
                user: "root".into(),
            },
            pin: None,
        };
        let err = join_here(&dir, &enrol).unwrap_err();
        assert!(err.contains("reserved"), "{err}");
        assert_eq!(state::load_record(&dir).unwrap(), None);
    }

    #[test]
    fn status_reads_an_empty_state_directory_as_not_joined() {
        let dir = std::env::temp_dir().join(format!("apz-ws-enrol-status-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let snapshot = status_here(&dir).unwrap();
        assert!(snapshot.record.is_none());
        assert!(snapshot.status.is_none());
    }
}
