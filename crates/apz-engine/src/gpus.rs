//! Graphics cards (`assets/hardware/gpus`): the reference-card specs, and the rules that map a GPU that
//! hardware detection reports to one of them. A port of `apps/web/src/views/floor3d/gpus.ts` (the validator and
//! `matchCard`); the format is described in `assets/hardware/README.md` ("Graphics cards").
//!
//! Both files are embedded at compile time and parsed once, on first use. serde rejects unknown fields and bad
//! enum values; `validate_spec`, `validate_specs` and `validate_match` check the rest, as the TS validator does.
//! An invalid embedded file panics on first use, which the tests catch.

use serde::Deserialize;
use std::sync::OnceLock;

const SPECS_SRC: &str = include_str!("../../../assets/hardware/gpus/specs.json");
const MATCH_SRC: &str = include_str!("../../../assets/hardware/gpus/match.json");

/// The vendor names a card (and a rule) may use. `*` in a rule matches any vendor.
pub const VENDORS: [&str; 5] = ["nvidia", "amd", "intel", "apple", "generic"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Vendor {
    Nvidia,
    Amd,
    Intel,
    Apple,
    Generic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Segment {
    Consumer,
    Workstation,
    Datacenter,
    Edge,
    Laptop,
    Integrated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Form {
    Pcie,
    Sxm,
    Oam,
    Module,
    Bga,
    Integrated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MachineKind {
    Server,
    Workstation,
    Laptop,
    Edge,
    Phone,
}

impl MachineKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MachineKind::Server => "server",
            MachineKind::Workstation => "workstation",
            MachineKind::Laptop => "laptop",
            MachineKind::Edge => "edge",
            MachineKind::Phone => "phone",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CoolingType {
    Axial,
    Flowthrough,
    Blower,
    Passive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FinDirection {
    AlongLength,
    AcrossLength,
    Vertical,
    TowardBracket,
    #[serde(rename = "none")]
    NoFins,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShapeKind {
    XFrame,
    Stripe,
    LightBar,
    FinStack,
    Chamfer,
    Logo,
    FrameRail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum PowerType {
    #[serde(rename = "12v-2x6")]
    V12v2x6,
    #[serde(rename = "8pin")]
    Pin8,
    #[serde(rename = "6pin")]
    Pin6,
    #[serde(rename = "eps8")]
    Eps8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PowerPosition {
    TopEnd,
    End,
    TopMid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PortType {
    Dp,
    Hdmi,
    Usbc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LightKind {
    Logo,
    LightBar,
    FanRing,
    StatusLed,
    Strip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LightPosition {
    Side,
    Top,
    Fan,
    End,
    Logo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VentLevel {
    #[serde(rename = "none")]
    NoVents,
    Partial,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Finish {
    Matte,
    Metal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Front,
    Back,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DieArrangement {
    Ring,
    OneSide,
    TwoSide,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dims {
    pub length: f64,
    pub height: f64,
    pub thickness: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fan {
    pub d_mm: Option<f64>,
    pub x_mm: Option<f64>,
    pub side: Side,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cooling {
    #[serde(rename = "type")]
    pub kind: CoolingType,
    pub fans: Vec<Fan>,
    pub fin_direction: FinDirection,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shape {
    pub kind: ShapeKind,
    pub color: Option<String>,
    pub at_mm: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shroud {
    pub base_color: String,
    pub accent_color: Option<String>,
    pub finish: Finish,
    pub shapes: Vec<Shape>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Backplate {
    pub color: Option<String>,
    pub vents: VentLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Power {
    #[serde(rename = "type")]
    pub kind: PowerType,
    pub count: u32,
    pub position: PowerPosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Port {
    #[serde(rename = "type")]
    pub kind: PortType,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bracket {
    pub color: Option<String>,
    pub ports: Vec<Port>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Light {
    pub kind: LightKind,
    pub color: String,
    pub at: LightPosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    pub rows: u32,
    pub cols: u32,
    pub pitch_mm: [f64; 2],
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Vram {
    pub chips: u32,
    pub arrangement: DieArrangement,
    pub gddr: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Die {
    pub die_mm: Option<[f64; 2]>,
    pub vram: Vram,
}

/// One reference card, module or baseboard. Fields follow the schema in `assets/hardware/README.md`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GpuSpec {
    pub id: String,
    pub name: String,
    pub vendor: Vendor,
    pub segment: Segment,
    pub form: Form,
    pub reference: bool,
    pub size_mm: Option<Dims>,
    pub slots: f64,
    pub cooling: Cooling,
    pub shroud: Shroud,
    pub backplate: Backplate,
    pub power: Vec<Power>,
    pub bracket: Bracket,
    pub lighting: Vec<Light>,
    pub layout: Option<Layout>,
    pub die: Option<Die>,
    pub sources: Vec<String>,
    pub approx: Vec<String>,
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GpuSpecFile {
    pub schema: u32,
    pub specs: Vec<GpuSpec>,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRange {
    pub min: Option<f64>,
    pub max: Option<f64>,
}

/// A rule maps a detected GPU to a card. `vendor` is a vendor name or `*`. Rules are tried in order and the
/// first match wins, so the specific names come first and the fallbacks (`fallback: true`) come last.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchRule {
    pub vendor: String,
    pub name_regex: String,
    pub machine_kind: Option<MachineKind>,
    pub memory_mib: Option<MemoryRange>,
    pub card: String,
    #[serde(default)]
    pub fallback: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchFile {
    pub schema: u32,
    pub rules: Vec<MatchRule>,
}

/// What hardware detection reports for one GPU (noded: vendor "NVIDIA" | "AMD" | "Intel" | "Apple").
#[derive(Debug, Clone, PartialEq)]
pub struct GpuInput {
    pub vendor: String,
    pub name: String,
    pub memory_mib: Option<f64>,
}

/// The card for one detected GPU: the spec, whether it is a fallback, and the index of the rule that matched.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Match {
    pub card: &'static GpuSpec,
    pub fallback: bool,
    pub rule: usize,
}

/// The embedded files, parsed, with each rule's name regex compiled.
pub struct Catalog {
    pub specs: GpuSpecFile,
    pub rules: MatchFile,
    patterns: Vec<fancy_regex::Regex>,
}

static CATALOG: OnceLock<Catalog> = OnceLock::new();

/// The embedded card catalog. Panics if either embedded file fails to parse or validate.
pub fn catalog() -> &'static Catalog {
    CATALOG.get_or_init(|| {
        let specs: GpuSpecFile =
            serde_json::from_str(SPECS_SRC).expect("assets/hardware/gpus/specs.json parses");
        let rules: MatchFile =
            serde_json::from_str(MATCH_SRC).expect("assets/hardware/gpus/match.json parses");
        let mut errs = validate_specs(&specs);
        errs.extend({
            let ids: Vec<&str> = specs.specs.iter().map(|s| s.id.as_str()).collect();
            validate_match(&rules, &ids)
        });
        assert!(errs.is_empty(), "invalid GPU assets:\n{}", errs.join("\n"));
        let patterns = rules
            .rules
            .iter()
            .map(|r| compile(&r.name_regex).expect("validated above"))
            .collect();
        Catalog {
            specs,
            rules,
            patterns,
        }
    })
}

/// The name regex, case-insensitive as in the TS matcher (`new RegExp(src, 'i')`).
fn compile(pattern: &str) -> Result<fancy_regex::Regex, fancy_regex::Error> {
    fancy_regex::Regex::new(&format!("(?i){pattern}"))
}

/// The card for one detected GPU, or `None` when no rule matches. Vendors compare case-insensitively.
pub fn match_card(gpu: &GpuInput, machine_kind: Option<&str>) -> Option<Match> {
    let cat = catalog();
    let vendor = gpu.vendor.to_ascii_lowercase();
    for (rule, (r, re)) in cat.rules.rules.iter().zip(&cat.patterns).enumerate() {
        if r.vendor != "*" && r.vendor != vendor {
            continue;
        }
        if let Some(want) = r.machine_kind
            && machine_kind != Some(want.as_str())
        {
            continue;
        }
        if let Some(range) = &r.memory_mib {
            let Some(m) = gpu.memory_mib else { continue };
            if range.min.is_some_and(|min| m < min) || range.max.is_some_and(|max| m > max) {
                continue;
            }
        }
        if !matches!(re.is_match(&gpu.name), Ok(true)) {
            continue;
        }
        let card = cat
            .specs
            .specs
            .iter()
            .find(|s| s.id == r.card)
            .expect("validated: a rule names a spec");
        return Some(Match {
            card,
            fallback: r.fallback,
            rule,
        });
    }
    None
}

fn is_id(id: &str) -> bool {
    !id.is_empty()
        && id.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

fn is_hex(v: &str) -> bool {
    v.len() == 7 && v.starts_with('#') && v[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

fn nullable_hex(v: &Option<String>) -> bool {
    v.as_deref().is_none_or(is_hex)
}

fn dim_ok(v: f64) -> bool {
    (0.5..=1000.0).contains(&v)
}

/// Whether `approx` may name this top-level field: it must be in the entry (a null `die` or `layout` is absent).
fn has_field(spec: &GpuSpec, key: &str) -> bool {
    match key {
        "die" => spec.die.is_some(),
        "layout" => spec.layout.is_some(),
        "id" | "name" | "vendor" | "segment" | "form" | "reference" | "size_mm" | "slots"
        | "cooling" | "shroud" | "backplate" | "power" | "bracket" | "lighting" | "sources"
        | "approx" | "notes" => true,
        _ => false,
    }
}

/// Every problem with one spec, as messages prefixed with its id. An empty list means the entry is valid.
pub fn validate_spec(spec: &GpuSpec) -> Vec<String> {
    let mut errs: Vec<String> = Vec::new();
    macro_rules! bad {
        ($($arg:tt)*) => {
            errs.push(format!("{}: {}", spec.id, format!($($arg)*)))
        };
    }

    if !is_id(&spec.id) {
        bad!("id must be lower-case words joined by hyphens");
    }
    if spec.name.is_empty() {
        bad!("name is required");
    }
    if (spec.form == Form::Integrated) != (spec.segment == Segment::Integrated) {
        bad!("form integrated and segment integrated go together");
    }
    if spec.segment == Segment::Laptop && spec.form != Form::Bga {
        bad!("a laptop GPU is a bga package");
    }
    if spec.form == Form::Bga && spec.segment != Segment::Laptop {
        bad!("bga is for laptop GPUs only");
    }
    if spec.vendor == Vendor::Apple && spec.segment != Segment::Integrated {
        bad!("Apple GPUs are integrated");
    }
    if matches!(spec.form, Form::Sxm | Form::Oam) && spec.segment != Segment::Datacenter {
        bad!("sxm and oam are data-centre forms");
    }
    if !spec.approx.iter().all(|k| has_field(spec, k)) {
        bad!("approx must list top-level field names of this entry");
    }
    let approx_has = |k: &str| spec.approx.iter().any(|a| a == k);

    // size_mm: a board, module or box. Integrated entries have none.
    let mut length: Option<f64> = None;
    match &spec.size_mm {
        None => {
            if spec.form != Form::Integrated && !approx_has("size_mm") {
                bad!("size_mm is null, so approx must say so");
            }
        }
        Some(d) => {
            if !dim_ok(d.length) || !dim_ok(d.height) || !dim_ok(d.thickness) {
                bad!("size_mm needs length, height and thickness, each 0.5 to 1000 mm");
            } else {
                let (l, h, t) = (d.length, d.height, d.thickness);
                length = Some(l);
                if spec.form == Form::Pcie && l > 340.0 {
                    bad!("PCIe length {l} mm is over 340");
                }
                if spec.form == Form::Pcie
                    && (!(30.0..=150.0).contains(&h) || !(15.0..=70.0).contains(&t))
                {
                    bad!("PCIe height {h} or thickness {t} mm is out of range");
                }
                if matches!(spec.form, Form::Sxm | Form::Oam) && !(5.0..=200.0).contains(&t) {
                    bad!("module thickness {t} mm is out of range");
                }
            }
        }
    }
    if !(0.0..=3.0).contains(&spec.slots) {
        bad!("slots must be 0 to 3");
    }
    if spec.form == Form::Pcie && spec.slots < 1.0 {
        bad!("a PCIe card takes at least one slot");
    }

    let c = &spec.cooling;
    let cooling_name = format!("{:?}", c.kind).to_lowercase();
    if c.kind == CoolingType::Passive && !c.fans.is_empty() {
        bad!("a passive card has no fans");
    }
    if c.kind != CoolingType::Passive && c.fans.is_empty() {
        bad!("{} cooling needs fans", cooling_name);
    }
    for f in &c.fans {
        if !f.d_mm.is_none_or(|d| (30.0..=200.0).contains(&d)) {
            bad!(
                "a fan needs d_mm (30 to 200, or null), x_mm (a number, or null) and side front or back"
            );
        } else if let (Some(x), Some(l)) = (f.x_mm, length)
            && (x < 0.0 || x > l)
        {
            bad!("fan x_mm {x} is outside the length");
        }
    }

    let sh = &spec.shroud;
    if !is_hex(&sh.base_color) || !nullable_hex(&sh.accent_color) {
        bad!("shroud needs base_color, accent_color, finish and shapes");
    }
    for s in &sh.shapes {
        if !nullable_hex(&s.color) {
            bad!("a shape needs a known kind and a hex colour");
        } else if let Some(at) = s.at_mm
            && (at < 0.0 || length.is_some_and(|l| at > l))
        {
            bad!("shape at_mm {at} is outside the length");
        }
    }

    if !nullable_hex(&spec.backplate.color) {
        bad!("backplate needs color (hex or null) and vents");
    }
    if spec.power.iter().any(|p| !(1..=8).contains(&p.count)) {
        bad!("power is a list of {{type, count 1 to 8, position}}");
    }

    if !nullable_hex(&spec.bracket.color)
        || spec
            .bracket
            .ports
            .iter()
            .any(|p| !(1..=8).contains(&p.count))
    {
        bad!("bracket needs color (hex or null) and ports");
    }
    if spec.lighting.iter().any(|l| !is_hex(&l.color)) {
        bad!("lighting is a list of {{kind, color, at}}");
    }

    if let Some(lay) = &spec.layout {
        let pitch_ok = lay.pitch_mm.iter().all(|&v| v > 0.0);
        if !(1..=16).contains(&lay.rows) || !(1..=16).contains(&lay.cols) || !pitch_ok {
            bad!("layout needs rows, cols and pitch_mm [x, z]");
        }
    }

    if spec.form == Form::Bga {
        match &spec.die {
            None => bad!("a bga entry needs die {{die_mm, vram {{chips, arrangement}}}}"),
            Some(d) => {
                let die_ok = d.die_mm.is_none_or(|[a, b]| dim_ok(a) && dim_ok(b));
                if !die_ok || !(1..=16).contains(&d.vram.chips) {
                    bad!("a bga entry needs die {{die_mm, vram {{chips, arrangement}}}}");
                } else if d.die_mm.is_none() && !approx_has("die") {
                    bad!("die_mm is null, so approx must say so");
                }
            }
        }
    } else if spec.die.is_some() {
        bad!("die is only for bga entries");
    }

    if spec.sources.iter().any(|u| u.trim().is_empty()) {
        bad!("sources must be citation strings (URLs where the doc gives one)");
    } else if spec.reference && spec.form != Form::Integrated && spec.sources.is_empty() {
        bad!("a reference card needs a source");
    }
    errs
}

/// Every problem with the specs file: the schema, each entry, and duplicate ids.
pub fn validate_specs(file: &GpuSpecFile) -> Vec<String> {
    if file.schema != 1 {
        return vec!["specs file: needs schema 1 and a specs list".to_string()];
    }
    let mut errs: Vec<String> = file.specs.iter().flat_map(validate_spec).collect();
    let mut reported: Vec<&str> = Vec::new();
    for s in &file.specs {
        let n = file.specs.iter().filter(|x| x.id == s.id).count();
        if n > 1 && !reported.contains(&s.id.as_str()) {
            reported.push(&s.id);
            errs.push(format!("{}: duplicate id", s.id));
        }
    }
    errs
}

/// Every problem with the match file against the spec ids: each rule's vendor and regex are valid, each rule
/// points at a spec, and the fallbacks come after every specific rule.
pub fn validate_match(file: &MatchFile, spec_ids: &[&str]) -> Vec<String> {
    if file.schema != 1 {
        return vec!["match file: needs schema 1 and a rules list".to_string()];
    }
    let mut errs: Vec<String> = Vec::new();
    let mut seen_fallback = false;
    for (i, r) in file.rules.iter().enumerate() {
        let at = format!("rules[{i}]");
        if r.vendor != "*" && !VENDORS.contains(&r.vendor.as_str()) {
            errs.push(format!("{at}: vendor"));
        }
        if compile(&r.name_regex).is_err() {
            errs.push(format!("{at}: name_regex does not compile"));
        }
        if !spec_ids.contains(&r.card.as_str()) {
            errs.push(format!("{at}: card \"{}\" is not a spec", r.card));
        }
        if r.fallback {
            seen_fallback = true;
        } else if seen_fallback {
            errs.push(format!("{at}: a specific rule comes after a fallback"));
        }
    }
    errs
}

/// What the machine view's hardware panel says about one detected GPU: its own name, and the reference card it matched
/// (None when no rule matched), with the card's segment, form factor, slot width, size in millimetres, cooling and fan
/// count, and whether the match is a generic fallback rather than that exact card.
pub fn card_summary(gpu: &GpuInput, machine_kind: Option<&str>) -> serde_json::Value {
    let lower = |d: &dyn std::fmt::Debug| format!("{d:?}").to_ascii_lowercase();
    let card = match_card(gpu, machine_kind).map(|m| {
        let c = m.card;
        serde_json::json!({
            "id": c.id,
            "name": c.name,
            "segment": lower(&c.segment),
            "form": lower(&c.form),
            "slots": c.slots,
            "size_mm": c.size_mm.map(|d| [d.length, d.height, d.thickness]),
            "cooling": lower(&c.cooling.kind),
            "fans": c.cooling.fans.len(),
            "generic": m.fallback,
        })
    });
    serde_json::json!({ "gpu": gpu.name, "vendor": gpu.vendor, "memory_mib": gpu.memory_mib, "card": card })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_files_parse_and_validate() {
        let cat = catalog();
        assert!(!cat.specs.specs.is_empty());
        assert!(validate_specs(&cat.specs).is_empty());
        let ids: Vec<&str> = cat.specs.specs.iter().map(|s| s.id.as_str()).collect();
        assert!(validate_match(&cat.rules, &ids).is_empty());
    }

    #[test]
    fn unknown_fields_are_rejected() {
        assert!(
            serde_json::from_str::<MatchRule>(
                r#"{"vendor":"*","name_regex":".","card":"x","bogus":1}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<Dims>(r#"{"length":1,"height":1,"thickness":1,"x":1}"#).is_err()
        );
    }

    #[test]
    fn validator_flags_bad_specs() {
        let mut s = catalog()
            .specs
            .specs
            .iter()
            .find(|s| s.form == Form::Pcie && s.size_mm.is_some())
            .expect("a pcie card")
            .clone();
        s.size_mm = Some(Dims {
            length: 400.0,
            height: 137.0,
            thickness: 40.0,
        });
        let errs = validate_spec(&s);
        assert!(
            errs.iter()
                .any(|e| e.ends_with("PCIe length 400 mm is over 340")),
            "{errs:?}"
        );

        let dup = GpuSpecFile {
            schema: 1,
            specs: vec![s.clone(), s.clone()],
        };
        assert!(
            validate_specs(&dup)
                .iter()
                .any(|e| e.ends_with("duplicate id"))
        );

        let rules = MatchFile {
            schema: 1,
            rules: vec![
                MatchRule {
                    vendor: "*".into(),
                    name_regex: ".".into(),
                    machine_kind: None,
                    memory_mib: None,
                    card: s.id.clone(),
                    fallback: true,
                },
                MatchRule {
                    vendor: "nvidia".into(),
                    name_regex: "RTX".into(),
                    machine_kind: None,
                    memory_mib: None,
                    card: s.id.clone(),
                    fallback: false,
                },
            ],
        };
        let ids = [s.id.as_str()];
        assert!(
            validate_match(&rules, &ids)
                .iter()
                .any(|e| e.contains("comes after a fallback"))
        );
    }

    /// (vendor, name, memory MiB, machine kind, card, fallback, rule index): the expected results are from
    /// `matchCard` in gpus.ts, run under esbuild and node on these same inputs.
    #[test]
    fn match_card_agrees_with_the_ts_matcher() {
        #[allow(clippy::type_complexity)]
        let cases: &[(
            &str,
            &str,
            Option<f64>,
            Option<&str>,
            Option<&str>,
            bool,
            usize,
        )] = &[
            (
                "nvidia",
                "NVIDIA H100 80GB HBM3",
                Some(81920.0),
                Some("server"),
                Some("nvidia-sxm5-module"),
                false,
                18,
            ),
            (
                "nvidia",
                "NVIDIA GeForce RTX 3090",
                Some(24576.0),
                None,
                Some("nvidia-rtx3090-fe"),
                false,
                8,
            ),
            (
                "nvidia",
                "NVIDIA GeForce RTX 5090",
                Some(32768.0),
                None,
                Some("nvidia-rtx5090-fe"),
                false,
                0,
            ),
            (
                "nvidia",
                "NVIDIA GeForce RTX 4070 Ti",
                Some(12288.0),
                None,
                Some("nvidia-rtx4070ti-generic"),
                false,
                6,
            ),
            (
                "nvidia",
                "NVIDIA RTX 6000 Ada Generation",
                Some(49140.0),
                Some("workstation"),
                Some("nvidia-rtx6000ada"),
                false,
                11,
            ),
            (
                "nvidia",
                "NVIDIA RTX PRO 6000 Blackwell Max-Q Workstation Edition",
                Some(98304.0),
                Some("workstation"),
                Some("nvidia-rtxpro6000-maxq"),
                false,
                9,
            ),
            (
                "nvidia",
                "NVIDIA RTX PRO 6000 Blackwell Workstation Edition",
                Some(98304.0),
                Some("workstation"),
                Some("nvidia-rtxpro6000-ws"),
                false,
                10,
            ),
            (
                "nvidia",
                "NVIDIA L40S",
                Some(49152.0),
                Some("server"),
                Some("nvidia-l40s"),
                false,
                22,
            ),
            (
                "nvidia",
                "NVIDIA L40",
                Some(46068.0),
                Some("server"),
                Some("nvidia-l40"),
                false,
                23,
            ),
            (
                "nvidia",
                "Jetson Orin",
                None,
                Some("edge"),
                Some("generic-edge-board"),
                true,
                73,
            ),
            (
                "nvidia",
                "NVIDIA Jetson AGX Orin Developer Kit",
                Some(65536.0),
                Some("edge"),
                Some("nvidia-jetson-agx-orin-devkit"),
                false,
                30,
            ),
            (
                "nvidia",
                "NVIDIA GeForce RTX 3080",
                Some(10240.0),
                None,
                Some("nvidia-rtx3090-fe"),
                true,
                68,
            ),
            (
                "nvidia",
                "NVIDIA Unknown Board X",
                None,
                None,
                Some("consumer-generic"),
                true,
                89,
            ),
            (
                "NVIDIA",
                "NVIDIA GeForce RTX 4090",
                Some(24564.0),
                Some("workstation"),
                Some("nvidia-rtx4090-fe"),
                false,
                4,
            ),
            (
                "amd",
                "AMD Instinct MI300X",
                Some(196608.0),
                Some("server"),
                Some("amd-mi300x-oam"),
                false,
                47,
            ),
            (
                "amd",
                "Radeon RX 7900 XTX",
                Some(24576.0),
                None,
                Some("amd-rx7900xtx-mba"),
                false,
                37,
            ),
            (
                "amd",
                "Radeon RX 7900 XT",
                Some(20480.0),
                None,
                Some("amd-rx7900xt-mba"),
                false,
                38,
            ),
            (
                "amd",
                "AMD Radeon 780M",
                Some(512.0),
                Some("laptop"),
                Some("amd-radeon-igpu"),
                false,
                52,
            ),
            (
                "intel",
                "Intel Arc A770",
                Some(16384.0),
                None,
                Some("intel-arc-a770-le"),
                false,
                56,
            ),
            (
                "intel",
                "Intel Data Center GPU Max 1100",
                Some(49152.0),
                Some("server"),
                Some("intel-max1100"),
                false,
                60,
            ),
            (
                "intel",
                "Intel Graphics",
                None,
                Some("laptop"),
                Some("laptop-generic"),
                true,
                83,
            ),
            (
                "apple",
                "Apple M3 Max",
                None,
                Some("laptop"),
                Some("apple-soc-gpu"),
                false,
                65,
            ),
            (
                "apple",
                "Apple M3 Max",
                None,
                None,
                Some("apple-soc-gpu"),
                false,
                65,
            ),
            (
                "nvidia",
                "NVIDIA H200 NVL",
                Some(143360.0),
                Some("server"),
                Some("nvidia-h200-nvl"),
                false,
                17,
            ),
            (
                "qualcomm",
                "Adreno 750",
                None,
                None,
                Some("consumer-generic"),
                true,
                89,
            ),
        ];
        assert!(cases.len() >= 12);
        for &(vendor, name, mem, kind, card, fallback, rule) in cases {
            let gpu = GpuInput {
                vendor: vendor.into(),
                name: name.into(),
                memory_mib: mem,
            };
            let got = match_card(&gpu, kind).map(|m| (m.card.id.as_str(), m.fallback, m.rule));
            assert_eq!(got, card.map(|c| (c, fallback, rule)), "{vendor} / {name}");
        }
    }

    #[test]
    fn a_card_summary_names_the_matched_card_and_its_build() {
        let gpu = GpuInput {
            vendor: "NVIDIA".into(),
            name: "NVIDIA H100 80GB HBM3".into(),
            memory_mib: Some(81_559.0),
        };
        let s = card_summary(&gpu, Some("server"));
        assert_eq!(s["gpu"], "NVIDIA H100 80GB HBM3");
        let card = &s["card"];
        assert!(
            card["name"].as_str().is_some_and(|n| n.contains("H100")),
            "{s}"
        );
        assert!(
            card["form"].is_string() && card["cooling"].is_string() && card["generic"].is_boolean(),
            "{s}"
        );
        let none = card_summary(
            &GpuInput {
                vendor: "acme".into(),
                name: "Mystery".into(),
                memory_mib: None,
            },
            None,
        );
        assert!(
            none["card"].is_null() || none["card"]["generic"] == true,
            "{none}"
        );
    }
}
