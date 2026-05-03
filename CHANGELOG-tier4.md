# Tier-4 Changelog — `twitter-rust-formal-verification`

Tracks Tier-4 changes (UI, deploy, shadow/diff-test, proof-discharge progress) for this repo. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

Every Tier-4 PR MUST append at least one line under the appropriate section. The `tier4-bootstrap-check` CI gate enforces this.

## [Unreleased]

### Added

- Phase 0 bootstrap: `TCB.md`, `CHANGELOG-tier4.md`, `CONTRIBUTING.md`, `.github/workflows/tier4-bootstrap-check.yml`.
- Initial trust surface inventory in `TCB.md` capturing the 5 `external_body` methods + opaque spec functions + `external_type_specification` wrappers + Cargo metadata + IO boundary at the Tier-3 baseline.

### Changed

### Deprecated

### Removed

### Fixed

### Trust-Boundary

- Trust surface inventoried; baseline = 5 external_body + 2 closed spec opaque + 2 external_type_specification + 2 Cargo-metadata + 4 IO/CI items. Future Tier-4 PRs adjust this delta.

---

_For Tier 1–3 history (the verified core), see git log._
