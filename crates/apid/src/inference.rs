//! Model agents' inference (docs/design/foundry.md, "The hub"). A promoted model is an ONNX graph
//! plus a model card: the card's input schema is checked, its feature specs turn the input object
//! into the model's float tensor, and its output (labels or values) comes back as a typed object
//! checked against the card's output schema. Pure Rust (tract): no native libraries, no GPU. Models
//! are only ever invoked here, never trained (that is the Foundry's trainer runtime).
//!
//! The foundry's jobs and registry routes and the `invoke_model` tool wire this up next.

// Not wired into a route yet: the foundry's registry and `invoke_model` call this next.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::sync::{Arc, Mutex, MutexGuard};

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
}

/// A model card with its ONNX graph compiled for inference at batch 1.
pub struct LoadedModel {
    card: ModelCard,
    plan: Arc<TypedRunnableModel>,
    width: usize,
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

impl LoadedModel {
    /// Compiles `onnx` for the card: checks the card and the bytes' SHA-256, then sets the input to
    /// one row of the feature width and optimizes the graph.
    pub fn load(onnx: &[u8], card: ModelCard) -> Result<Self, String> {
        card.check().map_err(|e| format!("model card: {e}"))?;
        let digest = sha256_hex(onnx);
        if !digest.eq_ignore_ascii_case(&card.onnx_sha256) {
            return Err(format!(
                "onnx sha256 is {digest}, the card says {}",
                card.onnx_sha256
            ));
        }
        let width = feature_width(&card.features);
        let model = tract_onnx::onnx()
            .model_for_read(&mut Cursor::new(onnx))
            .map_err(|e| format!("reading the ONNX graph: {e:#}"))?;
        let plan = model
            .with_input_fact(0, InferenceFact::dt_shape(f32::datum_type(), [1, width]))
            .and_then(|m| m.into_optimized())
            .and_then(|m| m.into_runnable())
            .map_err(|e| format!("compiling the ONNX graph for {width} features: {e:#}"))?;
        Ok(Self { card, plan, width })
    }

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
        let row = featurize(&self.card.features, input)?;
        debug_assert_eq!(row.len(), self.width);
        let tensor = Tensor::from_shape(&[1, row.len()], &row)
            .map_err(|e| format!("input tensor: {e:#}"))?;
        let outputs = self
            .plan
            .run(tvec!(TValue::from(tensor)))
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
        let output = match &self.card.labels {
            Some(labels) => classify(labels, &scores)?,
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
/// are probabilities (in [0, 1], summing to 1), else softmaxed.
fn classify(labels: &[String], scores: &[f32]) -> Result<Value, String> {
    if scores.len() != labels.len() {
        return Err(format!(
            "the model produced {} scores for {} labels",
            scores.len(),
            labels.len()
        ));
    }
    let sum: f64 = scores.iter().map(|&s| f64::from(s)).sum();
    let probabilities: Vec<f64> =
        if scores.iter().all(|s| (0.0..=1.0).contains(s)) && (sum - 1.0).abs() < 1e-3 {
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
    fn schema_rejects_a_missing_field() {
        let schema = numeric_input(&["x0", "x1"]);
        let err = schema.validate(&json!({ "x0": 1.0 })).unwrap_err();
        assert_eq!(err, "missing required field `x1`");
    }

    #[test]
    fn schema_rejects_a_wrong_type() {
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

    #[test]
    fn schema_rejects_a_bad_enum_and_unknown_fields() {
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
    fn card_checks_refuse_bad_features_and_labels() {
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

    #[test]
    fn a_known_input_gives_the_expected_class_and_probabilities() {
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

    #[test]
    fn raw_logits_are_softmaxed_to_the_same_probabilities() {
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
    fn the_registry_holds_loaded_models_by_id() {
        fn assert_shared<T: Send + Sync>() {}
        assert_shared::<Registry>();
        let registry = Registry::default();
        assert!(registry.list().is_empty());
        assert!(
            registry
                .insert(LoadedModel::load(LINEAR3, xyz_card(LINEAR3)).unwrap())
                .is_none()
        );
        let replaced =
            registry.insert(LoadedModel::load(LINEAR3_LOGITS, xyz_card(LINEAR3_LOGITS)).unwrap());
        assert!(replaced.is_some());
        let ids: Vec<String> = registry.list().into_iter().map(|c| c.id).collect();
        assert_eq!(ids, vec!["linear3".to_string()]);
        let got = registry.get("linear3").unwrap();
        assert_eq!(got.card().name, "Linear three");
        assert!(registry.remove("linear3").is_some());
        assert!(registry.get("linear3").is_none());
    }
}
