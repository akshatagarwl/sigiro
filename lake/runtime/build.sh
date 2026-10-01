#!/usr/bin/env bash
# Builds Sigiro's lake runtime: @ducklings/workers-ducklake at a pinned commit plus
# lake/runtime/ducklings.patch (json + icu extensions, exported fileSystem()).
# Output: lake/runtime/dist/ducklings-workers-ducklake-<version>.tgz
#
# Reproducible inputs only: the ducklings commit, the submodule commits it records
# (the upstream Makefile's `pin-versions` follows moving branches, so it is not used),
# and the Emscripten version upstream CI pins.
set -euo pipefail

DUCKLINGS_COMMIT=3ecf106fb696896112e5f80436308e6b8be2ce09
EMSDK_VERSION=4.0.22
here=$(cd "$(dirname "$0")" && pwd)
work=${SIGIRO_LAKE_RUNTIME_WORK:-"$here/.work"}
mkdir -p "$work" "$here/dist"

if [ ! -d "$work/emsdk" ]; then git clone -q https://github.com/emscripten-core/emsdk.git "$work/emsdk"; fi
"$work/emsdk/emsdk" install "$EMSDK_VERSION" >/dev/null
"$work/emsdk/emsdk" activate "$EMSDK_VERSION" >/dev/null
# shellcheck disable=SC1091
source "$work/emsdk/emsdk_env.sh" >/dev/null
[ "$(emcc --version | head -1 | grep -o '[0-9]\+\.[0-9]\+\.[0-9]\+' | head -1)" = "$EMSDK_VERSION" ] || { echo "emcc is not $EMSDK_VERSION" >&2; exit 1; }

src="$work/ducklings"
if [ ! -d "$src/.git" ]; then git clone -q https://github.com/tobilg/ducklings.git "$src"; fi
git -C "$src" fetch -q origin
git -C "$src" checkout -q --force "$(git -C "$src" rev-parse "$DUCKLINGS_COMMIT^{commit}")"
git -C "$src" clean -qfdx -e build -e vcpkg -e vcpkg_installed -e node_modules
git -C "$src" submodule sync -q --recursive
git -C "$src" submodule update -q --init --recursive --force
git -C "$src" apply --check "$here/ducklings.patch"
git -C "$src" apply "$here/ducklings.patch"

cd "$src"
SIGIRO_LAKE_RUNTIME=1 ./scripts/build-duckdb.sh workers-ducklake
pnpm install --frozen-lockfile >/dev/null
pnpm --filter @ducklings/workers-ducklake build
cd packages/ducklings-workers-ducklake
tarball=$(npm pack --pack-destination "$here/dist" 2>/dev/null | tail -1)
echo "$here/dist/$tarball"
