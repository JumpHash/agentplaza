//! The Foundry: training jobs across the fleet's GPUs (`docs/design/foundry.md`).

pub mod plan;
// The model registry and the model agents (inference.rs, tract).
pub mod registry;
// The breeder tools and invoke_model (S-006).
pub mod breeder;
// The APZW1 payload codec and the hub side of loose DiLoCo training.
pub mod wire;
pub mod diloco;
pub mod store;
pub mod jobs;
pub mod routes;
pub mod datasets;
// Hugging Face: the client (search, detail, egress) and the imports (docs/design/foundry.md).
pub mod hf;
pub mod hf_import;
// Text models: an ONNX graph beside its tokenizer.json, served with tract and tokenizers (phase 2).
pub mod text;
