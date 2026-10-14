//! Validation of the robot files (schema 1), as `validate.ts`. Each check returns its errors named by
//! path in the file ("parts[3].geo: ..."); an empty list is a valid file. The checks run on the parsed
//! JSON, so a misspelt field is named before the file is read into the typed structs.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use super::types::{
    EYE_SHAPES, FX_KINDS, GEO_TYPES, GRIPS, LOCOMOTIONS, MATS, PERSONALITY, REQUIRED_BONES, WAVES,
};

type Errs = Vec<String>;

/// The value of a key, or JSON null when it is missing (the web's `undefined`).
static NULL: Value = Value::Null;

const SPECIES_KEYS: &[&str] = &[
    "schema",
    "id",
    "name",
    "blurb",
    "accent",
    "height",
    "label_top",
    "locomotion",
    "gait",
    "bones",
    "parts",
    "eyes",
    "spin",
    "feet",
    "floats",
    "personality",
    "quirks",
    "clips",
    "overrides",
    "orchestrator",
];
const GAIT_KEYS: &[&str] = &["speed", "freq", "bob", "sway", "lean"];
const EYE_KEYS: &[&str] = &[
    "bone", "at", "r", "pupil", "shape", "glass", "stretch", "asym",
];
const PART_KEYS: &[&str] = &[
    "bone", "geo", "mat", "at", "rot", "scale", "outline", "name", "asym",
];
const GEO_KEYS: &[&str] = &[
    "type", "w", "h", "d", "r", "rt", "rb", "ri", "ro", "tube", "arc", "seg", "half", "points",
];
const INLINE_KEYS: &[&str] = &["color", "emissive", "glow", "opacity", "flat"];
const CLIP_KEYS: &[&str] = &["loop", "dur", "channels", "fx"];
const CHANNEL_KEYS: &[&str] = &[
    "bone", "prop", "base", "amp", "freq", "phase", "wave", "duty", "env", "snap", "rate",
];
const TOOL_KEYS: &[&str] = &["schema", "id", "name", "grip", "aim", "parts", "tip"];

fn field<'a>(o: &'a Map<String, Value>, key: &str) -> &'a Value {
    o.get(key).unwrap_or(&NULL)
}

fn defined(v: &Value) -> bool {
    !v.is_null()
}

fn is_num(v: &Value) -> bool {
    v.is_number()
}

fn is_vec3(v: &Value) -> bool {
    v.as_array()
        .is_some_and(|a| a.len() == 3 && a.iter().all(is_num))
}

fn is_hex(v: &Value) -> bool {
    let Some(s) = v.as_str().and_then(|s| s.strip_prefix('#')) else {
        return false;
    };
    (s.len() == 3 || s.len() == 6) && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn is_str(v: &Value) -> bool {
    v.as_str().is_some_and(|s| !s.is_empty())
}

/// An id: `[a-z][a-z0-9-]*` with dot-separated parts (`walk.treads`).
fn is_id(s: &str) -> bool {
    let ok_chars = |p: &str| {
        p.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    };
    let mut parts = s.split('.');
    let first = parts.next().unwrap_or("");
    let first_ok = first.starts_with(|c: char| c.is_ascii_lowercase()) && ok_chars(first);
    first_ok && parts.all(|p| !p.is_empty() && ok_chars(p))
}

fn in_list(list: &[&str], v: &Value) -> bool {
    v.as_str().is_some_and(|s| list.contains(&s))
}

/// An integer from `lo` to `hi` (a JSON number with no fraction).
fn is_int_in(v: &Value, lo: f64, hi: f64) -> bool {
    v.as_f64()
        .is_some_and(|x| x.fract() == 0.0 && x >= lo && x <= hi)
}

/// A value as the message names it: a string plain, a missing value as "undefined".
fn show(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "undefined".to_string(),
        other => other.to_string(),
    }
}

/// Keys outside `allowed` are errors: a typo in a field name must not pass silently.
fn unknown_keys(o: &Map<String, Value>, allowed: &[&str], path: &str, errs: &mut Errs) {
    for k in o.keys() {
        if !allowed.contains(&k.as_str()) {
            errs.push(format!("{path}.{k}: unknown field"));
        }
    }
}

