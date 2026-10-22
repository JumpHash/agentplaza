//! The initial role fleet: the roles manifest and the agent VMs it stands up.
//!
//! `docs/agents/initial-roles.json` is the service's initial setup — one entry
//! per role, with its title, permits, toolsets and budget. The daemon boots one
//! long-lived VM per role from it (`noded --agents <manifest>`), in the
//! manifest's slot order, each with its own guest CID and each with
//! `plaza.role=<role> plaza.keepalive=1` on its kernel command line. A role's
//! CID is `cid_base + slot`, so a role keeps its CID when others are added; a
//! manifest with no slots falls back to alphabetical order (see [`Fleet::parse`]).
//!
//! Every check runs before a single VM is booted, so a manifest the daemon
//! cannot stand up is an error at startup rather than a half-built fleet: the
//! set of roles is read, each role's name is checked against the key it is
//! filed under, each name is checked for what a kernel command line cannot
//! carry, and the fleet's shape (count, memory, vCPUs, CID span) is checked
//! against the same limits a normal run obeys.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use vmm::spawner::{MAX_GUEST_CID, MIN_GUEST_CID};

use crate::registry::{MAX_VCPUS, MAX_VMS, MIN_MEMORY_MIB};

/// The roles manifest's shape: `{"source": "...", "fleet": ["<role>", ...], "roles": {"<role>": {...}}}`.
///
/// Unknown fields are ignored (the manifest carries provenance a daemon has no
/// use for) and the fields a role documents are required: a role the daemon
/// cannot read in full is malformed, not silently a role with fewer permits.
#[derive(Debug, Deserialize)]
struct Manifest {
    roles: BTreeMap<String, Role>,
    /// The roles that boot, when only some do: the shipped manifest boots the
    /// orchestrator alone, and project agents are spawned when a project needs
    /// them. Each keeps the CID its slot gives it. Absent, every role boots.
    #[serde(default)]
    fleet: Option<Vec<String>>,
}

/// One role, as the manifest documents it.
///
/// Every field here is *required* by the deserializer: a manifest that omits
/// one is refused as malformed rather than read as a role with less to it. The
/// daemon checks the role's name against its key (below) and does not yet read
/// the rest — a later slice that gives roles their permits and budgets in-guest
/// is what would.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct Role {
    /// The role's name; it must agree with the key it is filed under.
    role: String,
    /// The role's human-readable title.
    title: String,
    /// The statutes the role acts under.
    permits: Vec<String>,
    /// The toolsets the role may use.
    toolsets: Vec<String>,
    /// The role's budget.
    budget: u64,
    /// The role's variant, when it has one.
    #[serde(default)]
    variant: Option<String>,
    /// The role's slot: its guest CID is `cid_base + slot`. Optional, but
    /// either every role has one or none does (see [`place_roles`]).
    #[serde(default)]
    slot: Option<u32>,
}

