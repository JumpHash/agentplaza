//! The macOS backend of the workstation's desktop interface.
//!
//! [`MacDesktop`] implements [`apz_desktop::Desktop`] with:
//!
//! - **Screen:** CoreGraphics (`CGDisplayCreateImage` on the main display), repacked into an RGBA frame at the
//!   display's native pixel size. ScreenCaptureKit is the modern path; both need the Screen Recording permission,
//!   which [`MacDesktop::capture`] checks first.
//! - **Input:** `CGEvent`s posted to the HID event tap, after the Accessibility permission is checked.
//! - **UI tree and actions:** the Accessibility API (`AXUIElement`), walked down to a depth with a node cap.
//! - **Apps:** the application bundles in the usual folders, read with CoreFoundation, and the running applications
//!   from `NSWorkspace`. Launching runs `open -a`, which also brings the app to the front.
//!
//! The pure parts are public modules and build on every host, so their tests run on Linux:
//!
//! - [`keys`]: the Carbon virtual key codes and the `CGEventFlags` masks.
//! - [`input`]: an [`apz_station::InputAction`] planned as the events it posts.
//! - [`pixels`]: the repack of a CoreGraphics image into RGBA.
//! - [`ax`]: Accessibility attributes decoded into [`apz_desktop::UiNode`]s, the actions a control offers, the AX
//!   call for an action, and selector matching.
//! - [`owner`]: whether the owner is using the machine, from the idle time and our last injected event.
//! - [`apps`]: bundle names, bridges and the app list rules.
//!
//! **Pixel sessions are not implemented.** A pixel session for a separate agent user needs that user's
//! screen-sharing session on a virtual display, which the daemon will set up in a later step (TODO: see the
//! daemon). Until then `open_session` refuses them with [`apz_desktop::DesktopError::Unsupported`].

pub mod apps;
pub mod ax;
pub mod input;
pub mod keys;
pub mod owner;
pub mod pixels;
pub mod simctl;

#[cfg(target_os = "macos")]
mod backend;
#[cfg(target_os = "macos")]
mod sys;
#[cfg(target_os = "macos")]
mod tree;
#[cfg(target_os = "macos")]
mod workspace;

#[cfg(target_os = "macos")]
pub use backend::MacDesktop;

/// Whether this process is trusted for Accessibility (the tree and the input need it). Never prompts.
#[cfg(target_os = "macos")]
pub fn accessibility_trusted() -> bool {
    sys::accessibility_trusted()
}

/// Whether this process may capture the screen (Screen Recording). Never prompts.
#[cfg(target_os = "macos")]
pub fn screen_recording_granted() -> bool {
    sys::screen_recording_granted()
}
