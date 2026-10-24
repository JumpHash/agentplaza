//! Live tests on this Mac: the Accessibility tree of a real app (Finder, which is always running) and, when Screen
//! Recording is granted, one capture of the screen. Each test checks the permission first and skips with a printed
//! reason when it is not granted: the tests never prompt for a permission and never open System Settings.

#![cfg(target_os = "macos")]

use apz_desktop::{Desktop, SessionSpec};
use apz_desktop_macos::{MacDesktop, accessibility_trusted, screen_recording_granted};
use apz_station::SessionKind;

/// The owner's console session, as the shared lane opens it: the agent number is the owner view's (0).
fn console() -> SessionSpec {
    SessionSpec {
        agent: 0,
        kind: SessionKind::Shared,
        width: 1,
        height: 1,
    }
}

#[test]
fn the_tree_of_a_running_app_is_read_through_accessibility() {
    if !accessibility_trusted() {
        eprintln!(
            "skipped the Accessibility tree test: this test process is not trusted for Accessibility \
             (System Settings > Privacy & Security > Accessibility). The test does not prompt for it."
        );
        return;
    }
    let mut desktop = MacDesktop::new();
    let session = desktop
        .open_session(&console())
        .expect("the console session");
    let root = desktop
        .ui_tree(session, Some("Finder"), 3)
        .expect("Finder's tree");
    assert_eq!(root.role, "application");
    assert_eq!(root.name.as_deref(), Some("Finder"));
    desktop.close_session(session).expect("the session closes");
}

#[test]
fn the_desktop_tree_lists_the_running_apps_with_windows() {
    if !accessibility_trusted() {
        eprintln!(
            "skipped the desktop tree test: this test process is not trusted for Accessibility."
        );
        return;
    }
    let mut desktop = MacDesktop::new();
    let session = desktop
        .open_session(&console())
        .expect("the console session");
    let root = desktop
        .ui_tree(session, None, 2)
        .expect("the desktop's tree");
    assert_eq!(root.role, "desktop");
    // Each child is an app that has windows; the list is never empty on a logged-in desktop, but a headless login
    // may have none, so only the shape is checked.
    assert!(root.children.iter().all(|app| app.role == "application"));
}

#[test]
fn the_screen_is_captured_when_screen_recording_is_granted() {
    if !screen_recording_granted() {
        eprintln!(
            "skipped the capture test: Screen Recording is not granted to this test process \
             (System Settings > Privacy & Security > Screen & System Audio Recording). The test does not prompt for it."
        );
        return;
    }
    let mut desktop = MacDesktop::new();
    let session = desktop
        .open_session(&console())
        .expect("the console session");
    let frame = desktop
        .capture(session)
        .expect("the main display's capture");
    frame.check().expect("the frame holds its pixels");
    assert!(frame.w > 0 && frame.h > 0);
}
