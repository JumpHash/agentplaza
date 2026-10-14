use super::*;
use clap::{CommandFactory, Parser};
use std::path::PathBuf;

/// The subcommand alone, so each test reads as `agentplaza stations <args>`.
#[derive(Parser, Debug)]
struct Probe {
    #[command(subcommand)]
    cmd: Cmd,
}

fn parse(args: &[&str]) -> Result<Cmd, String> {
    Probe::try_parse_from(std::iter::once("stations").chain(args.iter().copied()))
        .map(|p| p.cmd)
        .map_err(|e| e.to_string())
}

fn parsed(args: &[&str]) -> Cmd {
    parse(args).unwrap_or_else(|e| panic!("{args:?} should parse: {e}"))
}

fn all_settings() -> SetArgs {
    SetArgs {
        takeover: Some(TakeoverArg::KeepWorking),
        record: Some(OnOff::Off),
        retention_days: Some(7),
        fps: Some(1),
        record_takeovers: Some(OnOff::On),
        work_folder: Some("/srv/work".into()),
        snapshot_reset: Some(OnOff::On),
        rate_per_hour: Some(1500),
    }
}

#[test]
fn the_command_definition_is_valid() {
    Probe::command().debug_assert();
}

#[test]
fn list_show_rename_drain_undrain_and_revoke_parse() {
    assert!(matches!(parsed(&["list"]), Cmd::List));
    let Cmd::Show { id } = parsed(&["show", "w1"]) else {
        panic!("show")
    };
    assert_eq!(id, "w1");
    let Cmd::Rename { id, name } = parsed(&["rename", "w1", "Lab box"]) else {
        panic!("rename")
    };
    assert_eq!((id.as_str(), name.as_str()), ("w1", "Lab box"));
    let Cmd::Drain { id } = parsed(&["drain", "w1"]) else {
        panic!("drain")
    };
    assert_eq!(id, "w1");
    let Cmd::Undrain { id } = parsed(&["undrain", "w1"]) else {
        panic!("undrain")
    };
    assert_eq!(id, "w1");
    let Cmd::Revoke { id, yes } = parsed(&["revoke", "w1", "--yes"]) else {
        panic!("revoke --yes")
    };
    assert_eq!((id.as_str(), yes), ("w1", true));
    let Cmd::Revoke { yes, .. } = parsed(&["revoke", "w1"]) else {
        panic!("revoke")
    };
    assert!(!yes);
}

#[test]
fn codes_parse_with_list_as_the_default() {
    assert!(matches!(parsed(&["codes"]), Cmd::Codes { action: None }));
    assert!(matches!(
        parsed(&["codes", "list"]),
        Cmd::Codes {
            action: Some(CodesCmd::List)
        }
    ));
    let Cmd::Codes {
        action: Some(CodesCmd::Create { label, ttl }),
    } = parsed(&["codes", "create", "--label", "gpu-box", "--ttl", "30"])
    else {
        panic!("create")
    };
    assert_eq!((label.as_str(), ttl), ("gpu-box", 30));
    let Cmd::Codes {
        action: Some(CodesCmd::Create { label, ttl }),
    } = parsed(&["codes", "create"])
    else {
        panic!("create defaults")
    };
    assert_eq!((label.as_str(), ttl), ("workstation", 15));
    let Cmd::Codes {
        action: Some(CodesCmd::Revoke { id }),
    } = parsed(&["codes", "revoke", "j3"])
    else {
        panic!("revoke")
    };
    assert_eq!(id, "j3");
}

#[test]
fn leases_and_release_parse() {
    let Cmd::Leases { id } = parsed(&["leases", "w1"]) else {
        panic!("leases")
    };
    assert_eq!(id, "w1");
    let Cmd::Release { id, lease } = parsed(&["release", "w1", "l4"]) else {
        panic!("release")
    };
    assert_eq!((id.as_str(), lease.as_str()), ("w1", "l4"));
}

