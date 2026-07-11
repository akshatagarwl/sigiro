set dotenv-load
set fallback

_default:
    @just --list

build:
    cargo build --release --bin sigiro {{ if env("TARGET", "") != "" { "--target " + env("TARGET") } else { "" } }}

# Fast verification gate (debug). Run this before every commit.
check:
    cargo fmt --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo test

# Full release-mode gate. Run before building a deploy image / cutting a release.
check-release:
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

# Build multi-arch Docker image. Pass --push to push, or --set flags for cache config.
image *args:
    docker buildx bake sigiro {{args}}

# Cross-compile sigiro for Linux ARM64 Lima VMs.
lima-build:
    #!/usr/bin/env bash
    set -euo pipefail
    source scripts/lima-cross-env.sh
    ulimit -n 65536 2>/dev/null || true
    cargo build --release --target "$SIGIRO_LIMA_TARGET" --bin sigiro
    file "$SIGIRO_LIMA_SIGIRO_BIN"

clean:
    cargo clean
    docker compose -f docker-compose.dev.yml down -v --remove-orphans
