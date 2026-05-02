#!/usr/bin/env bash
# Bump the spec submodule to current origin/main, update SPEC_SHA, and stage
# the gitlink move. Run from the repo root.
set -euo pipefail

if [ ! -d specs/.git ] && [ ! -f specs/.git ]; then
  echo "specs/ submodule not initialized — run 'git submodule update --init' first" >&2
  exit 1
fi

if ! git -C specs diff --quiet || ! git -C specs diff --cached --quiet; then
  echo "specs/ submodule has uncommitted changes — refusing to bump" >&2
  exit 1
fi

git -C specs fetch origin main
NEW_SHA=$(git -C specs rev-parse origin/main)
git -C specs checkout --detach "$NEW_SHA"     # update submodule HEAD
echo "$NEW_SHA" > SPEC_SHA                    # update pinned SHA file
git add SPEC_SHA specs                        # stage SPEC_SHA + gitlink move
echo "SPEC_SHA + submodule pointer bumped to $NEW_SHA (staged for commit)"

# Acceptance check — same gate CI runs:
bash scripts/check_spec_sha.sh
