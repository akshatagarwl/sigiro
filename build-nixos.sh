#!/bin/bash
set -eux -o pipefail

export PATH="$HOME/.nix-profile/bin:$PATH"
export HOME=~/workspace/duckdb-home
export CARGO_HOME=~/workspace/sigiro/.cargo-home
export CARGO_REGISTRIES_DIR=~/workspace/sigiro/.cargo-registries
export TMPDIR=~/workspace/tmp

cd ~/workspace/sigiro
cargo build --release 2>&1 | tee ~/workspace/sigiro/build.log
echo "exit: $?"
