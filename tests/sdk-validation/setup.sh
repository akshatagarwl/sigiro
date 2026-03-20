#!/usr/bin/env bash
# Clone Pyroscope rideshare examples for SDK validation.
# Only clones once — skips if pyroscope-examples/ already exists.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

EXAMPLES_DIR="$SCRIPT_DIR/pyroscope-examples"

if [ -d "$EXAMPLES_DIR" ]; then
  echo "Pyroscope examples already cloned, skipping."
  exit 0
fi

echo "Cloning Pyroscope examples (sparse checkout)..."

git clone --depth=1 --filter=blob:none --sparse \
  https://github.com/grafana/pyroscope.git "$EXAMPLES_DIR"

cd "$EXAMPLES_DIR"
git sparse-checkout set \
  examples/language-sdk-instrumentation/golang-push/rideshare \
  examples/language-sdk-instrumentation/python/rideshare/flask \
  examples/language-sdk-instrumentation/nodejs/express \
  examples/language-sdk-instrumentation/java/rideshare

# Move example dirs to top level so docker-compose context paths work.
mv examples/language-sdk-instrumentation/* .
rm -rf examples .git

echo "Pyroscope examples ready at $EXAMPLES_DIR"