/// Why a manifest could not be turned into a fleet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FleetError {
    /// The manifest could not be read: {path}: {reason}
    Read {
        /// The path that was tried.
        path: PathBuf,
        /// Why reading it failed.
        reason: String,
    },
    /// The manifest is not the roles document: {reason}
    Json {
        /// What the parser said.
        reason: String,
    },
    /// The manifest lists no roles
    Empty,
    /// The manifest's `fleet` names a role it does not define: {name}
    UnknownFleetRole {
        /// The role the fleet names.
        name: String,
    },
    /// The manifest's `fleet` is an empty list
    EmptyFleet,
    /// A role is filed under a key that disagrees with its own name: {key} vs {role}
    RoleMismatch {
        /// The key the role is filed under.
        key: String,
        /// The name the role gives itself.
        role: String,
    },
    /// A role name cannot go on a kernel command line: {name:?} {reason}
    BadRoleName {
        /// The name as the manifest spells it.
        name: String,
        /// Why it cannot be a `plaza.role=` value.
        reason: &'static str,
    },
    /// The fleet has more roles than one run may have VMs ({roles} > {max})
    TooManyRoles {
        /// How many roles the manifest lists.
        roles: usize,
        /// The cap one run obeys.
        max: usize,
    },
    /// Guest memory per agent VM must be at least {min} MiB (the manifest fleet was asked for {memory_mib})
    BadMemory {
        /// What the daemon was asked for.
        memory_mib: u32,
        /// The least a guest may be given.
        min: u32,
    },
    /// vCPUs per agent VM must be between 1 and {max} (the manifest fleet was asked for {vcpus})
    BadVcpus {
        /// What the daemon was asked for.
        vcpus: u32,
        /// The most a VM may have.
        max: u32,
    },
    /// The first agent CID must be at least {min} (1 and 2 are reserved; the manifest fleet was given {cid_base})
    BadCidBase {
        /// What the daemon was given.
        cid_base: u64,
        /// The first CID a guest may use.
        min: u64,
    },
    /// Two roles have the same slot: {first} and {second} both have slot {slot}
    DuplicateSlot {
        /// The slot both roles claim.
        slot: u32,
        /// The first role that claims it, in key order.
        first: String,
        /// The second role that claims it.
        second: String,
    },
    /// Some roles have a slot and others do not, so the fleet has no order: {missing:?} have none
    SlotsPartial {
        /// The roles with no slot, in key order.
        missing: Vec<String>,
    },
    /// The agents' guest CIDs run from {cid_base} to {last}, past the last guest CID {max}
    CidRange {
        /// The first CID.
        cid_base: u64,
        /// The highest CID the fleet would use.
        last: u64,
        /// The last CID the transport has.
        max: u64,
    },
}

impl fmt::Display for FleetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, reason } => {
                write!(formatter, "cannot read {}: {reason}", path.display())
            }
            Self::Json { reason } => write!(formatter, "the roles manifest is not valid: {reason}"),
            Self::Empty => write!(formatter, "the roles manifest lists no roles"),
            Self::UnknownFleetRole { name } => write!(
                formatter,
                "the roles manifest's fleet names {name:?}, which it does not define"
            ),
            Self::EmptyFleet => write!(
                formatter,
                "the roles manifest's fleet boots no role: name one, or leave --agents off"
            ),
            Self::RoleMismatch { key, role } => write!(
                formatter,
                "the role filed under {key:?} calls itself {role:?}"
            ),
            Self::BadRoleName { name, reason } => {
                write!(
                    formatter,
                    "role {name:?} cannot be a plaza.role= value: {reason}"
                )
            }
            Self::TooManyRoles { roles, max } => write!(
                formatter,
                "the manifest's {roles} roles are more than one run's {max} VMs"
            ),
            Self::BadMemory { memory_mib, min } => write!(
                formatter,
                "agent memory must be at least {min} MiB, not {memory_mib}"
            ),
            Self::BadVcpus { vcpus, max } => {
                write!(
                    formatter,
                    "agent vCPUs must be between 1 and {max}, not {vcpus}"
                )
            }
            Self::BadCidBase { cid_base, min } => {
                write!(
                    formatter,
                    "the first agent CID must be at least {min}, not {cid_base}"
                )
            }
            Self::DuplicateSlot {
                slot,
                first,
                second,
            } => write!(
                formatter,
                "roles {first:?} and {second:?} both have slot {slot}"
            ),
            Self::SlotsPartial { missing } => write!(
                formatter,
                "the roles {missing:?} have no slot, but others do: give every role a slot or none"
            ),
            Self::CidRange {
                cid_base,
                last,
                max,
            } => write!(
                formatter,
                "the agents' guest CIDs run from {cid_base} to {last}, past the last guest CID {max}"
            ),
        }
    }
}

impl std::error::Error for FleetError {}

/// One agent VM of the fleet: the role it stands for, the guest CID it is
/// given, and the command-line flags that make its guest a keepalive agent of
/// that role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentVm {
    /// The role's name.
    pub role: String,
    /// The VM's vsock guest CID (`cid_base + index`).
    pub guest_cid: u64,
    /// The VM's own kernel command-line flags — `plaza.role=<role>
    /// plaza.keepalive=1 plaza.vm=<index>` — which the run's console device is
    /// prepended to.
    pub flags: String,
}

