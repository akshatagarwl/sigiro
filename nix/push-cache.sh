#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
token_file="${XDG_CONFIG_HOME:-$HOME/.config}/cachix/akshat-sigiro-builds.token"
if [ -z "${CACHIX_AUTH_TOKEN:-}" ] && [ -s "$token_file" ]; then
  export CACHIX_AUTH_TOKEN
  CACHIX_AUTH_TOKEN=$(< "$token_file")
fi
if [ -z "${CACHIX_AUTH_TOKEN:-}" ]; then
  printf 'No cache write token; using the public cache read-only.\n'
  exit 0
fi
paths=$(bash nix/cache-paths.sh)
if [ -z "$paths" ]; then
  printf 'No project outputs have been built yet.\n'
  exit 0
fi
printf '%s\n' "$paths" | nix run .#cache-client -- push akshat-sigiro-builds
