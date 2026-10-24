//! The hub-side settings of one workstation, pushed to the daemon with `Op::Settings`.

use serde::{Deserialize, Serialize};

/// What happens when the owner takes over a pixel session (plan section 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "store", derive(apz_store::Column))]
pub enum Takeover {
    /// The agent waits and resumes when the owner gives the session back.
    #[default]
    AgentWaits,
    /// The agent's lease is released and its task goes back to the queue.
    ReleaseLease,
    /// The agent keeps working through shells, runs, files and the UI tree; only its pointer pauses.
    KeepWorkingPointerPaused,
}

/// Per-workstation settings. Missing fields take the defaults, so a partial object is valid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[cfg_attr(feature = "store", derive(apz_store::Table))]
pub struct StationSettings {
    pub takeover: Takeover,
    /// Whether screen sessions are recorded.
    pub record: bool,
    /// How many days recordings are kept.
    pub retention_days: u32,
    /// Frames per second a recording keeps.
    pub record_fps: u8,
    /// Whether the owner's takeovers are recorded too.
    pub record_takeovers: bool,
    /// The work folder on the machine, under the agent's OS account, synced with the hub both ways.
    pub work_folder: String,
    /// For VMs: restore a clean snapshot after each lease.
    pub snapshot_reset: bool,
}

impl Default for StationSettings {
    fn default() -> Self {
        Self {
            takeover: Takeover::AgentWaits,
            record: true,
            retention_days: 30,
            record_fps: 2,
            record_takeovers: true,
            work_folder: "~/AgentPlaza".into(),
            snapshot_reset: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert_wire;

    #[test]
    fn takeover_policies_are_snake_case() {
        for (policy, json) in [
            (Takeover::AgentWaits, "agent_waits"),
            (Takeover::ReleaseLease, "release_lease"),
            (
                Takeover::KeepWorkingPointerPaused,
                "keep_working_pointer_paused",
            ),
        ] {
            assert_wire(&policy, &format!(r#""{json}""#));
        }
    }

    #[test]
    fn settings_encode_every_field() {
        assert_wire(
            &StationSettings::default(),
            r#"{"takeover":"agent_waits","record":true,"retention_days":30,"record_fps":2,"record_takeovers":true,"work_folder":"~/AgentPlaza","snapshot_reset":false}"#,
        );
        assert_wire(
            &StationSettings {
                takeover: Takeover::ReleaseLease,
                record: false,
                retention_days: 7,
                record_fps: 1,
                record_takeovers: false,
                work_folder: "/srv/work".into(),
                snapshot_reset: true,
            },
            r#"{"takeover":"release_lease","record":false,"retention_days":7,"record_fps":1,"record_takeovers":false,"work_folder":"/srv/work","snapshot_reset":true}"#,
        );
    }

    #[test]
    fn station_settings_decode_missing_fields_as_defaults() {
        // A partial object takes the defaults for the rest (was a_partial_object_takes_the_defaults_for_the_rest).
        {
            let decoded: StationSettings =
                serde_json::from_str(r#"{"record":false}"#).expect("a partial object decodes");
            assert_eq!(
                decoded,
                StationSettings {
                    record: false,
                    ..StationSettings::default()
                }
            );
        }
    }
}
