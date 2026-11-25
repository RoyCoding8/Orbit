//! Pure, dependency-free production domain logic shared between `src-tauri`
//! and Orbit's Kani formal verification suite (`formal/`).
//!
//! This crate depends exclusively on the Rust standard library (`std`) — zero
//! Tauri, SQLite, tokio, serde, or OS FFI dependencies — so `formal/` can prove
//! the exact production functions executed by `src-tauri`.

pub mod credentials;
pub mod path_safety;
pub mod state_machines;
pub mod transcript_and_parsing;
