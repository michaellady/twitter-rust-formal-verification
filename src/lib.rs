//! Top-level test harness crate. This exists solely so the `tests/` directory
//! at the workspace root is picked up as a cargo integration-test target.
//!
//! Real code lives under `crates/`. This crate re-exports the pieces tests
//! need so they don't have to import from six different paths.

pub use clock;
pub use server;
pub use service;