/// The geometry fields each type needs.
fn geo_required(kind: &str) -> &'static [&'static str] {
    match kind {
        "box" => &["w", "h", "d"],
        "rbox" => &["w", "h", "d", "r"],
        "cyl" => &["rt", "rb", "h"],
        "cone" => &["r", "h"],
        "sphere" => &["r"],
        "torus" => &["r", "tube"],
        "plane" => &["w", "d"],
        "ring" => &["ri", "ro"],
        "shape" => &["h", "points"],
        _ => &[],
    }
}

fn check_geo(g: &Value, path: &str, errs: &mut Errs) {
    let Some(o) = g.as_object() else {
        errs.push(format!("{path}: a geometry object is required"));
        return;
    };
    let kind_v = field(o, "type");
    if !in_list(GEO_TYPES, kind_v) {
        errs.push(format!("{path}.type: one of {}", GEO_TYPES.join(", ")));
        return;
    }
    let kind = kind_v.as_str().unwrap_or("");
    unknown_keys(o, GEO_KEYS, path, errs);
    for k in geo_required(kind) {
        if *k == "points" {
            if !field(o, "points").as_array().is_some_and(|a| a.len() >= 3) {
                errs.push(format!("{path}.points: at least 3 [x, z] points"));
            }
        } else if !is_num(field(o, k)) {
            errs.push(format!("{path}.{k}: a number is required for {kind}"));
        }
    }
    let seg = field(o, "seg");
    if defined(seg) {
        let ok = seg.as_array().is_some_and(|a| {
            (1..=2).contains(&a.len()) && a.iter().all(|n| is_int_in(n, 1.0, 64.0))
        });
        if !ok {
            errs.push(format!("{path}.seg: 1 or 2 whole numbers from 1 to 64"));
        }
    }
    let half = field(o, "half");
    if defined(half) && (kind != "sphere" || half.as_str() != Some("top")) {
        errs.push(format!("{path}.half: only \"top\", on a sphere"));
    }
}

fn check_mat(m: &Value, path: &str, errs: &mut Errs) {
    if let Some(s) = m.as_str() {
        if !MATS.contains(&s) {
            errs.push(format!(
                "{path}: unknown material \"{s}\" (one of {})",
                MATS.join(", ")
            ));
        }
        return;
    }
    let Some(o) = m.as_object().filter(|o| is_hex(field(o, "color"))) else {
        errs.push(format!(
            "{path}: a material name, or an inline material with a hex color"
        ));
        return;
    };
    unknown_keys(o, INLINE_KEYS, path, errs);
    let emissive = field(o, "emissive");
    if defined(emissive) && !is_hex(emissive) {
        errs.push(format!("{path}.emissive: a hex color"));
    }
}

/// A part. `bones` names the bones a part may hang on; None for a tool (its parts have no bone).
fn check_part(p: &Value, path: &str, bones: Option<&BTreeSet<String>>, errs: &mut Errs) {
    let Some(o) = p.as_object() else {
        errs.push(format!("{path}: a part object is required"));
        return;
    };
    unknown_keys(o, PART_KEYS, path, errs);
    let bone = field(o, "bone");
    match bones {
        None => {
            if defined(bone) {
                errs.push(format!("{path}.bone: a tool's parts have no bone"));
            }
        }
        Some(set) => {
            if !bone.as_str().is_some_and(|b| set.contains(b)) {
                errs.push(format!(
                    "{path}.bone: \"{}\" is not a bone of this species",
                    show(bone)
                ));
            }
        }
    }
    check_geo(field(o, "geo"), &format!("{path}.geo"), errs);
    check_mat(field(o, "mat"), &format!("{path}.mat"), errs);
    let at = field(o, "at");
    if defined(at) && !is_vec3(at) {
        errs.push(format!("{path}.at: [x, y, z]"));
    }
    let rot = field(o, "rot");
    if defined(rot) && !is_vec3(rot) {
        errs.push(format!("{path}.rot: [x, y, z] in degrees"));
    }
    let scale = field(o, "scale");
    if defined(scale) && !is_vec3(scale) && !is_num(scale) {
        errs.push(format!("{path}.scale: a number or [x, y, z]"));
    }
    let outline = field(o, "outline");
    if defined(outline) && !outline.is_boolean() {
        errs.push(format!("{path}.outline: true or false"));
    }
}

fn is_prop(s: &str) -> bool {
    matches!(
        s,
        "rot.x" | "rot.y" | "rot.z" | "pos.x" | "pos.y" | "pos.z" | "scale" | "lid"
    )
}