#[test]
fn settings_show_and_set_parse_every_flag() {
    let Cmd::Settings { id, action: None } = parsed(&["settings", "w1"]) else {
        panic!("settings show")
    };
    assert_eq!(id, "w1");
    let Cmd::Settings {
        id,
        action: Some(SettingsCmd::Set(set)),
    } = parsed(&[
        "settings",
        "w1",
        "set",
        "--takeover",
        "keep-working",
        "--record",
        "off",
        "--retention-days",
        "7",
        "--fps",
        "1",
        "--record-takeovers",
        "on",
        "--work-folder",
        "/srv/work",
        "--snapshot-reset",
        "on",
        "--rate-per-hour",
        "1500",
    ])
    else {
        panic!("settings set")
    };
    assert_eq!(id, "w1");
    assert_eq!(set, all_settings());
}

#[test]
fn each_takeover_policy_has_its_flag_spelling() {
    for (word, policy) in [
        ("agent-waits", TakeoverArg::AgentWaits),
        ("release-lease", TakeoverArg::ReleaseLease),
        ("keep-working", TakeoverArg::KeepWorking),
    ] {
        let Cmd::Settings {
            action: Some(SettingsCmd::Set(set)),
            ..
        } = parsed(&["settings", "w1", "set", "--takeover", word])
        else {
            panic!("takeover {word}")
        };
        assert_eq!(set.takeover, Some(policy));
    }
    assert!(parse(&["settings", "w1", "set", "--takeover", "bogus"]).is_err());
}

#[test]
fn audit_parses_with_its_defaults_and_filters() {
    let Cmd::Audit { station, limit } = parsed(&["audit"]) else {
        panic!("audit")
    };
    assert_eq!((station, limit), (None, 100));
    let Cmd::Audit { station, limit } = parsed(&["audit", "--station", "w1", "--limit", "5"])
    else {
        panic!("audit filters")
    };
    assert_eq!((station.as_deref(), limit), (Some("w1"), 5));
}

#[test]
fn ids_are_required_where_a_command_names_one() {
    let cases: [&[&str]; 5] = [
        &["show"],
        &["rename", "w1"],
        &["release", "w1"],
        &["codes", "revoke"],
        &["settings"],
    ];
    for args in cases {
        assert!(parse(args).is_err(), "{args:?} needs its id");
    }
}

#[test]
fn flags_merge_onto_the_hubs_settings_and_leave_the_rest() {
    let current = StationSettings {
        record: true,
        retention_days: 30,
        ..StationSettings::default()
    };
    let set = SetArgs {
        takeover: Some(TakeoverArg::ReleaseLease),
        record: Some(OnOff::Off),
        fps: Some(3),
        ..SetArgs::default()
    };
    let merged = merged(current, &set);
    assert_eq!(merged.takeover, Takeover::ReleaseLease);
    assert!(!merged.record);
    assert_eq!(merged.record_fps, 3);
    assert_eq!(
        merged.retention_days, 30,
        "a flag not given keeps the station's value"
    );
    assert_eq!(merged.work_folder, "~/AgentPlaza");
    assert_eq!(
        merged.rate_per_hour, 0,
        "a flag not given keeps the station's rate"
    );
}

#[test]
fn the_rate_reads_in_credits_and_dollars() {
    assert_eq!(rate_text(0), "0 credits/h (free)");
    assert_eq!(rate_text(1500), "1500 credits/h ($1.500/h)");
    assert_eq!(rate_text(7), "7 credits/h ($0.007/h)");
    let set = SetArgs {
        rate_per_hour: Some(250),
        ..SetArgs::default()
    };
    assert!(check_set(&set).is_ok(), "a rate alone is a setting");
    let merged = merged(
        StationSettings {
            rate_per_hour: 900,
            ..StationSettings::default()
        },
        &set,
    );
    assert_eq!(merged.rate_per_hour, 250);
    assert!(
        settings_fields(&json!({ "rate_per_hour": 1500 }))
            .contains(&("rate", "1500 credits/h ($1.500/h)".to_string()))
    );
}

