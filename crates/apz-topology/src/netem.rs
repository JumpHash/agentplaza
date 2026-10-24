//! Traffic shaping on the lab VM's internal network (PLAN 22.8): `tc qdisc ... netem` on the bridge, where the guest can.
//! Nothing is installed: the guest must already have `tc`, root, and the bridge device, or the scenario is not run and
//! says which of those is missing.

use crate::steps::Host;

/// The command that shapes every packet on `device` with `delay_ms` and `loss_percent`.
pub fn shaping(device: &str, delay_ms: u64, loss_percent: u32) -> Vec<String> {
    [
        "tc",
        "qdisc",
        "replace",
        "dev",
        device,
        "root",
        "netem",
        "delay",
        &format!("{delay_ms}ms"),
        "loss",
        &format!("{loss_percent}%"),
    ]
    .iter()
    .map(|part| (*part).to_string())
    .collect()
}

/// The command that lifts the shaping from `device`.
pub fn lifting(device: &str) -> Vec<String> {
    ["tc", "qdisc", "del", "dev", device, "root"]
        .iter()
        .map(|part| (*part).to_string())
        .collect()
}

/// Whether this guest can shape `device` now: Linux, `tc` on the PATH, root, and the device present. Says what is missing.
pub fn check(host: &dyn Host, device: &str) -> Result<(), String> {
    if !host.is_linux() {
        return Err("traffic shaping needs a Linux guest (tc netem)".into());
    }
    if !host.tool("tc") {
        return Err("traffic shaping needs tc on the guest's PATH; nothing is installed".into());
    }
    if !host.is_root() {
        return Err("traffic shaping needs root in the guest".into());
    }
    let sys = format!("/sys/class/net/{device}");
    if !host.exists(std::path::Path::new(&sys)) {
        return Err(format!(
            "traffic shaping: the device {device} is not in the guest"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::steps::tests::FakeHost;

    #[test]
    fn the_shaping_and_its_lifting_are_tc_commands_on_the_device() {
        assert_eq!(
            shaping("apz0", 50, 2).join(" "),
            "tc qdisc replace dev apz0 root netem delay 50ms loss 2%"
        );
        assert_eq!(lifting("apz0").join(" "), "tc qdisc del dev apz0 root");
    }

    #[test]
    fn a_guest_without_what_shaping_needs_says_what_is_missing() {
        let mac = FakeHost::default();
        assert!(check(&mac, "apz0").unwrap_err().contains("Linux"));
        let bare = FakeHost {
            linux: true,
            ..FakeHost::default()
        };
        assert!(check(&bare, "apz0").unwrap_err().contains("tc"));
        let unprivileged = FakeHost {
            linux: true,
            tools: vec!["tc".into()],
            ..FakeHost::default()
        };
        assert!(check(&unprivileged, "apz0").unwrap_err().contains("root"));
        let ready = FakeHost {
            linux: true,
            root: true,
            tools: vec!["tc".into()],
            paths: vec!["/sys/class/net/apz0".into()],
        };
        assert!(check(&ready, "apz0").is_ok());
        assert!(check(&ready, "apz9").unwrap_err().contains("apz9"));
    }
}
