#!/usr/bin/env bash
# Source this file to set GNU aarch64-linux cross-compiler env for Lima VM binaries.
# Usage: source scripts/lima-cross-env.sh
set -euo pipefail

export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="${CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER:-}"
export CC_aarch64_unknown_linux_gnu="${CC_aarch64_unknown_linux_gnu:-}"
export CXX_aarch64_unknown_linux_gnu="${CXX_aarch64_unknown_linux_gnu:-}"
export AR_aarch64_unknown_linux_gnu="${AR_aarch64_unknown_linux_gnu:-}"

_find_tool() {
  local name=$1
  if command -v "$name" >/dev/null 2>&1; then
    command -v "$name"
    return 0
  fi
  return 1
}

if [[ -z "${CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER}" ]]; then
  for prefix in /opt/homebrew/bin /usr/local/bin ""; do
    if [[ -n "$prefix" ]]; then
      gcc="${prefix}/aarch64-linux-gnu-gcc"
    else
      gcc="aarch64-linux-gnu-gcc"
    fi
    if [[ -x "$gcc" ]] || _find_tool "$(basename "$gcc")" >/dev/null; then
      [[ -x "$gcc" ]] || gcc=$(_find_tool "$(basename "$gcc")")
      export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="$gcc"
      export CC_aarch64_unknown_linux_gnu="$gcc"
      export CXX_aarch64_unknown_linux_gnu="${gcc%-gcc}-g++"
      export AR_aarch64_unknown_linux_gnu="${gcc%-gcc}-ar"
      break
    fi
  done
fi

if [[ -z "${CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER}" ]]; then
  echo "ERROR: aarch64-linux-gnu-gcc not found." >&2
  echo "  macOS: brew install aarch64-unknown-linux-gnu" >&2
  echo "  Debian/Ubuntu: apt install gcc-aarch64-linux-gnu g++-aarch64-linux-gnu" >&2
  return 1 2>/dev/null || exit 1
fi

export SIGIRO_LIMA_TARGET=aarch64-unknown-linux-gnu
export SIGIRO_LIMA_SIGIRO_BIN="${SIGIRO_LIMA_SIGIRO_BIN:-target/aarch64-unknown-linux-gnu/release/sigiro}"
