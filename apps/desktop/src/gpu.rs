//! Using the GPU when the NVIDIA driver is mid-update.
//!
//! After the NVIDIA packages are upgraded and before a reboot, the kernel module still loaded is
//! the old version while the userspace libraries on disk are the new one; the two refuse each
//! other, Vulkan finds no device and the app would fall back to software rendering. Programs
//! started before the upgrade keep working because they still hold the old libraries in memory.
//!
//! [`use_matching_driver`] gives this process the same: it extracts the userspace that matches
//! the running kernel module into `~/.cache/agentplaza/nvidia/<version>` (from pacman's package
//! cache, or failing that from a running process that still maps it), then re-executes the app
//! with the Vulkan and EGL loaders and the dynamic linker pointed at it. Nothing outside the
//! app's own cache is touched; once the versions match again (after a reboot) the cache is pruned.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Set on the re-executed process: never re-execute twice.
const MARK: &str = "APZ_GPU_DRIVER";
/// The libraries the GL/EGL/Vulkan driver loads (`libGLX_nvidia` is also the Vulkan ICD).
const LIBS: &[&str] = &[
    "libGLX_nvidia",
    "libEGL_nvidia",
    "libnvidia-glcore",
    "libnvidia-eglcore",
    "libnvidia-glsi",
    "libnvidia-tls",
    "libnvidia-gpucomp",
    "libnvidia-glvkspirv",
    "libnvidia-allocator",
];
/// Where distributions keep the NVIDIA userspace.
const LIB_DIRS: &[&str] = &["/usr/lib", "/usr/lib64", "/usr/lib/x86_64-linux-gnu"];

/// Re-executes the app on the userspace matching the running NVIDIA kernel module when the one on
/// disk differs. Returns (and the app goes on as it is) when there is nothing to do or it cannot.
pub fn use_matching_driver() {
    if std::env::var_os(MARK).is_some() || std::env::var_os("APZ_NO_DRIVER_MATCH").is_some() {
        return;
    }
    let Some(kernel) = kernel_version() else {
        return;
    };
    let Some(disk) = userspace_version() else {
        return;
    };
    let cache = cache_root();
    if kernel == disk {
        // In step again (rebooted): the extracted copies are no longer needed.
        let _ = fs::remove_dir_all(&cache);
        return;
    }
    let dir = cache.join(&kernel);
    let lib = dir.join("lib");
    let glx = lib.join(format!("libGLX_nvidia.so.{kernel}"));
    if !glx.exists() {
        if let Err(err) = prepare(&kernel, &lib) {
            eprintln!(
                "agentplaza-desktop: the NVIDIA kernel module is {kernel} but the libraries on disk are {disk}, \
                 and no matching {kernel} userspace was found ({err}); rendering may fall back to software. Reboot to fix."
            );
            let _ = fs::remove_dir_all(&dir);
            return;
        }
    }
    let icd = dir.join("nvidia_icd.json");
    let egl = dir.join("10_nvidia.json");
    let written = fs::write(
        &icd,
        format!("{{\n  \"file_format_version\": \"1.0.1\",\n  \"ICD\": {{ \"library_path\": \"{}\", \"api_version\": \"1.4\" }}\n}}\n", glx.display()),
    )
    .and_then(|()| {
        fs::write(
            &egl,
            format!(
                "{{\n  \"file_format_version\": \"1.0.0\",\n  \"ICD\": {{ \"library_path\": \"{}\" }}\n}}\n",
                lib.join(format!("libEGL_nvidia.so.{kernel}")).display()
            ),
        )
    });
    if let Err(err) = written {
        eprintln!(
            "agentplaza-desktop: cannot write the driver manifests in {}: {err}",
            dir.display()
        );
        return;
    }
    let mut ld_path = OsString::from(lib.as_os_str());
    if let Some(old) = std::env::var_os("LD_LIBRARY_PATH") {
        ld_path.push(":");
        ld_path.push(old);
    }
    eprintln!(
        "agentplaza-desktop: the NVIDIA kernel module is {kernel} but the libraries on disk are {disk}; \
         using the matching {kernel} userspace until the next reboot"
    );
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let err = Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env(MARK, &kernel)
        .env("LD_LIBRARY_PATH", ld_path)
        .env("VK_DRIVER_FILES", &icd)
        .env("VK_ICD_FILENAMES", &icd)
        .env("__EGL_VENDOR_LIBRARY_FILENAMES", &egl)
        .env("WGPU_BACKEND", "vulkan")
        .exec();
    eprintln!("agentplaza-desktop: cannot restart on the matching driver: {err}");
}

/// The loaded kernel module's version, e.g. "615.71.09" (from `/proc/driver/nvidia/version`).
fn kernel_version() -> Option<String> {
    let text = fs::read_to_string("/proc/driver/nvidia/version").ok()?;
    text.lines()
        .next()?
        .split_whitespace()
        .find(|t| is_version(t))
        .map(str::to_string)
}

