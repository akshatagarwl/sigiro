#!/usr/bin/env bash
set -euo pipefail
system=$(nix eval --impure --raw --expr builtins.currentSystem)
for package in rust-deps rust-workspace-deps rust-wasm-deps dashboard docs unbundled sigiro; do
  output=$(nix eval --raw ".#packages.$system.$package.outPath")
  if nix path-info "$output" > /dev/null 2>&1; then
    printf '%s\n' "$output"
  fi
done
for check in clippy clippy-wasm fmt test; do
  output=$(nix eval --raw ".#checks.$system.$check.outPath")
  if nix path-info "$output" > /dev/null 2>&1; then
    printf '%s\n' "$output"
  fi
done
