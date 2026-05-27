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

# Build multi-arch Docker image.
# Use: just image          — build and load into docker (local only)
#      just image --push   — build and push to registry
#      just image arm64    — build single platform (faster for testing)
image *args:
    docker buildx bake sigiro {{args}}



# Build using GitHub Actions cache only (for CI).
# Use: just image-ci --push
image-ci *args:
    docker buildx bake sigiro \
        --set "*.cache-from=type=gha" \
        --set "*.cache-to=type=gha,mode=max" \
        {{args}}

# Build and push to GHCR. Requires GH CLI login via `gh auth login`.
image-push:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Logging into GHCR..."
    TOKEN=$(gh auth token)
    echo "$TOKEN" | docker login ghcr.io -u "$(gh api user --jq .login)" --password-stdin
    echo "Building and pushing to GHCR..."
    docker buildx bake sigiro --push

# Cross-compile sigiro for Linux ARM64 Lima VMs (GNU toolchain, not zigbuild).
lima-build:
    #!/usr/bin/env bash
    set -euo pipefail
    source scripts/lima-cross-env.sh
    ulimit -n 65536 2>/dev/null || true
    cargo build --release --target "$SIGIRO_LIMA_TARGET" --bin sigiro
    file "$SIGIRO_LIMA_SIGIRO_BIN"

# Clean all build artifacts and dev containers.
clean:
    cargo clean
    docker compose -f docker-compose.dev.yml down -v --remove-orphans
