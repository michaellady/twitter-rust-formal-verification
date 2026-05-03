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

---

_For Tier 1–3 history (the verified core), see git log._