/// A role fleet: the roles to boot, in the order they are booted, and the
/// shape of their VMs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fleet {
    /// The roles, in boot order (slot order; alphabetical without slots).
    roles: Vec<String>,
    /// Each role's CID offset from `cid_base`, in boot order: its slot, or its
    /// index when the manifest has no slots. Ascending, so the last is the highest.
    offsets: Vec<u64>,
    /// The first agent's guest CID (the base the offsets are added to).
    cid_base: u64,
    /// Guest memory per agent VM, in MiB.
    memory_mib: u32,
    /// vCPUs per agent VM.
    vcpus: u8,
    /// MiB usable per agent VM at boot, when the VMs get a balloon (`0`: none).
    balloon_mib: u32,
}

impl Fleet {
    /// Loads the roles manifest at `path` and builds the fleet it describes.
    pub fn load(
        path: &Path,
        cid_base: u64,
        memory_mib: u32,
        vcpus: u32,
    ) -> Result<Self, FleetError> {
        let text = std::fs::read_to_string(path).map_err(|err| FleetError::Read {
            path: path.to_path_buf(),
            reason: err.to_string(),
        })?;
        Self::parse(&text, cid_base, memory_mib, vcpus)
    }

    /// Builds the fleet from a manifest document.
    pub fn parse(
        text: &str,
        cid_base: u64,
        memory_mib: u32,
        vcpus: u32,
    ) -> Result<Self, FleetError> {
        let manifest: Manifest = serde_json::from_str(text).map_err(|err| FleetError::Json {
            reason: err.to_string(),
        })?;
        // `BTreeMap` iterates in key order: alphabetical by role, and stable
        // across runs and hosts. That is the boot order only when no role has
        // a slot (see `place_roles`).
        let mut entries = Vec::with_capacity(manifest.roles.len());
        for (key, role) in &manifest.roles {
            // A manifest that disagrees with itself would make the role the
            // hub sees depend on which of the two a reader believed.
            if role.role != *key {
                return Err(FleetError::RoleMismatch {
                    key: key.clone(),
                    role: role.role.clone(),
                });
            }
            if let Err(reason) = check_role_name(key) {
                return Err(FleetError::BadRoleName {
                    name: key.clone(),
                    reason,
                });
            }
            // The documented shape is required in full: the deserializer above
            // refuses a role that omits any of its fields, so reaching here
            // means the manifest documented this role completely.
            entries.push((key.clone(), role.slot));
        }
        if entries.is_empty() {
            return Err(FleetError::Empty);
        }
        // Every role is placed, so a role keeps its CID whether or not it boots; then only the fleet's roles are
        // kept.
        let placed = place_roles(entries)?;
        let placed = match &manifest.fleet {
            None => placed,
            Some(fleet) => {
                if let Some(name) = fleet
                    .iter()
                    .find(|name| !manifest.roles.contains_key(*name))
                {
                    return Err(FleetError::UnknownFleetRole { name: name.clone() });
                }
                let kept: Vec<(String, u64)> = placed
                    .into_iter()
                    .filter(|(name, _)| fleet.contains(name))
                    .collect();
                if kept.is_empty() {
                    return Err(FleetError::EmptyFleet);
                }
                kept
            }
        };
        if placed.len() > MAX_VMS as usize {
            return Err(FleetError::TooManyRoles {
                roles: placed.len(),
                max: MAX_VMS as usize,
            });
        }
        if memory_mib < MIN_MEMORY_MIB {
            return Err(FleetError::BadMemory {
                memory_mib,
                min: MIN_MEMORY_MIB,
            });
        }
        if !(1..=MAX_VCPUS).contains(&vcpus) {
            return Err(FleetError::BadVcpus {
                vcpus,
                max: MAX_VCPUS,
            });
        }
        let vcpus = u8::try_from(vcpus).map_err(|_| FleetError::BadVcpus {
            vcpus,
            max: MAX_VCPUS,
        })?;
        if cid_base < MIN_GUEST_CID {
            return Err(FleetError::BadCidBase {
                cid_base,
                min: MIN_GUEST_CID,
            });
        }
        // The offsets ascend, so the last is the highest CID the fleet uses.
        let (roles, offsets): (Vec<String>, Vec<u64>) = placed.into_iter().unzip();
        let highest = offsets.last().copied().unwrap_or(0);
        let last = cid_base.saturating_add(highest);
        if last > MAX_GUEST_CID {
            return Err(FleetError::CidRange {
                cid_base,
                last,
                max: MAX_GUEST_CID,
            });
        }
        Ok(Self {
            roles,
            offsets,
            cid_base,
            memory_mib,
            vcpus,
            balloon_mib: 0,
        })
    }

