# Trusted Computing Base (TCB) — `twitter-rust-formal-verification`

This file enumerates everything in this repo that is **trusted but not formally verified**. Every Tier-4 PR that expands the trust surface MUST add a row here. The narrower this list, the stronger the project's verification claim.

The Tier-3 baseline below was inventoried during the Tier-3 verifier-strictness PR (Tier 3 PR #3). Tier-4 PRs may add rows for new admin endpoints, deploy infra, UI, observability hooks. Stream 3 PRs REMOVE rows as proof obligations get discharged.

## Tier-3 baseline trust surface

### Verifier escape hatches (`#[verifier::external_body]` and `external_type_specification`)

| File | Function | Why trusted | Validated by |
|---|---|---|---|
| `crates/clock/src/lib.rs` | `now_ensures` | wraps `Logical::now` which uses `std::sync::Mutex` (not yet lifted onto a vstd lock primitive — Stream 3 Phase 1b) | unit tests + TLC F7 invariant |
| `crates/clock/src/lib.rs` | `tick_ensures` | same | unit tests + TLC F7 invariant |
| `crates/clock/src/lib.rs` | `inner_state(c)` (closed spec, opaque) | Stream 3 Phase 1a: spec projector from opaque `Logical` to its `LockState` newtype, so `ts(c)` body can be written non-opaquely. Replaces the previous opaque `ts(c)` body. Discharged in Phase 1b when the projector becomes structural over a vstd lock primitive. | derived |
| `crates/clock/src/lib.rs` | `lock_state_value(s)` (closed spec, opaque) | Stream 3 Phase 1a: spec wrapper around `LockState::lock_value`. Body remains trusted until Phase 1b chains it through `vstd::rwlock::RwLock` postconditions. | unit tests asserting `lock_value() == now()` |
| `crates/clock/src/lib.rs` | `ExLogical` external_type_specification | `Logical` has private `inner` field; structural opaqueness | tests |
| `crates/clock/src/lib.rs` | `ExLockState` external_type_specification | Stream 3 Phase 1a: `LockState` newtype is opaque to Verus until Phase 1b lifts it to a vstd lock primitive | tests |
| `crates/ids/src/lib.rs` | `next_id_ensures` | wraps `Generator::next` (Mutex<i64>) | unit tests + TLC F8 invariant |
| `crates/ids/src/lib.rs` | `count(g)` (closed spec, opaque) | same as `ts` | derived |
| `crates/ids/src/lib.rs` | `ExGenerator` external_type_specification | `Generator` has private `inner` field | tests |
| `crates/store/src/lib.rs` | `verus_proof` (trusted skeleton) | F3/F6/F9 documented; full proofs require lifting `Mutex`/`HashMap` to vstd shims out of scope | unit + integration + conformance tests |
| `crates/service/src/lib.rs` | `verus_proof` (trusted skeleton) | composition obligations (F1+F6, F2 sort) documented; out of scope | unit + integration + conformance tests |

### Cargo metadata + dependencies

| File | Item | Why trusted | Validated by |
|---|---|---|---|
| `crates/*/Cargo.toml` | `[package.metadata.verus] verify = true` opt-in | controls which crates `cargo verus verify` processes; mis-config = silent skip | CI verifies all 5 crates produce verification output |
| `Cargo.toml` (workspace) | `vstd = "=0.0.0-2026-04-20-1748"` pin | pinned to match `VERUS_VERSION` in `verify.yml`; bumping requires bumping both | CI lockfile + manual review on bump |

### IO boundary

| File | Item | Why trusted | Validated by |
|---|---|---|---|
| `crates/server/src/main.rs` | tokio + axum bootstrap | wires verified core to `net::TcpListener`; not in spec | smoke tests; live demo |
| `crates/server/src/main.rs` | `seed_demo` (Phase 1b) | invokes Service methods at startup with hard-coded literals when `SEED_DEMO=true`; pre-populates demo state for fresh visitors | smoke tested locally; integration via deploy → curl /timeline?user=alice expecting non-empty |
| `crates/server/src/ui.rs` | entire module (Phase 2) | server-rendered HTMX UI on the primary backend; HMAC-signed actor cookie; routes `/`, `/_session`, `/u/:handle`, `/compose`, `/register`, `/_ui/follow`, `/_ui/unfollow` | manual smoke tested locally (login → home → compose → profile → follow); Phase 3 Playwright E2E covers end-to-end |
| `e2e/tests/f-properties.spec.ts` (Phase 3) | Playwright conformance suite | 8 specs mapped to F1-F9 at the UI/API layer; per-run namespace isolation (K4); runs on PR + main push + nightly cron | self-validates against the verified core; `cargo verus verify` and TLC are the upstream sources of truth |
| `crates/server/src/metrics.rs` (Phase 4) | Prometheus exposition + axum middleware + JSON log writer | observability of the HTTP boundary; `f_property_violations_total{f}` counter | counter values self-evident (visible in /metrics); error-code-to-F mapping in `note_violation` hand-maintained — extend in PR that adds new ServiceError variant |
| `crates/server/src/handlers.rs` | all HTTP handlers | JSON serde + error mapping; not in spec | conformance tests; stream 2 diff-test (when live) |
| `crates/server/src/handlers.rs` | `healthz`, `version` | Tier-4 Phase 1 endpoints; `/version` reads `/etc/version.json` baked into image at build time | post-deploy digest verification in deploy.yml |
| `crates/ids/src/lib.rs` | `Generator::set_current` (Stream 2 Phase 0) | bypasses F8's strict-monotonic-from-1 invariant if abused; only the snapshot/load admin path and seed loader call it | unit test pins the value bypass; admin endpoint requires `X-Admin-Token` |
| `crates/store/src/lib.rs` | `MemStore::replace` + `StoreSnapshot` (Stream 2 Phase 0) | bypasses F3/F6/F9 admission checks (`put_user`/`put_follow`/`put_tweet`); validation lives in the snapshot producer | unit tests; admin endpoint requires `X-Admin-Token`; Stream 2 Phase 1 diff-test will catch divergence |
| `crates/clock/src/lib.rs` | `Logical`'s `set_now` override (Stream 2 Phase 0) | single-mutex-op override of the slow trait default; can violate F7 monotonicity if `value < now()` is requested. Trait default (`tick` repeatedly) is safe and applies to any other `Clock` impl | unit tests cover both override and default paths |
| `crates/server/src/admin.rs` (Stream 2 Phase 0) | entire module — `POST /_admin/snapshot`, `POST /_admin/load-snapshot` | snapshot contract for the cross-impl shadow / diff-test layer. Both handlers bypass verified admission checks. Auth = constant-time compare of `X-Admin-Token` header against `ADMIN_TOKEN` env var; unset env → 503 `admin_disabled` (no per-process fallback, unlike UI cookies). JSON marshaled with `serde_json::json!` and parsed via `serde_json::Value` — verified `domain` types are NOT touched (no `Serialize` derives added) | unit tests for parser + auth helpers; smoke tests (positive + 401/400/422); Stream 2 Phase 1 diff-test consumer is the long-term watchdog |
| `crates/service/src/lib.rs` | `Service::snapshot_state` + `Service::load_state` + `ServiceState` (Stream 2 Phase 0) | service-layer orchestrator that composes `Generator::current/set_current`, `MemStore::snapshot/replace`, `Clock::now/set_now`. Trusted because it inherits the trust of every method it calls | unit tests round-trip a snapshot through a fresh service |
| `.github/workflows/verify.yml` | CI verification gate | enforces TLC + `cargo verus verify --workspace` strict on every push | self-check on every PR |
| `.github/workflows/verify.yml` | `build-image-pr` / `build-image-main` jobs | Tier-4 Phase 1: builds + pushes verified image to GHCR after every verifier passes; main-only push permission per K6 | image-digest artifact propagated to deploy.yml |
| `.github/workflows/deploy.yml` | deploy via image-digest promotion | Tier-4 Phase 1: pulls verified image by digest, never rebuilds; post-deploy /version digest check | mismatch fails the deploy |
| `Dockerfile` | runtime image build | distroless base; bakes `/etc/version.json` for image-digest provenance | image-digest verification gate |
| `fly.toml` | Fly app config | deploy target; healthz check, port, region | manual `flyctl deploy` validates on first run |
| `scripts/run_tlc.sh` | TLC runner | invokes Java + tla2tools.jar | macOS shim guard added in Tier 3 |

## Trust surface categories (for new entries)

- **Verifier escape hatches:** `external_body`, `external_type_specification`, `assume`, opaque `closed spec` bodies
- **vstd shim usage:** when introduced (currently none — opportunity in stream 3)
- **IO boundary:** HTTP shim, JSON, network calls
- **Deploy stack:** Dockerfile, deploy workflow, registry credentials
- **Admin endpoints:** `/_admin/*`, `/version`, snapshot endpoints (added in Tier 4)
- **UI:** all of the UI code (added in Tier 4)
- **Observability:** /metrics, structured logger (added in Tier 4)

## How rows get removed

A row is removed when the underlying item moves into the verified core (e.g., `external_body` is dropped because the proof obligation is now discharged via vstd shims). The PR removing the row updates `CHANGELOG-tier4.md` under `Trust-Boundary` with a brief note.

**Per-impl ratchet:** the count of rows in this file is Badge A (Formal proof coverage) — see the project README. Lower is better.
