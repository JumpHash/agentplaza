//! The Windows backend of the workstation's desktop control interface ([`apz_desktop::Desktop`]).
//!
//! - Screen: DXGI Desktop Duplication of the primary display, with a GDI BitBlt fallback. Windows.Graphics.Capture
//!   is not used yet.
//! - Input: SendInput. Pointer positions are screen points mapped over the virtual screen; keys are virtual-key
//!   codes (US layout); text goes as keystrokes, with characters that have no key sent as UTF-16 units.
//! - UI tree and actions: UI Automation. Actions run through the control's patterns, so they never move the
//!   owner's pointer or take focus.
//! - Apps: Start-menu shortcuts and App Paths registrations; launch through ShellExecuteW; focus through
//!   SetForegroundWindow.
//! - Sessions: `Shared` is the interactive session the process runs in. `Pixel` is refused: Windows client
//!   editions have one desktop session. Windows Server is a TODO.
//!
//! The Win32 code sits behind `cfg(windows)`. The pure parts (key tables, geometry, role names, the owner test,
//! the catalogue rules, pixel order) compile and test on any host.

#[cfg_attr(not(windows), allow(dead_code))]
mod catalog;
#[cfg_attr(not(windows), allow(dead_code))]
mod geometry;
#[cfg_attr(not(windows), allow(dead_code))]
mod keys;
#[cfg_attr(not(windows), allow(dead_code))]
mod owner;
#[cfg_attr(not(windows), allow(dead_code))]
mod pixels;
#[cfg_attr(not(windows), allow(dead_code))]
mod policy;
#[cfg_attr(not(windows), allow(dead_code))]
mod roles;

#[cfg(windows)]
mod win;

#[cfg(windows)]
pub use win::WindowsDesktop;
