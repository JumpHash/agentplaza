//! The node's vsock relay contract: how a workstation VM's daemon reaches its hub when the VM has no NIC (rule 0010).
//!
//! The guest connects to the host (CID [`HOST_CID`]) on a vsock port. A plaza VM's connection reaches the node as a
//! Unix-socket connection, on `{uds}_{port}`, which the VMM's muxer makes; a QEMU VM's reaches the node on the host's
//! AF_VSOCK listener. Either way the node's relay forwards its bytes to the hub, byte for byte. The daemon's TLS is end
//! to end with the hub: the relay never terminates it.
//!
//! The daemon's hub address may be `vsock://<cid>:<port>` (see [`parse_vsock`]). Such a hub is always pinned, with
//! the TLS name [`TLS_NAME`], because a vsock address names no host.
//!
//! The join config is not on the kernel command line (`/proc/cmdline` is readable in the guest). The node serves it
//! once, as [`NodeConfig`] JSON, on [`CONFIG_PORT`].

use serde::{Deserialize, Serialize};

/// The CID a guest uses for its host (`VMADDR_CID_HOST`).
pub const HOST_CID: u32 = 2;

/// The vsock port the station link is relayed on.
pub const LINK_PORT: u32 = 5100;

/// The vsock port the one-shot join config is served on.
pub const CONFIG_PORT: u32 = 5101;

/// The scheme of a vsock address.
pub const SCHEME: &str = "vsock://";

/// The TLS name a vsock hub is pinned under. The pin, not the name, is what is verified.
pub const TLS_NAME: &str = "hub";

/// The address a VM's daemon dials for its station link: the host, on [`LINK_PORT`].
pub fn link_address() -> String {
    format!("{SCHEME}{HOST_CID}:{LINK_PORT}")
}

/// The address the join config is read from: the host, on [`CONFIG_PORT`].
pub fn config_address() -> String {
    format!("{SCHEME}{HOST_CID}:{CONFIG_PORT}")
}

/// Reads `vsock://<cid>:<port>`: both numbers in decimal, nothing else. `None` for any other text.
pub fn parse_vsock(text: &str) -> Option<(u32, u32)> {
    let rest = text.strip_prefix(SCHEME)?;
    let (cid, port) = rest.split_once(':')?;
    if cid.is_empty()
        || port.is_empty()
        || !cid.bytes().all(|b| b.is_ascii_digit())
        || !port.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    Some((cid.parse().ok()?, port.parse().ok()?))
}

/// What the node hands a workstation VM's join, once: the hub's address as the VM dials it (a [`link_address`]), the
/// hub's certificate pin (base64url), and the join code.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeConfig {
    pub hub: String,
    pub pin: String,
    pub code: String,
}

impl std::fmt::Debug for NodeConfig {
    /// The code is a credential until it is used: it never reaches a log through `Debug`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeConfig")
            .field("hub", &self.hub)
            .field("pin", &self.pin)
            .field("code", &"<redacted>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vsock_address_is_a_cid_and_a_port_and_nothing_else() {
        assert_eq!(parse_vsock("vsock://2:5100"), Some((2, 5100)));
        assert_eq!(parse_vsock("vsock://1001:65535"), Some((1001, 65535)));
        for bad in [
            "vsock://",
            "vsock://2",
            "vsock://2:",
            "vsock://:5100",
            "vsock://2:5100/x",
            "vsock://-1:5100",
            "vsock://+2:5100",
            "vsock://2:51 00",
            "vsock://2:5100:1",
            "https://2:5100",
            "vsock://99999999999999:1",
            " vsock://2:5100",
        ] {
            assert_eq!(parse_vsock(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_node_addresses_are_the_host_on_the_relay_ports() {
        assert_eq!(link_address(), "vsock://2:5100");
        assert_eq!(config_address(), "vsock://2:5101");
        assert_eq!(parse_vsock(&link_address()), Some((HOST_CID, LINK_PORT)));
        assert_eq!(
            parse_vsock(&config_address()),
            Some((HOST_CID, CONFIG_PORT))
        );
    }

    #[test]
    fn the_config_round_trips_as_json_and_its_debug_form_hides_the_code() {
        let config = NodeConfig {
            hub: link_address(),
            pin: "cGlu".into(),
            code: "apzj1.secret".into(),
        };
        let text = serde_json::to_string(&config).expect("json");
        assert_eq!(
            text,
            r#"{"hub":"vsock://2:5100","pin":"cGlu","code":"apzj1.secret"}"#
        );
        let back: NodeConfig = serde_json::from_str(&text).expect("back");
        assert_eq!(back, config);
        let debug = format!("{config:?}");
        assert!(!debug.contains("apzj1.secret"), "{debug}");
        assert!(debug.contains("redacted"), "{debug}");
    }
}
