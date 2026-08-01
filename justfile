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

# Lint for unnecessary `pub` API surface (astral-sh/hawk). Closed-world
# analysis: anything `pub` the `sigiro` binary can't reach is dead or could be
# `pub(crate)`. Pinned to hawk's own toolchain (it links rustc_private).
# Denies the two actionable groups; a full-workspace compile, so it's a
# pre-push gate (`just install-hooks`), not pre-commit. `--fix` auto-applies
# the pub→pub(crate) fixes.
# The target dir is pinned into target/ via hawk's own `--target-dir` flag. Hawk
# defaults to $TMPDIR/cargo-hawk-target, which macOS prunes on its own schedule,
# leaving fingerprints that point at build-script output that is gone — the gate
# then fails on a missing libsqlite3-sys bindgen.rs and has to be cleared by hand.
# Setting CARGO_TARGET_DIR does NOT fix this: hawk overrides it with its own temp
# dir, so the env var looked right and changed nothing. Use the flag.
hawk *args:
    cargo +1.97.1 hawk check --manifest-path Cargo.toml --target-dir target/hawk \
      -A hawk::unnecessary_restricted_visibility \
      -D hawk::dead_public -D hawk::unnecessary_public {{ args }}

# Gate structural erosion + clone density (SlopCodeBench metrics). Catches the
# way agent-extended code rots: features patched into god functions instead of
# extracted, and per-variant copy-paste. Pre-push, like hawk. Ceilings live in
# the script; ratchet them down after every refactor that beats them.
slop:
    python3 scripts/slop_audit.py

# Point git at the tracked .githooks/ dir (one-time per clone) so the pre-push
# hawk + slop gates run. Teammates run this once after cloning.
install-hooks:
    git config core.hooksPath .githooks

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

# A permanent DuckLake attach failure (data_path / storage-backend mismatch)
# must fail fast, not wedge the pool in its 30x retry loop. Needs Docker Postgres.
check-attach-fail-fast-e2e:
    #!/usr/bin/env bash
    set -euo pipefail
    docker compose -f docker-compose.dev.yml up -d --wait postgres
    trap 'docker compose -f docker-compose.dev.yml down -v' EXIT
    cargo test --test attach_fail_fast_test -- --ignored --nocapture

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

# Fast LOCAL image build: native arch only (no amd64 QEMU emulation), no remote
# cache, loaded into docker so you can run it. Use this while iterating; use
# `just image --push` for the real multi-arch publish. Assumes an arm64 host.
image-dev *args:
    docker buildx bake sigiro \
      --set "*.platform=linux/arm64" \
      --set "*.cache-to=" \
      --set "*.cache-from=" \
      --load {{args}}

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

# Refresh www/openapi.json — the snapshot the API reference is generated from.
# Boots the binary, reads its own /openapi.json, and drops the /admin paths: key
# minting and tenant creation are deliberately not part of the published surface.
# Run this whenever a #[utoipa::path] changes, then `cd www && bun run api:docs`.
api-spec:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build --quiet --bin sigiro
    # A warm data dir under target/: with a fresh one DuckDB re-downloads its
    # extensions on every run and the wait below times out.
    dir=target/spec-db
    mkdir -p "$dir"
    SIGIRO_DATA_DIR="$dir" ./target/debug/sigiro serve >/dev/null 2>&1 &
    trap 'kill %1 2>/dev/null || true' EXIT
    for _ in $(seq 1 60); do
      curl -sf http://localhost:9999/openapi.json -o "$dir/spec.json" && break
      sleep 2
    done
    test -s "$dir/spec.json" || { echo "sigiro never served /openapi.json" >&2; exit 1; }
    python3 -c "
    import json
    spec = json.load(open('$dir/spec.json'))
    spec['paths'] = {p: v for p, v in spec['paths'].items() if not p.startswith('/admin')}
    json.dump(spec, open('www/openapi.json', 'w'), indent=2)
    print('wrote www/openapi.json -', len(spec['paths']), 'paths')
    "
