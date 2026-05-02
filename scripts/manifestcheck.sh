#!/usr/bin/env bash
# manifestcheck — verify every entry in COVERAGE.md is backed by a real test
# in tests/*.rs that contains at least 3 assertion calls (assert*!).
#
# Lightweight "tests aren't lying" gate. Runs from repo root.
set -euo pipefail

cd "$(dirname "$0")/.."

if [ ! -f COVERAGE.md ]; then
  echo "manifestcheck: COVERAGE.md missing" >&2
  exit 1
fi

# Extract every (F-NN, test_name) pair from COVERAGE.md.
# Lines look like:
#   - [F-01] description -> test_name (assertions: ...)
#   - F-01 description -> test_name
entries_raw=$(grep -E 'F-[0-9]+' COVERAGE.md \
  | grep -oE 'F-[0-9]+[^|]*->[[:space:]]*[A-Za-z_][A-Za-z0-9_]*' \
  | awk -F'->' '{
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", $1);
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", $2);
      print $1 "\t" $2;
    }')

if [ -z "$entries_raw" ]; then
  echo "manifestcheck: no F-NN entries found in COVERAGE.md" >&2
  exit 1
fi

count_entries=$(printf '%s\n' "$entries_raw" | wc -l | tr -d ' ')

fail=0
while IFS=$'\t' read -r left test_name; do
  [ -z "$left" ] && continue
  fid=$(printf '%s' "$left" | grep -oE 'F-[0-9]+')
  # Find the test function in any tests/*.rs or crates/*/src/lib.rs.
  test_file=$(grep -rEl "fn[[:space:]]+${test_name}[[:space:]]*\(" tests crates 2>/dev/null | head -1 || true)
  if [ -z "$test_file" ]; then
    echo "  [FAIL] $fid -> $test_name : test function not found" >&2
    fail=1
    continue
  fi
  # Extract the test body and count assertion calls.
  # Use awk to extract from `fn <name>(` to the matching closing brace at column 1.
  body=$(awk -v name="$test_name" '
    BEGIN { depth = 0; in_fn = 0 }
    !in_fn && $0 ~ ("fn[[:space:]]+" name "[[:space:]]*\\(") { in_fn = 1 }
    in_fn {
      print
      n = gsub(/\{/, "{")
      m = gsub(/\}/, "}")
      depth += n - m
      if (depth <= 0 && in_fn && $0 ~ /\}/) exit
    }
  ' "$test_file")
  # Count any `assert` macro variants: assert!, assert_eq!, assert_ne!, assert_matches!
  count=$(printf '%s' "$body" | grep -cE '\bassert[a-z_]*!' || true)
  if [ "$count" -lt 3 ]; then
    echo "  [FAIL] $fid -> $test_name in $test_file: only $count assert*! calls (need >= 3)" >&2
    fail=1
  else
    echo "  [OK]   $fid -> $test_name ($count assertions)"
  fi
done <<EOF
$entries_raw
EOF

if [ "$fail" -ne 0 ]; then
  echo "manifestcheck: FAILED" >&2
  exit 1
fi
echo "manifestcheck: OK ($count_entries entries)"