#[test]
fn the_recordings_subcommands_parse() {
    let Cmd::Recordings {
        action: None,
        station,
        limit,
        before,
    } = parsed(&["recordings"])
    else {
        panic!("the list")
    };
    assert_eq!((station, limit, before), (None, None, None));
    let Cmd::Recordings {
        action: None,
        station,
        limit,
        before,
    } = parsed(&[
        "recordings",
        "--station",
        "w1",
        "--limit",
        "5",
        "--before",
        "1700",
    ])
    else {
        panic!("the list's filters")
    };
    assert_eq!(
        (station.as_deref(), limit, before),
        (Some("w1"), Some(5), Some(1700))
    );
    let Cmd::Recordings {
        action: Some(recordings::Action::Show { id, frames }),
        ..
    } = parsed(&["recordings", "show", "r1"])
    else {
        panic!("show")
    };
    assert_eq!(id, "r1");
    assert!(!frames, "the frames are listed on request");
    let Cmd::Recordings {
        action: Some(recordings::Action::Show { frames, .. }),
        ..
    } = parsed(&["recordings", "show", "r1", "--frames"])
    else {
        panic!("show --frames")
    };
    assert!(frames);
    let Cmd::Recordings {
        action: Some(recordings::Action::Frames { id, out }),
        ..
    } = parsed(&["recordings", "frames", "r1", "-o", "/tmp/frames"])
    else {
        panic!("frames")
    };
    assert_eq!((id.as_str(), out), ("r1", PathBuf::from("/tmp/frames")));
    let Cmd::Recordings {
        action: Some(recordings::Action::Delete { id, yes }),
        ..
    } = parsed(&["recordings", "delete", "r1", "--yes"])
    else {
        panic!("delete")
    };
    assert_eq!((id.as_str(), yes), ("r1", true));
    let Cmd::Recordings {
        action: Some(recordings::Action::Defaults { action: None }),
        ..
    } = parsed(&["recordings", "defaults"])
    else {
        panic!("defaults show")
    };
    let Cmd::Recordings {
        action:
            Some(recordings::Action::Defaults {
                action: Some(recordings::DefaultsCmd::Set(set)),
            }),
        ..
    } = parsed(&[
        "recordings",
        "defaults",
        "set",
        "--record",
        "off",
        "--retention-days",
        "7",
        "--fps",
        "1",
        "--record-takeovers",
        "on",
    ])
    else {
        panic!("defaults set")
    };
    assert_eq!(
        set,
        recordings::DefaultsArgs {
            record: Some(OnOff::Off),
            retention_days: Some(7),
            fps: Some(1),
            record_takeovers: Some(OnOff::On),
        }
    );
    for fps in ["0", "6", "15"] {
        assert!(
            parse(&["recordings", "defaults", "set", "--fps", fps]).is_err(),
            "--fps {fps} is refused: frames are from 1 to 5 a second"
        );
    }
    for fps in ["0", "6"] {
        assert!(
            parse(&["settings", "w1", "set", "--fps", fps]).is_err(),
            "a workstation's --fps {fps} is refused too"
        );
    }
    assert!(
        parse(&["recordings", "show"]).is_err(),
        "show names a recording"
    );
    assert!(
        parse(&["recordings", "delete"]).is_err(),
        "delete names a recording"
    );
    assert!(
        parse(&["recordings", "frames", "r1"]).is_err(),
        "frames needs its folder"
    );
}

