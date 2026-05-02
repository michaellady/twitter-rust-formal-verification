#!/usr/bin/env bash
# Run the TLA+ TLC bounded model checker against the canonical spec.
#
# In CI we use a pinned tla2tools.jar; locally either set TLA_TOOLS to the
# jar path or the script will download a known-good release.
set -euo pipefail

cd "$(dirname "$0")/.."

TLA_VERSION="${TLA_VERSION:-1.8.0}"
TLA_JAR="${TLA_TOOLS:-/tmp/tla2tools-${TLA_VERSION}.jar}"

if [ ! -f "$TLA_JAR" ]; then
  echo "tla2tools.jar not at $TLA_JAR — downloading v${TLA_VERSION}…" >&2
  curl -fsSL -o "$TLA_JAR" \
    "https://github.com/tlaplus/tlaplus/releases/download/v${TLA_VERSION}/tla2tools.jar"
fi

if ! command -v java >/dev/null; then
  echo "java not found — install Java 11+ to run TLC" >&2
  exit 1
fi

echo "Running TLC on specs/twitter.tla …"
cd specs
java -XX:+UseParallelGC -cp "$TLA_JAR" tlc2.TLC -config twitter.cfg twitter.tla
