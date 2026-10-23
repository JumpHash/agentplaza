//! What a node can offer the test lab beyond its CPU, memory and GPUs (`apid`'s `test_lab`): the browsers it has, the
//! Android phones attached over `adb`, and whether it can shape its traffic (the resilience stages of a run). Each
//! probe only looks (PATH, the application folders, `adb devices`, the process's capability set): nothing is
//! installed or started. The parsers are pure and tested with fixed text.

use std::path::Path;

/// A browser the lab names, and the executables or application bundles that are that browser here.
const BROWSERS: [(&str, &[&str], &[&str]); 3] = [
    (
        "chromium",
        &[
            "chromium",
            "chromium-browser",
            "google-chrome",
            "google-chrome-stable",
        ],
        &[
            "/Applications/Google Chrome.app",
            "/Applications/Chromium.app",
        ],
    ),
    ("firefox", &["firefox"], &["/Applications/Firefox.app"]),
    (
        "webkit",
        &["WebKitWebDriver", "MiniBrowser"],
        &["/Applications/Safari.app"],
    ),
];

/// The browsers the node has (`chromium`, `firefox`, `webkit`), in the lab's names, in that order (pure over `has`, which
/// says whether a candidate is there: an executable name on the PATH, or an application bundle's absolute path).
pub fn browsers_where(has: &dyn Fn(&str) -> bool) -> Vec<String> {
    BROWSERS
        .iter()
        .filter(|(_, names, apps)| {
            names
                .iter()
                .chain(apps.iter())
                .any(|candidate| has(candidate))
        })
        .map(|(lab, _, _)| (*lab).to_string())
        .collect()
}

/// The browsers this node has now.
pub fn browsers() -> Vec<String> {
    browsers_where(&|candidate| {
        if candidate.starts_with('/') {
            Path::new(candidate).exists()
        } else {
            on_path(candidate)
        }
    })
}

/// The serials of the phones `adb devices` lists as `device` (pure). A phone that is `unauthorized` or `offline` is not
/// listed: a test cannot use it.
pub fn parse_adb_devices(text: &str) -> Vec<String> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let mut columns = line.split_whitespace();
            let serial = columns.next()?;
            (columns.next()? == "device").then(|| serial.to_string())
        })
        .collect()
}

/// The Android phones attached to this node now: none unless `adb` is on the PATH and answers.
pub fn phones() -> Vec<String> {
    if !on_path("adb") {
        return Vec::new();
    }
    std::process::Command::new("adb")
        .arg("devices")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| parse_adb_devices(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or_default()
}

/// Whether a process's capability set (the `CapEff` line of `/proc/self/status`) holds CAP_NET_ADMIN (bit 12) (pure).
pub fn net_admin_in(status: &str) -> bool {
    status
        .lines()
        .find_map(|line| line.strip_prefix("CapEff:"))
        .and_then(|hex| u64::from_str_radix(hex.trim(), 16).ok())
        .is_some_and(|caps| caps & (1 << 12) != 0)
}

/// Whether the node can shape its traffic: Linux with `tc` on the PATH and CAP_NET_ADMIN (a root daemon has it).
/// macOS and everything else cannot here, so the resilience stages there are reported as needing a Linux node.
pub fn traffic_shaping() -> bool {
    if !cfg!(target_os = "linux") || !on_path("tc") {
        return false;
    }
    std::fs::read_to_string("/proc/self/status")
        .map(|status| net_admin_in(&status))
        .unwrap_or(false)
}

/// Whether `name` is an executable file in one of the PATH's folders.
fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browsers_map_to_the_lab_names() {
        let have = ["google-chrome", "WebKitWebDriver"];
        let found = browsers_where(&|name| have.contains(&name));
        assert_eq!(found, vec!["chromium".to_string(), "webkit".to_string()]);
        assert!(browsers_where(&|_| false).is_empty());
    }

    #[test]
    fn adb_lists_only_authorized_devices() {
        let text = "List of devices attached\nR58N1\tdevice\nABC\tunauthorized\n192.168.1.106:5555\tdevice\nX9\toffline\n\n";
        assert_eq!(
            parse_adb_devices(text),
            vec!["R58N1".to_string(), "192.168.1.106:5555".to_string()]
        );
        assert!(parse_adb_devices("List of devices attached\n").is_empty());
    }

    #[test]
    fn net_admin_is_bit_twelve_of_the_effective_set() {
        assert!(net_admin_in("Name:\tx\nCapEff:\t0000001fffffffff\n"));
        assert!(net_admin_in("CapEff:\t0000000000001000\n"));
        assert!(!net_admin_in("CapEff:\t0000000000000000\n"));
        assert!(!net_admin_in("Name:\tx\n"));
    }
}
