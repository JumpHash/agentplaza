//! The desktop app: the console, opening on the startup chooser.

use std::os::unix::process::CommandExt;

fn main() -> iced::Result {
    snapshot_off_screen();
    // Mid NVIDIA update (old kernel module, new libraries on disk): restart on the matching ones.
    agentplaza_desktop::gpu::use_matching_driver();
    agentplaza_desktop::console::run()
}

/// `APZ_SNAPSHOT` captures the app's own frame from a hidden window. Wayland has no hidden
/// windows (winit maps them anyway), so a snapshot run restarts itself on X11, where it can stay
/// unmapped and never shows on the owner's screen.
fn snapshot_off_screen() {
    if std::env::var_os("APZ_SNAPSHOT").is_none() || std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let err = std::process::Command::new(exe).args(std::env::args_os().skip(1)).env_remove("WAYLAND_DISPLAY").exec();
    eprintln!("agentplaza-desktop: cannot restart the snapshot on X11: {err}");
}
