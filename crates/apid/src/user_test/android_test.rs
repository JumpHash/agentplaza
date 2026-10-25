//! The Android target on a scenario, through the loop (PLAN 19.3, 8.5), against a fake adb: a script on a directory of its own,
//! on the PATH that the runner is given (never the real adb, and no device). The fake answers `uiautomator dump` from XML
//! fixtures, moves to the screen a tap's point leads to (`links`), starts on the cart when the app is launched, and records
//! every line it is sent. The planner and the driver are the macOS tests' fakes (`desktop_targets_test`).

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use apz_android::adb::Local;
use serde_json::{Value, json};

use super::Ruling;
use super::android::{Device, NodeAdb, elements_of, finish_on, prepare_on};
use super::desk::Calls;
use super::desktop_targets_test::run;
use super::run::Failure;
use super::target::{Action, Target};
use crate::recipes::model::AndroidTarget;

const CART: &str = r#"<?xml version='1.0' encoding='UTF-8' standalone='yes' ?>
<hierarchy index="0" class="hierarchy" rotation="0"><node index="0" text="" resource-id="" class="android.widget.FrameLayout" package="com.shop" content-desc="" clickable="false" enabled="true" bounds="[0,0][1080,2400]"><node index="0" text="Cart" resource-id="com.shop:id/title" class="android.widget.TextView" package="com.shop" content-desc="" clickable="false" enabled="true" bounds="[40,100][1040,200]" /><node index="1" text="Pay" resource-id="com.shop:id/pay" class="android.widget.Button" package="com.shop" content-desc="" clickable="true" enabled="true" bounds="[40,1600][1040,1760]" /></node></hierarchy>"#;

const RECEIPT: &str = r#"<?xml version='1.0' encoding='UTF-8' standalone='yes' ?>
<hierarchy index="0" class="hierarchy" rotation="0"><node index="0" text="" resource-id="" class="android.widget.FrameLayout" package="com.shop" content-desc="" clickable="false" enabled="true" bounds="[0,0][1080,2400]"><node index="0" text="Receipt" resource-id="com.shop:id/title" class="android.widget.TextView" package="com.shop" content-desc="" clickable="false" enabled="true" bounds="[40,100][1040,200]" /><node index="1" text="Back to shop" resource-id="com.shop:id/home" class="android.widget.Button" package="com.shop" content-desc="" clickable="true" enabled="true" bounds="[40,1600][1040,1760]" /></node></hierarchy>"#;

const ERROR: &str = r#"<?xml version='1.0' encoding='UTF-8' standalone='yes' ?>
<hierarchy index="0" class="hierarchy" rotation="0"><node index="0" text="" resource-id="" class="android.widget.FrameLayout" package="com.shop" content-desc="" clickable="false" enabled="true" bounds="[0,0][1080,2400]"><node index="0" text="Payment failed" resource-id="com.shop:id/title" class="android.widget.TextView" package="com.shop" content-desc="" clickable="false" enabled="true" bounds="[40,100][1040,200]" /><node index="1" text="Try again" resource-id="com.shop:id/retry" class="android.widget.Button" package="com.shop" content-desc="" clickable="true" enabled="true" bounds="[40,1600][1040,1760]" /></node></hierarchy>"#;

