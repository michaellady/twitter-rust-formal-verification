#!/usr/bin/env bash
# tools/compare-impls.sh
#
# Stream 2 (early): a stand-in for the full diffsplitter that demonstrates
# the same value — runs the same conformance scenario against both live
# backends and diffs every response. Markdown report to stdout.
#
# Use cases:
#   - quick sanity check that the two impls agree at the HTTP layer
#   - regression bisect when one impl ships a behavior change
#   - the converged plan's stream 2 phase 4 (conformance scoreboard) is
#     this script run on a schedule with the diffs charted
#
# Limitations vs the full diffsplitter (Stream 2 Phase 1):
#   - no live traffic interception; replays a fixed scenario
#   - no durable write queue (K7); single-shot
#   - no auto-resync (K8/K12); each run uses unique namespaces so state
#     drift between backends doesn't poison results
#
# Usage:
#   tools/compare-impls.sh [--rust URL] [--go URL]
#   defaults: Rust=https://twitter-formal-rust.fly.dev
#             Go=https://twitter-formal-go.fly.dev

set -euo pipefail

RUST_URL="${RUST_URL:-https://twitter-formal-rust.fly.dev}"
GO_URL="${GO_URL:-https://twitter-formal-go.fly.dev}"
NS="cmp$(date +%s)$$"

while [ $# -gt 0 ]; do
  case "$1" in
    --rust) RUST_URL="$2"; shift 2;;
    --go)   GO_URL="$2";   shift 2;;
    --ns)   NS="$2";       shift 2;;
    -h|--help) sed -n '2,30p' "$0"; exit 0;;
    *) echo "unknown arg: $1" >&2; exit 2;;
  esac
done

# `command -v jq` instead of `which jq` for portability.
command -v jq >/dev/null || { echo "jq required" >&2; exit 1; }
command -v diff >/dev/null || { echo "diff required" >&2; exit 1; }

DIFFS=0
TOTAL=0
LOG=$(mktemp -t compare-impls.XXXXXX)
trap 'rm -f $LOG ${LOG}.rust ${LOG}.go ${LOG}.diff' EXIT

# Print a markdown report header.
cat <<EOF
# cross-impl comparison

| | URL |
|---|---|
| Rust primary | \`$RUST_URL\` |
| Go shadow | \`$GO_URL\` |
| Namespace | \`$NS\` (per-run, K4) |

## scenario steps

EOF

# Fire the same request at both backends, normalize, diff, score.
# $1=label, $2=method, $3=path, $4=body (or empty for GET)
step() {
  local label="$1" method="$2" path="$3" body="${4:-}"
  TOTAL=$((TOTAL + 1))

  local rargs=(-sS -o "${LOG}.rust" -w "%{http_code}")
  local gargs=(-sS -o "${LOG}.go"   -w "%{http_code}")
  if [ -n "$body" ]; then
    rargs+=(-X "$method" -H 'Content-Type: application/json' -d "$body")
    gargs+=(-X "$method" -H 'Content-Type: application/json' -d "$body")
  fi

  local rstatus gstatus
  rstatus=$(curl "${rargs[@]}" "$RUST_URL$path" || true)
  gstatus=$(curl "${gargs[@]}" "$GO_URL$path"   || true)

  # Normalize for *behavior* comparison, not *counter-value* comparison.
  # The two backends have independent state (no shared persistence yet —
  # that's Stream 2 Phase 0's snapshot contract). So absolute id values
  # and version metadata diverge by design. Strip:
  #   .id, .created_at, .git_sha, .image_digest, .process_uptime_seconds
  # to compare *shape, types, error codes, ordering rules*.
  local norm_filter='walk(
    if type == "object"
    then del(.id, .created_at, .git_sha, .image_digest,
             .process_uptime_seconds, .image_digest)
    else .
    end
  )'
  local rnorm gnorm
  if jq -e . <"${LOG}.rust" >/dev/null 2>&1; then
    rnorm=$(jq -S "$norm_filter" <"${LOG}.rust")
  else
    rnorm=$(cat "${LOG}.rust")
  fi
  if jq -e . <"${LOG}.go" >/dev/null 2>&1; then
    gnorm=$(jq -S "$norm_filter" <"${LOG}.go")
  else
    gnorm=$(cat "${LOG}.go")
  fi

  local agree="✅ same"
  if [ "$rstatus" != "$gstatus" ] || [ "$rnorm" != "$gnorm" ]; then
    DIFFS=$((DIFFS + 1))
    agree="❌ DIFFERS"
  fi

  cat <<EOF
### \`$label\` — $method $path  $agree

| | status | body (sorted-keys JSON or raw) |
|---|---|---|
| Rust | $rstatus | <pre>$(printf '%s' "$rnorm" | head -10)</pre> |
| Go   | $gstatus | <pre>$(printf '%s' "$gnorm" | head -10)</pre> |

EOF

  if [ "$agree" = "❌ DIFFERS" ]; then
    diff -u <(printf '%s\n' "$rnorm") <(printf '%s\n' "$gnorm") > "${LOG}.diff" 2>&1 || true
    echo "<details><summary>diff (rust → go)</summary>"
    echo
    echo '```diff'
    cat "${LOG}.diff" | head -40
    echo '```'
    echo
    echo "</details>"
    echo
  fi
}

# F1+F2: register, post, follow, read timeline.
step "register-alice" POST /users   "{\"handle\":\"${NS}_a\"}"
step "register-bob"   POST /users   "{\"handle\":\"${NS}_b\"}"
step "register-carol" POST /users   "{\"handle\":\"${NS}_c\"}"
step "alice-tweet-1"  POST /tweets  "{\"author\":\"${NS}_a\",\"text\":\"alice tweet 1\"}"
step "alice-tweet-2"  POST /tweets  "{\"author\":\"${NS}_a\",\"text\":\"alice tweet 2\"}"
step "bob-tweet"      POST /tweets  "{\"author\":\"${NS}_b\",\"text\":\"bob tweet\"}"
step "carol-tweet"    POST /tweets  "{\"author\":\"${NS}_c\",\"text\":\"carol tweet\"}"
step "alice-follow-bob" POST /follow "{\"from\":\"${NS}_a\",\"to\":\"${NS}_b\"}"
step "alice-timeline" GET  "/timeline?user=${NS}_a"

# F3: idempotent follow.
step "alice-follow-bob-again" POST /follow "{\"from\":\"${NS}_a\",\"to\":\"${NS}_b\"}"
step "alice-timeline-after"   GET  "/timeline?user=${NS}_a"

# F4: self-follow rejected.
step "self-follow" POST /follow "{\"from\":\"${NS}_a\",\"to\":\"${NS}_a\"}"

# F6: unknown author rejected.
step "unknown-author" POST /tweets "{\"author\":\"${NS}_ghost\",\"text\":\"reject\"}"

# F9: unknown follow target rejected.
step "unknown-target" POST /follow "{\"from\":\"${NS}_a\",\"to\":\"${NS}_phantom\"}"

# Liveness probes.
step "healthz" GET /healthz ""
step "version" GET /version ""

# Footer.
cat <<EOF

## summary

| | |
|---|---|
| total steps | $TOTAL |
| identical | $((TOTAL - DIFFS)) |
| differing | $DIFFS |

$(
  if [ "$DIFFS" -eq 0 ]; then
    echo "**Both impls agree on every step.** ✅"
  else
    echo "**$DIFFS step(s) diverged.** Investigate above."
  fi
)

_run namespace: \`$NS\`_
EOF

exit "$DIFFS"
