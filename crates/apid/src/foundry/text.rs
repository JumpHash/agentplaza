//! Text models served by the hub (`docs/design/foundry.md`, "Hugging Face", phase 2). An ONNX graph runs with
//! tract beside its `tokenizer.json`, read offline by the `tokenizers` crate. Two tasks:
//! - `text-classification`: `{text}` -> `{label, scores: [{label, score}]}`, softmax scores, labels from
//!   config.json's `id2label` (highest first);
//! - `feature-extraction`: `{text}` -> `{embedding}`, the mean of the unmasked token vectors, L2-normalised.
//!
//! Inputs are padded or truncated to the contract's `max_len`, so the graph runs at a fixed shape. The
//! derived contract is written to the download's `model_card.json` as its `serving` block.

use std::fs;
use std::io::Cursor;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};
use tract_onnx::prelude::{
    Framework, InferenceModelExt, IntoRunnable, TValue, Tensor, TypedRunnableModel, tvec,
};

use crate::inference::{PropType, Property, Schema};

pub const CLASSIFICATION: &str = "text-classification";
pub const EMBEDDING: &str = "feature-extraction";
/// Longest input the hub pads to when the config allows more (a longer input is truncated).
const MAX_LEN_CAP: usize = 128;
const FALLBACK_MAX_POSITIONS: usize = 512;

/// The serving contract of a text model: what `serving` holds in its card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextContract {
    /// Always `"text"`: the input is `{text}`.
    pub kind: String,
    pub task: String,
    pub max_len: usize,
    /// Class names by index (classification only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    /// The ONNX graph, relative to the download directory.
    pub graph: String,
}

/// The tasks the text runtime serves.
pub fn supported(task: &str) -> bool {
    task == CLASSIFICATION || task == EMBEDDING
}

/// The relative path of the download's ONNX graph: `model.onnx`, `onnx/model.onnx`, or the one `.onnx`
/// file under `onnx/` (a repo with several quantizations must be imported with one of them).
pub fn graph_in(dir: &Path) -> Result<String, String> {
    for candidate in ["model.onnx", "onnx/model.onnx"] {
        if dir.join(candidate).exists() {
            return Ok(candidate.to_string());
        }
    }
    let mut found: Vec<String> = fs::read_dir(dir.join("onnx"))
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|name| name.ends_with(".onnx"))
                .map(|name| format!("onnx/{name}"))
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err("no ONNX graph in the download".into()),
        n => Err(format!(
            "{n} ONNX graphs in the download: import the one you want by its file name"
        )),
    }
}

/// Derives the contract from the download's `config.json` (labels, length) and the repo's task.
pub fn derive(dir: &Path, graph: &str, task: &str) -> Result<TextContract, String> {
    if !supported(task) {
        return Err(format!(
            "the task {task:?} is not served yet (text-classification and feature-extraction are)"
        ));
    }
    if !dir.join("tokenizer.json").exists() {
        return Err("the download has no tokenizer.json".into());
    }
    let config: Value = fs::read_to_string(dir.join("config.json"))
        .map_err(|e| format!("reading config.json: {e}"))
        .and_then(|text| serde_json::from_str(&text).map_err(|e| format!("config.json: {e}")))?;
    let positions = config
        .get("max_position_embeddings")
        .and_then(Value::as_u64)
        .map_or(FALLBACK_MAX_POSITIONS, |n| n as usize);
    let labels = if task == CLASSIFICATION {
        Some(labels_of(&config)?)
    } else {
        None
    };
    Ok(TextContract {
        kind: "text".into(),
        task: task.to_string(),
        max_len: positions.min(MAX_LEN_CAP).max(2),
        labels,
        graph: graph.to_string(),
    })
}

/// Class names from config.json's `id2label`, by index.
fn labels_of(config: &Value) -> Result<Vec<String>, String> {
    let map = config
        .get("id2label")
        .and_then(Value::as_object)
        .ok_or("config.json has no id2label")?;
    let mut labels = Vec::with_capacity(map.len());
    for index in 0..map.len() {
        let name = map
            .get(&index.to_string())
            .and_then(Value::as_str)
            .ok_or_else(|| format!("config.json's id2label has no class {index}"))?;
        labels.push(name.to_string());
    }
    Ok(labels)
}

