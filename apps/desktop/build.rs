//! Embeds the AgentPlaza icon and version info in `agentplaza-desktop.exe`, so Explorer,
//! the taskbar and Start show the AgentPlaza icon. Only a Windows target built on a Windows
//! host gets the resource; elsewhere this does nothing (a warning when the target is Windows,
//! because the resource compiler exists only on Windows). The asset lists the renderer compiles
//! in (hardware models, robot files) are built by `crates/apz-engine/build.rs`.
use std::path::Path;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let icon = Path::new(&manifest_dir).join("../../packaging/icons/agentplaza.ico");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", icon.display());
    windows_icon(&icon);
}

fn windows_icon(icon: &Path) {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if !cfg!(windows) {
        println!(
            "cargo:warning=agentplaza-desktop: the Windows icon and version info are skipped; the resource compiler needs a Windows host"
        );
        return;
    }

    let icon = icon.to_str().expect("the icon path is valid UTF-8");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(icon)
        .set("ProductName", "AgentPlaza")
        .set("FileDescription", "AgentPlaza desktop app")
        .set("CompanyName", "JUMPHASH");
    if let Err(e) = res.compile() {
        panic!("failed to embed the icon and version info in agentplaza-desktop.exe: {e}");
    }
}
