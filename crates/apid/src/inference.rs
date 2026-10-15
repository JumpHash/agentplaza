//! Model agents' inference (docs/design/foundry.md, "The hub"). A promoted model is an ONNX graph
//! plus a model card: the card's input schema is checked, its feature specs turn the input object
//! into the model's float tensor, and its output (labels or values) comes back as a typed object
//! checked against the card's output schema. Pure Rust (tract): no native libraries, no GPU. Models
//! are only ever invoked here, never trained (that is the Foundry's trainer runtime).
//!
//! The foundry's jobs and registry routes and the `invoke_model` tool wire this up next.

use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::sync::{Arc, Mutex, MutexGuard};

use blake2::Blake2b;
use blake2::digest::consts::U8;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tract_onnx::prelude::{
    Datum, Framework, InferenceFact, InferenceModelExt, IntoRunnable, TValue, Tensor,
    TypedRunnableModel, tvec,
};

/// A JSON-schema subset: an object with typed properties, optional enums and required fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Schema {
    /// Always `"object"` (checked by [`Schema::check`]).
    #[serde(rename = "type")]
    pub kind: String,
    pub properties: BTreeMap<String, Property>,
    #[serde(default)]
    pub required: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Property {
    #[serde(rename = "type")]
    pub kind: PropType,
    /// Allowed values (`enum` in JSON schema).
    #[serde(default, rename = "enum", skip_serializing_if = "Option::is_none")]
    pub allowed: Option<Vec<Value>>,
}

/// The input types of the brief plus `array` and `object`, which output schemas use for `values`
/// and `probabilities`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PropType {
    Number,
    Integer,
    String,
    Boolean,
    Array,
    Object,
}

/// How one input field becomes float columns. Features are concatenated in card order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureSpec {
    pub name: String,
    pub kind: FeatureKind,
    /// `onehot`: the categories, in column order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<Value>,
    /// `hash`: the number of columns (the value's FNV-1a hash picks one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub buckets: Option<usize>,
    /// `numeric`: `(x - mean) / std`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<Scale>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeatureKind {
    /// One column: the number, scaled when `scale` is set.
    Numeric,
    /// One column per category in `values`; exactly one is 1.
    Onehot,
    /// `buckets` columns; the value's stable hash picks the one that is 1.
    Hash,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Scale {
    pub mean: f64,
    pub std: f64,
}

/// A model's identity, contract and provenance. Stored as JSON by the foundry's registry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelCard {
    pub id: String,
    pub name: String,
    pub version: String,
    pub input: Schema,
    pub output: Schema,
    pub features: Vec<FeatureSpec>,
    /// Class names for a classifier: the output is then `{label, probabilities}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    pub allowed_roles: Vec<String>,
    /// Lowercase hex SHA-256 of the ONNX bytes; [`LoadedModel::load`] refuses other bytes.
    pub onnx_sha256: String,
    pub created_ms: u64,
    #[serde(default)]
    pub metrics: Value,
    /// The trainer's `serving` block, for a card the trainer wrote (see [`featurize_serving`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serving: Option<Value>,
}

/// A model card with its ONNX graph compiled for inference at batch 1.
pub struct LoadedModel {
    card: ModelCard,
    plan: Arc<TypedRunnableModel>,
    inputs: Vec<InputPin>,
}

impl Schema {
    /// Checks that the schema is well formed: an object, required fields declared, enum members
    /// of the property's type.
    pub fn check(&self, what: &str) -> Result<(), String> {
        if self.kind != "object" {
            return Err(format!("{what} schema: type must be \"object\""));
        }
        for name in &self.required {
            if !self.properties.contains_key(name) {
                return Err(format!(
                    "{what} schema: required field `{name}` is not declared"
                ));
            }
        }
        for (name, prop) in &self.properties {
            let Some(allowed) = &prop.allowed else {
                continue;
            };
            if allowed.is_empty() {
                return Err(format!("{what} schema: `{name}` has an empty enum"));
            }
            for value in allowed {
                check_type(prop.kind, value).map_err(|_| {
                    format!(
                        "{what} schema: `{name}` enum member {value} is not a {}",
                        kind_name(prop.kind)
                    )
                })?;
            }
        }
        Ok(())
    }

    /// Validates `value` against the schema: an object, every required field present, no unknown
    /// fields, each field of its declared type and, when declared, one of its enum values.
    pub fn validate(&self, value: &Value) -> Result<(), String> {
        let object = value.as_object().ok_or("must be a JSON object")?;
        for name in &self.required {
            if !object.contains_key(name) {
                return Err(format!("missing required field `{name}`"));
            }
        }
        for (name, field) in object {
            let prop = self
                .properties
                .get(name)
                .ok_or_else(|| format!("unknown field `{name}`"))?;
            check_type(prop.kind, field)
                .map_err(|_| format!("field `{name}` must be a {}", kind_name(prop.kind)))?;
            if let Some(allowed) = &prop.allowed {
                if !allowed.iter().any(|a| value_eq(a, field)) {
                    return Err(format!(
                        "field `{name}` must be one of {}",
                        Value::Array(allowed.clone())
                    ));
                }
            }
        }
        Ok(())
    }
}

fn kind_name(kind: PropType) -> &'static str {
    match kind {
        PropType::Number => "number",
        PropType::Integer => "integer",
        PropType::String => "string",
        PropType::Boolean => "boolean",
        PropType::Array => "array",
        PropType::Object => "object",
    }
}

/// `Err(())` when `value` is not of `kind`. Integers accept `3.0` too (a JSON integer by value).
fn check_type(kind: PropType, value: &Value) -> Result<(), ()> {
    let ok = match kind {
        PropType::Number => value.is_number(),
        PropType::Integer => {
            value.is_i64() || value.is_u64() || value.as_f64().is_some_and(|f| f.fract() == 0.0)
        }
        PropType::String => value.is_string(),
        PropType::Boolean => value.is_boolean(),
        PropType::Array => value.is_array(),
        PropType::Object => value.is_object(),
    };
    if ok { Ok(()) } else { Err(()) }
}

