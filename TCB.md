# Trusted Computing Base (TCB) — `twitter-rust-formal-verification`

This file enumerates everything in this repo that is **trusted but not formally verified**. Every Tier-4 PR that expands the trust surface MUST add a row here. The narrower this list, the stronger the project's verification claim.

The Tier-3 baseline below was inventoried during the Tier-3 verifier-strictness PR (Tier 3 PR #3). Tier-4 PRs may add rows for new admin endpoints, deploy infra, UI, observability hooks. Stream 3 PRs REMOVE rows as proof obligations get discharged.

## Tier-3 baseline trust surface

### Verifier escape hatches (`#[verifier::external_body]` and `external_type_specification`)

| File | Function | Why trusted | Validated by |
|---|---|---|---|
| `crates/clock/src/lib.rs` | `now_ensures` | wraps `Logical::now` which uses `std::sync::Mutex` (not lifted to `vstd::sync::Mutex` yet) | unit tests + TLC F7 invariant |
| `crates/clock/src/lib.rs` | `tick_ensures` | same | unit tests + TLC F7 invariant |
| `crates/clock/src/lib.rs` | `ts(c)` (closed spec, opaque) | spec function whose body would reference vstd Mutex methods that don't exist on `std::sync::Mutex` | derived from external_body of now/tick |
| `crates/clock/src/lib.rs` | `ExLogical` external_type_specification | `Logical` has private `inner` field; structural opaqueness | tests |
| `crates/ids/src/lib.rs` | `next_id_ensures` | wraps `Generator::next` (Mutex<i64>) | unit tests + TLC F8 invariant |
| `crates/ids/src/lib.rs` | `count(g)` (closed spec, opaque) | same as `ts` | derived |
| `crates/ids/src/lib.rs` | `ExGenerator` external_type_specification | `Generator` has private `inner` field | tests |
| `crates/domain/src/lib.rs` | `verus_proof` (trusted skeleton) | F4 obligation documented in comments; full proof requires Verus `String`/`Result` specs out of scope | unit tests |
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
| `crates/server/src/handlers.rs` | all HTTP handlers | JSON serde + error mapping; not in spec | conformance tests; stream 2 diff-test (when live) |
| `crates/server/src/handlers.rs` | `healthz`, `version` | Tier-4 Phase 1 endpoints; `/version` reads `/etc/version.json` baked into image at build time | post-deploy digest verification in deploy.yml |
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
