//! The Linux X11 backend of the workstation's desktop control: an implementation of [`apz_desktop::Desktop`].
//!
//! - **Pixel sessions** get a private virtual display per agent: `Xvfb :N` started under the configured
//!   user ([`X11Config::run_as`], default the daemon's own), with an optional window manager. Nothing on the
//!   owner's screen is touched, and closing the session kills the server and everything started on it.
//! - **Shared sessions** are the station owner's own display (`DISPLAY`, or [`X11Config::owner_display`]).
//!   Owner activity is detected from the pointer and, when the XScreenSaver extension is there, from the idle
//!   time, so the agent can yield to the owner.
//! - **Capture** is XShm `GetImage` of the root window, with plain `GetImage` as the fallback.
//! - **Input** is XTest: motion, buttons, wheel (buttons 4 to 7), keys by keysym through the server's keyboard
//!   map, and text typed through a scratch keycode remapped for characters the map does not have.
//! - **Apps** are the `.desktop` entries of the XDG application directories, launched into the session's display.
//! - **Focus and quit** are EWMH on the session's display: an app's windows are those whose process the backend
//!   started (its `_NET_WM_PID` is in that process's tree) or, on a private display, whose class or title is the
//!   app's. `quit` asks gracefully (`_NET_CLOSE_WINDOW`, or `WM_DELETE_WINDOW` without a window manager); with
//!   `force` it also ends the processes the backend started, and never any other.
//! - **UI tree and UI actions** belong to the accessibility crate: [`X11Desktop::with_ui`] takes its
//!   [`UiHook`], and without one they return `Unsupported`.
//!
//! The whole crate is empty on other platforms; see the manifest.

#![cfg(target_os = "linux")]

mod apps;
mod backend;
mod capture;
mod display;
mod ewmh;
mod input;
mod keymap;
mod process;
mod session;

pub use backend::{UiHook, X11Config, X11Desktop};
pub use display::{DisplayPaths, FIRST_DISPLAY};
