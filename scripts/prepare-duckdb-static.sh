#!/usr/bin/env bash
# Download and prepare DuckDB's pinned, self-contained static bundle.
# Usage: eval "$(scripts/prepare-duckdb-static.sh TARGET_TRIPLE)"
set -euo pipefail

version="v2.0.0-alpha41489"
commit="10de9573794001c649621013bdd93553b54e00c9"
target="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
cache_root="${DUCKDB_STATIC_CACHE:-${CARGO_TARGET_DIR:-target}/duckdb-static}"
if [[ "$cache_root" != /* ]]; then
  cache_root="$(cd "$(dirname "$0")/.." && pwd)/$cache_root"
fi
cache_dir="${cache_root}/${version}-${commit:0:10}/${target}"
archive="${cache_dir}/static-libs.tar.gz"
lib="${cache_dir}/libduckdb_static.a"
header="${cache_dir}/duckdb.h"

case "$target" in
  x86_64-unknown-linux-gnu) platform=linux-amd64; checksum=f4997440399d63d789c7e85924f2a63ef4c5e54421cd50d397e06a561e7052ad ;;
  aarch64-unknown-linux-gnu) platform=linux-arm64; checksum=227cecab7a00fa2e2da1577f30af6a43b465e0ff5a82afc75ae1803e9951fd0b ;;
  x86_64-apple-darwin) platform=osx-amd64; checksum=c1657abf48c3c58180393c6e05dabdb91cd5d24dc8c45f98c4a91f487fb9031e ;;
  aarch64-apple-darwin) platform=osx-arm64; checksum=cf19469791067f6f6855debc8054f35802c8c091c4fe9b1009245b1e7263ccec ;;
  *) echo "prepare-duckdb-static: unsupported target: $target" >&2; exit 2 ;;
esac

mkdir -p "$cache_dir"
url="https://duckdb-staging.duckdb.org/${commit:0:10}/${version}/duckdb/duckdb/github_release/duckdb-static-libs-${platform}.tar.gz"
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
