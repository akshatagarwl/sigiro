# Sigiro — single entrypoint for everything.
# Usage: just <command>

set dotenv-load
set fallback

_default:
    @just --list

# Build release binary. Set TARGET env var for cross-compilation.
build:
    cargo build --release --bin sigiro {{ if env("TARGET", "") != "" { "--target " + env("TARGET") } else { "" } }}

# Full verification gate. Run this before every commit.
check:
    cargo fmt --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo test --release

# Run sigiro locally with dev dependencies (Postgres, MinIO).
run:
    #!/usr/bin/env bash
    set -euo pipefail
    docker compose -f docker-compose.dev.yml up -d
    trap 'docker compose -f docker-compose.dev.yml down' EXIT
    cargo run --release --bin sigiro

# Build multi-arch Docker image. Pass --push to publish.
#   just image
#   just image --push
#   just image --set "*.tags=myregistry/sigiro:v1.2.3"
image *args:
    docker buildx bake sigiro {{args}}

# Build spiced query engine from submodule with only required features.
# First run ~1h, cached ~5m. Override features via SPICED_FEATURES env var.
spiced:
    cd third_party/spiceai && SPICED_CUSTOM_FEATURES="{{ env('SPICED_FEATURES', 'duckdb,alloc-snmalloc') }}" make install

# Cross-compile Linux ARM64 binary for Lima VMs.
lima-build:
    #!/usr/bin/env bash
    set -euo pipefail
    export AR_aarch64_unknown_linux_gnu=/opt/homebrew/opt/llvm/bin/llvm-ar
    ulimit -n 65536
    cargo zigbuild --release --target aarch64-unknown-linux-gnu

# Run multi-node federation test on Lima VMs (central + worker).
# Spins up VMs, cross-compiles, installs spiced, and validates control plane.
fed-test:
    ./dev/lima/sigiro-federation-test.sh

# Start federation Lima VMs without running tests.
fed-start:
    limactl start --name=sigiro-fed-central dev/lima/sigiro-fed-central.yaml
    limactl start --name=sigiro-fed-worker dev/lima/sigiro-fed-worker.yaml

# Stop federation Lima VMs.
fed-stop:
    limactl stop sigiro-fed-central sigiro-fed-worker 2>/dev/null || true

# Delete federation Lima VMs.
fed-destroy:
    limactl delete -f sigiro-fed-central sigiro-fed-worker 2>/dev/null || true

# Tail central node logs.
fed-logs-central:
    limactl shell sigiro-fed-central bash -lc 'tail -f /tmp/sigiro-central.log'

# Tail worker node logs.
fed-logs-worker:
    limactl shell sigiro-fed-worker bash -lc 'tail -f /tmp/sigiro-worker.log'

# SSH into central VM.
fed-ssh-central:
    limactl shell sigiro-fed-central

# SSH into worker VM.
fed-ssh-worker:
    limactl shell sigiro-fed-worker

# Clean all build artifacts and dev containers.
clean:
    cargo clean
    docker compose -f docker-compose.dev.yml down -v --remove-orphans
    docker compose -f docker-compose.federation.yml down -v --remove-orphans 2>/dev/null || true
    just fed-destroy