/// The userspace version on disk, from `libnvidia-glcore.so.<version>`.
fn userspace_version() -> Option<String> {
    LIB_DIRS.iter().find_map(|dir| {
        fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let version = name.strip_prefix("libnvidia-glcore.so.")?;
            is_version(version).then(|| version.to_string())
        })
    })
}

fn is_version(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    parts.len() >= 2
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

fn cache_root() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("agentplaza").join("nvidia")
}

/// Fills `lib` with the driver libraries of `version`: from pacman's package cache, else copied
/// from a running process that still maps them.
fn prepare(version: &str, lib: &Path) -> Result<(), String> {
    fs::create_dir_all(lib).map_err(|e| format!("{}: {e}", lib.display()))?;
    match from_pacman_cache(version, lib) {
        Ok(()) => Ok(()),
        Err(pacman) => {
            from_running_process(version, lib).map_err(|proc| format!("{pacman}; {proc}"))
        }
    }
}

/// Extracts the libraries from `/var/cache/pacman/pkg/nvidia-utils-<version>-*.pkg.tar.zst`. The
/// package must be root-owned and not writable by others: pacman verified it when it fetched it.
fn from_pacman_cache(version: &str, lib: &Path) -> Result<(), String> {
    let cache = Path::new("/var/cache/pacman/pkg");
    let prefix = format!("nvidia-utils-{version}-");
    let package = fs::read_dir(cache)
        .map_err(|e| format!("{}: {e}", cache.display()))?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".pkg.tar.zst"))
        })
        .ok_or_else(|| format!("no {prefix}*.pkg.tar.zst in the pacman cache"))?;
    let meta = fs::metadata(&package).map_err(|e| e.to_string())?;
    if meta.uid() != 0 || meta.mode() & 0o022 != 0 {
        return Err(format!(
            "{} is not a root-owned, root-only-writable package",
            package.display()
        ));
    }
    let staging = lib.with_file_name("extract");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let mut tar = Command::new("tar");
    tar.arg("--zstd")
        .arg("-xf")
        .arg(&package)
        .arg("-C")
        .arg(&staging)
        .arg("--wildcards");
    for name in LIBS {
        tar.arg(format!("usr/lib/{name}.so*"));
    }
    let status = tar.status().map_err(|e| format!("tar: {e}"))?;
    if !status.success() {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!("tar could not extract {}", package.display()));
    }
    let extracted = staging.join("usr/lib");
    for entry in fs::read_dir(&extracted)
        .map_err(|e| e.to_string())?
        .flatten()
    {
        fs::rename(entry.path(), lib.join(entry.file_name())).map_err(|e| e.to_string())?;
    }
    let _ = fs::remove_dir_all(&staging);
    link_sonames(version, lib);
    Ok(())
}

/// Copies `<lib>.so.<version> (deleted)` mappings out of a running process (`/proc/*/map_files`;
/// only processes this user may inspect).
fn from_running_process(version: &str, lib: &Path) -> Result<(), String> {
    let wanted: Vec<String> = LIBS.iter().map(|l| format!("{l}.so.{version}")).collect();
    let mut found = std::collections::BTreeSet::new();
    for proc in fs::read_dir("/proc").map_err(|e| e.to_string())?.flatten() {
        let Ok(maps) = fs::read_dir(proc.path().join("map_files")) else {
            continue;
        };
        for map in maps.flatten() {
            let Ok(target) = fs::read_link(map.path()) else {
                continue;
            };
            let Some(name) = target
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            if !wanted.contains(&name) || found.contains(&name) {
                continue;
            }
            if fs::copy(map.path(), lib.join(&name)).is_ok() {
                found.insert(name);
            }
        }
        if found.len() == wanted.len() {
            break;
        }
    }
    // The driver cannot run without these four; the others are optional.
    for needed in [
        "libGLX_nvidia",
        "libnvidia-glcore",
        "libnvidia-glsi",
        "libnvidia-tls",
    ] {
        if !found.contains(&format!("{needed}.so.{version}")) {
            return Err(format!("no running process maps {needed}.so.{version}"));
        }
    }
    link_sonames(version, lib);
    Ok(())
}

/// The `libGLX_nvidia.so.0` / `libEGL_nvidia.so.0` names the loaders ask for.
fn link_sonames(version: &str, lib: &Path) {
    for (link, target) in [
        ("libGLX_nvidia.so.0", "libGLX_nvidia"),
        ("libEGL_nvidia.so.0", "libEGL_nvidia"),
    ] {
        let path = lib.join(link);
        if !path.exists() {
            let _ = std::os::unix::fs::symlink(format!("{target}.so.{version}"), path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_version;

    #[test]
    fn driver_versions_are_dotted_numbers() {
        assert!(is_version("615.71.09"));
        assert!(is_version("550.120"));
        assert!(!is_version("615"));
        assert!(!is_version("x86_64"));
        assert!(!is_version("615.71.09-1"));
    }
}
