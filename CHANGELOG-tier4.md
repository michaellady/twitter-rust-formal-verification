# Tier-4 Changelog — `twitter-rust-formal-verification`

Tracks Tier-4 changes (UI, deploy, shadow/diff-test, proof-discharge progress) for this repo. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

Every Tier-4 PR MUST append at least one line under the appropriate section. The `tier4-bootstrap-check` CI gate enforces this.

## [Unreleased]

### Added

- Phase 0 bootstrap: `TCB.md`, `CHANGELOG-tier4.md`, `CONTRIBUTING.md`, `.github/workflows/tier4-bootstrap-check.yml`.
- Initial trust surface inventory in `TCB.md` capturing the 5 `external_body` methods + opaque spec functions + `external_type_specification` wrappers + Cargo metadata + IO boundary at the Tier-3 baseline.
- **Phase 1: deploy infrastructure.** `Dockerfile` (distroless multi-stage), `fly.toml`, `DECISION.md`, `DEPLOY.md`. Two GitHub Actions workflows: `verify.yml` gains `build-image-pr` (PR validation only, no GHCR write) and `build-image-main` (main-only push to GHCR, two-pass build to bake the image-digest into `/etc/version.json`). New `deploy.yml` listens on `workflow_run` of verify, downloads the digest artifact, and runs `flyctl deploy --image @sha256:<digest>` — never `--remote-only`. Post-deploy `/version` digest check fails the deploy on mismatch.
- `GET /healthz` (load balancer probe) and `GET /version` (baked-image provenance) endpoints in `crates/server/src/handlers.rs`.

### Changed

### Deprecated

### Removed

### Fixed

### Trust-Boundary

- Trust surface inventoried; baseline = 5 external_body + 2 closed spec opaque + 2 external_type_specification + 2 Cargo-metadata + 4 IO/CI items. Future Tier-4 PRs adjust this delta.
- **Phase 1 added 6 trusted items:** `healthz`+`version` handlers, `build-image-pr`+`build-image-main` workflow jobs, `deploy.yml`, `Dockerfile`, `fly.toml`. All inventoried with rationale + validation strategy in `TCB.md`.

---

_For Tier 1–3 history (the verified core), see git log._
