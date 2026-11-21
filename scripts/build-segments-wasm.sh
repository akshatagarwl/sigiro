#!/usr/bin/env bash
set -euo pipefail
# Paths below are repo-relative; `pnpm run build:wasm` starts in cloud/.
cd "$(dirname "$0")/.."

# `nix develop` supplies wasm-bindgen; it must match the locked crate exactly.
expected=$(awk '/^name = "wasm-bindgen"$/ { getline; gsub(/"/, "", $3); print $3 }' Cargo.lock)
[[ -n "$expected" ]] || { echo "wasm-bindgen is missing from Cargo.lock" >&2; exit 1; }
command -v wasm-bindgen >/dev/null || { echo "wasm-bindgen not found; run inside nix develop" >&2; exit 1; }
actual=$(wasm-bindgen --version | awk '{ print $2 }')
if [[ "$actual" != "$expected" ]]; then
  echo "wasm-bindgen $actual does not match Cargo.lock $expected; update flake.lock or Cargo.lock" >&2
  exit 1
fi

cargo build -p sigiro-segments-wasm --target wasm32-unknown-unknown --release --locked
rm -rf cloud/vendor/segments-wasm
mkdir -p cloud/vendor/segments-wasm
wasm-bindgen --target web --out-dir cloud/vendor/segments-wasm target/wasm32-unknown-unknown/release/sigiro_segments_wasm.wasm
gzip -9 -c cloud/vendor/segments-wasm/sigiro_segments_wasm_bg.wasm | wc -c | tr -d ' '
