use super::*;
use serde_json::json;

fn sample_settings() -> Settings {
    Settings {
        takeover: Takeover::KeepWorkingPointerPaused,
        record: true,
        retention_days: 30,
        record_fps: 5,
        record_takeovers: false,
        work_folder: "~/AgentPlaza".to_string(),
        snapshot_reset: true,
        rate_per_hour: 120,
    }
}

fn sample_form() -> SettingsForm {
    SettingsForm::from(&sample_settings())
}

#[test]
fn settings_serialize_with_the_hub_keys_and_policy_names() {
    let value = serde_json::to_value(sample_settings()).expect("settings serialize");
    assert_eq!(
        value,
        json!({
            "takeover": "keep_working_pointer_paused",
            "record": true,
            "retention_days": 30,
            "record_fps": 5,
            "record_takeovers": false,
            "work_folder": "~/AgentPlaza",
            "snapshot_reset": true,
            "rate_per_hour": 120,
        })
    );
    assert_eq!(
        serde_json::to_value(Takeover::AgentWaits).unwrap(),
        json!("agent_waits")
    );
    assert_eq!(
        serde_json::to_value(Takeover::ReleaseLease).unwrap(),
        json!("release_lease")
    );
}

#[test]
fn settings_of_checks_the_work_folder_and_the_bounds() {
    // settings_of_trims_the_folder_and_checks_each_bound
    {
        let mut form = sample_form();
        form.work_folder = "  ~/Work  ".to_string();
        form.retention_days = " 1 ".to_string();
        form.record_fps = "5".to_string();
        let settings = settings_of(&form).expect("in bounds");
        assert_eq!(settings.work_folder, "~/Work");
        assert_eq!((settings.retention_days, settings.record_fps), (1, 5));

        form.retention_days = "3650".to_string();
        assert!(settings_of(&form).is_ok());
        for bad in ["0", "3651", "abc", "", "-3", "2.5", "+4"] {
            form.retention_days = bad.to_string();
            assert!(
                settings_of(&form)
                    .unwrap_err()
                    .starts_with("Keep recordings for whole days"),
                "accepted {bad:?}"
            );
        }
        form.retention_days = "30".to_string();
        for bad in ["0", "6", "31", "x", ""] {
            form.record_fps = bad.to_string();
            assert!(
                settings_of(&form)
                    .unwrap_err()
                    .starts_with("Record whole frames"),
                "accepted fps {bad:?}"
            );
        }
        form.record_fps = "1".to_string();
        assert!(
            settings_of(&form).is_ok(),
            "one frame a second is the least"
        );
    }
    // the_hourly_rate_is_whole_credits_from_free_to_the_bound
    {
        let mut form = sample_form();
        form.rate_per_hour = "0".to_string();
        assert_eq!(settings_of(&form).map(|s| s.rate_per_hour), Ok(0));
        form.rate_per_hour = RATE_MAX.to_string();
        assert!(settings_of(&form).is_ok());
        for bad in ["-1", "1.5", "", "1000001"] {
            form.rate_per_hour = bad.to_string();
            assert!(settings_of(&form).is_err(), "accepted rate {bad:?}");
        }
        assert_eq!(rate_text(0), "Free: a lease here costs its task nothing.");
        assert!(rate_text(120).starts_with("120 credits an hour: $0.120"));
    }
    // settings_of_refuses_an_empty_long_or_control_folder
    {
        let mut form = sample_form();
        form.work_folder = "   ".to_string();
        assert_eq!(
            settings_of(&form),
            Err("The work folder cannot be empty.".to_string())
        );
        form.work_folder = "a".repeat(WORK_FOLDER_MAX + 1);
        assert!(
            settings_of(&form)
                .unwrap_err()
                .starts_with("The work folder is a path")
        );
        form.work_folder = "a".repeat(WORK_FOLDER_MAX);
        assert!(settings_of(&form).is_ok());
        form.work_folder = "~/A\u{7}".to_string();
        assert!(
            settings_of(&form)
                .unwrap_err()
                .starts_with("The work folder is a path")
        );
    }
}

#[test]
fn station_json_parses_a_hub_reply_and_tolerates_a_partial_one() {
    let full = r#"{
        "id": "st_1", "name": "studio-mac", "status": "draining", "online": true, "draining": true, "revoked": false,
        "version": "0.0.9", "code": "studio-mac", "joined_ms": 1700000000000, "last_seen_ms": 1700000100000,
        "address": "10.0.0.7",
        "facts": {"os": "macos", "os_version": "15.1", "arch": "arm64", "hostname": "studio", "form": "mini",
                  "cpus": 10, "ram_mb": 32768, "gpus": [{"vendor": "apple", "name": "M3 Max", "vram_mb": 0}],
                  "displays": [{"id": 1, "w": 3024, "h": 1964, "scale": 2.0}],
                  "account": {"mode": "owner", "name": "irfan"}, "apps": 42, "virtual_displays": true, "ui_tree": true},
        "settings": {"takeover": "release_lease", "record": false, "retention_days": 7, "record_fps": 2,
                     "record_takeovers": true, "work_folder": "~/AgentPlaza", "snapshot_reset": false},
        "leases": [{"id": "l1", "station": "st_1", "agent": 3, "task": "render", "kind": "pixel", "session": 9,
                    "started_ms": 1700000050000, "state": "queued", "position": 1}]
    }"#;
    let s: Station = serde_json::from_str(full).expect("station parses");
    assert_eq!(s.status, "draining");
    assert!(s.draining && !s.revoked);
    assert_eq!(s.facts.gpus[0].name, "M3 Max");
    assert_eq!(s.facts.displays[0].h, 1964);
    assert_eq!(
        account_badge(&s.facts.account).as_deref(),
        Some("agents act as irfan")
    );
    assert_eq!(s.settings.takeover, Takeover::ReleaseLease);
    assert_eq!(s.leases[0].position, Some(1));
    assert_eq!(s.leases[0].task.as_deref(), Some("render"));

    let partial: Station =
        serde_json::from_str(r#"{"id": "st_2", "name": "bare"}"#).expect("partial station parses");
    assert_eq!(partial.name, "bare");
    assert!(partial.leases.is_empty() && partial.facts.gpus.is_empty());
    assert_eq!(partial.settings, Settings::default());
}

#[test]
fn a_settings_answer_parses_with_and_without_a_push_error() {
    let pushed: SettingsAnswer = serde_json::from_value(
        json!({"settings": sample_settings(), "pushed": true, "push_error": null}),
    )
    .expect("answer parses");
    assert!(pushed.pushed && pushed.push_error.is_none());
    let held: SettingsAnswer = serde_json::from_value(
        json!({"settings": sample_settings(), "pushed": false, "push_error": "offline"}),
    )
    .expect("answer parses");
    assert_eq!(held.push_error.as_deref(), Some("offline"));
}

#[test]
fn station_events_are_the_station_and_lease_frames() {
    assert!(station_event("station"));
    assert!(station_event("lease_granted"));
    assert!(station_event("lease_released"));
    assert!(station_event("lease_owner_input"));
    assert!(station_event("station_takeover"));
    assert!(station_event("station_give_back"));
    assert!(station_event("station_sync_conflict"));
    assert!(station_event("station_sync_paused"));
    assert!(
        !station_event("station_recording"),
        "a frame the page does not show is not a reload"
    );
    assert!(!station_event("agent"));
    assert!(!station_event("stationary"));
}