/// The input and output schemas a text task answers with, for the card.
pub fn schemas(task: &str) -> (Schema, Schema) {
    let prop = |kind| Property {
        kind,
        allowed: None,
    };
    let input = Schema {
        kind: "object".into(),
        properties: [("text".to_string(), prop(PropType::String))]
            .into_iter()
            .collect(),
        required: vec!["text".into()],
    };
    let output_props: Vec<(&str, PropType)> = if task == CLASSIFICATION {
        vec![("label", PropType::String), ("scores", PropType::Array)]
    } else {
        vec![("embedding", PropType::Array)]
    };
    let output = Schema {
        kind: "object".into(),
        properties: output_props
            .into_iter()
            .map(|(name, kind)| (name.to_string(), prop(kind)))
            .collect(),
        required: Vec::new(),
    };
    (input, output)
}

/// Writes the contract into `model_card.json` as its `serving` block (the download's own card, when it
/// has one, keeps its other fields).
pub fn write_contract(dir: &Path, contract: &TextContract) -> Result<(), String> {
    let path = dir.join("model_card.json");
    let mut card: Value = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({}));
    let serving = serde_json::to_value(contract).map_err(|e| e.to_string())?;
    if let Value::Object(map) = &mut card {
        map.insert("serving".into(), serving);
    }
    let text = serde_json::to_string_pretty(&card).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| format!("writing model_card.json: {e}"))
}

/// A loaded text model: the tokenizer and the graph at the contract's fixed length.
pub struct TextModel {
    contract: TextContract,
    tokenizer: Tokenizer,
    plan: std::sync::Arc<TypedRunnableModel>,
    /// The graph's inputs, in its order.
    inputs: Vec<String>,
}

