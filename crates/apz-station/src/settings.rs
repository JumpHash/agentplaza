//! The hub-side settings of one workstation, pushed to the daemon with `Op::Settings`.

use serde::{Deserialize, Serialize};

/// What happens when the owner takes over a pixel session (plan section 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    /// What an hour of a lease on this workstation costs the lease's task, in plaza credits (1 credit is $0.001).
    /// The hub charges the lease's wall time, from its activation to its release, rounded up to a whole credit, to
    /// the task's escrow. 0 (the default) makes the workstation free.
    pub rate_per_hour: u64,
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
            rate_per_hour: 0,
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
            r#"{"takeover":"agent_waits","record":true,"retention_days":30,"record_fps":2,"record_takeovers":true,"work_folder":"~/AgentPlaza","snapshot_reset":false,"rate_per_hour":0}"#,
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
                rate_per_hour: 120,
            },
            r#"{"takeover":"release_lease","record":false,"retention_days":7,"record_fps":1,"record_takeovers":false,"work_folder":"/srv/work","snapshot_reset":true,"rate_per_hour":120}"#,
        );
    }

    #[test]
    fn station_settings_decode_missing_fields_as_defaults() {
        // A station without a rate is free, and an object from before rates decodes (was a_station_without_a_rate_is_free_and_an_object_from_before_rates_decodes).
        {
            let old: StationSettings =
                serde_json::from_str(r#"{"record":true,"snapshot_reset":false}"#)
                    .expect("an object from before rates");
            assert_eq!(old.rate_per_hour, 0);
            assert_eq!(StationSettings::default().rate_per_hour, 0);
        }
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