#[test]
fn the_sync_and_files_subcommands_parse() {
    assert!(matches!(
        parsed(&["sync", "w1"]),
        Cmd::Sync { action: None, .. }
    ));
    let Cmd::Sync {
        id,
        action: Some(files::SyncCmd::Resume { apply_deletions }),
    } = parsed(&["sync", "w1", "resume", "--apply-deletions"])
    else {
        panic!("sync resume")
    };
    assert_eq!((id.as_str(), apply_deletions), ("w1", true));
    let Cmd::Files {
        id,
        prefix,
        action: None,
    } = parsed(&["files", "w1", "docs/"])
    else {
        panic!("files prefix")
    };
    assert_eq!((id.as_str(), prefix.as_deref()), ("w1", Some("docs/")));
    assert!(matches!(
        parsed(&["files", "w1"]),
        Cmd::Files {
            prefix: None,
            action: None,
            ..
        }
    ));
    let Cmd::Files {
        action: Some(files::FilesCmd::Get { path, out }),
        ..
    } = parsed(&["files", "w1", "get", "a.txt", "-o", "x.txt"])
    else {
        panic!("get")
    };
    assert_eq!(
        (path.as_str(), out),
        ("a.txt", Some(PathBuf::from("x.txt")))
    );
    let Cmd::Files {
        action: Some(files::FilesCmd::Put { path, file }),
        ..
    } = parsed(&["files", "w1", "put", "a.txt", "./local.txt"])
    else {
        panic!("put")
    };
    assert_eq!(
        (path.as_str(), file),
        ("a.txt", PathBuf::from("./local.txt"))
    );
    let Cmd::Files {
        action: Some(files::FilesCmd::Rm { path, yes }),
        ..
    } = parsed(&["files", "w1", "rm", "a.txt", "--yes"])
    else {
        panic!("rm")
    };
    assert_eq!((path.as_str(), yes), ("a.txt", true));
    let Cmd::Files {
        action:
            Some(files::FilesCmd::Deliver {
                path,
                title,
                description,
                task,
            }),
        ..
    } = parsed(&[
        "files", "w1", "deliver", "a.txt", "--title", "Report", "--task", "t9",
    ])
    else {
        panic!("deliver")
    };
    assert_eq!(
        (
            path.as_str(),
            title.as_deref(),
            description,
            task.as_deref()
        ),
        ("a.txt", Some("Report"), None, Some("t9"))
    );
    assert!(parse(&["files"]).is_err(), "a station is named");
    assert!(
        parse(&["files", "w1", "put", "a.txt"]).is_err(),
        "put takes a local file too"
    );
    assert!(parse(&["sync"]).is_err(), "sync names a station");
}

#[test]
fn the_screen_subcommands_parse_with_their_defaults() {
    let Cmd::Screenshot {
        id,
        session,
        scale,
        out,
    } = parsed(&["screenshot", "w1", "-o", "shot.png"])
    else {
        panic!("screenshot")
    };
    assert_eq!(
        (id.as_str(), session.as_str(), scale, out),
        ("w1", "shared", 1.0, PathBuf::from("shot.png"))
    );
    let Cmd::Screenshot { session, scale, .. } = parsed(&[
        "screenshot",
        "w1",
        "--session",
        "7",
        "--scale",
        "0.5",
        "-o",
        "s.jpg",
    ]) else {
        panic!("screenshot flags")
    };
    assert_eq!((session.as_str(), scale), ("7", 0.5));
    assert!(
        parse(&["screenshot", "w1"]).is_err(),
        "screenshot needs its file"
    );
    let Cmd::Watch {
        id,
        session,
        fps,
        save,
    } = parsed(&["watch", "w1"])
    else {
        panic!("watch")
    };
    assert_eq!(
        (id.as_str(), session.as_str(), fps, save),
        ("w1", "shared", 5, None)
    );
    let Cmd::Watch {
        session, fps, save, ..
    } = parsed(&[
        "watch",
        "w1",
        "--session",
        "3",
        "--fps",
        "2",
        "--save",
        "/tmp/frames",
    ])
    else {
        panic!("watch flags")
    };
    assert_eq!(
        (session.as_str(), fps, save),
        ("3", 2, Some(PathBuf::from("/tmp/frames")))
    );
    assert!(parse(&["watch"]).is_err(), "watch names a station");
}

