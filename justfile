set dotenv-load
set fallback

_default:
    @just --list

build:
    cargo build --release --bin sigiro {{ if env("TARGET", "") != "" { "--target " + env("TARGET") } else { "" } }}

check:
    cargo fmt --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo test --release

run:
    #!/usr/bin/env bash
    set -euo pipefail
    docker compose -f docker-compose.dev.yml up -d
    trap 'docker compose -f docker-compose.dev.yml down' EXIT
    cargo run --release --bin sigiro

image *args:
    docker buildx bake sigiro {{args}}

image-ci *args:
    docker buildx bake sigiro \
        --set "*.cache-from=type=gha" \
        --set "*.cache-to=type=gha,mode=max" \
        {{args}}

image-push:
    #!/usr/bin/env bash
    set -euo pipefail
    TOKEN=$(gh auth token)
    echo "$TOKEN" | docker login ghcr.io -u "$(gh api user --jq .login)" --password-stdin
    docker buildx bake sigiro --push

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
