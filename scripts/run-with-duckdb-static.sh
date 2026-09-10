#!/usr/bin/env bash
# Run one repository command with the target's prepared DuckDB archive exported.
set -euo pipefail
[[ $# -gt 0 ]] || { echo "usage: $0 COMMAND [ARG...]" >&2; exit 2; }
target="${TARGET:-$(rustc -vV | sed -n 's/^host: //p')}"
eval "$(scripts/prepare-duckdb-static.sh "$target")"
exec "$@"
