#!/usr/bin/env bash
set -euo pipefail
# Paths below are repo-relative; `aube run build:wasm` starts in cloud/.
cd "$(dirname "$0")/.."
version=0.2.114
root="target/wasm-bindgen-cli-$version"
bin="$root/bin/wasm-bindgen"
if [[ ! -x "$bin" ]]; then
  cargo +stable install wasm-bindgen-cli --version "$version" --locked --root "$root"
fi
cargo +stable build -p sigiro-segments-wasm --target wasm32-unknown-unknown --release --locked
rm -rf cloud/vendor/segments-wasm
mkdir -p cloud/vendor/segments-wasm
"$bin" --target web --out-dir cloud/vendor/segments-wasm target/wasm32-unknown-unknown/release/sigiro_segments_wasm.wasm
gzip -9 -c cloud/vendor/segments-wasm/sigiro_segments_wasm_bg.wasm | wc -c | tr -d ' '
