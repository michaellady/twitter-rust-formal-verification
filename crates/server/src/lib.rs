//! Trusted HTTP boundary (TCB).
//!
//! This crate is *not* verified by Verus — it adapts axum/tokio/tower to the
//! verified service core. Discipline: handlers do exactly three things —
//! decode the request, call a verified service function, encode the
//! response. No business logic here.
//!
//! Correctness of this layer is established by the functional, integration,
//! conformance, and race tests.

pub mod admin;
pub mod handlers;
pub mod metrics;
pub mod ui;

pub use handlers::router;
