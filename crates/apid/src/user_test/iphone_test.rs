//! The iPhone target (PLAN 19.3, 8.5) against a fake `xcrun`: a script on a directory of its own that records every line it is
//! given and writes a picture when asked for one. Nothing here runs Xcode or touches a device. The station is a fake too: its
//! device lease is granted or refused, and its calls are recorded.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use apz_desktop_macos::simctl::Xcrun;
use serde_json::{Value, json};

use super::desk::Calls;
use super::iphone::{INPUT_UNWIRED, Phone, finish_on, prepare_on};
use super::target::{Action, Target};
use crate::recipes::model::IphoneTarget;

const UDID: &str = "00008110-ABCDEF";

/// A fake `xcrun`: each line is recorded in `log`; a screenshot line writes its picture to the path it names (the last word).
fn fake_xcrun(dir: &Path) -> PathBuf {
    let program = dir.join("xcrun");
    let script = format!(
        r#"#!/bin/sh
printf '%s\n' "$*" >> '{log}'
eval "last=\${{$#}}"
case "$*" in
  *" screenshot "*) printf 'PNG-IPHONE' > "$last" ;;
esac
exit 0
"#,
        log = dir.join("log").display()
    );
    std::fs::write(&program, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    program
}

fn log_of(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("log")).unwrap_or_default()
}

/// The station as the test answers it: a device lease comes back `state`, and every call is recorded.
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

fn config(app: Option<&str>) -> IphoneTarget {
    IphoneTarget {
        station: "mac1".into(),
        agent: 7,
        udid: UDID.into(),
        bundle: "com.acme.shop".into(),
        app: app.map(str::to_string),
        task: Some("t1".into()),
    }
}

#[test]
fn an_iphone_is_launched_pictured_through_devicectl_and_refuses_a_tap() {
    let dir = tempfile::tempdir().unwrap();
    let xcrun = Xcrun::at(fake_xcrun(dir.path()));
    let mut phone = Phone::open(Box::new(xcrun), UDID, dir.path(), "com.acme.shop").unwrap();
    assert_eq!(phone.kind(), "iphone");
    let log = log_of(dir.path());
    assert!(
        log.contains(&format!(
            "devicectl device process launch --device {UDID} com.acme.shop"
        )),
        "{log}"
    );

    assert!(
        phone.tree().unwrap().is_empty(),
        "no tree is read from a physical iPhone"
    );
    let shot = phone.screenshot().unwrap();
    assert_eq!(shot.mime, "image/png");
    assert_eq!(shot.bytes, b"PNG-IPHONE");
    let log = log_of(dir.path());
    let path = dir.path().join("apz-iphone-shot-1.png");
    assert!(
        log.contains(&format!(
            "devicectl device screenshot --device {UDID} {}",
            path.display()
        )),
        "{log}"
    );
    assert_eq!(
        phone
            .act(&Action::Click {
                element: "pay".into()
            })
            .unwrap_err(),
        INPUT_UNWIRED
    );
}

#[test]
fn a_step_leases_the_iphone_installs_its_app_and_gives_the_lease_back() {
    let dir = tempfile::tempdir().unwrap();
    let xcrun = Xcrun::at(fake_xcrun(dir.path()));
    let station = Station {
        state: "active",
        calls: RefCell::new(Vec::new()),
    };
    let cfg = config(Some("/Users/ci/Shop.app"));
    let held = prepare_on(&station, &xcrun, &cfg).unwrap();
    {
        let calls = station.calls.borrow();
        assert_eq!(calls[0].0, "station_lease");
        assert_eq!(
            calls[0].1["kind"], "device",
            "a phone is leased as a device, like an Android one"
        );
        assert_eq!(calls[0].1["device"], UDID);
        assert_eq!(calls[0].1["station"], "mac1");
    }
    let log = log_of(dir.path());
    assert!(
        log.contains(&format!(
            "devicectl device install app --device {UDID} /Users/ci/Shop.app"
        )),
        "{log}"
    );
    assert_eq!(finish_on(&station, &cfg, Some(held)), "");
    let calls = station.calls.borrow();
    let release = calls
        .iter()
        .find(|(tool, _)| tool == "station_release")
        .unwrap();
    assert_eq!(release.1["lease"], "l9");
}

#[test]
fn a_phone_another_agent_holds_is_not_prepared_and_nothing_is_installed() {
    let dir = tempfile::tempdir().unwrap();
    let xcrun = Xcrun::at(fake_xcrun(dir.path()));
    let station = Station {
        state: "queued",
        calls: RefCell::new(Vec::new()),
    };
    let why = prepare_on(&station, &xcrun, &config(Some("/Users/ci/Shop.app")))
        .err()
        .unwrap();
    assert!(why.contains("not granted"), "{why}");
    assert!(
        log_of(dir.path()).is_empty(),
        "no line reaches a phone that was not granted"
    );
}

#[test]
fn an_iphone_target_acts_as_an_agent_and_its_app_is_an_absolute_path() {
    assert!(config(None).check().is_ok());
    let mut bad = config(None);
    bad.agent = 0;
    assert!(
        bad.check()
            .unwrap_err()
            .contains("never the owner's client")
    );
    assert!(
        config(Some("Shop.app"))
            .check()
            .unwrap_err()
            .contains("absolute path")
    );
    let mut bad = config(None);
    bad.udid = " ".into();
    assert!(bad.check().is_err());
}
