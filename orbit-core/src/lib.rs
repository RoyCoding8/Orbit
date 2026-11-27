//! Deterministic production decisions for paths, credentials, journals, and
//! transcript parsing. The desktop backend supplies filesystem observations,
//! provider data, and persisted state at the boundary.

pub mod credentials;
pub mod path_safety;
pub mod state_machines;
pub mod transcript_and_parsing;
