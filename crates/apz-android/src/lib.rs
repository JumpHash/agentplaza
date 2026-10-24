//! Android over adb (PLAN 19.3, 8.5): an Android emulator or phone is named by its adb serial, and a user test drives it
//! through three things this crate builds and runs.
//!
//! - [`adb`]: the `adb -s <serial> …` argument lists (the tree's dump, taps, swipes, text and keys, screencap and
//!   screenrecord, pull, install, `am start` and `am force-stop`), each checked before it is built, and the [`adb::Runner`]
//!   that runs them. A node runs the lines through its shell with [`adb::shell_line`]; [`adb::Local`] runs the `adb` found
//!   on given directories (a fake in the tests, never the real one).
//! - [`uiautomator`]: the XML a `uiautomator dump` writes, read into nodes with their bounds, so the user test can
//!   name a node and tap its centre.
//!
//! Nothing here connects to a device: the lines are arguments until a runner runs them.

pub mod adb;
pub mod uiautomator;
