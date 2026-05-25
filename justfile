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

# Build multi-arch Docker image using pre-built spiced binaries (fast, ~3min vs ~15min).
# Prerequisites:
#   just spiced           # build spiced for host (macOS)
#   just spiced-lima      # cross-compile spiced for linux-arm64 (if target Lima VMs)
# Use: just image-hybrid           — build both platforms
#      just image-hybrid --push    — build and push
#      just image-hybrid amd64     — build single platform
image-hybrid *args:
    just _ensure-spiced-binaries
    docker buildx bake sigiro-hybrid {{args}}

_ensure-spiced-binaries:
    #!/usr/bin/env bash
    set -euo pipefail
    NATIVE_SPICED="${HOME}/.spice/bin/spiced"
    LIMA_SPICED="target/aarch64-unknown-linux-gnu/release/spiced"
    if [[ ! -f "$NATIVE_SPICED" ]]; then
        echo "ERROR: spiced not built. Run 'just spiced' first." >&2
        exit 1
    fi
    mkdir -p spiced-binaries/linux-amd64 spiced-binaries/linux-arm64
    cp -v "$NATIVE_SPICED" spiced-binaries/linux-amd64/spiced
    chmod +x spiced-binaries/linux-amd64/spiced
    echo "Copied native spiced to spiced-binaries/linux-amd64/"
    if [[ -f target/aarch64-unknown-linux-gnu/release/spiced ]]; then
        cp -v target/aarch64-unknown-linux-gnu/release/spiced spiced-binaries/linux-arm64/spiced
        chmod +x spiced-binaries/linux-arm64/spiced
        echo "Copied Lima spiced to spiced-binaries/linux-arm64/"
    else
        echo "ERROR: Lima spiced not found at $LIMA_SPICED" >&2
        echo "ERROR: Cannot build linux-arm64 image — macOS binary will not run on linux-arm64" >&2
        echo "ERROR: Run 'just spiced-lima' to cross-compile spiced for Linux ARM64" >&2
        exit 1
    fi

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

# Build spiced for this host (macOS/Linux). First run ~30-60m, cached ~5m.
# Override: SPICED_FEATURES=duckdb,alloc-snmalloc
spiced:
    chmod +x scripts/build-spiced.sh
    scripts/build-spiced.sh

# Cross-compile spiced for Linux ARM64 Lima VMs. Requires aarch64-linux-gnu-gcc.
spiced-lima:
    chmod +x scripts/build-spiced.sh scripts/lima-cross-env.sh
    scripts/build-spiced.sh --lima

# Copy host-built spiced next to release sigiro (native macOS dev).
bundle-spiced:
    #!/usr/bin/env bash
    set -euo pipefail
    SPICED_SRC="${SPICED_INSTALL:-${HOME}/.spice/bin}/spiced}"
    [[ -f "$SPICED_SRC" ]] || SPICED_SRC="${HOME}/.spice/bin/spiced"
    if [[ ! -f "$SPICED_SRC" ]]; then
        echo "ERROR: spiced not found — run 'just spiced' first"
        exit 1
    fi
    mkdir -p target/release
    cp -v "$SPICED_SRC" target/release/spiced
    chmod +x target/release/spiced

# Cross-compile sigiro for Linux ARM64 Lima VMs (GNU toolchain, not zigbuild).
lima-build:
    #!/usr/bin/env bash
    set -euo pipefail
    source scripts/lima-cross-env.sh
    ulimit -n 65536 2>/dev/null || true
    cargo build --release --target "$SIGIRO_LIMA_TARGET" --bin sigiro
    file "$SIGIRO_LIMA_SIGIRO_BIN"

# Build sigiro + spiced for Lima, then run federation test.
fed-test-deps:
    just lima-build
    just spiced-lima

# Run multi-node federation test (requires: just fed-test-deps or prebuilt binaries).
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
