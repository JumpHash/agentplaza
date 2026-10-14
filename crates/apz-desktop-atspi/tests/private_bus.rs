//! A live check of the AT-SPI connection against a private session: never the owner's session bus, and never the
//! accessibility bus at the owner's runtime directory.
//!
//! The test starts its own session bus and its own accessibility bus on abstract sockets, with configs in a
//! temporary directory and no service directories, so nothing can be activated from the owner's installation.
//! `org.a11y.Bus` is answered in this process, and the AT-SPI registry is started by hand on the private
//! accessibility bus. `at-spi-bus-launcher` is not used: it always binds `$XDG_RUNTIME_DIR`'s fixed path, which
//! is the owner's accessibility socket.
//!
//! The test skips with a message when `dbus-daemon` or the AT-SPI registry is not installed.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use apz_desktop::{DesktopError, Result};
use apz_desktop_atspi::AtspiTree;
use apz_station::{Selector, UiAction};

/// The accessibility bus's answer to `org.a11y.Bus.GetAddress`, served in this process on the private session bus.
struct AccessibilityBus {
    address: String,
}

#[zbus::interface(name = "org.a11y.Bus")]
impl AccessibilityBus {
    fn get_address(&self) -> String {
        self.address.clone()
    }
}

/// The processes and files of the test, removed when the test ends, pass or fail.
struct Harness {
    dir: PathBuf,
    daemons: Vec<u32>,
    registry: Option<Child>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(mut registry) = self.registry.take() {
            let _ = registry.kill();
            let _ = registry.wait();
        }
        for pid in &self.daemons {
            let _ = Command::new("kill").arg(pid.to_string()).status();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn find_executable(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

fn first_existing(candidates: &[&str]) -> Option<PathBuf> {
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|candidate| candidate.is_file())
}

/// A session bus config: listens on an abstract socket and has no service directories.
fn bus_config(socket: &str) -> String {
    format!(
        r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-BUS Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:abstract={socket}</listen>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#
    )
}

/// Starts a forked dbus-daemon from `config` and returns its address and pid. The daemon is not inherited from
/// the owner's environment: the child gets an empty environment and no DBUS_SESSION_BUS_ADDRESS.
fn start_daemon(dbus: &Path, config: &Path, harness: &mut Harness) -> Result<String> {
    let output = Command::new(dbus)
        .arg(format!("--config-file={}", config.display()))
        .args([
            "--fork",
            "--print-address=1",
            "--print-pid=1",
            "--nopidfile",
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .output()
        .map_err(|error| DesktopError::Platform(format!("dbus-daemon did not start: {error}")))?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let mut lines = text.lines();
    let address = lines.next().unwrap_or_default().trim().to_owned();
    let pid: u32 = lines
        .next()
        .unwrap_or_default()
        .trim()
        .parse()
        .map_err(|_| DesktopError::Platform(format!("dbus-daemon printed no pid: {text:?}")))?;
    harness.daemons.push(pid);
    if address.is_empty() {
        return Err(DesktopError::Platform(
            "dbus-daemon printed no address".into(),
        ));
    }
    Ok(address)
}

#[test]
fn a_private_registry_lists_no_apps_and_refuses_unknown_targets() {
    let Some(dbus) = find_executable("dbus-daemon") else {
        eprintln!("skipped: dbus-daemon is not installed");
        return;
    };
    let Some(registry_bin) = first_existing(&[
        "/usr/lib/at-spi2-registryd",
        "/usr/libexec/at-spi2-registryd",
    ]) else {
        eprintln!("skipped: the AT-SPI registry (at-spi2-registryd) is not installed");
        return;
    };

    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("apz-atspi-test-{pid}"));
    std::fs::create_dir_all(&dir).expect("create the test directory");
    let mut harness = Harness {
        dir: dir.clone(),
        daemons: Vec::new(),
        registry: None,
    };

    let session_config = dir.join("session.conf");
    let a11y_config = dir.join("a11y.conf");
    std::fs::write(
        &session_config,
        bus_config(&format!("apz-atspi-test-{pid}-session")),
    )
    .expect("write config");
    std::fs::write(
        &a11y_config,
        bus_config(&format!("apz-atspi-test-{pid}-a11y")),
    )
    .expect("write config");

    let session =
        start_daemon(&dbus, &session_config, &mut harness).expect("the private session bus");
    let a11y =
        start_daemon(&dbus, &a11y_config, &mut harness).expect("the private accessibility bus");

    // org.a11y.Bus on the private session bus, answered from this process.
    let _served = async_io::block_on(async {
        zbus::connection::Builder::address(session.as_str())
            .expect("session address")
            .name("org.a11y.Bus")
            .expect("well-known name")
            .serve_at(
                "/org/a11y/bus",
                AccessibilityBus {
                    address: a11y.clone(),
                },
            )
            .expect("serve org.a11y.Bus")
            .build()
            .await
            .expect("the private session bus accepts this process")
    });

    // The registry on the private accessibility bus. Its environment names that bus and nothing of the owner's.
    harness.registry = Some(
        Command::new(&registry_bin)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("DBUS_SESSION_BUS_ADDRESS", &a11y)
            .env("AT_SPI_BUS_ADDRESS", &a11y)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start the registry on the private accessibility bus"),
    );

    // The registry takes a moment to claim its bus name, so connecting is retried until it answers.
    let deadline = Instant::now() + Duration::from_secs(20);
    let tree = loop {
        match AtspiTree::connect(&session).and_then(|tree| tree.apps().map(|_| tree)) {
            Ok(tree) => break tree,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(200)),
            Err(error) => panic!("the private registry did not answer: {error}"),
        }
    };

    // Nothing is running on this bus, so there are no applications and no focused one.
    assert_eq!(
        tree.apps().expect("list applications"),
        Vec::<String>::new()
    );
    assert!(matches!(tree.tree(None, 2), Err(DesktopError::NotFound(_))));
    assert!(matches!(
        tree.tree(Some("no such app"), 1),
        Err(DesktopError::NotFound(_))
    ));

    let unknown_app = Selector {
        role: Some("button".into()),
        name: Some("Ping".into()),
        app: Some("no such app".into()),
        ..Selector::default()
    };
    assert!(matches!(
        tree.act(&unknown_app, &UiAction::Invoke),
        Err(DesktopError::NotFound(_))
    ));
    assert!(matches!(
        tree.act(&Selector::default(), &UiAction::Invoke),
        Err(DesktopError::Invalid(_))
    ));

    // A live control is not covered here: no toolkit application can register on this bus in this environment.
    // The action and tree paths are covered by the pure tests in the crate's modules.
    eprintln!(
        "note: no accessible application was run; tree and act on a live control are not exercised"
    );
}
