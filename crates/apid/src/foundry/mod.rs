//! The Foundry: training jobs across the fleet's GPUs (`docs/design/foundry.md`).

// The jobs module consumes the planner next; until it lands nothing in the binary calls it.
#[allow(dead_code)]
pub mod plan;