/// Numbers compare by value (`1` equals `1.0`); everything else by JSON equality.
fn value_eq(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

impl FeatureSpec {
    /// Float columns this feature produces.
    fn width(&self) -> usize {
        match self.kind {
            FeatureKind::Numeric => 1,
            FeatureKind::Onehot => self.values.len(),
            FeatureKind::Hash => self.buckets.unwrap_or(0),
        }
    }

    fn check(&self) -> Result<(), String> {
        let name = &self.name;
        if name.is_empty() {
            return Err("a feature has an empty name".into());
        }
        match self.kind {
            FeatureKind::Numeric => {
                if !self.values.is_empty() || self.buckets.is_some() {
                    return Err(format!(
                        "feature `{name}`: numeric takes neither values nor buckets"
                    ));
                }
                if let Some(s) = self.scale {
                    if !s.mean.is_finite() || !s.std.is_finite() || s.std == 0.0 {
                        return Err(format!(
                            "feature `{name}`: scale needs a finite mean and a non-zero std"
                        ));
                    }
                }
            }
            FeatureKind::Onehot => {
                if self.values.is_empty() || self.buckets.is_some() || self.scale.is_some() {
                    return Err(format!(
                        "feature `{name}`: onehot takes values only, and at least one"
                    ));
                }
                for (i, v) in self.values.iter().enumerate() {
                    if !(v.is_string() || v.is_number() || v.is_boolean()) {
                        return Err(format!("feature `{name}`: onehot values must be scalars"));
                    }
                    if self.values[..i].iter().any(|w| value_eq(v, w)) {
                        return Err(format!(
                            "feature `{name}`: onehot value {v} is listed twice"
                        ));
                    }
                }
            }
            FeatureKind::Hash => {
                if self.buckets.unwrap_or(0) == 0 || !self.values.is_empty() || self.scale.is_some()
                {
                    return Err(format!(
                        "feature `{name}`: hash takes buckets only, at least one"
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Total float columns the features produce: the model's input length.
pub fn feature_width(features: &[FeatureSpec]) -> usize {
    features.iter().map(FeatureSpec::width).sum()
}

/// Turns the input object into the model's float row, features in card order. A missing field is
/// an error: a model never sees a silently zero-filled input.
pub fn featurize(features: &[FeatureSpec], input: &Value) -> Result<Vec<f32>, String> {
    let object = input.as_object().ok_or("input must be a JSON object")?;
    let mut row = Vec::with_capacity(feature_width(features));
    for feature in features {
        let name = &feature.name;
        let field = object
            .get(name)
            .ok_or_else(|| format!("missing feature field `{name}`"))?;
        match feature.kind {
            FeatureKind::Numeric => {
                let x = field
                    .as_f64()
                    .ok_or_else(|| format!("feature `{name}` must be a number"))?;
                let x = match feature.scale {
                    Some(s) => (x - s.mean) / s.std,
                    None => x,
                };
                row.push(x as f32);
            }
            FeatureKind::Onehot => {
                let hot = feature
                    .values
                    .iter()
                    .position(|v| value_eq(v, field))
                    .ok_or_else(|| {
                        format!("feature `{name}`: {field} is not one of its categories")
                    })?;
                row.extend((0..feature.values.len()).map(|i| if i == hot { 1.0 } else { 0.0 }));
            }
            FeatureKind::Hash => {
                let buckets = feature
                    .buckets
                    .filter(|n| *n > 0)
                    .ok_or_else(|| format!("feature `{name}` has no buckets"))?;
                let key = match field {
                    Value::String(s) => s.clone(),
                    Value::Number(_) | Value::Bool(_) => field.to_string(),
                    _ => {
                        return Err(format!(
                            "feature `{name}` must be a string, number or boolean"
                        ));
                    }
                };
                let hot = (fnv1a64(key.as_bytes()) % buckets as u64) as usize;
                row.extend((0..buckets).map(|i| if i == hot { 1.0 } else { 0.0 }));
            }
        }
    }
    Ok(row)
}

/// What the loader pins on one graph input: its name (`None` for a card without `serving.inputs`),
/// its element type and its width (columns per row).
#[derive(Debug, Clone, PartialEq)]
struct InputPin {
    name: Option<String>,
    dtype: Dtype,
    width: usize,
}

/// The card's graph inputs in the graph's order. A card with a serving block takes its serving
/// inputs (a trainer card's features, one column each); a card without one takes the typed columns
/// as one float32 row.
fn input_pins(card: &ModelCard) -> Result<Vec<InputPin>, String> {
    match &card.serving {
        Some(serving) => Ok(serving_inputs(serving)?
            .into_iter()
            .map(|input| InputPin {
                name: input.name,
                dtype: input.dtype,
                width: input.features.len(),
            })
            .collect()),
        None => Ok(vec![InputPin {
            name: None,
            dtype: Dtype::Float32,
            width: feature_width(&card.features),
        }]),
    }
}

/// Refuses a graph whose inputs are not the card's: the same count, and each named input in the
/// card's (graph's) order.
fn check_graph_inputs(graph: &[String], pins: &[InputPin]) -> Result<(), String> {
    if graph.len() != pins.len() {
        return Err(format!(
            "the ONNX graph takes {} inputs ({}) but the card declares {}",
            graph.len(),
            graph.join(", "),
            pins.len()
        ));
    }
    for (i, (found, pin)) in graph.iter().zip(pins).enumerate() {
        if pin.name.as_deref().is_some_and(|name| name != found) {
            return Err(format!(
                "the ONNX graph's input {i} is `{found}` but the card's serving.inputs names `{}` there; the card lists the graph's inputs in graph order ({})",
                pin.name.as_deref().unwrap_or_default(),
                graph.join(", ")
            ));
        }
    }
    Ok(())
}

/// How the pinned inputs are named in the load error: the single-input card keeps its serving width.
fn describe_pins(pins: &[InputPin]) -> String {
    match pins {
        [
            InputPin {
                name: None, width, ..
            },
        ] => format!("{width} features (its serving block width)"),
        _ => format!(
            "inputs ({})",
            pins.iter()
                .map(|p| format!(
                    "{} {:?} x{}",
                    p.name.as_deref().unwrap_or("input"),
                    p.dtype,
                    p.width
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// The error for an input whose row is not the width the card gives it.
fn width_error(pin: &InputPin, got: usize) -> String {
    match &pin.name {
        None => format!(
            "the feature vector has {got} columns but the model takes {} (the card's features)",
            pin.width
        ),
        Some(name) => format!(
            "input `{name}` has {got} columns but the model takes {}",
            pin.width
        ),
    }
}

/// The trainer's featurization, from a card's `serving` block (the trainer's `serving_schema()`),
/// one column per feature in card order. `numeric` with transform `signlog` is sign(x) * ln(1 + |x|)
/// (0 maps to 0); `zscore` is (x - mean) / std with `mean` and `std` on the feature. `hash` with
/// blake2b (digest_size 8, little byte order) is the bucket of "key=value", where key is the feature
/// name: the digest as a little-endian u64 modulo `buckets`, one column holding that index.
const HASH_MISSING: &str = "__missing__";

/// The trainer's `encode` rules for the values it can meet: a missing or null categorical is the bucket
/// of "__missing__"; a numeric takes an int, float or bool as its number (a bool as 1 or 0) and anything
/// else, a missing or null field included, as 0.
pub fn featurize_serving(serving: &Value, input: &Value) -> Result<Vec<f64>, String> {
    let object = input.as_object().ok_or("input must be a JSON object")?;
    let null = Value::Null;
    let features = serving
        .get("features")
        .and_then(Value::as_array)
        .ok_or("serving: features must be an array")?;
    let mut row = Vec::with_capacity(features.len());
    for feature in features {
        let name = feature
            .get("name")
            .and_then(Value::as_str)
            .ok_or("serving: a feature has no name")?;
        let field = object.get(name).unwrap_or(&null);
        let value = match feature.get("kind").and_then(Value::as_str) {
            Some("numeric") => {
                let x = match field {
                    Value::Number(n) => n.as_f64().unwrap_or(0.0),
                    Value::Bool(b) => f64::from(u8::from(*b)),
                    _ => 0.0,
                };
                match feature.get("transform").and_then(Value::as_str) {
                    Some("signlog") => x.signum() * x.abs().ln_1p(),
                    Some("zscore") => {
                        let mean = serving_f64(feature, "mean", name)?;
                        let std = serving_f64(feature, "std", name)?;
                        if std == 0.0 {
                            return Err(format!("feature `{name}`: std is zero"));
                        }
                        (x - mean) / std
                    }
                    other => return Err(format!("feature `{name}`: unknown transform {other:?}")),
                }
            }
            Some("hash") => {
                let served = feature.get("hash").and_then(Value::as_str) == Some("blake2b")
                    && feature.get("digest_size").and_then(Value::as_u64) == Some(8)
                    && feature.get("byte_order").and_then(Value::as_str) == Some("little");
                if !served {
                    return Err(format!(
                        "feature `{name}`: only blake2b, digest_size 8, little byte order is served"
                    ));
                }
                let buckets = feature
                    .get("buckets")
                    .and_then(Value::as_u64)
                    .filter(|n| *n > 0)
                    .ok_or_else(|| format!("feature `{name}` has no buckets"))?;
                let text = match field {
                    Value::Null => HASH_MISSING,
                    Value::String(s) => s.as_str(),
                    _ => return Err(format!("feature `{name}` must be a string")),
                };
                let digest = Blake2b::<U8>::digest(format!("{name}={text}").as_bytes());
                let bytes: [u8; 8] = digest.into();
                (u64::from_le_bytes(bytes) % buckets) as f64
            }
            other => return Err(format!("feature `{name}`: unknown kind {other:?}")),
        };
        row.push(value);
    }
    Ok(row)
}

fn serving_f64(feature: &Value, key: &str, name: &str) -> Result<f64, String> {
    feature
        .get(key)
        .and_then(Value::as_f64)
        .filter(|x| x.is_finite())
        .ok_or_else(|| format!("feature `{name}` needs a finite `{key}`"))
}

/// A graph input's element type, as `serving.inputs` names it: float32 for numeric features, int64
/// for hash bucket indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dtype {
    Float32,
    Int64,
}

/// One graph input's values, in its features' order.
#[derive(Debug, Clone, PartialEq)]
pub enum InputValues {
    Float32(Vec<f32>),
    Int64(Vec<i64>),
}

/// A graph input as the serving block declares it: its name (`None` for a card without
/// `serving.inputs`), its element type and the serving features that fill it, by index.
#[derive(Debug, Clone, PartialEq)]
struct ServingInput {
    name: Option<String>,
    dtype: Dtype,
    features: Vec<usize>,
}

/// The graph inputs of a serving block, in the card's order (which is the graph's order). Without
/// `inputs` the card is single-input: one float32 input carrying every feature. With `inputs`, each
/// feature belongs to exactly one input, a float32 input takes numeric features and an int64 input
/// takes hash features.
fn serving_inputs(serving: &Value) -> Result<Vec<ServingInput>, String> {
    let features = serving
        .get("features")
        .and_then(Value::as_array)
        .ok_or("serving: features must be an array")?;
    let Some(declared) = serving.get("inputs") else {
        return Ok(vec![ServingInput {
            name: None,
            dtype: Dtype::Float32,
            features: (0..features.len()).collect(),
        }]);
    };
    let declared = declared
        .as_array()
        .ok_or("serving: inputs must be an array")?;
    if declared.is_empty() {
        return Err("serving: inputs is empty".into());
    }
    let mut inputs: Vec<ServingInput> = Vec::with_capacity(declared.len());
    let mut uses = vec![0usize; features.len()];
    for input in declared {
        let name = input
            .get("name")
            .and_then(Value::as_str)
            .filter(|n| !n.is_empty())
            .ok_or("serving: an input has no name")?;
        if inputs
            .iter()
            .any(|other| other.name.as_deref() == Some(name))
        {
            return Err(format!("serving: the input `{name}` is listed twice"));
        }
        let dtype = match input.get("dtype").and_then(Value::as_str) {
            Some("float32") => Dtype::Float32,
            Some("int64") => Dtype::Int64,
            other => {
                return Err(format!(
                    "serving: input `{name}` has dtype {other:?}; only float32 and int64 are served"
                ));
            }
        };
        let wanted = match dtype {
            Dtype::Float32 => "numeric",
            Dtype::Int64 => "hash",
        };
        let names = input
            .get("features")
            .and_then(Value::as_array)
            .filter(|names| !names.is_empty())
            .ok_or_else(|| format!("serving: input `{name}` needs a non-empty features array"))?;
        let mut indices = Vec::with_capacity(names.len());
        for feature in names {
            let feature = feature.as_str().ok_or_else(|| {
                format!("serving: input `{name}` lists a feature that is not a name")
            })?;
            let index = features
                .iter()
                .position(|f| f.get("name").and_then(Value::as_str) == Some(feature))
                .ok_or_else(|| {
                    format!("serving: input `{name}` lists `{feature}`, which the serving block does not have")
                })?;
            let kind = features[index].get("kind").and_then(Value::as_str);
            if kind != Some(wanted) {
                return Err(format!(
                    "serving: input `{name}` ({dtype:?}) takes {wanted} features, but `{feature}` is {kind:?}"
                ));
            }
            uses[index] += 1;
            indices.push(index);
        }
        inputs.push(ServingInput {
            name: Some(name.to_string()),
            dtype,
            features: indices,
        });
    }
    if let Some(index) = uses.iter().position(|&n| n != 1) {
        let feature = features[index]
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("?");
        return Err(format!(
            "serving: feature `{feature}` is in {} inputs; each feature belongs to exactly one",
            uses[index]
        ));
    }
    Ok(inputs)
}

/// The model's graph inputs for one input object, in the graph's order, each with its features in
/// order: float32 for a numeric feature, the int64 bucket index for a hash feature. A card without
/// `serving.inputs` gives one float32 input carrying every feature, as [`featurize_serving`] does.
pub fn featurize_inputs(serving: &Value, input: &Value) -> Result<Vec<InputValues>, String> {
    let row = featurize_serving(serving, input)?;
    Ok(serving_inputs(serving)?
        .iter()
        .map(|declared| match declared.dtype {
            Dtype::Float32 => {
                InputValues::Float32(declared.features.iter().map(|&i| row[i] as f32).collect())
            }
            Dtype::Int64 => {
                InputValues::Int64(declared.features.iter().map(|&i| row[i] as i64).collect())
            }
        })
        .collect())
}

#[cfg(test)]
mod serving_parity {
    use super::*;

    /// The trainer's parity fixture: a serving block and rows of (input, expected features).
    const FIXTURE: &str = include_str!("../../../trainer/tests/fixtures/featurize_parity.json");

    #[test]
    fn featurize_serving_reproduces_the_trainer_rows() {
        let fixture: Value = serde_json::from_str(FIXTURE).expect("the parity fixture parses");
        let rows = fixture["rows"].as_array().expect("the fixture has rows");
        assert!(!rows.is_empty());
        for (i, row) in rows.iter().enumerate() {
            let got = featurize_serving(&fixture["serving"], &row["input"])
                .unwrap_or_else(|e| panic!("row {i}: {e}"));
            let want: Vec<f64> = row["features"]
                .as_array()
                .expect("expected features")
                .iter()
                .map(|x| x.as_f64().expect("a number"))
                .collect();
            assert_eq!(got.len(), want.len(), "row {i}: width");
            for (j, (g, w)) in got.iter().zip(&want).enumerate() {
                let hashed = fixture["serving"]["features"][j]["kind"] == "hash";
                let ok = if hashed {
                    g == w
                } else {
                    (g - w).abs() <= 1e-12
                };
                assert!(ok, "row {i} column {j}: got {g}, want {w}");
            }
        }
    }

    #[test]
    fn featurize_inputs_splits_the_parity_rows_per_graph_input() {
        let fixture: Value = serde_json::from_str(FIXTURE).expect("the parity fixture parses");
        let declared = fixture["serving"]["inputs"]
            .as_array()
            .expect("the fixture declares serving.inputs");
        for (i, row) in fixture["rows"]
            .as_array()
            .expect("the fixture has rows")
            .iter()
            .enumerate()
        {
            let split = featurize_inputs(&fixture["serving"], &row["input"])
                .unwrap_or_else(|e| panic!("row {i}: {e}"));
            assert_eq!(split.len(), declared.len(), "row {i}: inputs");
            for (input, values) in declared.iter().zip(&split) {
                let name = input["name"].as_str().expect("an input name");
                let want = row["inputs"][name]
                    .as_array()
                    .unwrap_or_else(|| panic!("row {i}: no `{name}` input in the fixture"));
                match values {
                    InputValues::Float32(got) => {
                        assert_eq!(got.len(), want.len(), "row {i} `{name}`: width");
                        for (j, (g, w)) in got.iter().zip(want).enumerate() {
                            // The trainer's float64 value cast to float32, exactly.
                            let w = w.as_f64().expect("a number") as f32;
                            assert!(*g == w, "row {i} `{name}` column {j}: got {g}, want {w}");
                        }
                    }
                    InputValues::Int64(got) => {
                        let want: Vec<i64> = want
                            .iter()
                            .map(|x| x.as_i64().expect("an integer"))
                            .collect();
                        assert_eq!(got, &want, "row {i} `{name}`");
                    }
                }
            }
        }
    }

    #[test]
    fn a_card_without_inputs_takes_every_feature_as_one_float32_input() {
        let fixture: Value = serde_json::from_str(FIXTURE).expect("the parity fixture parses");
        let mut serving = fixture["serving"].clone();
        serving
            .as_object_mut()
            .expect("a serving object")
            .remove("inputs");
        let row = &fixture["rows"][0]["input"];
        let flat: Vec<f32> = featurize_serving(&serving, row)
            .expect("the row featurizes")
            .iter()
            .map(|x| *x as f32)
            .collect();
        assert_eq!(
            featurize_inputs(&serving, row).unwrap(),
            vec![InputValues::Float32(flat)]
        );
    }

    #[test]
    fn logits_are_softmaxed_even_when_they_look_like_probabilities() {
        let labels: Vec<String> = ["cheap", "mid", "frontier"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let out = classify(&labels, &[0.2, 0.3, 0.5], true).unwrap();
        let p = out["probabilities"]["frontier"].as_f64().unwrap();
        let total: f64 = out["probabilities"]
            .as_object()
            .unwrap()
            .values()
            .map(|v| v.as_f64().unwrap())
            .sum();
        assert_eq!(out["label"], "frontier");
        assert!((p - 0.5).abs() > 0.1 && (total - 1.0).abs() < 1e-9);
    }
}

/// FNV-1a, 64-bit. Unlike `DefaultHasher` its output is fixed, so a hashed feature keeps its
/// columns across builds and machines.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl ModelCard {
    fn check(&self) -> Result<(), String> {
        if self.id.is_empty() {
            return Err("the card has no id".into());
        }
        self.input.check("input")?;
        self.output.check("output")?;
        if self.features.is_empty() {
            return Err("the card has no features".into());
        }
        for feature in &self.features {
            feature.check()?;
        }
        if let Some(labels) = &self.labels {
            if labels.is_empty() {
                return Err("the card's labels are empty".into());
            }
            for (i, label) in labels.iter().enumerate() {
                if labels[..i].contains(label) {
                    return Err(format!("the card lists the label `{label}` twice"));
                }
            }
        }
        Ok(())
    }
}

/// The input shape the loader pins on the graph: one row of `width` features, `[1, width]`. It is set
/// before the graph is analysed and optimised, so a symbolic batch dim elsewhere cannot fail to unify.
pub(crate) fn input_shape(width: usize) -> [usize; 2] {
    [1, width]
}

impl LoadedModel {
    /// Compiles `onnx` for the card: checks the card and the bytes' SHA-256, checks the graph's inputs
    /// are the card's (by count and name, in graph order), pins each input to one row of its width and
    /// dtype, and optimizes the graph.
    pub fn load(onnx: &[u8], card: ModelCard) -> Result<Self, String> {
        card.check().map_err(|e| format!("model card: {e}"))?;
        let digest = sha256_hex(onnx);
        if !digest.eq_ignore_ascii_case(&card.onnx_sha256) {
            return Err(format!(
                "onnx sha256 is {digest}, the card says {}",
                card.onnx_sha256
            ));
        }
        let pins = input_pins(&card).map_err(|e| format!("model card: {e}"))?;
        let mut model = tract_onnx::onnx()
            .model_for_read(&mut Cursor::new(onnx))
            .map_err(|e| format!("reading the ONNX graph: {e:#}"))?;
        let graph: Vec<String> = model
            .input_outlets()
            .map_err(|e| format!("the ONNX graph's inputs: {e:#}"))?
            .iter()
            .map(|outlet| model.node(outlet.node).name.clone())
            .collect();
        check_graph_inputs(&graph, &pins)?;
        // The declared output facts can carry a symbolic batch dim ("batch") that the pinned input
        // contradicts during analysis. Clear them so tract infers each output from the graph.
        let outputs = model
            .output_outlets()
            .map_err(|e| format!("the ONNX graph's outputs: {e:#}"))?
            .len();
        for output in 0..outputs {
            model
                .set_output_fact(output, InferenceFact::default())
                .map_err(|e| format!("clearing the declared fact of output {output}: {e:#}"))?;
        }
        let mut pinned = model;
        for (i, pin) in pins.iter().enumerate() {
            let fact = match pin.dtype {
                Dtype::Float32 => {
                    InferenceFact::dt_shape(f32::datum_type(), input_shape(pin.width))
                }
                Dtype::Int64 => InferenceFact::dt_shape(i64::datum_type(), input_shape(pin.width)),
            };
            pinned = pinned
                .with_input_fact(i, fact)
                .map_err(|e| format!("pinning the ONNX graph's input {i}: {e:#}"))?;
        }
        let plan = pinned
            .into_optimized()
            .and_then(|m| m.into_runnable())
            .map_err(|e| {
                format!(
                    "the ONNX graph does not take the card's {}: {e:#}",
                    describe_pins(&pins)
                )
            })?;
        Ok(Self {
            card,
            plan,
            inputs: pins,
        })
    }

    #[allow(dead_code)] // for holders of a loaded model alone; the registry reads its own records
    pub fn card(&self) -> &ModelCard {
        &self.card
    }

    /// Validates `input`, runs the model and shapes the output, which is checked against the
    /// card's output schema before it is returned. A classifier answers `{label, probabilities}`;
    /// any other model answers `{values}`.
    pub fn invoke(&self, input: &Value) -> Result<Value, String> {
        self.card
            .input
            .validate(input)
            .map_err(|e| format!("input: {e}"))?;
        let values = match &self.card.serving {
            Some(serving) => featurize_inputs(serving, input)?,
            None => vec![InputValues::Float32(featurize(&self.card.features, input)?)],
        };
        if values.len() != self.inputs.len() {
            return Err(format!(
                "the card's features give {} inputs but the model takes {}",
                values.len(),
                self.inputs.len()
            ));
        }
        // One row per graph input, in the graph's order (the loader checked the names).
        let mut tensors = tvec!();
        for (pin, value) in self.inputs.iter().zip(values) {
            let (width, tensor) = match value {
                InputValues::Float32(row) => (row.len(), Tensor::from_shape(&[1, row.len()], &row)),
                InputValues::Int64(row) => (row.len(), Tensor::from_shape(&[1, row.len()], &row)),
            };
            if width != pin.width {
                return Err(width_error(pin, width));
            }
            let tensor = tensor.map_err(|e| format!("input tensor: {e:#}"))?;
            tensors.push(TValue::from(tensor));
        }
        let outputs = self
            .plan
            .run(tensors)
            .map_err(|e| format!("running the model: {e:#}"))?;
        let first = outputs.first().ok_or("the model has no outputs")?;
        let scores: Vec<f32> = first
            .to_plain_array_view::<f32>()
            .map_err(|e| format!("the model's output is not f32: {e:#}"))?
            .iter()
            .copied()
            .collect();
        if scores.iter().any(|s| !s.is_finite()) {
            return Err("the model produced a non-finite score".into());
        }
        let logits = self
            .card
            .serving
            .as_ref()
            .and_then(|s| s.get("output_activation"))
            .and_then(Value::as_str)
            == Some("logits");
        let output = match &self.card.labels {
            Some(labels) => classify(labels, &scores, logits)?,
            None => json!({ "values": scores }),
        };
        self.card
            .output
            .validate(&output)
            .map_err(|e| format!("output: {e}"))?;
        Ok(output)
    }
}

/// `{label, probabilities}` for the best class. The scores are used as they are when they already
/// are probabilities (in [0, 1], summing to 1) and the card does not say `logits`, else softmaxed.
fn classify(labels: &[String], scores: &[f32], logits: bool) -> Result<Value, String> {
    if scores.len() != labels.len() {
        return Err(format!(
            "the model produced {} scores for {} labels",
            scores.len(),
            labels.len()
        ));
    }
    let sum: f64 = scores.iter().map(|&s| f64::from(s)).sum();
    let probabilities: Vec<f64> =
        if !logits && scores.iter().all(|s| (0.0..=1.0).contains(s)) && (sum - 1.0).abs() < 1e-3 {
            scores.iter().map(|&s| f64::from(s)).collect()
        } else {
            softmax(scores)
        };
    let mut best = 0;
    for (i, p) in probabilities.iter().enumerate() {
        if *p > probabilities[best] {
            best = i;
        }
    }
    let mut by_label = Map::new();
    for (label, p) in labels.iter().zip(&probabilities) {
        by_label.insert(label.clone(), json!(p));
    }
    Ok(json!({ "label": labels[best], "probabilities": by_label }))
}

fn softmax(scores: &[f32]) -> Vec<f64> {
    let max = scores
        .iter()
        .map(|&s| f64::from(s))
        .fold(f64::MIN, f64::max);
    let exps: Vec<f64> = scores.iter().map(|&s| (f64::from(s) - max).exp()).collect();
    let total: f64 = exps.iter().sum();
    exps.iter().map(|e| e / total).collect()
}

/// The loaded models by id. Shared by the foundry's routes and the `invoke_model` tool.
#[derive(Default)]
pub struct Registry {
    models: Mutex<HashMap<String, Arc<LoadedModel>>>,
}

impl Registry {
    fn models(&self) -> MutexGuard<'_, HashMap<String, Arc<LoadedModel>>> {
        self.models
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Adds the model under its card's id, returning the one it replaced.
    pub fn insert(&self, model: LoadedModel) -> Option<Arc<LoadedModel>> {
        let id = model.card.id.clone();
        self.models().insert(id, Arc::new(model))
    }

    pub fn get(&self, id: &str) -> Option<Arc<LoadedModel>> {
        self.models().get(id).cloned()
    }

    pub fn remove(&self, id: &str) -> Option<Arc<LoadedModel>> {
        self.models().remove(id)
    }

    /// The cards of every loaded model, by id.
    #[allow(dead_code)] // the registry lists its own records; kept for callers of this registry
    pub fn list(&self) -> Vec<ModelCard> {
        let mut cards: Vec<ModelCard> = self.models().values().map(|m| m.card.clone()).collect();
        cards.sort_by(|a, b| a.id.cmp(&b.id));
        cards
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixtures: a 3-input -> 3-class graph with fixed weights W = [[2,-1,-1],[-1,2,-1],[-1,-1,2]],
    // B = 0. linear3.onnx ends in Softmax (its output is probabilities); linear3_logits.onnx ends
    // in Identity (raw scores). Regenerate with (needs onnx and numpy):
    //
    //   python3 - <<'EOF'
    //   import onnx, numpy as np
    //   from onnx import helper as h, numpy_helper as nh, TensorProto as T
    //   W = np.array([[2, -1, -1], [-1, 2, -1], [-1, -1, 2]], np.float32)
    //   B = np.zeros(3, np.float32)
    //   x = h.make_tensor_value_info("x", T.FLOAT, [1, 3])
    //   def save(name, last):
    //       nodes = [h.make_node("MatMul", ["x", "W"], ["h"]), h.make_node("Add", ["h", "B"], ["z"])] + last
    //       y = h.make_tensor_value_info("y", T.FLOAT, [1, 3])
    //       g = h.make_graph(nodes, name, [x], [y], initializer=[nh.from_array(W, "W"), nh.from_array(B, "B")])
    //       m = h.make_model(g, opset_imports=[h.make_opsetid("", 13)])
    //       onnx.checker.check_model(m)
    //       onnx.save(m, f"crates/apid/tests/fixtures/{name}.onnx")
    //   save("linear3", [h.make_node("Softmax", ["z"], ["y"], axis=1)])
    //   save("linear3_logits", [h.make_node("Identity", ["z"], ["y"])])
    //   EOF
    const LINEAR3: &[u8] = include_bytes!("../tests/fixtures/linear3.onnx");
    const LINEAR3_LOGITS: &[u8] = include_bytes!("../tests/fixtures/linear3_logits.onnx");

    const CLASSES: [&str; 3] = ["cat", "dog", "bird"];

    fn labels() -> Vec<String> {
        CLASSES.iter().map(|s| s.to_string()).collect()
    }

    fn classifier_output() -> Schema {
        serde_json::from_value(json!({
            "type": "object",
            "properties": {
                "label": { "type": "string", "enum": CLASSES },
                "probabilities": { "type": "object" }
            },
            "required": ["label", "probabilities"]
        }))
        .unwrap()
    }

    fn numeric_input(names: &[&str]) -> Schema {
        let properties: Map<String, Value> = names
            .iter()
            .map(|n| (n.to_string(), json!({ "type": "number" })))
            .collect();
        serde_json::from_value(
            json!({ "type": "object", "properties": properties, "required": names }),
        )
        .unwrap()
    }

    fn numeric(name: &str) -> FeatureSpec {
        FeatureSpec {
            name: name.into(),
            kind: FeatureKind::Numeric,
            values: vec![],
            buckets: None,
            scale: None,
        }
    }

    fn card(
        onnx: &[u8],
        input: Schema,
        features: Vec<FeatureSpec>,
        labels: Option<Vec<String>>,
    ) -> ModelCard {
        ModelCard {
            id: "linear3".into(),
            name: "Linear three".into(),
            version: "1".into(),
            input,
            output: classifier_output(),
            features,
            labels,
            allowed_roles: vec!["breeder".into()],
            onnx_sha256: sha256_hex(onnx),
            created_ms: 1,
            metrics: Value::Null,
            serving: None,
        }
    }

    fn xyz_card(onnx: &[u8]) -> ModelCard {
        card(
            onnx,
            numeric_input(&["x0", "x1", "x2"]),
            vec![numeric("x0"), numeric("x1"), numeric("x2")],
            Some(labels()),
        )
    }

    fn input_schema(value: Value) -> Schema {
        serde_json::from_value(value).unwrap()
    }

    fn approx(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-4, "{a} is not {b}");
    }

    #[test]
    fn schema_rejects_bad_input() {
        // A missing required field is named in the error.
        {
            let schema = numeric_input(&["x0", "x1"]);
            let err = schema.validate(&json!({ "x0": 1.0 })).unwrap_err();
            assert_eq!(err, "missing required field `x1`");
        }
        // Each field must match its declared type, and the input must be an object.
        {
            let schema = input_schema(json!({
                "type": "object",
                "properties": { "n": { "type": "integer" }, "s": { "type": "string" }, "b": { "type": "boolean" } },
                "required": ["n", "s", "b"]
            }));
            assert!(
                schema
                    .validate(&json!({ "n": 3.0, "s": "a", "b": true }))
                    .is_ok()
            );
            assert_eq!(
                schema
                    .validate(&json!({ "n": 1.5, "s": "a", "b": true }))
                    .unwrap_err(),
                "field `n` must be a integer"
            );
            assert_eq!(
                schema
                    .validate(&json!({ "n": 1, "s": 7, "b": true }))
                    .unwrap_err(),
                "field `s` must be a string"
            );
            assert_eq!(
                schema
                    .validate(&json!({ "n": 1, "s": "a", "b": "yes" }))
                    .unwrap_err(),
                "field `b` must be a boolean"
            );
            assert_eq!(
                schema.validate(&json!([1, 2])).unwrap_err(),
                "must be a JSON object"
            );
        }
        // An enum field takes only its listed values, and unknown fields are refused.
        {
            let schema = input_schema(json!({
                "type": "object",
                "properties": { "color": { "type": "string", "enum": ["red", "blue"] } },
                "required": ["color"]
            }));
            assert!(schema.validate(&json!({ "color": "red" })).is_ok());
            let err = schema.validate(&json!({ "color": "green" })).unwrap_err();
            assert!(err.starts_with("field `color` must be one of"), "{err}");
            assert_eq!(
                schema
                    .validate(&json!({ "color": "red", "size": 2 }))
                    .unwrap_err(),
                "unknown field `size`"
            );
        }
    }

    #[test]
    fn schema_check_refuses_malformed_schemas() {
        let not_object = input_schema(json!({ "type": "array", "properties": {}, "required": [] }));
        assert!(not_object.check("input").is_err());
        let undeclared =
            input_schema(json!({ "type": "object", "properties": {}, "required": ["x"] }));
        assert!(undeclared.check("input").is_err());
        let mistyped = input_schema(json!({
            "type": "object", "properties": { "n": { "type": "number", "enum": ["one"] } }, "required": []
        }));
        assert!(mistyped.check("input").is_err());
    }

    #[test]
    fn featurize_onehot_and_numeric_scaling() {
        let features = vec![
            FeatureSpec {
                name: "color".into(),
                kind: FeatureKind::Onehot,
                values: vec![json!("red"), json!("green"), json!("blue")],
                buckets: None,
                scale: None,
            },
            FeatureSpec {
                name: "temp".into(),
                kind: FeatureKind::Numeric,
                values: vec![],
                buckets: None,
                scale: Some(Scale {
                    mean: 20.0,
                    std: 5.0,
                }),
            },
        ];
        assert_eq!(feature_width(&features), 4);
        let row = featurize(&features, &json!({ "color": "green", "temp": 30.0 })).unwrap();
        assert_eq!(row, vec![0.0, 1.0, 0.0, 2.0]);
        assert!(
            featurize(&features, &json!({ "color": "pink", "temp": 30.0 }))
                .unwrap_err()
                .contains("not one of its categories")
        );
        assert!(
            featurize(&features, &json!({ "color": "red" }))
                .unwrap_err()
                .contains("missing feature field `temp`")
        );
    }

    #[test]
    fn featurize_hash_is_stable_and_one_hot() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        let features = vec![FeatureSpec {
            name: "tag".into(),
            kind: FeatureKind::Hash,
            values: vec![],
            buckets: Some(8),
            scale: None,
        }];
        let a = featurize(&features, &json!({ "tag": "invoice" })).unwrap();
        let b = featurize(&features, &json!({ "tag": "invoice" })).unwrap();
        assert_eq!(a.len(), 8);
        assert_eq!(a, b);
        assert_eq!(a.iter().filter(|x| **x == 1.0).count(), 1);
        assert_eq!(a.iter().sum::<f32>(), 1.0);
        let hot = (fnv1a64(b"invoice") % 8) as usize;
        assert_eq!(a[hot], 1.0);
    }

    #[test]
    fn load_refuses_a_bad_card() {
        // A zero feature scale, duplicate labels and a wrong onnx hash are each refused at load.
        {
            let mut bad = xyz_card(LINEAR3);
            bad.features[0].scale = Some(Scale {
                mean: 0.0,
                std: 0.0,
            });
            assert!(LoadedModel::load(LINEAR3, bad).is_err());
            let mut dup = xyz_card(LINEAR3);
            dup.labels = Some(vec!["cat".into(), "cat".into(), "bird".into()]);
            assert!(LoadedModel::load(LINEAR3, dup).is_err());
            let mut wrong_hash = xyz_card(LINEAR3);
            wrong_hash.onnx_sha256 = "0".repeat(64);
            assert!(
                LoadedModel::load(LINEAR3, wrong_hash)
                    .err()
                    .unwrap()
                    .contains("onnx sha256 is")
            );
        }
        // A serving block listing 4 features must not load a 3-input graph.
        {
            let mut card = xyz_card(LINEAR3);
            card.serving = Some(json!({
                "features": (0..4)
                    .map(|i| json!({ "name": format!("x{i}"), "kind": "numeric", "transform": "signlog" }))
                    .collect::<Vec<_>>(),
                "output_activation": "logits"
            }));
            let err = LoadedModel::load(LINEAR3, card)
                .err()
                .expect("a 4-feature serving block must not load a 3-input graph");
            assert!(err.contains("does not take the card's 4 features"), "{err}");
        }
        // linear3 takes one input, which the card does not name `num`; a two-input
        // split and an int64 input are refused too.
        {
            let numeric_feature =
                |n: &str| json!({ "name": n, "kind": "numeric", "transform": "signlog" });
            let mut card = xyz_card(LINEAR3);
            card.serving = Some(json!({
                "features": [numeric_feature("x0"), numeric_feature("x1"), numeric_feature("x2")],
                "inputs": [{ "name": "num", "dtype": "float32", "features": ["x0", "x1", "x2"] }]
            }));
            let err = LoadedModel::load(LINEAR3, card.clone())
                .err()
                .expect("a mismatched input name must not load");
            assert!(err.contains("serving.inputs names `num`"), "{err}");

            card.serving = Some(json!({
                "features": [numeric_feature("x0"), numeric_feature("x1"), numeric_feature("x2")],
                "inputs": [
                    { "name": "a", "dtype": "float32", "features": ["x0"] },
                    { "name": "b", "dtype": "float32", "features": ["x1", "x2"] }
                ]
            }));
            let err = LoadedModel::load(LINEAR3, card.clone())
                .err()
                .expect("two declared inputs must not load a one-input graph");
            assert!(
                err.contains("takes 1 inputs") && err.contains("declares 2"),
                "{err}"
            );

            card.serving = Some(json!({
                "features": [numeric_feature("x0"), numeric_feature("x1"), numeric_feature("x2")],
                "inputs": [{ "name": "x", "dtype": "int64", "features": ["x0", "x1", "x2"] }]
            }));
            let err = LoadedModel::load(LINEAR3, card)
                .err()
                .expect("an int64 input takes hash features only");
            assert!(err.contains("takes hash features"), "{err}");
        }
    }

    #[test]
    fn inference_gives_the_expected_class_and_probabilities() {
        // A known input gives the expected class, and the probabilities sum to 1 and match the softmax.
        {
            let model = LoadedModel::load(LINEAR3, xyz_card(LINEAR3)).unwrap();
            let out = model
                .invoke(&json!({ "x0": 1.0, "x1": 0.0, "x2": 0.0 }))
                .unwrap();
            assert_eq!(out["label"], "cat");
            let probs = out["probabilities"].as_object().unwrap();
            let total: f64 = probs.values().map(|p| p.as_f64().unwrap()).sum();
            approx(total, 1.0);
            // softmax([2, -1, -1]) = [e^2, e^-1, e^-1] / (e^2 + 2 e^-1)
            approx(probs["cat"].as_f64().unwrap(), 0.909_465);
            approx(probs["dog"].as_f64().unwrap(), 0.045_268);
            approx(probs["bird"].as_f64().unwrap(), 0.045_268);
            let dog = model
                .invoke(&json!({ "x0": 0.0, "x1": 1.0, "x2": 0.0 }))
                .unwrap();
            assert_eq!(dog["label"], "dog");
        }
        // Raw logits are softmaxed to the same probabilities.
        {
            let model = LoadedModel::load(LINEAR3_LOGITS, xyz_card(LINEAR3_LOGITS)).unwrap();
            let out = model
                .invoke(&json!({ "x0": 1.0, "x1": 0.0, "x2": 0.0 }))
                .unwrap();
            assert_eq!(out["label"], "cat");
            approx(out["probabilities"]["cat"].as_f64().unwrap(), 0.909_465);
            let total: f64 = out["probabilities"]
                .as_object()
                .unwrap()
                .values()
                .map(|p| p.as_f64().unwrap())
                .sum();
            approx(total, 1.0);
        }
    }

    #[test]
    fn featurized_inputs_reach_the_model_scaled() {
        // Input length 3 = onehot(2) + numeric(1). With temp scaled by (mean 1, std 0.5), temp = 1
        // becomes 0, so the row is [0, 1, 0] and the class is dog. Unscaled it would be [0, 1, 1].
        let features = vec![
            FeatureSpec {
                name: "color".into(),
                kind: FeatureKind::Onehot,
                values: vec![json!("a"), json!("b")],
                buckets: None,
                scale: None,
            },
            FeatureSpec {
                name: "temp".into(),
                kind: FeatureKind::Numeric,
                values: vec![],
                buckets: None,
                scale: Some(Scale {
                    mean: 1.0,
                    std: 0.5,
                }),
            },
        ];
        let input = input_schema(json!({
            "type": "object",
            "properties": { "color": { "type": "string", "enum": ["a", "b"] }, "temp": { "type": "number" } },
            "required": ["color", "temp"]
        }));
        let model =
            LoadedModel::load(LINEAR3, card(LINEAR3, input, features, Some(labels()))).unwrap();
        let out = model.invoke(&json!({ "color": "b", "temp": 1.0 })).unwrap();
        assert_eq!(out["label"], "dog");
        approx(out["probabilities"]["dog"].as_f64().unwrap(), 0.909_465);
    }

    #[test]
    fn invoke_validates_input_and_output() {
        let model = LoadedModel::load(LINEAR3, xyz_card(LINEAR3)).unwrap();
        assert!(
            model
                .invoke(&json!({ "x0": 1.0, "x1": 0.0 }))
                .unwrap_err()
                .contains("missing required field `x2`")
        );
        assert!(
            model
                .invoke(&json!({ "x0": "1", "x1": 0.0, "x2": 0.0 }))
                .unwrap_err()
                .contains("must be a number")
        );
        // An output schema that does not allow "cat" rejects the answer the model gives.
        let mut strict = xyz_card(LINEAR3);
        strict.output.properties.get_mut("label").unwrap().allowed =
            Some(vec![json!("dog"), json!("bird")]);
        let strict = LoadedModel::load(LINEAR3, strict).unwrap();
        let err = strict
            .invoke(&json!({ "x0": 1.0, "x1": 0.0, "x2": 0.0 }))
            .unwrap_err();
        assert!(
            err.starts_with("output: field `label` must be one of"),
            "{err}"
        );
    }

    #[test]
    fn a_regression_card_answers_values() {
        let mut regression = xyz_card(LINEAR3_LOGITS);
        regression.labels = None;
        regression.output = input_schema(json!({
            "type": "object",
            "properties": { "values": { "type": "array" } },
            "required": ["values"]
        }));
        let model = LoadedModel::load(LINEAR3_LOGITS, regression).unwrap();
        let out = model
            .invoke(&json!({ "x0": 1.0, "x1": 0.0, "x2": 0.0 }))
            .unwrap();
        let values: Vec<f64> = out["values"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        assert_eq!(values.len(), 3);
        approx(values[0], 2.0);
        approx(values[1], -1.0);
    }

    #[test]
    fn a_dynamic_batch_graph_loads_and_runs_one_row() {
        // Exported with dynamic_axes on the input and the output (trainer/tests/make_tiny_onnx.py):
        // the declared output is ["batch", 3], which the loader must not let fail the analysis.
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/tiny_dynamic.onnx");
        let onnx = std::fs::read(&path).expect("the tiny_dynamic.onnx fixture");
        let names = ["x0", "x1", "x2", "x3", "x4", "x5", "x6", "x7", "x8", "x9"];
        let features: Vec<FeatureSpec> = names.iter().map(|n| numeric(n)).collect();
        let model = LoadedModel::load(&onnx, card(&onnx, numeric_input(&names), features, None))
            .expect("the dynamic-batch fixture loads");
        let row = Tensor::from_shape(&[1, 10], &[0.5f32; 10]).unwrap();
        let out = model.plan.run(tvec!(TValue::from(row))).unwrap();
        assert_eq!(out[0].shape(), &[1, 3]);
    }

    #[test]
    fn invoke_refuses_a_feature_vector_of_the_wrong_width() {
        let mut model = LoadedModel::load(LINEAR3, xyz_card(LINEAR3)).unwrap();
        model.inputs[0].width = 4;
        let err = model
            .invoke(&json!({ "x0": 1.0, "x1": 0.0, "x2": 0.0 }))
            .unwrap_err();
        assert!(
            err.contains("the feature vector has 3 columns but the model takes 4"),
            "{err}"
        );
    }

    /// A served hash feature over `buckets` buckets, as a serving block lists it.
    fn hash_feature(name: &str, buckets: u64) -> Value {
        json!({
            "name": name, "kind": "hash", "hash": "blake2b", "digest_size": 8,
            "buckets": buckets, "input": "key=value", "byte_order": "little"
        })
    }

    /// A text whose hash feature `name` lands in bucket `want`, found with the served featurization.
    fn text_for_bucket(name: &str, buckets: u64, want: u64) -> String {
        let serving = json!({ "features": [hash_feature(name, buckets)] });
        (0u64..)
            .map(|i| format!("v{i}"))
            .find(|text| {
                let mut input = Map::new();
                input.insert(name.to_string(), json!(text));
                featurize_serving(&serving, &Value::Object(input)).unwrap()[0] == want as f64
            })
            .expect("some text lands in every bucket")
    }

    /// The card of the trainer's two-input graph: `num` is `num_width` numeric features passed through
    /// (zscore with mean 0 and std 1), `cat` is `cat_width` hash features over eight buckets.
    fn two_input_card(onnx: &[u8], num_width: usize, cat_width: usize) -> ModelCard {
        let num: Vec<String> = (0..num_width).map(|j| format!("n{j}")).collect();
        let cat: Vec<String> = (0..cat_width).map(|j| format!("c{j}")).collect();
        let mut features: Vec<Value> = num
            .iter()
            .map(|n| json!({ "name": n, "kind": "numeric", "transform": "zscore", "mean": 0.0, "std": 1.0 }))
            .collect();
        features.extend(cat.iter().map(|c| hash_feature(c, 8)));
        let mut properties = Map::new();
        for n in &num {
            properties.insert(n.clone(), json!({ "type": "number" }));
        }
        for c in &cat {
            properties.insert(c.clone(), json!({ "type": "string" }));
        }
        let required: Vec<&String> = num.iter().chain(&cat).collect();
        ModelCard {
            id: "two_input".into(),
            name: "Two input".into(),
            version: "1".into(),
            input: serde_json::from_value(json!({
                "type": "object", "properties": properties, "required": required
            }))
            .unwrap(),
            output: serde_json::from_value(json!({
                "type": "object", "properties": { "values": { "type": "array" } }, "required": ["values"]
            }))
            .unwrap(),
            features: num
                .iter()
                .map(|n| numeric(n))
                .chain(cat.iter().map(|c| FeatureSpec {
                    name: c.clone(),
                    kind: FeatureKind::Hash,
                    values: vec![],
                    buckets: Some(8),
                    scale: None,
                }))
                .collect(),
            labels: None,
            allowed_roles: vec!["breeder".into()],
            onnx_sha256: sha256_hex(onnx),
            created_ms: 1,
            metrics: Value::Null,
            serving: Some(json!({
                "features": features,
                "inputs": [
                    { "name": "num", "dtype": "float32", "features": num },
                    { "name": "cat", "dtype": "int64", "features": cat }
                ],
                "output_activation": "logits"
            })),
        }
    }

    #[test]
    fn a_two_input_graph_answers_the_trainer_logits() {
        // Written by trainer/tests/make_two_input_onnx.py: the graph and two rows of num, cat and logits.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
        let onnx = std::fs::read(dir.join("tiny_two_input.onnx"))
            .expect("crates/apid/testdata/tiny_two_input.onnx (the trainer writes it)");
        let fixture: Value = serde_json::from_slice(
            &std::fs::read(dir.join("tiny_two_input.json"))
                .expect("tiny_two_input.json, written with the .onnx"),
        )
        .expect("tiny_two_input.json parses");
        let num = fixture["num"].as_array().expect("num rows");
        let cat = fixture["cat"].as_array().expect("cat rows");
        let logits = fixture["logits"].as_array().expect("logits rows");
        // The widths come from the trainer's rows, not from a constant.
        let num_width = num[0].as_array().expect("a num row").len();
        let cat_width = cat[0].as_array().expect("a cat row").len();
        let model = LoadedModel::load(&onnx, two_input_card(&onnx, num_width, cat_width))
            .expect("the two-input graph loads for its card");
        for r in 0..num.len() {
            let mut input = Map::new();
            for (j, x) in num[r].as_array().expect("a num row").iter().enumerate() {
                input.insert(format!("n{j}"), x.clone());
            }
            // The embedding takes the bucket index, so each cat value is a text that hashes to it.
            for (j, bucket) in cat[r].as_array().expect("a cat row").iter().enumerate() {
                let name = format!("c{j}");
                let text = text_for_bucket(&name, 8, bucket.as_u64().expect("a bucket"));
                input.insert(name, json!(text));
            }
            let out = model
                .invoke(&Value::Object(input))
                .unwrap_or_else(|e| panic!("row {r}: {e}"));
            let got = out["values"].as_array().expect("values");
            let want = logits[r].as_array().expect("logits");
            assert_eq!(got.len(), want.len(), "row {r}: logits");
            for (k, (g, w)) in got.iter().zip(want).enumerate() {
                let (g, w) = (g.as_f64().unwrap(), w.as_f64().unwrap());
                assert!((g - w).abs() < 1e-4, "row {r} logit {k}: {g} is not {w}");
            }
        }
    }
}