/// A fake adb in `dir`. The screen it is on is `state` (its fixture is `<screen>.xml`); a tap at a point moves it along the
/// lines of `links` (`screen x y next`); launching or force-stopping the app puts it on the cart; every call is in `log`.
fn fake_adb(dir: &Path, links: &str) -> PathBuf {
    std::fs::write(dir.join("cart.xml"), CART).unwrap();
    std::fs::write(dir.join("receipt.xml"), RECEIPT).unwrap();
    std::fs::write(dir.join("error.xml"), ERROR).unwrap();
    std::fs::write(dir.join("links"), links).unwrap();
    std::fs::write(dir.join("state"), "cart").unwrap();
    std::fs::create_dir_all(dir.join("device")).unwrap();
    let script = format!(
        r#"#!/bin/sh
D='{dir}'
printf '%s\n' "$*" >> "$D/log"
eval "last=\${{$#}}"
case "$*" in
  *" shell uiautomator dump"*)
    echo 'UI hierchary dumped to: /dev/tty'
    cat "$D/$(cat "$D/state").xml" ;;
  *" shell screencap "*)
    printf 'PNG-FIXTURE' > "$D/device/$(basename "$last")" ;;
  *" pull "*)
    cp "$D/device/$(basename "$4")" "$5" ;;
  *" shell rm -f "*)
    rm -f "$D/device/$(basename "$last")" ;;
  *" shell input tap "*)
    cur=$(cat "$D/state")
    next=$(awk -v s="$cur" -v x="$6" -v y="$7" '$1==s && $2==x && $3==y {{print $4}}' "$D/links")
    [ -n "$next" ] && echo "$next" > "$D/state" ;;
  *" shell am start "*|*" shell am force-stop "*)
    echo cart > "$D/state" ;;
  *" shell screenrecord "*)
    printf 'MP4' > "$D/device/$(basename "$last")"
    while [ ! -f "$D/device/stop" ]; do sleep 0.05; done ;;
  *" shell pkill "*)
    touch "$D/device/stop" ;;
  *" install "*|*" input text "*|*" input keyevent "*) ;;
  *) echo "the fake adb does not answer: $*" >&2; exit 1 ;;
esac
exit 0
"#,
        dir = dir.display()
    );
    let program = dir.join("adb");
    std::fs::write(&program, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    program
}

fn fixture(links: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fake_adb(dir.path(), links);
    dir
}

/// The target on the fake adb in `dir`: the app started fresh on the device, as the step's opener does.
fn open(dir: &Path) -> Result<Box<dyn Target>, String> {
    let program = Local::in_dirs(&[dir.to_path_buf()])?;
    let target = Device::open(
        Box::new(program),
        "emulator-5554",
        dir,
        "com.shop/.MainActivity",
    )?;
    Ok(Box::new(target))
}

fn log_of(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("log")).unwrap_or_default()
}

#[test]
fn an_android_scenario_fails_on_the_seeded_bug_and_passes_the_fixed_build() {
    let buggy = fixture("cart 540 1680 error\n");
    let bug = run(&mut || open(buggy.path()));
    assert_eq!(bug.target, "android");
    assert_eq!(
        bug.found.len(),
        1,
        "the seeded bug is a finding: {}",
        bug.summary()
    );
    assert!(
        matches!(bug.found[0].failure, Failure::Feature { .. }),
        "{:?}",
        bug.found[0].failure
    );
    assert!(matches!(bug.found[0].ruling, Ruling::Confirmed { .. }));
    assert_eq!(bug.confirmed(), 1);
    // The press was a tap at the Pay button's centre, the evidence carries the tree of each look, and each re-run launched the
    // app fresh (the force-stop and the start come before the tap).
    assert!(bug.evidence.iter().any(|row| row.action == "click pay"));
    assert!(bug.evidence.iter().all(|row| !row.tree.is_empty()));
    let log = log_of(buggy.path());
    assert!(
        log.contains("-s emulator-5554 shell input tap 540 1680"),
        "{log}"
    );
    assert!(log.contains("shell am force-stop com.shop"), "{log}");
    assert!(
        log.contains("shell am start -n com.shop/.MainActivity"),
        "{log}"
    );

    let fixed = fixture("cart 540 1680 receipt\n");
    let ok = run(&mut || open(fixed.path()));
    assert!(
        ok.found.is_empty(),
        "the fixed build passes: {}",
        ok.summary()
    );
    assert_eq!(ok.confirmed(), 0);
}

#[test]
fn the_tree_is_the_dump_read_into_elements_with_their_points() {
    let dir = fixture("");
    let mut target = open(dir.path()).unwrap();
    let tree = target.tree().unwrap();
    assert_eq!(tree.title, "Cart");
    assert!(tree.shows("Pay"));
    let pay = tree
        .elements
        .iter()
        .find(|e| e.id == "pay")
        .expect("the pay button by its resource name");
    assert_eq!(pay.role, "button");
    assert_eq!(pay.label, "Pay");
    let (elements, points) = elements_of(&apz_android::uiautomator::parse_dump(CART).unwrap());
    assert_eq!(elements.len(), 2);
    assert_eq!(points.get("pay"), Some(&(540, 1680)));
    assert_eq!(points.get("title"), Some(&(540, 150)));
}

