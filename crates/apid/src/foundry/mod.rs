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
