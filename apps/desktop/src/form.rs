//! The spawn form: five text fields, and the validation that mirrors
//! `noded`'s (`crates/noded/src/registry.rs`, `CreateRun::spec`).
//!
//! The daemon is the authority; this only spares the operator a round trip for
//! a request it would refuse, with the same ranges and the same wording, so a
//! refusal reads the same whether the dashboard or the daemon said it.

use crate::api::CreateRun;

/// The most VMs one run may boot (`noded::registry::MAX_VMS`).
pub const MAX_VMS: u32 = 64;

/// The most vCPUs one VM may have (`noded::registry::MAX_VCPUS`).
pub const MAX_VCPUS: u32 = 64;

/// The least guest memory one VM may have, in MiB
/// (`noded::registry::MIN_MEMORY_MIB`).
pub const MIN_MEMORY_MIB: u32 = 128;

/// The first guest CID a run's vsock devices may use: 1 and 2 are reserved
/// (`vmm::spawner::MIN_GUEST_CID`).
pub const MIN_GUEST_CID: u64 = 3;

/// The last usable guest CID: the kernel refuses `VMADDR_CID_ANY` and above
/// (`vmm::spawner::MAX_GUEST_CID`).
pub const MAX_GUEST_CID: u64 = u32::MAX as u64 - 1;

/// One field of the spawn form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    /// How many VMs to boot.
    N,
    /// vCPUs per VM.
    Vcpus,
    /// Guest memory per VM, in MiB.
    MemoryMib,
    /// Heartbeats before the guest reboots itself.
    Beats,
    /// Base guest CID for the run's vsock devices.
    VsockCid,
}

impl Field {
    /// Every field, in the order the form shows them.
    pub const ALL: [Field; 5] = [
        Field::N,
        Field::Vcpus,
        Field::MemoryMib,
        Field::Beats,
        Field::VsockCid,
    ];

    /// The field's name, as the API and the daemon's messages use it.
    pub fn name(self) -> &'static str {
        match self {
            Field::N => "n",
            Field::Vcpus => "vcpus",
            Field::MemoryMib => "memory_mib",
            Field::Beats => "beats",
            Field::VsockCid => "vsock_cid",
        }
    }

    /// The range or meaning the form shows beside the field.
    pub fn hint(self) -> &'static str {
        match self {
            Field::N => "1–64 VMs",
            Field::Vcpus => "1–64 vCPUs per VM",
            Field::MemoryMib => "≥ 128 MiB per VM",
            Field::Beats => "blank runs long-lived",
            Field::VsockCid => "≥ 3, blank boots without vsock",
        }
    }
}

/// The spawn form's raw input: what the operator typed, before validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnForm {
    /// The raw `n` field.
    pub n: String,
    /// The raw `vcpus` field.
    pub vcpus: String,
    /// The raw `memory_mib` field.
    pub memory_mib: String,
    /// The raw `beats` field.
    pub beats: String,
    /// The raw `vsock_cid` field.
    pub vsock_cid: String,
}

impl Default for SpawnForm {
    fn default() -> Self {
        Self {
            n: "1".to_string(),
            vcpus: "2".to_string(),
            memory_mib: "512".to_string(),
            beats: String::new(),
            vsock_cid: String::new(),
        }
    }
}

impl SpawnForm {
    /// The raw text of one field.
    pub fn field(&self, field: Field) -> &str {
        match field {
            Field::N => &self.n,
            Field::Vcpus => &self.vcpus,
            Field::MemoryMib => &self.memory_mib,
            Field::Beats => &self.beats,
            Field::VsockCid => &self.vsock_cid,
        }
    }

    /// Sets one field from raw text.
    pub fn set(&mut self, field: Field, value: String) {
        match field {
            Field::N => self.n = value,
            Field::Vcpus => self.vcpus = value,
            Field::MemoryMib => self.memory_mib = value,
            Field::Beats => self.beats = value,
            Field::VsockCid => self.vsock_cid = value,
        }
    }

    /// The request the form describes, or every field the daemon would refuse.
    ///
    /// Errors come back in [`Field::ALL`] order so the form shows them in a
    /// stable place.
    pub fn validate(&self) -> Result<CreateRun, Vec<(Field, String)>> {
        let mut errors = Vec::new();

        let n = self.count(Field::N, 1, MAX_VMS, &mut errors);
        let vcpus = self.count(Field::Vcpus, 1, MAX_VCPUS, &mut errors);
        let memory_mib = self.count_floor(Field::MemoryMib, MIN_MEMORY_MIB, &mut errors);
        let beats = self.optional_count(Field::Beats, &mut errors);
        let vsock_cid = self.vsock_cid(n, &mut errors);

        if !errors.is_empty() {
            return Err(errors);
        }
        Ok(CreateRun {
            // The defaults never show: every parse that returned `None` also
            // pushed an error, and a non-empty error list returned above.
            n: n.unwrap_or(0),
            vcpus: vcpus.unwrap_or(0),
            memory_mib: memory_mib.unwrap_or(0),
            beats,
            vsock_cid,
        })
    }

