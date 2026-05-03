#!/usr/bin/env bash
# Run the TLA+ TLC bounded model checker against the canonical spec.
#
# Usage: bash scripts/run_tlc.sh [config-path]
#   config-path: path to the TLA config relative to repo root.
#                Defaults to specs/twitter.cfg.
#                For PR (smaller bound), use specs/twitter-pr.cfg.
#
# In CI we use a pinned tla2tools.jar; locally either set TLA_TOOLS to the
# jar path or the script will download a known-good release.
set -euo pipefail

cd "$(dirname "$0")/.."

CFG="${1:-specs/twitter.cfg}"
if [ ! -f "$CFG" ]; then
  echo "TLC config not found: $CFG" >&2
  exit 1
fi

TLA_VERSION="${TLA_VERSION:-1.8.0}"
TLA_JAR="${TLA_TOOLS:-/tmp/tla2tools-${TLA_VERSION}.jar}"

if [ ! -f "$TLA_JAR" ]; then
  echo "tla2tools.jar not at $TLA_JAR — downloading v${TLA_VERSION}…" >&2
  curl -fsSL -o "$TLA_JAR" \
    "https://github.com/tlaplus/tlaplus/releases/download/v${TLA_VERSION}/tla2tools.jar"
fi

# Use `java -version` rather than `command -v java`: on macOS without a JRE
# installed, `/usr/bin/java` is a shim that exits 0 from `command -v` but
# prints the "install Java" prompt and exits 0 from `-version`. Checking the
# version output explicitly catches the shim and any other broken installs.
if ! java -version >/dev/null 2>&1; then
  echo "java not found or unable to launch — install Java 17+ to run TLC" >&2
  java -version 2>&1 | head -3 >&2
  exit 1
fi

WORKERS="$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4)"

# Resolve the .cfg path relative to specs/ since TLC runs from there.
CFG_NAME="$(basename "$CFG")"
echo "Running TLC on specs/twitter.tla with config $CFG ($WORKERS workers) …"
cd specs
java -XX:+UseParallelGC -cp "$TLA_JAR" tlc2.TLC \
  -workers "$WORKERS" -config "$CFG_NAME" twitter.tla
