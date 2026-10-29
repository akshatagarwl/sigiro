#!/usr/bin/env bash
# Download and prepare DuckDB's pinned, self-contained static bundle.
# Usage: eval "$(scripts/prepare-duckdb-static.sh TARGET_TRIPLE)"
set -euo pipefail

version="v2.0.0"
target="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
# Never under target/: mbx's GC deletes managed targets mid-build when disk runs
# low, which surfaced as "cannot find -lduckdb" at link time.
if [[ "$(uname -s)" == Darwin ]]; then
  default_cache="$HOME/Library/Caches/sigiro/duckdb-static"
else
  default_cache="${XDG_CACHE_HOME:-$HOME/.cache}/sigiro/duckdb-static"
fi
cache_root="${DUCKDB_STATIC_CACHE:-$default_cache}"
cache_dir="${cache_root}/${version}/${target}"
archive="${cache_dir}/static-libs.tar.gz"
lib="${cache_dir}/libduckdb_static.a"
header="${cache_dir}/duckdb.h"

case "$target" in
  x86_64-unknown-linux-gnu) platform=linux-amd64; checksum=112d62c980f5447cd303a70b3dacbc09acd41ccb4ef6d154e8629f543a0f8568 ;;
  aarch64-unknown-linux-gnu) platform=linux-arm64; checksum=8171b4842e68a85b2e82b342d3bf7b542a88fd6666e0c18438d7b459012494ee ;;
  x86_64-apple-darwin) platform=osx-amd64; checksum=7e6101ddc8de73e8dcc66f790e658e07891165d026992b40ceba20e50b01f8ec ;;
  aarch64-apple-darwin) platform=osx-arm64; checksum=c3a571e6d4a0da97534c631df463ab13c07f8cf78049625f5df936b7beaab9ce ;;
  *) echo "prepare-duckdb-static: unsupported target: $target" >&2; exit 2 ;;
esac

mkdir -p "$cache_dir"
url="https://github.com/duckdb/duckdb/releases/download/${version}/duckdb-static-libs-${platform}.tar.gz"
if command -v sha256sum >/dev/null 2>&1; then
  checksum_cmd=(sha256sum)
else
  checksum_cmd=(shasum -a 256)
fi
if [[ ! -f "$archive" ]]; then
  tmp="${archive}.tmp.$$"
  trap 'rm -f "$tmp"' EXIT
  curl --fail --location --retry 3 --retry-delay 1 "$url" -o "$tmp"
  printf '%s  %s\n' "$checksum" "$tmp" | "${checksum_cmd[@]}" -c - >/dev/null
  mv "$tmp" "$archive"
  trap - EXIT
fi
printf '%s  %s\n' "$checksum" "$archive" | "${checksum_cmd[@]}" -c - >/dev/null

if [[ ! -s "$lib" || ! -s "$header" ]]; then
  work="$(mktemp -d "${cache_dir}/extract.XXXXXX")"
  trap 'rm -rf "$work"' EXIT
  tar -xzf "$archive" -C "$work"
  fragments=()
  while IFS= read -r fragment; do fragments+=("$fragment"); done < <(find "$work" -type f -name '*.a' -print | sort)
  [[ ${#fragments[@]} -gt 0 ]] || { echo "prepare-duckdb-static: archive has no .a fragments" >&2; exit 1; }
  found_header="$(find "$work" -type f -name duckdb.h -print -quit)"
  [[ -n "$found_header" ]] || { echo "prepare-duckdb-static: archive has no duckdb.h" >&2; exit 1; }
  output="${work}/libduckdb_static.a"
  case "$target" in
    *-apple-darwin)
      libtool -static -o "$output" "${fragments[@]}"
      ranlib "$output"
      ;;
    x86_64-unknown-linux-gnu)
      if [[ "$(uname -s)" == Linux ]]; then
        ar_bin="$(command -v ar)"
        ranlib_bin="$(command -v ranlib)"
      else
        ar_bin="$(command -v x86_64-linux-gnu-ar)"
        ranlib_bin="$(command -v x86_64-linux-gnu-ranlib)"
      fi
      ;;
    aarch64-unknown-linux-gnu)
      if [[ "$(uname -s)" == Linux ]]; then
        ar_bin="$(command -v ar)"
        ranlib_bin="$(command -v ranlib)"
      else
        ar_bin="$(command -v aarch64-linux-gnu-ar)"
        ranlib_bin="$(command -v aarch64-linux-gnu-ranlib)"
      fi
      ;;
  esac
  if [[ "$target" == *-unknown-linux-gnu ]]; then
    (
      cd "$work"
      { printf 'create libduckdb_static.a\n'; for fragment in ./*.a; do printf 'addlib %s\n' "$fragment"; done; printf 'save\nend\n'; } | "$ar_bin" -M
      "$ranlib_bin" libduckdb_static.a
    )
  fi
  test -s "$output"
  mv "$output" "$lib"
  cp "$found_header" "$header"
  trap - EXIT
  rm -rf "$work"
fi

printf 'export DUCKDB_LIB_DIR=%q\n' "$cache_dir"
printf 'export DUCKDB_INCLUDE_DIR=%q\n' "$cache_dir"
printf 'export DUCKDB_STATIC=1\n'
printf 'export DUCKDB_STATIC_PLATFORM=%q\n' "$platform"