    /// A required field inside `min..=max`, with the daemon's wording.
    fn count(
        &self,
        field: Field,
        min: u32,
        max: u32,
        errors: &mut Vec<(Field, String)>,
    ) -> Option<u32> {
        match self.parse_u32(field, errors) {
            None => None,
            Some(value) if (min..=max).contains(&value) => Some(value),
            Some(_) => {
                errors.push((
                    field,
                    format!("{} must be between {min} and {max}", field.name()),
                ));
                None
            }
        }
    }

    /// A required field with a lower bound only, with the daemon's wording.
    fn count_floor(
        &self,
        field: Field,
        min: u32,
        errors: &mut Vec<(Field, String)>,
    ) -> Option<u32> {
        match self.parse_u32(field, errors) {
            None => None,
            Some(value) if value >= min => Some(value),
            Some(_) => {
                errors.push((
                    field,
                    format!("{} must be at least {min}", field.name()),
                ));
                None
            }
        }
    }

    /// A field that may be left blank.
    fn optional_count(&self, field: Field, errors: &mut Vec<(Field, String)>) -> Option<u32> {
        if self.field(field).trim().is_empty() {
            return None;
        }
        self.parse_u32(field, errors)
    }

    /// The vsock CID, checked against the guest CID space and against `n`: a
    /// run's last VM's CID is `vsock_cid + n - 1`, and every one must fit.
    fn vsock_cid(&self, n: Option<u32>, errors: &mut Vec<(Field, String)>) -> Option<u64> {
        let field = Field::VsockCid;
        let raw = self.field(field).trim();
        if raw.is_empty() {
            return None;
        }
        let cid = match raw.parse::<u64>() {
            Ok(cid) => cid,
            Err(_) => {
                errors.push((field, format!("{} must be a whole number", field.name())));
                return None;
            }
        };
        if cid < MIN_GUEST_CID {
            errors.push((
                field,
                format!(
                    "{} must be at least {MIN_GUEST_CID} (1 and 2 are reserved)",
                    field.name()
                ),
            ));
            return None;
        }
        if cid > MAX_GUEST_CID {
            errors.push((
                field,
                format!(
                    "{} must be at most {MAX_GUEST_CID}: the kernel refuses \
                     VMADDR_CID_ANY and above",
                    field.name()
                ),
            ));
            return None;
        }
        if let Some(n) = n {
            let last = cid + u64::from(n) - 1;
            if last > MAX_GUEST_CID {
                errors.push((
                    field,
                    format!(
                        "{} and n together run past the last guest CID",
                        field.name()
                    ),
                ));
                return None;
            }
        }
        Some(cid)
    }

