// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port (PLAN 22.6, W58). A segment's addresses are laid out once, by the topology's
// planner, and every VM of the topology gets its interface, its name and its route from the same plan.
// The guest's default route goes to the segment's gateway and nowhere else, so the egress proxy is the
// only way out: a guest that tries to reach anything else sends it to the gateway, which refuses it.

//! A segment's plan: each VM's name, address and MAC, the hosts file that resolves the names, and the
//! commands that bring a guest's interface up.

use std::collections::BTreeSet;
use std::net::Ipv4Addr;

use super::Mac;

/// The name the gateway answers to in the hosts file (the egress proxy's address).
pub const GATEWAY_NAME: &str = "egress";

/// Most members a segment holds: the host part of a /24, less the network, the broadcast and the gateway.
pub const MAX_MEMBERS: usize = 252;

/// One interface of the segment: a VM's name, its address and its MAC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// The VM's name in the topology (a hostname).
    pub name: String,
    /// The VM's address on the segment.
    pub ip: Ipv4Addr,
    /// The MAC the VM's device presents.
    pub mac: Mac,
}

/// A segment's layout: its members and its gateway, in the /24 `10.<index>.0.0/24`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The segment's name.
    pub segment: String,
    /// The members, in the order they were named.
    pub members: Vec<Member>,
    /// The gateway: the egress proxy's interface.
    pub gateway: Member,
}