    /// The fleet's roles, in boot order.
    pub fn roles(&self) -> &[String] {
        &self.roles
    }

    /// The guest CID of the agent at `index` (boot order), if there is one.
    pub fn guest_cid(&self, index: usize) -> Option<u64> {
        self.offsets.get(index).map(|offset| self.cid_base + offset)
    }

    /// The fleet's VMs, in boot order.
    pub fn vms(&self) -> Vec<AgentVm> {
        self.roles
            .iter()
            .enumerate()
            .map(|(index, role)| AgentVm {
                role: role.clone(),
                guest_cid: self.cid_base + self.offsets[index],
                flags: agent_flags(role, index),
            })
            .collect()
    }

    /// Guest memory per agent VM, in MiB.
    pub fn memory_mib(&self) -> u32 {
        self.memory_mib
    }

    /// vCPUs per agent VM.
    pub fn vcpus(&self) -> u8 {
        self.vcpus
    }

    /// The fleet's VMs boot with a balloon: `usable_mib` MiB usable at boot and
    /// the rest of their memory held in the balloon. `0`, or no less than the
    /// VMs' memory, leaves the fleet without one.
    pub fn with_balloon(mut self, usable_mib: u32) -> Self {
        self.balloon_mib = usable_mib;
        self
    }

    /// The usable guest memory per agent VM at boot, when the VMs get a balloon.
    pub fn balloon_usable_mib(&self) -> Option<u32> {
        (self.balloon_mib > 0 && self.balloon_mib < self.memory_mib).then_some(self.balloon_mib)
    }
}

/// Orders the fleet's roles for boot and gives each its CID offset.
///
/// `entries` are the roles in key order, each with its slot when it has one.
/// With every role slotted the fleet boots in slot order and each role's
/// offset is its slot, so a role keeps its CID when roles are added. With no
/// slot at all the fleet is the legacy one: key order, offset = index. A mix
/// has no order that is not guesswork, so it is refused, and so is a slot two
/// roles share.
fn place_roles(entries: Vec<(String, Option<u32>)>) -> Result<Vec<(String, u64)>, FleetError> {
    let mut by_slot: BTreeMap<u32, String> = BTreeMap::new();
    let mut unslotted = Vec::new();
    for (name, slot) in entries {
        let Some(slot) = slot else {
            unslotted.push(name);
            continue;
        };
        if let Some(first) = by_slot.get(&slot) {
            return Err(FleetError::DuplicateSlot {
                slot,
                first: first.clone(),
                second: name,
            });
        }
        by_slot.insert(slot, name);
    }
    if by_slot.is_empty() {
        return Ok(unslotted
            .into_iter()
            .enumerate()
            .map(|(index, name)| (name, index as u64))
            .collect());
    }
    if !unslotted.is_empty() {
        return Err(FleetError::SlotsPartial { missing: unslotted });
    }
    Ok(by_slot
        .into_iter()
        .map(|(slot, name)| (name, u64::from(slot)))
        .collect())
}

/// The command-line flags that make VM `index` of a run a keepalive agent of
/// `role`: the role fleet's VMs and a roled run's VMs share them.
pub fn agent_flags(role: &str, index: usize) -> String {
    format!("plaza.role={role} plaza.keepalive=1 plaza.vm={index}")
}

