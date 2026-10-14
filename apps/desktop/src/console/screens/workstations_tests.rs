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
fn status_chip_names_each_state_and_keeps_an_unknown_one() {
    assert_eq!(status_chip("online"), ("online".to_string(), ui::Tone::Ok));
    assert_eq!(
        status_chip("offline"),
        ("offline".to_string(), ui::Tone::Muted)
    );
    assert_eq!(
        status_chip("draining"),
        ("draining".to_string(), ui::Tone::Warn)
    );
    assert_eq!(
        status_chip("revoked"),
        ("revoked".to_string(), ui::Tone::Danger)
    );
    assert_eq!(
        status_chip("mystery"),
        ("mystery".to_string(), ui::Tone::Muted)
    );
}

#[test]
fn account_badge_flags_only_the_owner_account() {
    let owner = Account {
        mode: "owner".to_string(),
        name: "irfan".to_string(),
    };
    let agent = Account {
        mode: "agent_user".to_string(),
        name: "apz-agent".to_string(),
    };
    assert_eq!(
        account_badge(&owner).as_deref(),
        Some("agents act as irfan")
    );
    assert_eq!(account_badge(&agent), None);
    assert!(account_text(&owner).contains("agents act as the owner"));
    assert!(account_text(&agent).contains("a separate agent account (apz-agent)"));
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
fn the_form_round_trips_through_the_settings() {
    let form = sample_form();
    assert_eq!(form.retention_days, "30");
    assert_eq!(form.record_fps, "5");
    assert_eq!(settings_of(&form), Ok(sample_settings()));
}

#[test]
fn settings_of_trims_the_folder_and_checks_each_bound() {
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

#[test]
fn the_hourly_rate_is_whole_credits_from_free_to_the_bound() {
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

#[test]
fn settings_of_refuses_an_empty_long_or_control_folder() {
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

#[test]
fn name_problem_checks_empty_length_and_control_characters() {
    assert_eq!(
        name_problem("  "),
        Some("Give the workstation a name.".to_string())
    );
    assert_eq!(
        name_problem(&"n".repeat(MAX_NAME_CHARS + 1)),
        Some(format!("The name is up to {MAX_NAME_CHARS} characters."))
    );
    assert_eq!(
        name_problem("studio\tmac"),
        Some("The name has no control characters.".to_string())
    );
    assert_eq!(name_problem("  studio-mac  "), None);
}

#[test]
fn join_command_and_countdown_format() {
    assert_eq!(
        join_command("apzw1.abc"),
        "agentplaza workstation join apzw1.abc"
    );
    assert_eq!(countdown(0), "00:00");
    assert_eq!(countdown(-5_000), "00:00");
    assert_eq!(countdown(65_999), "01:05");
    assert_eq!(countdown(30 * 60 * 1000), "30:00");
}

#[test]
fn summary_reads_os_form_memory_and_gpus() {
    let facts = Facts {
        os: "linux".to_string(),
        os_version: "Arch 2026".to_string(),
        form: "desktop".to_string(),
        ram_mb: 65_536,
        gpus: vec![Gpu {
            name: "RTX 4090".to_string(),
            vram_mb: 24_576,
        }],
        ..Facts::default()
    };
    assert_eq!(
        station_summary(&facts),
        "Linux Arch 2026 · Desktop · 64 GB RAM · RTX 4090"
    );
    let bare = Facts {
        os: "windows".to_string(),
        ..Facts::default()
    };
    assert_eq!(station_summary(&bare), "Windows · no GPU");
}

#[test]
fn gpu_line_shows_vram_or_shared_memory() {
    assert_eq!(
        gpu_line(&Gpu {
            name: "RTX 4090".to_string(),
            vram_mb: 24_576
        }),
        "RTX 4090 · 24 GB"
    );
    assert_eq!(
        gpu_line(&Gpu {
            name: "Apple M3".to_string(),
            vram_mb: 0
        }),
        "Apple M3 · shared memory"
    );
    assert_eq!(
        gpu_line(&Gpu {
            name: "Old card".to_string(),
            vram_mb: 512
        }),
        "Old card · 512 MB"
    );
    assert_eq!(
        displays_text(&[Display { w: 2560, h: 1440 }, Display { w: 1920, h: 1080 }]),
        "2560×1440, 1920×1080"
    );
    assert_eq!(displays_text(&[]), "none");
}

#[test]
fn lease_chip_names_the_place_in_the_queue() {
    let queued = Lease {
        state: "queued".to_string(),
        position: Some(2),
        ..Lease::default()
    };
    assert_eq!(
        lease_chip(&queued),
        ("queued #2".to_string(), ui::Tone::Warn)
    );
    let waiting = Lease {
        state: "queued".to_string(),
        ..Lease::default()
    };
    assert_eq!(lease_chip(&waiting), ("queued".to_string(), ui::Tone::Warn));
    let opening = Lease {
        state: "opening".to_string(),
        ..Lease::default()
    };
    assert_eq!(
        lease_chip(&opening),
        ("opening".to_string(), ui::Tone::Info)
    );
    let active = Lease {
        state: "active".to_string(),
        ..Lease::default()
    };
    assert_eq!(lease_chip(&active), ("active".to_string(), ui::Tone::Ok));
    assert_eq!(lease_counts(&[queued, active, waiting]), (1, 2));
    assert_eq!(lease_kind_label("pixel"), "Pixel session");
    assert_eq!(lease_kind_label("shared"), "Shared");
}

#[test]
fn code_chip_and_audit_labels_name_the_states_and_events() {
    assert_eq!(code_chip("active"), ("Active".to_string(), ui::Tone::Ok));
    assert_eq!(code_chip("used"), ("Used".to_string(), ui::Tone::Info));
    assert_eq!(
        code_chip("expired"),
        ("Expired".to_string(), ui::Tone::Muted)
    );
    assert_eq!(
        code_chip("revoked"),
        ("Revoked".to_string(), ui::Tone::Danger)
    );
    assert_eq!(audit_label("join_refused"), "Join refused");
    assert_eq!(audit_label("lease_released"), "Lease released");
    assert_eq!(audit_label("some_new_event"), "some new event");
}

#[test]
fn takeover_text_is_the_policy_explanation() {
    assert!(takeover_text(Takeover::ReleaseLease).contains("goes back to the queue"));
    assert!(takeover_text(Takeover::AgentWaits).contains("resumes when you give the session back"));
    assert_eq!(TAKEOVERS.len(), 3);
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

#[test]
fn a_failed_refresh_keeps_what_the_list_shows() {
    let mut slot: Load<Vec<Code>> = Load::Pending;
    settle_into(&mut slot, Err("down".to_string()));
    assert!(matches!(slot, Load::Failed));
    settle_into(&mut slot, Ok(vec![Code::default()]));
    settle_into(&mut slot, Err("down".to_string()));
    assert!(matches!(&slot, Load::Ready(list) if list.len() == 1));
}