/// A number, or a gait reference: "gait.bob".
fn is_param(v: &Value) -> bool {
    is_num(v)
        || v.as_str().is_some_and(|s| {
            s.strip_prefix("gait.")
                .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_lowercase()))
        })
}

/// A clip's channels. `bones` is the set they may name: a species' bones, or the required ones for a shared clip.
pub fn validate_clip(c: &Value, path: &str, bones: &BTreeSet<String>) -> Errs {
    let mut errs = Vec::new();
    let Some(o) = c.as_object() else {
        return vec![format!("{path}: a clip object is required")];
    };
    unknown_keys(o, CLIP_KEYS, path, &mut errs);
    let looped = field(o, "loop");
    if defined(looped) && !looped.is_boolean() {
        errs.push(format!("{path}.loop: true or false"));
    }
    if looped.as_bool() == Some(false) && !is_num(field(o, "dur")) {
        errs.push(format!(
            "{path}.dur: a one-shot needs its length in seconds"
        ));
    }
    let Some(channels) = field(o, "channels").as_array() else {
        errs.push(format!("{path}.channels: a list"));
        return errs;
    };
    for (i, ch) in channels.iter().enumerate() {
        let cp = format!("{path}.channels[{i}]");
        let Some(co) = ch.as_object() else {
            errs.push(format!("{cp}: a channel object"));
            continue;
        };
        unknown_keys(co, CHANNEL_KEYS, &cp, &mut errs);
        let bone = field(co, "bone");
        if !bone.as_str().is_some_and(|b| bones.contains(b)) {
            errs.push(format!("{cp}.bone: \"{}\" is not a bone here", show(bone)));
        }
        let prop = field(co, "prop");
        if !prop.as_str().is_some_and(is_prop) {
            errs.push(format!("{cp}.prop: rot.x|y|z, pos.x|y|z, scale or lid"));
        }
        for k in ["base", "amp", "freq"] {
            let v = field(co, k);
            if defined(v) && !is_param(v) {
                errs.push(format!("{cp}.{k}: a number or \"gait.<name>\""));
            }
        }
        let wave = field(co, "wave");
        if defined(wave) && !in_list(WAVES, wave) {
            errs.push(format!("{cp}.wave: one of {}", WAVES.join(", ")));
        }
        let phase = field(co, "phase");
        if defined(phase) && !is_num(phase) {
            errs.push(format!("{cp}.phase: a number"));
        }
        let duty = field(co, "duty");
        if defined(duty) && !duty.as_f64().is_some_and(|d| (0.0..=1.0).contains(&d)) {
            errs.push(format!("{cp}.duty: 0 to 1"));
        }
        let env = field(co, "env");
        if defined(env) && env.as_str() != Some("in-out") {
            errs.push(format!("{cp}.env: \"in-out\""));
        }
        let snap = field(co, "snap");
        if defined(snap) && !snap.is_boolean() {
            errs.push(format!("{cp}.snap: true or false"));
        }
        let rate = field(co, "rate");
        if defined(rate) && !is_num(rate) {
            errs.push(format!("{cp}.rate: a number (rad/s)"));
        }
        if defined(rate) && defined(prop) && !prop.as_str().is_some_and(|p| p.starts_with("rot.")) {
            errs.push(format!("{cp}.rate: only for rot props"));
        }
    }
    let fx = field(o, "fx");
    if defined(fx) {
        match fx.as_array() {
            None => errs.push(format!("{path}.fx: a list")),
            Some(list) => {
                for (i, f) in list.iter().enumerate() {
                    let fp = format!("{path}.fx[{i}]");
                    let Some(fo) = f
                        .as_object()
                        .filter(|fo| in_list(FX_KINDS, field(fo, "kind")))
                    else {
                        errs.push(format!("{fp}.kind: one of {}", FX_KINDS.join(", ")));
                        continue;
                    };
                    if !is_str(field(fo, "at")) {
                        errs.push(format!(
                            "{fp}.at: an anchor (head, gripR, gripL, body, carry, or a bone)"
                        ));
                    }
                    let every = field(fo, "every");
                    if defined(every) && !every.as_f64().is_some_and(|e| e > 0.0) {
                        errs.push(format!("{fp}.every: seconds, above 0"));
                    }
                }
            }
        }
    }
    errs
}

fn is_schema_1(o: &Map<String, Value>) -> bool {
    field(o, "schema").as_u64() == Some(1)
}