#[test]
fn a_typed_value_a_key_and_a_picture_are_sent_as_their_lines_and_a_bad_key_is_refused() {
    let dir = fixture("");
    let mut target = open(dir.path()).unwrap();
    target.tree().unwrap();
    target
        .act(&Action::Type {
            element: "pay".into(),
            text: "no ice".into(),
        })
        .unwrap();
    target.act(&Action::Press { key: "back".into() }).unwrap();
    assert!(target.act(&Action::Press { key: "fly".into() }).is_err());
    assert!(
        target
            .act(&Action::Click {
                element: "nowhere".into()
            })
            .is_err()
    );
    let shot = target.screenshot().unwrap();
    assert_eq!(shot.mime, "image/png");
    assert_eq!(shot.bytes, b"PNG-FIXTURE");
    let log = log_of(dir.path());
    assert!(log.contains("shell input tap 540 1680"), "{log}");
    assert!(log.contains("shell input text no%sice"), "{log}");
    assert!(log.contains("shell input keyevent KEYCODE_BACK"), "{log}");
    assert!(
        log.contains("shell screencap -p /sdcard/apz-shot-1.png"),
        "{log}"
    );
    assert!(log.contains("shell rm -f /sdcard/apz-shot-1.png"), "{log}");
}

/// The station's shell as the test answers it: every line is recorded, and each answers exit 0 with `output`.
struct Shell<'a> {
    lines: &'a RefCell<Vec<String>>,
    output: String,
}

impl Calls for Shell<'_> {
    fn call(&self, tool: &str, input: &Value) -> Result<Value, String> {
        assert_eq!(tool, "app_script", "the node's lines go through app_script");
        assert_eq!(input["bridge"], "cli");
        self.lines
            .borrow_mut()
            .push(input["script"].as_str().unwrap().to_string());
        Ok(serde_json::json!({"exit_code": 0, "output": self.output}))
    }
}

#[test]
fn the_node_runs_adb_as_one_line_of_the_station_shell_and_a_recording_answers_its_pid() {
    use apz_android::adb::{Adb, Runner, exec};
    let lines = RefCell::new(Vec::new());
    let shell = Shell {
        lines: &lines,
        output: "4242\n".into(),
    };
    let node = NodeAdb::over(Box::new(shell), "mac1");
    exec(&node, "emulator-5554", &Adb::Tap { x: 540, y: 1680 }).unwrap();
    let pid = node
        .start(
            &Adb::Record {
                path: "/sdcard/apz-step.mp4".into(),
                seconds: 180,
            }
            .args("emulator-5554")
            .unwrap(),
        )
        .unwrap();
    assert_eq!(pid, 4242);
    // The lines are the adb command, quoted word by word, with `adb` as the program.
    let sent = lines.borrow();
    assert!(
        sent[0].starts_with("'adb' '-s' 'emulator-5554' 'shell' 'input' 'tap' '540' '1680'"),
        "{sent:?}"
    );
    assert!(
        sent[1].contains("'screenrecord' '--time-limit' '180' '/sdcard/apz-step.mp4' </dev/null"),
        "{sent:?}"
    );
}

#[test]
fn an_android_target_acts_as_an_agent_and_names_its_device_and_its_app() {
    let mut target = AndroidTarget {
        station: "mac1".into(),
        agent: 7,
        serial: "R58N1234".into(),
        component: "com.shop/.MainActivity".into(),
        apk: Some("/Users/ci/shop.apk".into()),
        record: true,
        task: Some("t1".into()),
    };
    assert!(target.check().is_ok());
    target.agent = 0;
    assert!(
        target
            .check()
            .unwrap_err()
            .contains("never the owner's client")
    );
    target.agent = 7;
    target.apk = Some("shop.apk".into());
    assert!(target.check().unwrap_err().contains("absolute path"));
    target.apk = None;
    target.component = "com.shop".into();
    assert!(target.check().unwrap_err().contains("package/.Activity"));
    target.component = "com.shop/.MainActivity".into();
    target.serial = " ".into();
    assert!(target.check().is_err());
}