#[test]
fn the_settings_rate_parses_as_credits() {
    let Cmd::Settings {
        action: Some(SettingsCmd::Set(set)),
        ..
    } = parsed(&["settings", "w1", "set", "--rate-per-hour", "0"])
    else {
        panic!("rate 0")
    };
    assert_eq!(set.rate_per_hour, Some(0), "0 is free, and it is a setting");
    assert!(parse(&["settings", "w1", "set", "--rate-per-hour", "-5"]).is_err());
}

#[test]
fn the_merged_settings_encode_as_the_hub_reads_them() {
    let body = serde_json::to_value(merged(StationSettings::default(), &all_settings())).unwrap();
    assert_eq!(
        body,
        json!({
            "takeover": "keep_working_pointer_paused",
            "record": false,
            "retention_days": 7,
            "record_fps": 1,
            "record_takeovers": true,
            "work_folder": "/srv/work",
            "snapshot_reset": true,
            "rate_per_hour": 1500
        })
    );
}

#[test]
fn a_set_with_no_flag_or_an_empty_folder_is_refused_before_the_hub() {
    assert!(check_set(&SetArgs::default()).is_err());
    let blank = SetArgs {
        work_folder: Some("  ".into()),
        ..SetArgs::default()
    };
    assert_eq!(check_set(&blank).unwrap_err(), "the work folder is empty");
    assert!(check_set(&all_settings()).is_ok());
}

#[test]
fn a_station_row_shows_its_account_and_lease_count() {
    let station = json!({
        "id": "w1", "name": "lab", "status": "online", "last_seen_ms": 0,
        "facts": { "os": "linux", "account": { "mode": "owner", "name": "irfan" } },
        "leases": [{ "id": "l1" }, { "id": "l2" }]
    });
    assert_eq!(
        row(&station),
        [
            "w1",
            "lab",
            "online",
            "linux",
            "irfan (owner)",
            "2",
            "never"
        ]
    );
    assert_eq!(
        account_warning(&station).as_deref(),
        Some("warning: agents act as irfan (the owner's own account)")
    );
    let agent = json!({ "facts": { "os": "macos", "account": { "mode": "agent_user", "name": "agentws" } } });
    assert_eq!(account(&agent), "agentws");
    assert_eq!(account_warning(&agent), None);
    assert_eq!(account(&json!({})), "-");
}

#[test]
fn lease_and_code_rows_keep_the_hubs_words() {
    let lease = json!({ "id": "l4", "agent": 12, "kind": "pixel", "state": "queued", "position": 2, "task": null, "started_ms": 0 });
    assert_eq!(lease_row(&lease)[..5], ["l4", "12", "pixel", "queued", "2"]);
    assert_eq!(lease_row(&lease)[5], "-");
    let active = json!({ "id": "l5", "agent": 3, "kind": "shared", "state": "active" });
    assert_eq!(
        lease_row(&active)[4],
        "-",
        "an active lease has no queue position"
    );
    let code = json!({ "id": "j1", "label": "gpu", "state": "used", "created_ms": 0, "expires_ms": 0, "used_by": "lab-1" });
    assert_eq!(code_row(&code)[2], "used");
    assert_eq!(code_row(&code)[5], "lab-1");
}

#[test]
fn audit_rows_put_the_extra_fields_in_the_detail() {
    let row = json!({ "at_ms": 0, "actor": "owner", "station": "w1", "agent": null, "kind": "lease_granted", "lease": "l2", "session": 9 });
    let cells = audit_row(&row);
    assert_eq!(cells[1], "owner");
    assert_eq!(cells[3], "-");
    assert_eq!(cells[4], "lease_granted");
    assert_eq!(cells[5], "lease=l2 session=9");
}

