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

/// A quick-boot preset: the numbers one named scenario wants, filled into the
/// form so the operator sees (and may change) exactly what will be sent — a
/// preset never boots anything on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// Two VMs with vsock devices from CID 10, so each session can be watched
    /// end to end.
    Pair,
    /// Eight VMs at 512 MiB: a working fleet.
    Eight,
    /// Fifty VMs at the daemon's floor of 128 MiB: the scale scenario.
    Scale,
}

/// What a preset puts in the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresetShape {
    /// How many VMs to boot.
    pub n: u32,
    /// vCPUs per VM.
    pub vcpus: u32,
    /// Guest memory per VM, in MiB.
    pub memory_mib: u32,
    /// The base guest CID, or `None` for a run without vsock devices.
    pub vsock_cid: Option<u64>,
}

impl PresetShape {
    /// The guest memory the whole run asks for, in MiB.
    pub fn total_mib(self) -> u64 {
        u64::from(self.n) * u64::from(self.memory_mib)
    }
}

impl Preset {
    /// Every preset, in the order the form offers them.
    pub const ALL: [Preset; 3] = [Preset::Pair, Preset::Eight, Preset::Scale];

    /// The word on the preset's button.
    pub fn label(self) -> &'static str {
        match self {
            Preset::Pair => "2 VMs + vsock",
            Preset::Eight => "8 VMs",
            Preset::Scale => "50-VM scale",
        }
    }

    /// What the preset is for, as the form's second line of label.
    pub fn what(self) -> &'static str {
        match self {
            Preset::Pair => "two VMs on cid 10, a full session each",
            Preset::Eight => "a working fleet, 512 MiB each",
            Preset::Scale => "the daemon's floor, 128 MiB each",
        }
    }

    /// The numbers the preset fills in.
    pub fn shape(self) -> PresetShape {
        match self {
            Preset::Pair => PresetShape {
                n: 2,
                vcpus: 2,
                memory_mib: 512,
                vsock_cid: Some(10),
            },
            Preset::Eight => PresetShape {
                n: 8,
                vcpus: 2,
                memory_mib: 512,
                vsock_cid: None,
            },
            Preset::Scale => PresetShape {
                n: 50,
                vcpus: 1,
                memory_mib: 128,
                vsock_cid: None,
            },
        }
    }

    /// Fills the form: every field, so the boxes hold exactly what the preset
    /// means. A preset has no opinion on heartbeats, so the run it describes
    /// is long-lived.
    pub fn fill(self, form: &mut SpawnForm) {
        let shape = self.shape();
        form.set(Field::N, shape.n.to_string());
        form.set(Field::Vcpus, shape.vcpus.to_string());
        form.set(Field::MemoryMib, shape.memory_mib.to_string());
        form.set(Field::Beats, String::new());
        form.set(
            Field::VsockCid,
            shape.vsock_cid.map(|cid| cid.to_string()).unwrap_or_default(),
        );
    }
}

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

    #[test]
    fn a_preset_fills_every_box_and_the_filled_form_validates() {
        let cases = [
            (
                Preset::Pair,
                form("2", "2", "512", "", "10"),
                CreateRun {
                    n: 2,
                    vcpus: 2,
                    memory_mib: 512,
                    beats: None,
                    vsock_cid: Some(10),
                },
            ),
            (
                Preset::Eight,
                form("8", "2", "512", "", ""),
                CreateRun {
                    n: 8,
                    vcpus: 2,
                    memory_mib: 512,
                    beats: None,
                    vsock_cid: None,
                },
            ),
            (
                Preset::Scale,
                form("50", "1", "128", "", ""),
                CreateRun {
                    n: 50,
                    vcpus: 1,
                    memory_mib: 128,
                    beats: None,
                    vsock_cid: None,
                },
            ),
        ];
        for (preset, expected, create) in cases {
            let mut form = SpawnForm::default();
            preset.fill(&mut form);
            assert_eq!(form, expected, "{} fills every box", preset.label());
            // The request is exactly the daemon's own body, and the daemon
            // accepts it: a preset that the form itself refuses would be a
            // preset nobody can boot.
            assert_eq!(form.validate().expect("the preset is valid"), create);
        }
    }

    #[test]
    fn the_presets_bodies_are_byte_for_byte_the_daemons_own() {
        let mut form = SpawnForm::default();
        Preset::Pair.fill(&mut form);
        assert_eq!(
            serde_json::to_string(&form.validate().expect("valid")).expect("a body"),
            r#"{"n":2,"vcpus":2,"memory_mib":512,"beats":null,"vsock_cid":10}"#
        );

        Preset::Eight.fill(&mut form);
        assert_eq!(
            serde_json::to_string(&form.validate().expect("valid")).expect("a body"),
            r#"{"n":8,"vcpus":2,"memory_mib":512,"beats":null,"vsock_cid":null}"#
        );

        Preset::Scale.fill(&mut form);
        assert_eq!(
            serde_json::to_string(&form.validate().expect("valid")).expect("a body"),
            r#"{"n":50,"vcpus":1,"memory_mib":128,"beats":null,"vsock_cid":null}"#
        );
    }

    #[test]
    fn a_preset_leaves_nothing_of_the_form_it_replaced() {
        let mut form = form("7", "3", "4096", "9", "70");
        Preset::Scale.fill(&mut form);
        assert_eq!(form.field(Field::N), "50");
        assert_eq!(form.field(Field::MemoryMib), "128");
        assert_eq!(form.field(Field::Beats), "", "the old heartbeats are gone");
        assert_eq!(form.field(Field::VsockCid), "", "the old cid is gone");

        // And a preset with a cid puts one back.
        Preset::Pair.fill(&mut form);
        assert_eq!(form.field(Field::VsockCid), "10");
    }

    #[test]
    fn every_preset_has_a_label_a_scenario_and_a_footprint() {
        for preset in Preset::ALL {
            assert!(!preset.label().is_empty());
            assert!(!preset.what().is_empty());
            let shape = preset.shape();
            assert!((1..=MAX_VMS).contains(&shape.n), "{}", preset.label());
            assert!((1..=MAX_VCPUS).contains(&shape.vcpus), "{}", preset.label());
            assert!(shape.memory_mib >= MIN_MEMORY_MIB, "{}", preset.label());
            if let Some(cid) = shape.vsock_cid {
                assert!((MIN_GUEST_CID..=MAX_GUEST_CID).contains(&cid));
            }
        }
        assert_eq!(Preset::Pair.shape().total_mib(), 1_024);
        assert_eq!(Preset::Eight.shape().total_mib(), 4_096);
        assert_eq!(Preset::Scale.shape().total_mib(), 6_400);
    }

    #[test]
    fn the_labels_say_what_the_presets_boot() {
        assert_eq!(Preset::Pair.label(), "2 VMs + vsock");
        assert_eq!(Preset::Eight.label(), "8 VMs");
        assert_eq!(Preset::Scale.label(), "50-VM scale");
        assert_eq!(Preset::ALL.len(), 3);
    }
}