/// The station as the test answers it: a device lease comes back `state` (active, or queued when another agent holds the
/// device), and every call is recorded.
struct Station {
    state: &'static str,
    calls: RefCell<Vec<(String, Value)>>,
}

impl Calls for Station {
    fn call(&self, tool: &str, input: &Value) -> Result<Value, String> {
        self.calls
            .borrow_mut()
            .push((tool.to_string(), input.clone()));
        match tool {
            "station_lease" => Ok(json!({"id": "l9", "state": self.state})),
            "station_release" => Ok(json!({})),
            other => Err(format!("the fake station does not answer {other}")),
        }
    }
}

fn step_config(record: bool) -> AndroidTarget {
    AndroidTarget {
        station: "mac1".into(),
        agent: 7,
        serial: "emulator-5554".into(),
        component: "com.shop/.MainActivity".into(),
        apk: Some("/Users/ci/shop.apk".into()),
        record,
        task: Some("t1".into()),
    }
}

#[test]
fn a_step_leases_its_device_installs_its_app_records_and_lands_the_video_then_releases_the_lease() {
    let dir = fixture("");
    let scratch = tempfile::tempdir().unwrap();
    let station = Station {
        state: "active",
        calls: RefCell::new(Vec::new()),
    };
    let runner = Local::in_dirs(&[dir.path().to_path_buf()]).unwrap();
    let config = step_config(true);
    let held = prepare_on(&station, &runner, &config, scratch.path()).unwrap();
    {
        let calls = station.calls.borrow();
        assert_eq!(calls[0].0, "station_lease");
        assert_eq!(
            calls[0].1["kind"], "device",
            "a device is leased as a device"
        );
        assert_eq!(calls[0].1["device"], "emulator-5554");
        assert_eq!(calls[0].1["station"], "mac1");
        assert_eq!(calls[0].1["task"], "t1");
    }
    let log = log_of(dir.path());
    assert!(
        log.contains("-s emulator-5554 install -r /Users/ci/shop.apk"),
        "{log}"
    );

    let mut stored = Vec::new();
    let note = finish_on(&station, &runner, &config, Some(held), |bytes| {
        stored = bytes.to_vec();
        Ok("blob-7".into())
    });
    assert_eq!(
        stored, b"MP4",
        "the video is pulled from the device and landed"
    );
    assert_eq!(note, "the recording of emulator-5554 is artifact blob-7");
    // The recording runs in the background: its line is in the log once the device's screenrecord has run, which the stop waited for.
    let log = log_of(dir.path());
    assert!(
        log.contains("-s emulator-5554 shell screenrecord --time-limit 180 /sdcard/apz-step.mp4"),
        "{log}"
    );
    assert!(
        log.contains("-s emulator-5554 shell pkill -2 screenrecord"),
        "{log}"
    );
    let calls = station.calls.borrow();
    let release = calls
        .iter()
        .find(|(tool, _)| tool == "station_release")
        .unwrap();
    assert_eq!(
        release.1["lease"], "l9",
        "the lease is released at the step's end"
    );
}

#[test]
fn a_device_another_agent_holds_is_not_prepared_and_nothing_is_sent_to_it() {
    let dir = fixture("");
    let scratch = tempfile::tempdir().unwrap();
    let station = Station {
        state: "queued",
        calls: RefCell::new(Vec::new()),
    };
    let runner = Local::in_dirs(&[dir.path().to_path_buf()]).unwrap();
    let why = prepare_on(&station, &runner, &step_config(true), scratch.path()).unwrap_err();
    assert!(
        why.contains("not granted") && why.contains("another agent"),
        "{why}"
    );
    let calls = station.calls.borrow();
    assert!(
        calls
            .iter()
            .any(|(tool, input)| tool == "station_release" && input["lease"] == "l9"),
        "the lease that was not granted is given back"
    );
    assert!(
        log_of(dir.path()).is_empty(),
        "no adb line reaches a device that was not granted"
    );
}