#[test]
fn settings_read_the_takeover_in_its_flag_spelling() {
    let fields = settings_fields(
        &json!({ "takeover": "release_lease", "record": true, "retention_days": 30, "record_fps": 2 }),
    );
    assert_eq!(fields[0], ("takeover", "release-lease".to_string()));
    assert_eq!(fields[1], ("record", "on".to_string()));
    assert_eq!(fields[2], ("retention", "30 days".to_string()));
    assert_eq!(takeover_word("keep_working_pointer_paused"), "keep-working");
}

#[test]
fn take_and_input_parse_with_the_shared_session_by_default() {
    let Cmd::Take { id, session } = parsed(&["take", "w1"]) else {
        panic!("take")
    };
    assert_eq!((id.as_str(), session.as_str()), ("w1", "shared"));
    let Cmd::Take { session, .. } = parsed(&["take", "w1", "--session", "7"]) else {
        panic!("take --session")
    };
    assert_eq!(session, "7");
}

#[test]
fn input_takes_its_command_words_including_negative_numbers() {
    let Cmd::Input { id, session, command } = parsed(&["input", "w1", "scroll", "0", "-3", "10", "20"]) else {
        panic!("input scroll")
    };
    assert_eq!((id.as_str(), session.as_str()), ("w1", "shared"));
    assert_eq!(command, ["scroll", "0", "-3", "10", "20"]);
    let Cmd::Input { command, .. } = parsed(&["input", "w1", "--session", "3", "type", "hello world"]) else {
        panic!("input type")
    };
    assert_eq!(command, ["type", "hello world"]);
    assert!(parse(&["input", "w1"]).is_err(), "a command is required");
}

#[test]
fn vms_parse_with_the_list_as_the_default() {
    assert!(matches!(parsed(&["vms"]), Cmd::Vms { action: None }));
    assert!(matches!(
        parsed(&["vms", "list"]),
        Cmd::Vms {
            action: Some(vms::VmsCmd::List)
        }
    ));
    let Cmd::Vms {
        action: Some(vms::VmsCmd::Delete { id, yes }),
    } = parsed(&["vms", "delete", "v3", "--yes"])
    else {
        panic!("vms delete")
    };
    assert_eq!((id.as_str(), yes), ("v3", true));
}

#[test]
fn vms_create_takes_the_name_and_its_sizes_and_defaults_the_os_and_backend() {
    let Cmd::Vms {
        action: Some(vms::VmsCmd::Create(args)),
    } = parsed(&["vms", "create", "desk-1", "--node", "n1", "--cpus", "4", "--ram-mb", "8192", "--disk-gb", "40"])
    else {
        panic!("vms create")
    };
    assert_eq!(args.name, "desk-1");
    assert_eq!(args.node, "n1");
    assert_eq!(args.os, vms::OsArg::Linux);
    assert_eq!(args.backend, vms::BackendArg::Auto);
    assert_eq!((args.cpus, args.ram_mb, args.disk_gb), (4, 8192, 40));
    assert_eq!((args.gpu, args.image), (None, None));

    let Cmd::Vms {
        action: Some(vms::VmsCmd::Create(args)),
    } = parsed(&[
        "vms", "create", "win-1", "--node", "n2", "--os", "windows", "--cpus", "4", "--ram-mb", "8192", "--disk-gb", "80",
        "--image", "/srv/win.img", "--gpu", "vfio:0000:01:00.0", "--backend", "qemu",
    ])
    else {
        panic!("vms create windows")
    };
    assert_eq!(args.os, vms::OsArg::Windows);
    assert_eq!(args.backend, vms::BackendArg::Qemu);
    assert_eq!(args.image.as_deref(), Some("/srv/win.img"));
    assert_eq!(args.gpu.as_deref(), Some("vfio:0000:01:00.0"));
    assert!(parse(&["vms", "create", "x", "--node", "n1", "--cpus", "2", "--ram-mb", "4096"]).is_err(), "disk is required");
    assert!(parse(&["vms", "create", "x", "--node", "n1", "--cpus", "2", "--ram-mb", "4096", "--disk-gb", "8", "--os", "beos"]).is_err());
}
