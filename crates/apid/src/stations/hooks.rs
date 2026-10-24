//! The hooks the workstation actions and the leases' ends pass through.
//!
//! - `after_agent_op`, after every operation an agent or the owner sends to a station (`act::agent_op`): the action
//!   goes into its lease's recording (recordings.rs), refused or not.
//! - `lease_ended`, when a lease ends (released, its session ended, its station revoked): the station's VM is reverted
//!   if it is a snapshot_reset VM (vms.rs).
//!
//! A workstation lease is not charged to its task.

use apz_station::Op;
use serde_json::{Value, json};

use super::act;
use super::ops::Failure;
use super::recordings;
use crate::state::App;

/// Called after every operation an agent or the owner sends to a station, with its station, its lease (if any), the
/// operation and its outcome, whether it ran or was refused.
pub fn after_agent_op(
    app: &App,
    station: &str,
    lease: Option<&str>,
    op: &Op,
    result: &Result<Value, Failure>,
) {
    recordings::on_action(app, station, lease, &action_summary(op), result.is_ok());
}

/// The operation as its lease's recording keeps it: the audit's summary, with the workstation tool's name as its
/// `kind` (`click`, `screenshot`, `shell`, ...; `action` for an operation no tool names).
pub(super) fn action_summary(op: &Op) -> Value {
    let mut summary = act::summary(op);
    let kind = act::law_name(op).map_or("action", |name| name.trim_start_matches("station_"));
    if let Value::Object(map) = &mut summary {
        map.insert("kind".into(), json!(kind));
    }
    summary
}

/// Called when a lease ends on `station`.
pub fn lease_ended(app: &App, station: &str) {
    // A workstation VM with snapshot_reset is reverted once its last active lease has ended (vms.rs).
    super::vms::after_lease(app, station);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use apz_station::{Button, ImageFormat, InputAction, StationSettings};

    use super::*;
    use crate::stations::leases::{Kind, Lease, LeaseView, State};
    use crate::stations::recordings::Recordings;

    #[test]
    fn an_action_is_recorded_under_its_tool_name() {
        let click = Op::Input {
            session: 9,
            action: InputAction::Click {
                x: 1,
                y: 2,
                button: Button::Left,
                count: 1,
            },
        };
        assert_eq!(action_summary(&click)["kind"], "click");
        let shot = Op::Screenshot {
            session: 9,
            scale: 0.5,
            grid: false,
            format: ImageFormat::Png,
        };
        assert_eq!(action_summary(&shot)["kind"], "screenshot");
        assert_eq!(
            action_summary(&Op::Facts)["kind"],
            "action",
            "an operation no tool names"
        );
    }

    #[test]
    fn an_action_lands_in_its_leases_recording_with_its_outcome() {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir: PathBuf = std::env::temp_dir().join(format!(
            "apid-hooks-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let store = crate::store::open(&dir).expect("the store opens");
        let db = store.db(crate::store::PLAZA);
        let recordings = Recordings::default();
        let lease = LeaseView {
            lease: Lease {
                id: "l1".into(),
                station: "w1".into(),
                agent: 7,
                task: Some("t1".into()),
                kind: Kind::Pixel,
                session: Some(9),
                started_ms: 1000,
                state: State::Active,
                paused: false,
                active_ms: Some(1000),
            },
            position: None,
        };
        let id = recordings
            .begin(db, &StationSettings::default(), "w1", &lease, 1000)
            .unwrap()
            .expect("the station records");
        let op = Op::Input {
            session: 9,
            action: InputAction::Click {
                x: 1,
                y: 2,
                button: Button::Left,
                count: 1,
            },
        };
        assert!(
            recordings
                .action(db, "w1", "l1", &action_summary(&op), false, 1100)
                .unwrap()
        );
        let events = crate::stations::recordings::events_of(db, &id, 10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            (events[0].kind.as_str(), events[0].ok),
            ("click", false),
            "a refused click is kept as refused"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
