# twitter-rust-formal-verification

Formally-verified Twitter clone in Rust. Verifier: [Verus](https://github.com/verus-lang/verus). Spec: [twitter-formal-spec](https://github.com/michaellady/twitter-formal-spec) (TLA+).

This is one of two parallel reference implementations. The other is in Go at [michaellady/twitter-golang-formal-verification](https://github.com/michaellady/twitter-golang-formal-verification). Both consume the same TLA+ spec and pass the same conformance suite byte-for-byte.

## What this is

- A small in-memory Twitter clone (users, follows, tweets, home timeline) whose **synchronous core is annotated for Verus** so the F-properties below are deductively verified.
- A **trusted axum/tokio HTTP shim** (the `server` crate) that adapts the verified core to the HTTP API. The shim is in the trusted computing base (TCB) — Verus does not verify axum, tokio, or tower.
- A conformance harness that replays `specs/conformance.jsonl` byte-identically through the in-process axum router. The Go impl runs the same JSONL through *its* router. They cross-check each other.

## What this is not

- Not authenticated, rate-limited, or persisted (those are explicit assumptions A1, A2, A3).
- Not async-verified. F5's Rust scope is data-race freedom on the synchronous core via Rust's ownership system + Verus exclusivity. Tokio's scheduler, axum's routing, and tower's middleware are all in the TCB.
- Not a full Twitter clone. The contract is exactly the four endpoints in `crates/server/src/handlers.rs`.

## Quick start

```bash
git clone --recurse-submodules https://github.com/michaellady/twitter-rust-formal-verification.git
cd twitter-rust-formal-verification
make test                  # unit + integration + functional + conformance
make run                   # axum server on :8080 (set $PORT to override)
make tlc                   # bounded model check the TLA+ spec
make cov                   # 100% line coverage on the verified crates
make manifest              # COVERAGE.md tests-aren't-lying gate
```

## Architecture

```
HTTP request
  │
  ▼
crates/server   (TCB — axum / tokio / tower)
  │  decode JSON → call service::* → encode JSON
  ▼
crates/service  (verified — F1, F2, F4 dispatched here)
  │
  ├─► crates/clock  (verified — F7)
  ├─► crates/ids    (verified — F8)
  ├─► crates/domain (verified — F4)
  └─► crates/store  (verified — F3, F5-rust, F6, F9)
```

### Verified vs trusted

| Component                | Status   | LOC | F-properties                |
|--------------------------|----------|----:|-----------------------------|
| `crates/clock`           | verified | 182 | F7                          |
| `crates/ids`             | verified | 129 | F8                          |
| `crates/domain`          | verified | 144 | F4                          |
| `crates/store`           | verified | 367 | F3, F5-rust, F6, F9         |
| `crates/service`         | verified | 350 | F1, F2, F4 (composition)    |
| `crates/server`          | **TCB**  | 232 | none (HTTP shim only)       |

LOC counts include Verus annotations and unit tests; the verified-core
cargo-llvm-cov gate is set to **100% line coverage**.

### F-properties

| Id  | Property                                                                 |
|-----|--------------------------------------------------------------------------|
| F1  | Timeline visibility: tweet visible iff `user == author` or `user → author` follow exists. |
| F2  | Timeline ordering: `(created_at desc, tweet_id desc)`.                    |
| F3  | Idempotency: repeated `Follow`/`Unfollow` are no-ops after the first.     |
| F4  | No self-follow: rejected at `domain::Follow::new`.                        |
| F5 (Rust scope) | Data-race freedom on the synchronous verified core via ownership + Verus exclusivity. tokio/axum/tower are TCB. |
| F6  | No orphan tweet authors: `put_tweet` rejects unknown handles.             |
| F7  | Logical clock non-strict: `now()` is non-decreasing, ties allowed.        |
| F8  | Tweet ID uniqueness + per-author monotonic.                               |
| F9  | No orphan follow edges: `put_follow` rejects unknown handles.             |

### A-assumptions (out of scope)

- **A1**: unauthenticated. Anyone can call any endpoint with any handle.
- **A2**: no rate limiting.
- **A3**: TCB enumeration — `crates/server` is in the TCB. Anything reachable from a `tokio::spawn` outside the verified core is in the TCB.

## How to run Verus

Verus is a Rust deductive verifier built on Z3. The annotations in this repo are written so they **compile under stable rustc with no Verus installed** (the proof obligations live inside `#[cfg(verus)]` modules, see `crates/clock/src/lib.rs` for the canonical pattern). When Verus is installed:

```bash
# Install: see https://github.com/verus-lang/verus
verus --crate-type=lib crates/clock/src/lib.rs
verus --crate-type=lib crates/ids/src/lib.rs
verus --crate-type=lib crates/domain/src/lib.rs
verus --crate-type=lib crates/store/src/lib.rs
verus --crate-type=lib crates/service/src/lib.rs
```

CI runs Verus best-effort with `continue-on-error: true`. The compile-time path (`cargo check`, `cargo test`) does not require Verus.

**Trusted wrappers**: `vstd::hash_map`, `vstd::vec`, and `vstd::sync::RwLock` are part of the TCB. They are thin wrappers over `std::collections::HashMap`, `std::vec::Vec`, and `std::sync::RwLock` whose contracts Verus assumes rather than verifies.

## How to run TLC

The canonical spec lives in `specs/twitter.tla` (consumed via submodule from [michaellady/twitter-formal-spec](https://github.com/michaellady/twitter-formal-spec) at SHA `a534406`).

```bash
make tlc
# or:
TLA_VERSION=1.8.0 bash scripts/run_tlc.sh
```

Requires Java 11+. The script will download `tla2tools.jar` to `/tmp/` if it isn't present.

## Bumping the spec

The spec submodule is **pinned** at the SHA in `SPEC_SHA`. CI's first job is the SPEC_SHA gate — it refuses to build against an unpinned spec.

To bump:

```bash
git -C specs fetch
git -C specs checkout <new-sha>
git -C specs rev-parse HEAD > SPEC_SHA
make tlc                           # spec must still pass
make test                          # impl must still match
git add SPEC_SHA specs
git commit -m "Bump spec to <new-sha>"
```

Both this repo and the Go impl must be bumped in lockstep, with both passing CI before either is merged.

## Links

- TLA+ spec: <https://github.com/michaellady/twitter-formal-spec>
- Go impl: <https://github.com/michaellady/twitter-golang-formal-verification>
- Verus: <https://github.com/verus-lang/verus>

## License

MIT — see `LICENSE`.