impl TextModel {
    /// Loads the graph and `tokenizer.json` from `dir`. Offline: nothing is fetched.
    pub fn load(dir: &Path, contract: &TextContract) -> Result<TextModel, String> {
        let max_len = contract.max_len;
        let mut tokenizer = Tokenizer::from_file(dir.join("tokenizer.json"))
            .map_err(|e| format!("reading tokenizer.json: {e}"))?;
        let pad_id = tokenizer.token_to_id("[PAD]").unwrap_or(0);
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: max_len,
                ..Default::default()
            }))
            .map_err(|e| format!("the tokenizer's truncation: {e}"))?;
        tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::Fixed(max_len),
            pad_id,
            pad_token: "[PAD]".into(),
            ..Default::default()
        }));

        let bytes = fs::read(dir.join(&contract.graph))
            .map_err(|e| format!("reading {}: {e}", contract.graph))?;
        let model = tract_onnx::onnx()
            .model_for_read(&mut Cursor::new(bytes))
            .map_err(|e| format!("reading the ONNX graph: {e:#}"))?;
        // The graph keeps its symbolic batch and sequence dims: pinning them to [1, max_len] makes tract's
        // shape inference fail on the attention masks. The run feeds [1, max_len] tensors instead.
        let outlets = model
            .input_outlets()
            .map_err(|e| format!("the graph's inputs: {e:#}"))?
            .to_vec();
        let inputs: Vec<String> = outlets
            .iter()
            .map(|outlet| model.node(outlet.node).name.clone())
            .collect();
        for name in &inputs {
            if !matches!(
                name.as_str(),
                "input_ids" | "attention_mask" | "token_type_ids"
            ) {
                return Err(format!(
                    "the graph takes {name:?}, which the text runtime does not feed"
                ));
            }
        }
        let plan = model
            .into_optimized()
            .and_then(|m| m.into_runnable())
            .map_err(|e| format!("the ONNX graph does not run at length {max_len}: {e:#}"))?;
        Ok(TextModel {
            contract: contract.clone(),
            tokenizer,
            plan,
            inputs,
        })
    }

    /// Runs the model on `{text}` and shapes the answer for the task.
    pub fn invoke(&self, input: &Value) -> Result<Value, String> {
        let object = input
            .as_object()
            .ok_or("the input must be an object: {text: string}")?;
        let text = object
            .get("text")
            .and_then(Value::as_str)
            .ok_or("the input needs a text string")?;
        if object.len() != 1 {
            return Err("the input takes only text".into());
        }
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| format!("tokenizing: {e}"))?;
        let max_len = self.contract.max_len;
        let ids: Vec<i64> = encoding.get_ids().iter().map(|&x| i64::from(x)).collect();
        let mask: Vec<i64> = encoding
            .get_attention_mask()
            .iter()
            .map(|&x| i64::from(x))
            .collect();
        let types: Vec<i64> = encoding
            .get_type_ids()
            .iter()
            .map(|&x| i64::from(x))
            .collect();
        let mut feeds = tvec!();
        for name in &self.inputs {
            let values = match name.as_str() {
                "input_ids" => &ids,
                "attention_mask" => &mask,
                _ => &types,
            };
            let tensor = Tensor::from_shape(&[1, max_len], values)
                .map_err(|e| format!("input tensor: {e:#}"))?;
            feeds.push(TValue::from(tensor));
        }
        let outputs = self
            .plan
            .run(feeds)
            .map_err(|e| format!("running the model: {e:#}"))?;
        let first = outputs.first().ok_or("the model has no outputs")?;
        let shape = first.shape().to_vec();
        let data: Vec<f32> = first
            .to_plain_array_view::<f32>()
            .map_err(|e| format!("the model's output is not f32: {e:#}"))?
            .iter()
            .copied()
            .collect();
        match self.contract.task.as_str() {
            CLASSIFICATION => self.classify(&shape, &data),
            _ => self.embed(&shape, &data, &mask),
        }
    }

    fn classify(&self, shape: &[usize], logits: &[f32]) -> Result<Value, String> {
        let labels = self.contract.labels.as_deref().unwrap_or_default();
        if shape.len() != 2 || shape[0] != 1 || shape[1] != labels.len() {
            return Err(format!(
                "the output has shape {shape:?}, not [1, {}] logits",
                labels.len()
            ));
        }
        let scores = softmax(logits);
        let mut ranked: Vec<(usize, f32)> = scores.iter().copied().enumerate().collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        let top = ranked[0].0;
        Ok(json!({
            "label": labels[top],
            "scores": ranked.iter().map(|(i, s)| json!({ "label": labels[*i], "score": s })).collect::<Vec<_>>(),
        }))
    }

    fn embed(&self, shape: &[usize], hidden: &[f32], mask: &[i64]) -> Result<Value, String> {
        let max_len = self.contract.max_len;
        if shape.len() != 3 || shape[0] != 1 || shape[1] != max_len {
            return Err(format!(
                "the output has shape {shape:?}, not [1, {max_len}, hidden]"
            ));
        }
        let width = shape[2];
        let mut pooled = vec![0f32; width];
        let mut count = 0f32;
        for (token, &keep) in mask.iter().enumerate().take(max_len) {
            if keep == 0 {
                continue;
            }
            count += 1.0;
            for (j, slot) in pooled.iter_mut().enumerate() {
                *slot += hidden[token * width + j];
            }
        }
        let count = count.max(1.0);
        for value in &mut pooled {
            *value /= count;
        }
        let norm = pooled.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for value in &mut pooled {
                *value /= norm;
            }
        }
        Ok(json!({ "embedding": pooled }))
    }
}

