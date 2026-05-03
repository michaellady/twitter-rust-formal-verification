# Contributing to `twitter-rust-formal-verification`

Rust implementation verified with [Verus](https://github.com/verus-lang/verus). Spec source of truth: [twitter-formal-spec](https://github.com/michaellady/twitter-formal-spec).

## Spec-first rule (cross-repo, merge-blocking)

Any new F-property MUST land in `twitter-formal-spec` first. Then bump `SPEC_SHA` + the submodule pointer here via `bash scripts/bump_spec_sha.sh`. Don't add F-property logic in this repo first and back-port it.

## Tier-4 PR checklist (merge-blocking)

Every PR labeled or scoped as Tier-4 work MUST:

- [ ] Touch `CHANGELOG-tier4.md` — at least one new line under the appropriate section
- [ ] Touch `TCB.md` if the PR expands, shrinks, or modifies the trust surface (new admin endpoint, new trusted shim, discharged proof obligation, etc.) — and add a `Trust-Boundary` line in the changelog as well
- [ ] Pass the `tier4-bootstrap-check` workflow (CI gate)

Skip rule: dependency-bump-only PRs (Renovate, Dependabot) may pass with the changelog auto-appended via the bot, no TCB.md change required. Apply the `dependencies` label to skip the gate.

## Stream 3 (proof discharge) workflow

When discharging an `external_body` annotation:
1. Lift the underlying state to a vstd shim (e.g., `vstd::sync::Mutex`).
2. Drop `external_body`; ensure `cargo verus verify -p <crate>` reports `N verified, 0 errors` (NOT trivially passed).
3. Update `TCB.md` — remove the corresponding row.
4. Update `CHANGELOG-tier4.md` under `Trust-Boundary` — note which method was discharged and what vstd shim made it possible.

This mechanically increases Badge A (Formal proof coverage) — see README.
