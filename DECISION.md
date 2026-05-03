# DECISION — Rust as primary impl

**Decision:** for the public-facing demo of the verified twitter clone, the **Rust** implementation is the primary user-facing backend. The Go implementation stays alive as a deployed shadow for stream 2's cross-impl diff-test.

## Why Rust over Go (the honest version)

This was contested. Two reasonable picks; we chose Rust for one specific reason.

**Where Rust wins (and is the reason for the pick):**
Verus expresses memory-safety obligations Go simply can't articulate — `Send`/`Sync` bounds, lifetime-checked references that prove no aliased mutation, ownership-of-resources contracts. The F-properties in the spec are about state evolution (and TLC checks them on the abstract model regardless of which impl is primary), but the trust boundary is about memory safety in the deployed bytes. On that axis Verus has strictly more to say than Gobra.

**Where Go would win and we're paying the price:**
Single static binary deploys are simpler than tokio/axum's runtime; Go's deploy templates (Fly, Render, Cloud Run) are more mature; Gobra's proof-discharge path is actually easier than Verus's because Verus requires lifting `std` types to `vstd` shims (we hit this in Tier 3 with `Mutex<i64>`).

**Net trade:** marginally harder deploy + harder proof story → strongest possible memory-safety claim on the user-facing bytes.

## What this doesn't decide

- Both impls are oracles for each other regardless of which is primary; the Go shadow is just as canonical for spec-conformance purposes.
- Switching primary later is fine — the trust boundary in `TCB.md` documents what's specific to each.
- The choice doesn't bind future `twitter-formal-*` projects to either language.

## When to revisit

- If `vstd` shim work for `Mutex`/`HashMap`/`Vec` makes Verus's proof path materially harder than projected and Gobra has caught up
- If tokio/axum operational overhead (memory at idle, cold-start latency on Fly) becomes a noticeable demo problem
- If the user community for the demo skews so heavily toward Go that the implementation language affects adoption

Until any of those holds, Rust stays primary.