/// The shared clip file (`clips.json`): its clips name the required bones only.
pub fn validate_clips(file: &Value) -> Errs {
    let clips = file
        .as_object()
        .filter(|o| is_schema_1(o))
        .and_then(|o| field(o, "clips").as_object());
    let Some(clips) = clips else {
        return vec!["clips: { schema: 1, clips: {...} }".to_string()];
    };
    let bones: BTreeSet<String> = REQUIRED_BONES.iter().map(|b| b.to_string()).collect();
    let mut errs = Vec::new();
    for (id, c) in clips {
        if is_id(id) {
            errs.extend(validate_clip(c, &format!("clips.{id}"), &bones));
        } else {
            errs.push(format!("clips.{id}: a clip id"));
        }
    }
    errs
}

/// A species (`species/<id>.json`): its bones, parts, eyes, motion, personality and clips. `shared` is
/// the set of shared clip ids (quirks and overrides may name them).
pub fn validate_species(file: &Value, name: &str, shared: &BTreeSet<String>) -> Errs {
    let mut errs = Vec::new();
    let Some(o) = file.as_object() else {
        return vec![format!("{name}: a JSON object")];
    };
    unknown_keys(o, SPECIES_KEYS, name, &mut errs);
    if field(o, "schema").as_u64() != Some(1) {
        errs.push(format!("{name}.schema: 1"));
    }
    let id = field(o, "id");
    if id.as_str() != Some(name) {
        errs.push(format!(
            "{name}.id: \"{}\" must match the file name \"{name}\"",
            show(id)
        ));
    }
    if !is_str(field(o, "name")) {
        errs.push(format!("{name}.name: a nickname"));
    }
    if !field(o, "blurb").is_string() {
        errs.push(format!("{name}.blurb: a line of text"));
    }
    if !is_hex(field(o, "accent")) {
        errs.push(format!("{name}.accent: a hex color"));
    }
    if !field(o, "height").as_f64().is_some_and(|h| h > 0.0) {
        errs.push(format!("{name}.height: a positive number"));
    }
    if !is_num(field(o, "label_top")) {
        errs.push(format!("{name}.label_top: a height"));
    }
    if !in_list(LOCOMOTIONS, field(o, "locomotion")) {
        errs.push(format!(
            "{name}.locomotion: one of {}",
            LOCOMOTIONS.join(", ")
        ));
    }
    if !field(o, "floats").is_boolean() {
        errs.push(format!("{name}.floats: true or false"));
    }

    match field(o, "gait").as_object() {
        None => errs.push(format!("{name}.gait: an object")),
        Some(g) => {
            for k in GAIT_KEYS {
                if !is_num(field(g, k)) {
                    errs.push(format!("{name}.gait.{k}: a number"));
                }
            }
            if !field(g, "sideways").is_boolean() {
                errs.push(format!("{name}.gait.sideways: true or false"));
            }
        }
    }

    // Bones: the required set, every parent a known bone (or "root"), and no cycles.
    let mut bones: BTreeSet<String> = BTreeSet::new();
    match field(o, "bones").as_object() {
        None => errs.push(format!("{name}.bones: an object")),
        Some(bo) => {
            for (b, def) in bo {
                match def.as_object() {
                    Some(d) if is_vec3(field(d, "at")) => {}
                    _ => errs.push(format!("{name}.bones.{b}: {{ parent, at: [x, y, z] }}")),
                }
                bones.insert(b.clone());
            }
            for req in REQUIRED_BONES {
                if !bones.contains(*req) {
                    errs.push(format!(
                        "{name}.bones: the required bone \"{req}\" is missing"
                    ));
                }
            }
            for (b, def) in bo {
                let Some(d) = def.as_object() else { continue };
                let mut p: &Value = field(d, "parent");
                let mut depth = 0usize;
                while p.as_str() != Some("root") {
                    let known = p
                        .as_str()
                        .filter(|s| bones.contains(*s))
                        .filter(|_| depth <= bo.len());
                    let Some(pname) = known else {
                        errs.push(format!("{name}.bones.{b}.parent: \"{}\" is not a bone (or \"root\"), or a cycle", show(p)));
                        break;
                    };
                    p = bo
                        .get(pname)
                        .and_then(|d| d.as_object())
                        .map_or(&NULL, |d| field(d, "parent"));
                    depth += 1;
                }
            }
        }
    }

    match field(o, "parts").as_array() {
        None => errs.push(format!("{name}.parts: a list")),
        Some(parts) => {
            for (i, p) in parts.iter().enumerate() {
                check_part(p, &format!("{name}.parts[{i}]"), Some(&bones), &mut errs);
            }
        }
    }

    match field(o, "eyes").as_array() {
        None => errs.push(format!("{name}.eyes: a list")),
        Some(eyes) => {
            for (i, e) in eyes.iter().enumerate() {
                let ep = format!("{name}.eyes[{i}]");
                let Some(eo) = e.as_object() else {
                    errs.push(format!("{ep}: an eye object"));
                    continue;
                };
                unknown_keys(eo, EYE_KEYS, &ep, &mut errs);
                if !field(eo, "bone")
                    .as_str()
                    .is_some_and(|b| bones.contains(b))
                {
                    errs.push(format!("{ep}.bone: not a bone of this species"));
                }
                if !is_vec3(field(eo, "at")) {
                    errs.push(format!("{ep}.at: [x, y, z]"));
                }
                if !field(eo, "pupil").as_f64().is_some_and(|p| p > 0.0) {
                    errs.push(format!("{ep}.pupil: a radius"));
                }
                if !in_list(EYE_SHAPES, field(eo, "shape")) {
                    errs.push(format!("{ep}.shape: one of {}", EYE_SHAPES.join(", ")));
                }
                let glass = field(eo, "glass");
                if defined(glass)
                    && (!glass.is_boolean()
                        || (glass.as_bool() == Some(true) && !is_num(field(eo, "r"))))
                {
                    errs.push(format!("{ep}.glass: true or false (true needs r)"));
                }
                let r = field(eo, "r");
                if defined(r) && !is_num(r) {
                    errs.push(format!("{ep}.r: a radius"));
                }
                let stretch = field(eo, "stretch");
                if defined(stretch) && !is_num(stretch) {
                    errs.push(format!("{ep}.stretch: a number"));
                }
            }
        }
    }

    match field(o, "spin").as_array() {
        None => errs.push(format!("{name}.spin: a list")),
        Some(spins) => {
            for (i, s) in spins.iter().enumerate() {
                let sp = format!("{name}.spin[{i}]");
                let Some(so) = s.as_object().filter(|so| {
                    field(so, "bone")
                        .as_str()
                        .is_some_and(|b| bones.contains(b))
                }) else {
                    errs.push(format!("{sp}.bone: not a bone of this species"));
                    continue;
                };
                if !in_list(&["x", "y", "z"], field(so, "axis")) {
                    errs.push(format!("{sp}.axis: x, y or z"));
                }
                if !(is_num(field(so, "walk")) && is_num(field(so, "idle"))) {
                    errs.push(format!("{sp}: walk and idle are rad/s numbers"));
                }
                let offline = field(so, "offline");
                if defined(offline) && !is_num(offline) {
                    errs.push(format!("{sp}.offline: a number"));
                }
            }
        }
    }

    let feet_ok = field(o, "feet").as_array().is_some_and(|f| {
        f.iter()
            .all(|b| b.as_str().is_some_and(|b| bones.contains(b)))
    });
    if !feet_ok {
        errs.push(format!("{name}.feet: a list of bones"));
    }

    match field(o, "personality").as_object() {
        None => errs.push(format!("{name}.personality: an object")),
        Some(p) => {
            for k in PERSONALITY {
                if !is_int_in(field(p, k), 0.0, 9.0) {
                    errs.push(format!("{name}.personality.{k}: a whole number 0 to 9"));
                }
            }
        }
    }

    let own_clips = field(o, "clips").as_object();
    match own_clips {
        None => errs.push(format!("{name}.clips: an object")),
        Some(cl) => {
            for (id, c) in cl {
                if !is_id(id) {
                    errs.push(format!("{name}.clips.{id}: a clip id"));
                }
                errs.extend(validate_clip(c, &format!("{name}.clips.{id}"), &bones));
            }
        }
    }
    let own = |k: &str| own_clips.is_some_and(|c| c.contains_key(k));

    match field(o, "quirks").as_array() {
        None => errs.push(format!("{name}.quirks: a list")),
        Some(quirks) => {
            for (i, q) in quirks.iter().enumerate() {
                let qp = format!("{name}.quirks[{i}]");
                let Some(qo) = q
                    .as_object()
                    .filter(|qo| field(qo, "clip").is_string() && is_num(field(qo, "weight")))
                else {
                    errs.push(format!("{qp}: {{ clip, weight }}"));
                    continue;
                };
                let clip = field(qo, "clip").as_str().unwrap_or("");
                if !(own(clip) || shared.contains(clip)) {
                    errs.push(format!("{qp}.clip: \"{clip}\" is not a clip"));
                }
            }
        }
    }

    match field(o, "overrides").as_object() {
        None => errs.push(format!("{name}.overrides: an object")),
        Some(ov) => {
            for (key, target) in ov {
                if !shared.contains(key) {
                    errs.push(format!("{name}.overrides.{key}: not a shared clip"));
                }
                if !target.as_str().is_some_and(&own) {
                    errs.push(format!(
                        "{name}.overrides.{key}: \"{}\" is not a clip of this species",
                        show(target)
                    ));
                }
            }
        }
    }

    let orchestrator = field(o, "orchestrator");
    if defined(orchestrator) {
        match orchestrator
            .as_object()
            .and_then(|oo| field(oo, "parts").as_array())
        {
            None => errs.push(format!("{name}.orchestrator: {{ parts: [...] }}")),
            Some(parts) => {
                for (i, p) in parts.iter().enumerate() {
                    check_part(
                        p,
                        &format!("{name}.orchestrator.parts[{i}]"),
                        Some(&bones),
                        &mut errs,
                    );
                }
            }
        }
    }
    errs
}