/// Whether `name` is a hostname the hosts file can carry: lowercase letters, digits and `-`, not
/// starting or ending with `-`, at most 63 characters.
pub fn is_hostname(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The MAC of the interface with host number `host` in the segment with number `index`: a locally
/// administered address, unique per (segment, host).
fn mac_for(index: u8, host: u8) -> Mac {
    [0x02, 0x41, 0x50, index, 0x00, host]
}

impl Plan {
    /// Lays out a segment named `segment` with number `index` (1 to 254: its subnet is `10.<index>.0.0/24`)
    /// and the VMs `names`. The gateway is `.1`, the first VM `.2`, and so on. Errors name the problem:
    /// an index out of range, a name that is not a hostname or is taken twice, or too many VMs.
    pub fn new(segment: &str, index: u8, names: &[&str]) -> Result<Self, String> {
        if !(1..=254).contains(&index) {
            return Err(format!(
                "a segment's number is 1 to 254, not {index}: {segment:?}"
            ));
        }
        if names.len() > MAX_MEMBERS {
            return Err(format!(
                "a segment holds at most {MAX_MEMBERS} VMs, not {}",
                names.len()
            ));
        }
        let mut seen = BTreeSet::new();
        let mut members = Vec::with_capacity(names.len());
        for (offset, name) in names.iter().enumerate() {
            if !is_hostname(name) {
                return Err(format!("{name:?} is not a hostname"));
            }
            if name == &GATEWAY_NAME || !seen.insert(*name) {
                return Err(format!(
                    "the name {name:?} is taken twice or is the gateway's"
                ));
            }
            let host = u8::try_from(offset + 2).map_err(|_| "too many VMs".to_owned())?;
            members.push(Member {
                name: (*name).to_owned(),
                ip: Ipv4Addr::new(10, index, 0, host),
                mac: mac_for(index, host),
            });
        }
        Ok(Self {
            segment: segment.to_owned(),
            members,
            gateway: Member {
                name: GATEWAY_NAME.to_owned(),
                ip: Ipv4Addr::new(10, index, 0, 1),
                mac: mac_for(index, 1),
            },
        })
    }

    /// The member called `name`.
    pub fn member(&self, name: &str) -> Option<&Member> {
        self.members.iter().find(|member| member.name == name)
    }

    /// The hosts file every VM of the segment gets: each member and the gateway by name.
    pub fn hosts_file(&self) -> String {
        let mut out = String::new();
        for member in std::iter::once(&self.gateway).chain(&self.members) {
            out.push_str(&format!("{} {}\n", member.ip, member.name));
        }
        out
    }

    /// The commands that bring the interface `device` of the VM `name` up: its address, the gateway's
    /// neighbour entry (the switch does not answer ARP for the gateway, so it is set statically) and the
    /// default route through the gateway. `None` when the VM is not in the plan.
    pub fn guest_commands(&self, name: &str, device: &str) -> Option<Vec<Vec<String>>> {
        let member = self.member(name)?;
        let words = |items: &[&str]| {
            items
                .iter()
                .map(|item| (*item).to_owned())
                .collect::<Vec<_>>()
        };
        let gateway = self.gateway.ip.to_string();
        let gateway_mac = super::mac_text(&self.gateway.mac);
        Some(vec![
            words(&[
                "ip",
                "addr",
                "add",
                &format!("{}/24", member.ip),
                "dev",
                device,
            ]),
            words(&["ip", "link", "set", device, "up"]),
            words(&[
                "ip",
                "neigh",
                "replace",
                &gateway,
                "lladdr",
                &gateway_mac,
                "dev",
                device,
                "nud",
                "permanent",
            ]),
            words(&[
                "ip", "route", "add", "default", "via", &gateway, "dev", device,
            ]),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn members_get_consecutive_addresses_after_the_gateway_and_unique_macs() {
        let plan = Plan::new("web", 7, &["api", "db"]).unwrap();
        assert_eq!(plan.gateway.ip, Ipv4Addr::new(10, 7, 0, 1));
        assert_eq!(plan.member("api").unwrap().ip, Ipv4Addr::new(10, 7, 0, 2));
        assert_eq!(plan.member("db").unwrap().ip, Ipv4Addr::new(10, 7, 0, 3));
        let macs: BTreeSet<Mac> = plan
            .members
            .iter()
            .map(|m| m.mac)
            .chain([plan.gateway.mac])
            .collect();
        assert_eq!(macs.len(), 3, "every interface has its own MAC");
    }

    #[test]
    fn the_hosts_file_resolves_every_name_in_the_segment() {
        let plan = Plan::new("web", 7, &["api", "db"]).unwrap();
        let hosts = plan.hosts_file();
        assert!(hosts.contains("10.7.0.2 api\n"));
        assert!(hosts.contains("10.7.0.3 db\n"));
        assert!(hosts.contains("10.7.0.1 egress\n"));
    }

    #[test]
    fn the_guest_routes_everything_through_the_gateway_and_nothing_else() {
        let plan = Plan::new("web", 7, &["api"]).unwrap();
        let commands = plan.guest_commands("api", "eth0").unwrap();
        let route = commands
            .iter()
            .find(|c| c.contains(&"route".to_owned()))
            .unwrap();
        assert_eq!(
            route,
            &[
                "ip", "route", "add", "default", "via", "10.7.0.1", "dev", "eth0"
            ]
            .map(str::to_owned)
            .to_vec()
        );
        assert!(plan.guest_commands("nobody", "eth0").is_none());
    }

    #[test]
    fn a_bad_name_a_repeat_a_gateway_name_or_a_bad_number_is_refused() {
        assert!(Plan::new("web", 0, &["a"]).is_err());
        assert!(Plan::new("web", 255, &["a"]).is_err());
        assert!(
            Plan::new("web", 1, &["Api"]).is_err(),
            "uppercase is not a hostname"
        );
        assert!(Plan::new("web", 1, &["-api"]).is_err());
        assert!(Plan::new("web", 1, &["api", "api"]).is_err());
        assert!(Plan::new("web", 1, &[GATEWAY_NAME]).is_err());
        let too_many: Vec<String> = (0..=MAX_MEMBERS).map(|i| format!("vm{i}")).collect();
        let refs: Vec<&str> = too_many.iter().map(String::as_str).collect();
        assert!(Plan::new("web", 1, &refs).is_err());
    }
}