/// Checks that `name` can be a `plaza.role=` value, and says why not when it
/// cannot.
///
/// The name is written into the guest's kernel command line, which is
/// whitespace-separated `key=value`: a name carrying either would not survive
/// the trip, and one carrying a control character could forge the command line,
/// the transcript or a log line.
fn check_role_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("it is empty");
    }
    if name.chars().any(char::is_whitespace) {
        return Err("it contains whitespace");
    }
    if name.chars().any(char::is_control) {
        return Err("it contains a control character");
    }
    if name.contains('=') {
        return Err("it contains '='");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One role entry, as the manifest writes it.
    fn role(name: &str) -> String {
        format!(
            r#"{{"role":"{name}","title":"The {name}","permits":["S-001"],"toolsets":["git"],"budget":1000,"variant":null}}"#
        )
    }

    /// A manifest over `names`, in the order given.
    fn manifest(names: &[&str]) -> String {
        let entries: Vec<String> = names
            .iter()
            .map(|name| format!("\"{name}\":{}", role(name)))
            .collect();
        format!(r#"{{"source":"test","roles":{{{}}}}}"#, entries.join(","))
    }

    /// The fleet's defaults for the tests that are not about its shape.
    fn fleet(text: &str) -> Result<Fleet, FleetError> {
        Fleet::parse(text, 100, MIN_MEMORY_MIB, 1)
    }

    /// The shipped manifest, as the daemon reads it at startup.
    fn shipped_manifest() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/agents/initial-roles.json")
    }

    /// One role with a slot, or without one, in a manifest over `names`.
    fn slotted(names: &[(&str, Option<u32>)]) -> String {
        let entries: Vec<String> = names
            .iter()
            .map(|(name, slot)| {
                let slot = slot.map(|slot| format!(r#","slot":{slot}"#)).unwrap_or_default();
                format!(
                    r#""{name}":{{"role":"{name}","title":"The {name}","permits":[],"toolsets":[],"budget":1,"variant":null{slot}}}"#
                )
            })
            .collect();
        format!(r#"{{"source":"test","roles":{{{}}}}}"#, entries.join(","))
    }

    /// Each role's guest CID in the fleet built from `text`.
    fn cids_by_role(text: &str) -> BTreeMap<String, u64> {
        fleet(text)
            .expect("a fleet")
            .vms()
            .into_iter()
            .map(|vm| (vm.role, vm.guest_cid))
            .collect()
    }

    #[test]
    fn the_shipped_manifest_boots_the_orchestrator_alone() {
        let fleet = Fleet::load(&shipped_manifest(), 100, 128, 1).expect("the shipped manifest");
        let booted: Vec<(String, u64)> = fleet
            .vms()
            .into_iter()
            .map(|vm| (vm.role, vm.guest_cid))
            .collect();
        // Its CID is the one its slot gives it, as when every role booted.
        assert_eq!(booted, [("orchestrator".to_string(), 109)]);
    }

    #[test]
    fn the_shipped_manifest_pins_each_role_to_its_slot() {
        // Every role, as it would boot without the `fleet` list.
        let text = std::fs::read_to_string(shipped_manifest()).expect("the shipped manifest");
        let mut doc: serde_json::Value = serde_json::from_str(&text).expect("JSON");
        doc.as_object_mut().expect("an object").remove("fleet");
        let fleet = Fleet::parse(&doc.to_string(), 100, 128, 1).expect("every role");
        // Slot order. Each role keeps the slot its manifest entry gives it, so
        // no CID moves when a role is retired: the slots 4, 7 and 8, which the
        // retired roles held, stay empty.
        let pinned: [(&str, u64); 14] = [
            ("architect", 0),
            ("auditor", 1),
            ("builder", 2),
            ("edge-operator", 3),
            ("incident", 5),
            ("librarian", 6),
            ("orchestrator", 9),
            ("planner", 10),
            ("qa", 11),
            ("release", 12),
            ("researcher", 13),
            ("reviewer", 14),
            ("security-reviewer", 15),
            ("breeder", 16),
        ];
        let booted: Vec<(String, u64)> = fleet
            .vms()
            .into_iter()
            .map(|vm| (vm.role, vm.guest_cid))
            .collect();
        let expected: Vec<(String, u64)> = pinned
            .iter()
            .map(|(role, slot)| (role.to_string(), 100 + slot))
            .collect();
        assert_eq!(booted, expected);
    }

    #[test]
    fn a_fleet_list_boots_only_its_roles_at_their_slots() {
        let roles = slotted(&[
            ("reviewer", Some(0)),
            ("builder", Some(5)),
            ("orchestrator", Some(2)),
        ]);
        let with = |fleet: &str| {
            roles.replacen(
                r#"{"source":"test","#,
                &format!(r#"{{"source":"test","fleet":{fleet},"#),
                1,
            )
        };
        let fleet = fleet(&with(r#"["builder","orchestrator"]"#)).expect("a fleet of two");
        assert_eq!(
            fleet.roles().iter().map(String::as_str).collect::<Vec<_>>(),
            ["orchestrator", "builder"]
        );
        assert_eq!(fleet.guest_cid(0), Some(102));
        assert_eq!(fleet.guest_cid(1), Some(105));
        assert_eq!(
            fleet.vms()[1].flags,
            "plaza.role=builder plaza.keepalive=1 plaza.vm=1"
        );
        assert_eq!(
            super::Fleet::parse(&with(r#"["auditor"]"#), 100, MIN_MEMORY_MIB, 1),
            Err(FleetError::UnknownFleetRole {
                name: "auditor".into()
            })
        );
        assert_eq!(
            super::Fleet::parse(&with("[]"), 100, MIN_MEMORY_MIB, 1),
            Err(FleetError::EmptyFleet)
        );
    }

    #[test]
    fn slots_set_the_cids_and_the_boot_order() {
        let text = slotted(&[
            ("reviewer", Some(0)),
            ("builder", Some(5)),
            ("auditor", Some(2)),
        ]);
        let fleet = fleet(&text).expect("a slotted fleet");
        // Boot order is slot order, not key order, and the CID is base + slot.
        assert_eq!(
            fleet.roles().iter().map(String::as_str).collect::<Vec<_>>(),
            ["reviewer", "auditor", "builder"]
        );
        assert_eq!(fleet.guest_cid(0), Some(100));
        assert_eq!(fleet.guest_cid(1), Some(102));
        assert_eq!(fleet.guest_cid(2), Some(105));
        assert_eq!(fleet.guest_cid(3), None);
        assert_eq!(
            fleet.vms()[2].flags,
            "plaza.role=builder plaza.keepalive=1 plaza.vm=2"
        );
    }

    #[test]
    fn a_role_added_later_keeps_the_cids_of_the_others() {
        let before = slotted(&[("builder", Some(0)), ("reviewer", Some(1)), ("qa", Some(2))]);
        // A new role that sorts first alphabetically, at the next free slot.
        let after = slotted(&[
            ("aaa-new", Some(3)),
            ("builder", Some(0)),
            ("reviewer", Some(1)),
            ("qa", Some(2)),
        ]);
        let before = cids_by_role(&before);
        let after = cids_by_role(&after);
        assert_eq!(after["aaa-new"], 103);
        for (role, cid) in &before {
            assert_eq!(after[role], *cid, "{role} moved");
        }
    }

    #[test]
    fn a_role_added_without_slots_would_shift_the_others() {
        // The legacy manifest's order: adding a role that sorts first moves
        // every role after it. This is what slots exist to prevent.
        let before = cids_by_role(&slotted(&[("builder", None), ("reviewer", None)]));
        let after = cids_by_role(&slotted(&[
            ("aaa-new", None),
            ("builder", None),
            ("reviewer", None),
        ]));
        assert_eq!(before["builder"], 100);
        assert_eq!(after["builder"], 101);
    }

    #[test]
    fn bad_slot_assignments_are_refused() {
        // Two roles cannot share a slot; the error names both roles and the slot.
        {
            let text = slotted(&[("builder", Some(4)), ("reviewer", Some(4))]);
            let err = fleet(&text).expect_err("two roles cannot share a slot");
            assert_eq!(
                err,
                FleetError::DuplicateSlot {
                    slot: 4,
                    first: "builder".to_string(),
                    second: "reviewer".to_string(),
                }
            );
            assert_eq!(
                err.to_string(),
                "roles \"builder\" and \"reviewer\" both have slot 4"
            );
        }
        // A manifest where only some roles have a slot is refused, naming the rest.
        {
            let text = slotted(&[("builder", Some(0)), ("reviewer", None), ("qa", None)]);
            assert_eq!(
                fleet(&text),
                Err(FleetError::SlotsPartial {
                    missing: vec!["qa".to_string(), "reviewer".to_string()],
                })
            );
        }
        // A slot past the transport's last guest CID is refused.
        {
            let text = slotted(&[("builder", Some(u32::MAX))]);
            let err = fleet(&text).expect_err("that slot is past every guest CID");
            assert!(
                matches!(err, FleetError::CidRange { cid_base: 100, .. }),
                "{err:?}"
            );
        }
    }

    #[test]
    fn every_role_becomes_a_keepalive_vm_with_its_own_cid() {
        let fleet = fleet(&manifest(&["reviewer", "builder"])).expect("a two-role manifest");
        // Alphabetical order decides the boot order and therefore the CIDs.
        assert_eq!(
            fleet.vms(),
            vec![
                AgentVm {
                    role: "builder".to_string(),
                    guest_cid: 100,
                    flags: "plaza.role=builder plaza.keepalive=1 plaza.vm=0".to_string(),
                },
                AgentVm {
                    role: "reviewer".to_string(),
                    guest_cid: 101,
                    flags: "plaza.role=reviewer plaza.keepalive=1 plaza.vm=1".to_string(),
                },
            ]
        );
        assert_eq!(fleet.guest_cid(0), Some(100));
        assert_eq!(fleet.guest_cid(1), Some(101));
        assert_eq!(fleet.guest_cid(2), None);
        assert_eq!(fleet.memory_mib(), MIN_MEMORY_MIB);
        assert_eq!(fleet.vcpus(), 1);
    }

    #[test]
    fn the_balloon_is_on_only_below_the_vms_memory() {
        let text = manifest(&["builder"]);
        let plain = Fleet::parse(&text, 100, 128, 1).expect("a one-role fleet");
        assert_eq!(plain.balloon_usable_mib(), None);
        assert_eq!(
            plain.clone().with_balloon(32).balloon_usable_mib(),
            Some(32)
        );
        assert_eq!(plain.clone().with_balloon(0).balloon_usable_mib(), None);
        assert_eq!(plain.with_balloon(128).balloon_usable_mib(), None);
    }

    #[test]
    fn a_malformed_roles_manifest_is_refused() {
        // A manifest that is not JSON is reported as a JSON error.
        {
            let err = fleet("not json at all").expect_err("that is not a manifest");
            assert!(matches!(err, FleetError::Json { .. }), "{err:?}");
        }
        // A role missing its documented fields is refused.
        {
            // No title: the manifest does not document this role, so it is
            // malformed rather than a role with a missing field.
            let text =
                r#"{"roles":{"builder":{"role":"builder","permits":[],"toolsets":[],"budget":1}}}"#;
            assert!(matches!(fleet(text), Err(FleetError::Json { .. })));
            // No budget either.
            let text = r#"{"roles":{"builder":{"role":"builder","title":"Builder","permits":[],"toolsets":[]}}}"#;
            assert!(matches!(fleet(text), Err(FleetError::Json { .. })));
        }
        // A manifest with no roles is refused.
        {
            let err = fleet(r#"{"roles":{}}"#).expect_err("no roles is not a fleet");
            assert_eq!(err, FleetError::Empty);
            assert_eq!(err.to_string(), "the roles manifest lists no roles");
        }
        // A role that disagrees with the key it is filed under is refused.
        {
            let text = r#"{"roles":{"builder":{"role":"reviewer","title":"Builder","permits":[],"toolsets":[],"budget":1}}}"#;
            let err = fleet(text).expect_err("the manifest disagrees with itself");
            assert_eq!(
                err,
                FleetError::RoleMismatch {
                    key: "builder".to_string(),
                    role: "reviewer".to_string(),
                }
            );
        }
    }

    #[test]
    fn a_role_name_unfit_for_the_command_line_is_refused() {
        // A name that cannot go on a command line (empty, whitespace, or '=')
        // is refused, with the reason.
        {
            for (name, reason) in [
                ("", "it is empty"),
                ("two words", "it contains whitespace"),
                ("role=other", "it contains '='"),
            ] {
                let text = manifest(&[name]);
                let err = fleet(&text).expect_err("that name cannot be a command-line value");
                assert_eq!(
                    err,
                    FleetError::BadRoleName {
                        name: name.to_string(),
                        reason,
                    },
                    "{name:?}"
                );
            }
        }
        // A role name with an invisible character is refused.
        {
            // JSON carries a control character as an escape, so a manifest can name
            // a role that would be invisible in a log line (or split a command
            // line); the name arrives intact and the check is what refuses it.
            let text = "{\"roles\":{\"role\\u001b[2K\":{\"role\":\"role\\u001b[2K\",\
                        \"title\":\"t\",\"permits\":[],\"toolsets\":[],\"budget\":1}}}";
            let err = fleet(text).expect_err("an invisible role name is not a role name");
            assert_eq!(
                err,
                FleetError::BadRoleName {
                    name: "role\u{1b}[2K".to_string(),
                    reason: "it contains a control character",
                }
            );
            assert_eq!(
                err.to_string(),
                "role \"role\\u{1b}[2K\" cannot be a plaza.role= value: it contains a control character"
            );
        }
    }

    #[test]
    fn a_fleet_outside_the_parse_limits_is_refused() {
        // A fleet that does not fit the transport is refused before anything boots.
        {
            // The last CID (base + roles - 1) must still be a guest CID.
            let text = manifest(&["builder", "reviewer"]);
            let err = Fleet::parse(&text, MAX_GUEST_CID, 128, 1)
                .expect_err("the second agent runs past the last CID");
            assert!(matches!(err, FleetError::CidRange { .. }), "{err:?}");
            assert!(Fleet::parse(&text, MAX_GUEST_CID - 1, 128, 1).is_ok());

            // CIDs 0, 1 and 2 belong to the vsock stack, not to guests.
            assert!(matches!(
                Fleet::parse(&text, 2, 128, 1),
                Err(FleetError::BadCidBase {
                    cid_base: 2,
                    min: 3
                })
            ));
        }
        // A fleet larger than one run is refused; exactly the cap is a fleet.
        {
            let names: Vec<String> = (0..=MAX_VMS).map(|n| format!("role-{n}")).collect();
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            let err = fleet(&manifest(&names)).expect_err("more roles than a run has VMs");
            assert_eq!(
                err,
                FleetError::TooManyRoles {
                    roles: MAX_VMS as usize + 1,
                    max: MAX_VMS as usize,
                }
            );
            // And exactly the cap is a fleet.
            let names: Vec<String> = (0..MAX_VMS).map(|n| format!("role-{n}")).collect();
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            assert!(fleet(&manifest(&names)).is_ok());
        }
        // A guest shape outside the contract's ranges is refused.
        {
            let text = manifest(&["builder"]);
            assert_eq!(
                Fleet::parse(&text, 100, 127, 1),
                Err(FleetError::BadMemory {
                    memory_mib: 127,
                    min: 128
                })
            );
            assert_eq!(
                Fleet::parse(&text, 100, 128, 0),
                Err(FleetError::BadVcpus { vcpus: 0, max: 64 })
            );
            assert_eq!(
                Fleet::parse(&text, 100, 128, 65),
                Err(FleetError::BadVcpus { vcpus: 65, max: 64 })
            );
            assert!(Fleet::parse(&text, 100, 128, 64).is_ok());
        }
    }
}