/// A tool (`tools/<id>.json`): parts at its origin, held at the grip (one, two or carry).
pub fn validate_tool(file: &Value, name: &str) -> Errs {
    let mut errs = Vec::new();
    let Some(o) = file.as_object() else {
        return vec![format!("{name}: a JSON object")];
    };
    unknown_keys(o, TOOL_KEYS, name, &mut errs);
    let aim = field(o, "aim");
    if defined(aim) && !in_list(&["forward", "down", "up"], aim) {
        errs.push(format!("{name}.aim: forward, down or up"));
    }
    if field(o, "schema").as_u64() != Some(1) {
        errs.push(format!("{name}.schema: 1"));
    }
    if field(o, "id").as_str() != Some(name) {
        errs.push(format!("{name}.id: must match the file name \"{name}\""));
    }
    if !is_str(field(o, "name")) {
        errs.push(format!("{name}.name: a name"));
    }
    if !in_list(GRIPS, field(o, "grip")) {
        errs.push(format!("{name}.grip: one of {}", GRIPS.join(", ")));
    }
    if !is_vec3(field(o, "tip")) {
        errs.push(format!("{name}.tip: [x, y, z]"));
    }
    match field(o, "parts").as_array() {
        Some(parts) if !parts.is_empty() => {
            for (i, p) in parts.iter().enumerate() {
                check_part(p, &format!("{name}.parts[{i}]"), None, &mut errs);
            }
        }
        _ => errs.push(format!("{name}.parts: a non-empty list")),
    }
    errs
}

