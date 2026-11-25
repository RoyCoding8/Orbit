//! Kani formal verification proof harnesses for Orbit.
//!
//! Every module directly imports and verifies the exact production functions
//! exported by `orbit-core` (which `src-tauri` also depends on), eliminating
//! model-to-code drift while keeping zero Tauri, SQLite, or OS FFI dependencies.

pub use orbit_core;

pub mod credentials;
pub mod path_safety;
pub mod state_machines;
pub mod transcript_and_parsing;
