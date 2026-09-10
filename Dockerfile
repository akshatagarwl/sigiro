# syntax=docker/dockerfile:1

# Toolchain image, bookworm/glibc 2.36 to match the distroless runtime. Pinned to
# $BUILDPLATFORM so rustc runs NATIVELY and cross-links to the target arch (no
# QEMU). Carries both GNU cross toolchains + cargo-chef for dep-layer caching.
FROM --platform=$BUILDPLATFORM rust:1.98.1-slim-bookworm@sha256:ebd900bae66fd508b466cef82d64a83a5fb34682e4c8b2797a42908bddc95a57 AS chef
RUN apt-get update && apt-get install -y --no-install-recommends \
      pkg-config protobuf-compiler git binutils libssl-dev make mold \
      gcc-x86-64-linux-gnu g++-x86-64-linux-gnu \
      gcc-aarch64-linux-gnu g++-aarch64-linux-gnu \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef --locked && \
    rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu
WORKDIR /build

# Prebuilt DuckDB static libs for the TARGET arch: download + ar-merge, NO
# compile. All build surfaces use the same pinned preparation script.
FROM --platform=$BUILDPLATFORM debian:bookworm-slim AS duckdb-static
ARG DUCKDB_VERSION=1.5.5
ARG TARGETARCH
COPY scripts/prepare-duckdb-static.sh /usr/local/bin/prepare-duckdb-static
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl unzip binutils && rm -rf /var/lib/apt/lists/* && \
    case "$TARGETARCH" in amd64) T=x86_64-unknown-linux-gnu ;; arm64) T=aarch64-unknown-linux-gnu ;; *) echo "unsupported TARGETARCH=$TARGETARCH" >&2; exit 1 ;; esac && \
    eval "$(DUCKDB_VERSION="$DUCKDB_VERSION" DUCKDB_STATIC_CACHE=/opt/duckdb-cache /usr/local/bin/prepare-duckdb-static "$T")" && \
    mkdir -p /opt/duckdb && cp "$DUCKDB_LIB_DIR/libduckdb_static.a" "$DUCKDB_INCLUDE_DIR/duckdb.h" /opt/duckdb/

FROM chef AS planner
COPY Cargo.toml Cargo.lock build.rs ./
COPY src/ src/
COPY benches/ benches/
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
ARG TARGETARCH
COPY --from=duckdb-static /opt/duckdb /opt/duckdb
ENV DUCKDB_LIB_DIR=/opt/duckdb DUCKDB_INCLUDE_DIR=/opt/duckdb DUCKDB_STATIC=1 \
    CARGO_NET_GIT_FETCH_WITH_CLI=true
COPY --from=planner /build/recipe.json recipe.json
# Native cross-compile: rustc runs on $BUILDPLATFORM and emits target-arch code,
# linked by the matching gcc-<arch>-linux-gnu (correct libstdc++ ABI for the
# gcc-built DuckDB static). Cook deps, then build the crate.
#
# `mold -run` wraps the build so mold (perf-neutral, only faster linking — no
# runtime cost) intercepts the linker exec via LD_PRELOAD instead of `-fuse-ld`.
# The cross gcc can't resolve `-fuse-ld=mold` (collect2 can't find ld.mold), but
# -run transparently redirects its `<arch>-linux-gnu-ld` call to mold, which
# cross-links target-arch objects fine. Linking the huge merged DuckDB static lib
# is the build's bottleneck, so this is what keeps `just image` under 3 minutes.
#
# Per-arch cache id: amd64 + arm64 build concurrently, so a SHARED cargo registry
# cache races on .cargo-ok during crate extraction ("File exists (os error 17)",
# rust-lang/cargo#1700) — latent until a big new dep subtree (augurs →
# nalgebra/argmin/rv) gave both arches many crates to extract at once. Separate
# ids per arch keep the builds parallel with zero contention.
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-$TARGETARCH \
    set -eux; \
    case "$TARGETARCH" in \
      amd64) T=x86_64-unknown-linux-gnu; P=x86_64-linux-gnu ;; \
      arm64) T=aarch64-unknown-linux-gnu; P=aarch64-linux-gnu ;; \
      *) echo "unsupported TARGETARCH=$TARGETARCH" >&2; exit 1 ;; \
    esac; \
    V=$(echo "$T" | tr 'a-z-' 'A-Z_'); Tu=$(echo "$T" | tr '-' '_'); \
    export CARGO_TARGET_${V}_LINKER="${P}-gcc" CC_${Tu}="${P}-gcc" CXX_${Tu}="${P}-g++" AR_${Tu}="${P}-ar"; \
    mold -run cargo chef cook --release --target "$T" --recipe-path recipe.json
COPY Cargo.toml Cargo.lock build.rs ./
COPY src/ src/
COPY benches/ benches/
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-$TARGETARCH \
    set -eux; \
    case "$TARGETARCH" in \
      amd64) T=x86_64-unknown-linux-gnu; P=x86_64-linux-gnu ;; \
      arm64) T=aarch64-unknown-linux-gnu; P=aarch64-linux-gnu ;; \
    esac; \
    V=$(echo "$T" | tr 'a-z-' 'A-Z_'); Tu=$(echo "$T" | tr '-' '_'); \
    export CARGO_TARGET_${V}_LINKER="${P}-gcc" CC_${Tu}="${P}-gcc" CXX_${Tu}="${P}-g++" AR_${Tu}="${P}-ar"; \
    mold -run cargo build --release --target "$T" --bin sigiro && \
    cp "target/${T}/release/sigiro" /build/sigiro

# The Linux binaries we publish for download, uncompressed.
#
# Same build as the image — same pinned DuckDB static archive, same cross-gcc,
# same mold link — so a downloaded binary and the one in the container are the
# same artifact.
#
# `scratch`, so `--output type=local` writes exactly one file per platform and
# nothing else.
FROM scratch AS dist
COPY --from=builder /build/sigiro /sigiro

# Runtime-prep (has a shell): bake the loadable extensions that aren't static in
# the binary. Runs the TARGET binary, so pinned to $TARGETPLATFORM.
FROM --platform=$TARGETPLATFORM debian:bookworm-slim AS runtime-prep
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/sigiro /usr/local/bin/sigiro
ENV SIGIRO_EXTENSION_DIR=/opt/sigiro/extensions
RUN /usr/local/bin/sigiro fetch-extensions
# An empty data dir to COPY --chown into distroless, which has no shell to mkdir
# with. Without this, `docker run <image>` dies on "failed to create data
# directory /var/lib/sigiro: Permission denied" because nonroot can't write /var/lib.
RUN mkdir -p /var/lib/sigiro

# Final image: distroless/cc (glibc 2.36 + libstdc++ + ca-certificates), no shell,
# runs as the built-in nonroot user.
FROM gcr.io/distroless/cc-debian12
COPY --from=builder /build/sigiro /usr/local/bin/sigiro
COPY --from=runtime-prep /opt/sigiro/extensions /opt/sigiro/extensions
COPY --from=runtime-prep --chown=nonroot:nonroot /var/lib/sigiro /var/lib/sigiro
# Keep the extension dir OUTSIDE the data dir: a hosted volume mounts over
# SIGIRO_DATA_DIR and would mask baked extensions, forcing a cold re-download.
ENV SIGIRO_DATA_DIR=/var/lib/sigiro \
    SIGIRO_EXTENSION_DIR=/opt/sigiro/extensions
USER nonroot
EXPOSE 4317 4318 9999
# Shell-free healthcheck: distroless has no curl/sh, so the usual
# `curl -f /health` can't run. `sigiro healthcheck` exits 0 only when Ready.
# Deploy configs (e.g. kamal accessory health-cmd) must use this exec form too,
# not a curl string, or the container is wrongly reported unhealthy.
HEALTHCHECK --interval=10s --timeout=5s --start-period=120s --retries=6 \
  CMD ["/usr/local/bin/sigiro", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/sigiro"]
CMD ["serve"]
