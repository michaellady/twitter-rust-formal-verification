# Tier-4 Changelog — `twitter-rust-formal-verification`

Tracks Tier-4 changes (UI, deploy, shadow/diff-test, proof-discharge progress) for this repo. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

Every Tier-4 PR MUST append at least one line under the appropriate section. The `tier4-bootstrap-check` CI gate enforces this.

## [Unreleased]

### Added

- Phase 0 bootstrap: `TCB.md`, `CHANGELOG-tier4.md`, `CONTRIBUTING.md`, `.github/workflows/tier4-bootstrap-check.yml`.
- Initial trust surface inventory in `TCB.md` capturing the 5 `external_body` methods + opaque spec functions + `external_type_specification` wrappers + Cargo metadata + IO boundary at the Tier-3 baseline.
- **Phase 1: deploy infrastructure.** `Dockerfile` (distroless multi-stage), `fly.toml`, `DECISION.md`, `DEPLOY.md`. Two GitHub Actions workflows: `verify.yml` gains `build-image-pr` (PR validation only, no GHCR write) and `build-image-main` (main-only push to GHCR, two-pass build to bake the image-digest into `/etc/version.json`). New `deploy.yml` listens on `workflow_run` of verify, downloads the digest artifact, and runs `flyctl deploy --image @sha256:<digest>` — never `--remote-only`. Post-deploy `/version` digest check fails the deploy on mismatch.
- `GET /healthz` (load balancer probe) and `GET /version` (baked-image provenance) endpoints in `crates/server/src/handlers.rs`.
- **Phase 1b: state lifecycle (seed loader).** `crates/server/src/main.rs` reads `SEED_DEMO=true` env var and pre-populates alice/bob/carol with sample tweets + alice→bob follow on every startup, so the public demo always shows something interesting after a Fly machine restart. `fly.toml` sets `SEED_DEMO=true` for the production app. The verified core remains in-memory by design; this is honest about what persists ("nothing across restarts; restart loads seed; future stream 2 phase 1b adds peer-resync").
- **Phase 2: HTMX UI on the primary backend (no separate Node deploy).** New `crates/server/src/ui.rs` mounts pages on the same axum router as the JSON API. Routes: `GET /` (handle picker / home timeline depending on cookie), `GET /u/:handle` (profile + follow/unfollow), `POST /_session` (set HMAC-signed cookie), `POST /_session/clear`, `POST /compose`, `POST /register`, `POST /_ui/follow`, `POST /_ui/unfollow`. UI mutation routes are namespaced under `/_ui/` to avoid colliding with the JSON `POST /follow` API. New deps: `hmac`, `sha2`, `hex`, `urlencoding`. Cookie signing uses `UI_COOKIE_HMAC_KEY` env var (Fly secret in prod; per-process random fallback for local dev). Honest TCB framing: this is NOT auth — anyone can claim any handle.
- **Phase 3: Playwright E2E conformance suite.** New `e2e/` directory with 8 specs (one per F-property: F1, F2, F3, F4, F6, F7, F8, F9) mapped to UI flows + JSON API checks. Per-run namespace isolation per K4: every test handle is suffixed with `PLAYWRIGHT_RUN_ID` (= `github.run_id` in CI, random short prefix locally). Acceptance: two consecutive runs against shared production state both pass with no manual cleanup — verified locally before the PR. New `.github/workflows/e2e.yml` runs the suite on every PR (against a locally-booted server), on every main push (against production via `workflow_run` after deploy completes), and on a 06:00 UTC nightly cron.

### Changed

### Deprecated

### Removed

### Fixed

- Quote two workflow step names in `.github/workflows/verify.yml` (`Build and push (pass 1: …)` and `Re-tag with version baked in (pass 2)`) so the YAML parser doesn't trip on `+` in the unquoted scalar. The Phase 1 squash-merge raced ahead of the original fix commit and landed the broken YAML on main; this patches it.
- Dockerfile: distroless final stage has no `/bin/sh`, so `RUN printf …` to bake `/etc/version.json` failed at image build. Move the printf into the builder stage and COPY the resulting `version.json` into the distroless final stage. Image-digest provenance still works the same way at runtime.
- `deploy.yml` post-deploy verification: switch the check from `image_digest` to `git_sha`. The two-pass image build inherently can't bake pass2's own digest (chicken/egg), so `/version.image_digest` always reports pass1's digest while the deploy artifact is pass2's. `git_sha` is deterministic across both passes and across rollbacks; that's what we actually want to verify. Image-byte provenance is still established by deploy.yml pulling by exact digest.

### Trust-Boundary

- Trust surface inventoried; baseline = 5 external_body + 2 closed spec opaque + 2 external_type_specification + 2 Cargo-metadata + 4 IO/CI items. Future Tier-4 PRs adjust this delta.
- **Phase 1 added 6 trusted items:** `healthz`+`version` handlers, `build-image-pr`+`build-image-main` workflow jobs, `deploy.yml`, `Dockerfile`, `fly.toml`. All inventoried with rationale + validation strategy in `TCB.md`.
- **Phase 1b added 1 trusted item:** `seed_demo` function in `crates/server/src/main.rs`. Documented in `TCB.md` under IO boundary (it directly invokes Service methods at startup, no spec). Acceptable trust because the seed payload is hard-coded literals in the binary, not user input.
- **Phase 2 added 1 trusted module:** `crates/server/src/ui.rs` with 8 route handlers + the HMAC cookie signing helpers. Listed in `TCB.md` under UI category. The HMAC verification uses constant-time comparison (we re-implement; no `subtle` dep). The cookie's only payload is the actor handle — there is no auth claim, so a leaked cookie is exactly as harmful as someone typing the handle into the picker (i.e., zero, by design — anyone can claim any handle).
- **Phase 3 added 0 trusted items in the verified core; 1 new TCB row for the e2e suite itself.** The Playwright tests live in `e2e/` and exercise the UI + JSON API as a black box. The suite is itself trusted (it tells us when conformance breaks, but we trust the suite to test the right things). Listed in `TCB.md` under observability/test infrastructure.

---

_For Tier 1–3 history (the verified core), see git log._