/// Softmax of logits, shifted by their maximum for stability.
fn softmax(logits: &[f32]) -> Vec<f32> {
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = logits.iter().map(|x| (x - max).exp()).collect();
    let sum: f32 = exps.iter().sum();
    exps.into_iter().map(|e| e / sum).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/text");

    fn fixture() -> &'static Path {
        Path::new(FIXTURES)
    }

    fn classifier_contract() -> TextContract {
        let dir = fixture();
        let contract = derive(dir, "classifier.onnx", CLASSIFICATION).expect("derives");
        assert_eq!(contract.max_len, 8, "the fixture's max_position_embeddings");
        contract
    }

    fn embedder_contract() -> TextContract {
        derive(fixture(), "embedder.onnx", EMBEDDING).expect("derives")
    }

    #[test]
    fn the_contract_takes_its_labels_and_length_from_the_config() {
        let contract = classifier_contract();
        assert_eq!(contract.kind, "text");
        assert_eq!(
            contract.labels,
            Some(vec!["NEGATIVE".to_string(), "POSITIVE".to_string()])
        );
        assert_eq!(embedder_contract().labels, None);
        assert!(
            derive(fixture(), "classifier.onnx", "translation").is_err(),
            "an unserved task is refused"
        );
    }

    #[test]
    fn a_classifier_answers_its_label_and_softmax_scores_deterministically() {
        let model = TextModel::load(fixture(), &classifier_contract()).expect("loads offline");
        // "good great movie" -> [CLS] good great movie [SEP]: the id mean is 5.0, so the logits are [-1, 1].
        let positive = model
            .invoke(&json!({ "text": "Good GREAT movie" }))
            .unwrap();
        assert_eq!(positive["label"], "POSITIVE");
        let top = positive["scores"][0]["score"].as_f64().unwrap();
        assert!((top - 0.880_797).abs() < 1e-4, "{positive}");
        assert_eq!(positive["scores"][1]["label"], "NEGATIVE");
        // "awful awful": the id mean is 5.25, so the logits are [1.5, -1.5].
        let negative = model.invoke(&json!({ "text": "awful awful" })).unwrap();
        assert_eq!(negative["label"], "NEGATIVE");
        assert!(
            (negative["scores"][0]["score"].as_f64().unwrap() - 0.952_574).abs() < 1e-4,
            "{negative}"
        );
        // The same input twice gives the same answer.
        assert_eq!(
            model.invoke(&json!({ "text": "awful awful" })).unwrap(),
            negative
        );
    }

    #[test]
    fn an_embedding_is_the_mean_of_the_unmasked_tokens_normalised() {
        let model = TextModel::load(fixture(), &embedder_contract()).expect("loads offline");
        // Tokens [2,5,6,9,3]: channel 0 is the id (mean 5.0), channel 1 is 1; normalised: [5, 1] / sqrt(26).
        let out = model
            .invoke(&json!({ "text": "good great movie" }))
            .unwrap();
        let embedding: Vec<f64> = out["embedding"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_f64().unwrap())
            .collect();
        assert_eq!(embedding.len(), 2);
        assert!(
            (embedding[0] - 5.0 / 26f64.sqrt()).abs() < 1e-4,
            "{embedding:?}"
        );
        assert!(
            (embedding[1] - 1.0 / 26f64.sqrt()).abs() < 1e-4,
            "{embedding:?}"
        );
        let norm: f64 = embedding.iter().map(|x| x * x).sum::<f64>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }

    #[test]
    fn the_input_is_checked_and_long_text_is_truncated_to_the_length() {
        let model = TextModel::load(fixture(), &classifier_contract()).expect("loads offline");
        assert!(model.invoke(&json!({ "text": 3 })).is_err());
        assert!(model.invoke(&json!({ "text": "x", "extra": 1 })).is_err());
        assert!(model.invoke(&json!(["x"])).is_err());
        // Eight positions in all: [CLS], eight words, [SEP] is cut to the first eight tokens, and it still runs.
        let long = "good ".repeat(40);
        assert!(model.invoke(&json!({ "text": long })).is_ok());
    }

    #[test]
    fn the_derived_contract_is_written_into_the_card_and_read_back() {
        let dir = std::env::temp_dir().join(format!("apid-text-card-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for name in ["tokenizer.json", "config.json"] {
            fs::copy(fixture().join(name), dir.join(name)).unwrap();
        }
        let contract = classifier_contract();
        write_contract(&dir, &contract).unwrap();
        let card: Value =
            serde_json::from_str(&fs::read_to_string(dir.join("model_card.json")).unwrap())
                .unwrap();
        assert_eq!(card["serving"]["kind"], "text");
        assert_eq!(card["serving"]["task"], CLASSIFICATION);
        assert_eq!(card["serving"]["max_len"], 8);
        assert_eq!(card["serving"]["labels"][1], "POSITIVE");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_graph_is_found_by_the_layout_the_hub_downloads_it_in() {
        let dir = std::env::temp_dir().join(format!("apid-text-graph-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("onnx")).unwrap();
        assert!(graph_in(&dir).is_err(), "no graph yet");
        fs::write(dir.join("onnx").join("model_quantized.onnx"), b"x").unwrap();
        assert_eq!(graph_in(&dir).unwrap(), "onnx/model_quantized.onnx");
        fs::write(dir.join("onnx").join("model_fp16.onnx"), b"x").unwrap();
        assert!(graph_in(&dir).is_err(), "two graphs must be named");
        let _ = fs::remove_dir_all(&dir);
    }
}