    /// A whole number, or an error in the daemon's voice.
    fn parse_u32(&self, field: Field, errors: &mut Vec<(Field, String)>) -> Option<u32> {
        let raw = self.field(field).trim();
        match raw.parse::<u32>() {
            Ok(value) => Some(value),
            Err(_) => {
                errors.push((field, format!("{} must be a whole number", field.name())));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(n: &str, vcpus: &str, memory: &str, beats: &str, cid: &str) -> SpawnForm {
        SpawnForm {
            n: n.to_string(),
            vcpus: vcpus.to_string(),
            memory_mib: memory.to_string(),
            beats: beats.to_string(),
            vsock_cid: cid.to_string(),
        }
    }

    fn errors_of(form: &SpawnForm) -> Vec<(Field, String)> {
        form.validate().expect_err("the form should be refused")
    }

    #[test]
    fn the_defaults_are_a_request_the_daemon_accepts() {
        let create = SpawnForm::default().validate().expect("valid");
        assert_eq!(
            create,
            CreateRun {
                n: 1,
                vcpus: 2,
                memory_mib: 512,
                beats: None,
                vsock_cid: None,
            }
        );
    }

    #[test]
    fn a_full_form_becomes_the_request() {
        let create = form("4", "8", "1024", "12", "50").validate().expect("valid");
        assert_eq!(
            create,
            CreateRun {
                n: 4,
                vcpus: 8,
                memory_mib: 1024,
                beats: Some(12),
                vsock_cid: Some(50),
            }
        );
    }

    #[test]
    fn surrounding_whitespace_is_not_a_field_value() {
        let create = form(" 2 ", " 4 ", " 256 ", " ", " 7 ").validate().expect("valid");
        assert_eq!(create.n, 2);
        assert_eq!(create.vcpus, 4);
        assert_eq!(create.memory_mib, 256);
        assert_eq!(create.beats, None);
        assert_eq!(create.vsock_cid, Some(7));
    }

    #[test]
    fn the_vm_count_range_is_the_daemons() {
        assert_eq!(form("1", "1", "128", "", "").validate().expect("n=1").n, 1);
        assert_eq!(form("64", "1", "128", "", "").validate().expect("n=64").n, 64);
        assert_eq!(
            errors_of(&form("0", "1", "128", "", ""))[0],
            (Field::N, "n must be between 1 and 64".to_string())
        );
        assert_eq!(
            errors_of(&form("65", "1", "128", "", ""))[0],
            (Field::N, "n must be between 1 and 64".to_string())
        );
    }

    #[test]
    fn the_vcpu_range_is_the_daemons() {
        assert_eq!(form("1", "64", "128", "", "").validate().expect("ok").vcpus, 64);
        assert_eq!(
            errors_of(&form("1", "0", "128", "", ""))[0],
            (Field::Vcpus, "vcpus must be between 1 and 64".to_string())
        );
        assert_eq!(
            errors_of(&form("1", "65", "128", "", ""))[0],
            (Field::Vcpus, "vcpus must be between 1 and 64".to_string())
        );
    }

    #[test]
    fn the_memory_floor_is_the_daemons() {
        assert_eq!(
            form("1", "1", "128", "", "").validate().expect("ok").memory_mib,
            128
        );
        assert_eq!(
            errors_of(&form("1", "1", "127", "", ""))[0],
            (Field::MemoryMib, "memory_mib must be at least 128".to_string())
        );
    }

    #[test]
    fn a_text_field_that_is_not_a_number_is_refused() {
        assert_eq!(
            errors_of(&form("two", "1", "128", "", ""))[0],
            (Field::N, "n must be a whole number".to_string())
        );
        assert_eq!(
            errors_of(&form("1", "1", "lots", "", ""))[0],
            (Field::MemoryMib, "memory_mib must be a whole number".to_string())
        );
        assert_eq!(
            errors_of(&form("1", "1", "128", "many", ""))[0],
            (Field::Beats, "beats must be a whole number".to_string())
        );
        assert_eq!(
            errors_of(&form("1", "1", "128", "", "cid"))[0],
            (Field::VsockCid, "vsock_cid must be a whole number".to_string())
        );
    }

    #[test]
    fn the_guest_cid_floor_and_ceiling_are_the_daemons() {
        assert_eq!(
            form("1", "1", "128", "", "3").validate().expect("cid=3").vsock_cid,
            Some(3)
        );
        assert_eq!(
            errors_of(&form("1", "1", "128", "", "2"))[0],
            (
                Field::VsockCid,
                "vsock_cid must be at least 3 (1 and 2 are reserved)".to_string()
            )
        );
        assert_eq!(
            errors_of(&form("1", "1", "128", "", "4294967295"))[0],
            (
                Field::VsockCid,
                "vsock_cid must be at most 4294967294: the kernel refuses \
                 VMADDR_CID_ANY and above"
                    .to_string()
            )
        );
    }

    #[test]
    fn a_cid_run_that_would_run_past_the_last_guest_cid_is_refused() {
        assert!(form("2", "1", "128", "", "4294967293").validate().is_ok());
        assert_eq!(
            errors_of(&form("3", "1", "128", "", "4294967293"))[0],
            (
                Field::VsockCid,
                "vsock_cid and n together run past the last guest CID".to_string()
            )
        );
    }

    #[test]
    fn every_bad_field_is_reported_in_form_order() {
        let errors = errors_of(&form("0", "99", "1", "x", "1"));
        let fields: Vec<Field> = errors.iter().map(|(field, _)| *field).collect();
        assert_eq!(
            fields,
            vec![Field::N, Field::Vcpus, Field::MemoryMib, Field::Beats, Field::VsockCid]
        );
    }

    #[test]
    fn a_bad_bare_cid_is_reported_when_the_count_is_bad_too() {
        // With no trustworthy `n`, the run's last CID cannot be checked; the
        // CID's own bounds still are.
        let errors = errors_of(&form("nope", "1", "128", "", "2"));
        assert_eq!(
            errors.iter().map(|(field, _)| *field).collect::<Vec<_>>(),
            vec![Field::N, Field::VsockCid]
        );
    }

    #[test]
    fn fields_round_trip_through_get_and_set() {
        let mut form = SpawnForm::default();
        for field in Field::ALL {
            assert_eq!(form.field(field), form.field(field));
            form.set(field, "9".to_string());
            assert_eq!(form.field(field), "9");
        }
        assert_eq!(form.n, "9");
        assert_eq!(form.vsock_cid, "9");
    }

    #[test]
    fn every_field_has_a_name_and_a_hint() {
        for field in Field::ALL {
            assert!(!field.name().is_empty());
            assert!(!field.hint().is_empty());
        }
        assert_eq!(Field::MemoryMib.name(), "memory_mib");
    }
}