/// The index (`index.json`): the species, tools and props, in order.
pub fn validate_index(file: &Value) -> Errs {
    let Some(o) = file.as_object().filter(|o| is_schema_1(o)) else {
        return vec!["index: { schema: 1, species, tools, props }".to_string()];
    };
    let mut errs = Vec::new();
    for k in ["species", "tools", "props"] {
        match field(o, k).as_array() {
            Some(list) if list.iter().all(|v| v.as_str().is_some_and(is_id)) => {
                let unique: BTreeSet<&str> = list.iter().filter_map(Value::as_str).collect();
                if unique.len() != list.len() {
                    errs.push(format!("index.{k}: ids must be unique"));
                }
            }
            _ => errs.push(format!("index.{k}: a list of ids")),
        }
    }
    errs
}

/// The palette (`palette.json`): the eight fixed robot colours.
pub fn validate_palette(file: &Value) -> Errs {
    let Some(o) = file.as_object() else {
        return vec!["palette: a JSON object".to_string()];
    };
    [
        "ink", "sheet", "paper", "rule", "signal", "ok", "info", "inkMuted",
    ]
    .into_iter()
    .filter(|k| !is_hex(field(o, k)))
    .map(|k| format!("palette.{k}: a hex color"))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_follow_the_format() {
        assert!(is_id("walk.treads"));
        assert!(is_id("t-square"));
        assert!(!is_id("Walk"));
        assert!(!is_id("walk..x"));
        assert!(!is_id("1walk"));
    }
}
