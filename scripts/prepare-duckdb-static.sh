#!/usr/bin/env bash
# Download and prepare DuckDB's pinned, self-contained static bundle.
# Usage: eval "$(scripts/prepare-duckdb-static.sh TARGET_TRIPLE)"
set -euo pipefail

version="${DUCKDB_VERSION:-1.5.5}"
target="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
cache_root="${DUCKDB_STATIC_CACHE:-${CARGO_TARGET_DIR:-target}/duckdb-static}"
if [[ "$cache_root" != /* ]]; then
  cache_root="$(cd "$(dirname "$0")/.." && pwd)/$cache_root"
fi
cache_dir="${cache_root}/${version}/${target}"
archive="${cache_dir}/static-libs.zip"
lib="${cache_dir}/libduckdb_static.a"
header="${cache_dir}/duckdb.h"

case "$target" in
  x86_64-unknown-linux-gnu) platform=linux-amd64; checksum=deb47c5300f3c99725e84cdb14d214c3b12bbd748b613b1698b938c894cb68eb ;;
  aarch64-unknown-linux-gnu) platform=linux-arm64; checksum=ea6a34cb49ec2db5ed23d9e8311237c53c32abf9cdbf5dd608c4176c3dd8bfeb ;;
  x86_64-apple-darwin) platform=osx-amd64; checksum=a27d36fa1247a3ffa1692e7aa0bf4ea4d1e0ee51da7c4df7a5db5217357b1b4d ;;
  aarch64-apple-darwin) platform=osx-arm64; checksum=d79ec66b8a4054b866faada82e9e31f859a713c555b3f1c4b71c4a43d3273e9c ;;
  *) echo "prepare-duckdb-static: unsupported target: $target" >&2; exit 2 ;;
esac

mkdir -p "$cache_dir"
url="https://github.com/duckdb/duckdb/releases/download/v${version}/static-libs-${platform}.zip"
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
  unzip -q "$archive" -d "$work"
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
