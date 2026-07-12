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

# S3-backed distributed "default" project quack E2E (needs Docker: Postgres + MinIO).
check-s3-e2e:
    #!/usr/bin/env bash
    set -euo pipefail
    # --wait only on postgres/minio (they have healthchecks); minio-setup is a
    # one-shot job that exits 0 once it creates the bucket, which --wait treats
    # as "not running" and fails on if targeted directly. Run it separately,
    # attached, so this recipe blocks until the bucket actually exists.
    docker compose -f docker-compose.dev.yml up -d --wait postgres minio
    docker compose -f docker-compose.dev.yml up minio-setup
    trap 'docker compose -f docker-compose.dev.yml down -v' EXIT
    cargo test --test quack_s3_query_surface_test -- --ignored --nocapture

# Storage-mode matrix E2E: proves ingest->flush->query->restart-persistence in
# all three modes (local file / PG+local data / PG+S3). Needs Docker (Postgres +
# MinIO), like check-s3-e2e.
check-storage-matrix-e2e:
    #!/usr/bin/env bash
    set -euo pipefail
    docker compose -f docker-compose.dev.yml up -d --wait postgres minio
    docker compose -f docker-compose.dev.yml up minio-setup
    trap 'docker compose -f docker-compose.dev.yml down -v' EXIT
    cargo test --test storage_mode_matrix_test -- --ignored --nocapture

# Hosted control-plane E2E: multi-tenant provisioning + key/revoke/suspend
# lifecycle + restart-persistence against the shared PG `sigiro_control` catalog.
# Needs only Docker Postgres (data files stay local — no MinIO).
check-control-plane-e2e:
    #!/usr/bin/env bash
    set -euo pipefail
    docker compose -f docker-compose.dev.yml up -d --wait postgres
    trap 'docker compose -f docker-compose.dev.yml down -v' EXIT
    cargo test --test control_plane_e2e_test -- --ignored --nocapture

# Hosted multi-tenant ISOLATION attack matrix (needs Docker: Postgres catalog).
# A client with tenant A's key must never read tenant B's data or escape the
# SQL firewall (cross-schema, ATTACH, file/URL/S3 readers, getenv, CTE, time-travel).
check-hosted-isolation-e2e:
    #!/usr/bin/env bash
    set -euo pipefail
    docker compose -f docker-compose.dev.yml up -d --wait postgres
    trap 'docker compose -f docker-compose.dev.yml down -v' EXIT
    cargo test --test hosted_isolation_attack_test -- --ignored --nocapture

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
