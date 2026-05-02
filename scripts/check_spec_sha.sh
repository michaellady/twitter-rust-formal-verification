#!/usr/bin/env bash
# Verify the spec submodule HEAD matches the pinned SPEC_SHA.
# Run from the repo root.
set -euo pipefail

if [ ! -f SPEC_SHA ]; then
  echo "SPEC_SHA file missing" >&2
  exit 1
fi
if [ ! -d specs ]; then
  echo "specs/ submodule not initialized — run 'git submodule update --init'" >&2
  exit 1
fi

want=$(cat SPEC_SHA | tr -d '[:space:]')
got=$(git -C specs rev-parse HEAD | tr -d '[:space:]')

if [ "$want" != "$got" ]; then
  cat <<EOM >&2
SPEC_SHA mismatch — refusing to build against an unpinned spec.

  pinned: $want
  actual: $got

If this is intentional (you're bumping the spec):
  1. Verify the new SHA is what you want
  2. Update SPEC_SHA: git -C specs rev-parse HEAD > SPEC_SHA
  3. Re-run TLC, conformance, and all tests
  4. Commit SPEC_SHA + the submodule pointer in the same commit
EOM
  exit 1
fi

echo "SPEC_SHA OK ($got)"
