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

# Build Docker image for local use.
image:
    docker buildx bake local

# Build spiced query engine from submodule (first run ~1h, cached ~5m).
spiced:
    cd third_party/spiceai && make install

# Clean all build artifacts and dev containers.
clean:
    cargo clean
    docker compose -f docker-compose.dev.yml down -v --remove-orphans
    docker compose -f docker-compose.federation.yml down -v --remove-orphans 2>/dev/null || true
