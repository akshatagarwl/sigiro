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

# Clean all build artifacts and dev containers.
clean:
    cargo clean
    docker compose -f docker-compose.dev.yml down -v --remove-orphans
    docker compose -f docker-compose.federation.yml down -v --remove-orphans 2>/dev/null || true
