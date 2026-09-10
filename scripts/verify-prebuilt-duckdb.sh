#!/usr/bin/env bash
# Fast black-box guard for the repository's DuckDB build path.
set -euo pipefail

target="${TARGET:-$(rustc -vV | sed -n 's/^host: //p')}"
cache="$(mktemp -d)"
first="$(mktemp)"
second="$(mktemp)"
log="$(mktemp)"
tree="$(mktemp)"
trap 'rm -rf "$cache"; rm -f "$first" "$second" "$log" "$tree"' EXIT

DUCKDB_STATIC_CACHE="$cache" scripts/prepare-duckdb-static.sh "$target" >"$first"
eval "$(<"$first")"
lib="$DUCKDB_LIB_DIR/libduckdb_static.a"
test -s "$lib"
mtime1="$(stat -f %m "$lib" 2>/dev/null || stat -c %Y "$lib")"
DUCKDB_STATIC_CACHE="$cache" scripts/prepare-duckdb-static.sh "$target" >"$second"
mtime2="$(stat -f %m "$lib" 2>/dev/null || stat -c %Y "$lib")"
test "$mtime1" = "$mtime2"
test "$(sed -n 's/^export DUCKDB_LIB_DIR=//p' "$first")" = "$(sed -n 's/^export DUCKDB_LIB_DIR=//p' "$second")"

mbx build --bin sigiro 2>&1 | tee "$log"
if grep -Eiq 'libduckdb-sys.*(cc|c\+\+)|duckdb.*(\.cpp|c\+\+|cmake)' "$log"; then
  echo "verify-prebuilt-duckdb: DuckDB C++ compilation detected" >&2
  exit 1
fi
mbx tree -e features -i libduckdb-sys >"$tree"
if grep -Eq 'feature "(bundled|bundled-cmake)"' "$tree"; then
  echo "verify-prebuilt-duckdb: bundled DuckDB feature detected" >&2
  exit 1
fi
printf 'prebuilt DuckDB verified: %s (reused), build linked without DuckDB C++ compilation\n' "$DUCKDB_STATIC_PLATFORM"
