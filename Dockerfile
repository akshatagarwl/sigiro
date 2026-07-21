# syntax=docker/dockerfile:1

# Toolchain image, bookworm/glibc 2.36 to match the distroless runtime. Pinned to
# $BUILDPLATFORM so rustc runs NATIVELY and cross-links to the target arch (no
# QEMU). Carries both GNU cross toolchains + cargo-chef for dep-layer caching.
FROM --platform=$BUILDPLATFORM rust:1-slim-bookworm AS chef
RUN apt-get update && apt-get install -y --no-install-recommends \
      pkg-config protobuf-compiler git curl unzip xz-utils binutils libssl-dev mold \
      gcc-x86-64-linux-gnu g++-x86-64-linux-gnu \
      gcc-aarch64-linux-gnu g++-aarch64-linux-gnu \
    && rm -rf /var/lib/apt/lists/*
# UPX (from GitHub release; not in this base's apt). Runs on $BUILDPLATFORM and
# can pack cross-arch ELF, so it compresses either target binary.
ARG UPX_VERSION=4.2.4
RUN curl -fsSL "https://github.com/upx/upx/releases/download/v${UPX_VERSION}/upx-${UPX_VERSION}-$(dpkg --print-architecture)_linux.tar.xz" \
      | tar -xJ -C /tmp && cp /tmp/upx-${UPX_VERSION}-*/upx /usr/local/bin/upx && rm -rf /tmp/upx-*
RUN cargo install cargo-chef --locked && \
    rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu
WORKDIR /build

# Prebuilt DuckDB static libs for the TARGET arch: download + ar-merge, NO
# compile. Runs on $BUILDPLATFORM (ar/unzip are arch-agnostic). All fragments
# are merged: the bundle's generated_extension_loader references every extension,
# so it's link-all-or-nothing.
FROM --platform=$BUILDPLATFORM debian:bookworm-slim AS duckdb-static
ARG DUCKDB_VERSION=1.5.3
ARG TARGETARCH
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl unzip binutils && rm -rf /var/lib/apt/lists/* && \
    mkdir -p /opt/duckdb /tmp/d && cd /tmp/d && \
    curl -fSL --retry 3 "https://github.com/duckdb/duckdb/releases/download/v${DUCKDB_VERSION}/static-libs-linux-${TARGETARCH}.zip" -o s.zip && \
    unzip -q s.zip && test -f libduckdb_static.a && test -f duckdb.h && \
    { echo "create m.a"; for a in *.a; do echo "addlib $a"; done; echo save; echo end; } | ar -M && \
    ranlib m.a && cp m.a /opt/duckdb/libduckdb_static.a && cp duckdb.h /opt/duckdb/ && cd / && rm -rf /tmp/d

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
    cp "target/${T}/release/sigiro" /build/sigiro && \
    upx -6 /build/sigiro

# Runtime-prep (has a shell): bake the loadable extensions that aren't static in
# the binary. Runs the TARGET binary, so pinned to $TARGETPLATFORM.
FROM --platform=$TARGETPLATFORM debian:bookworm-slim AS runtime-prep
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/sigiro /usr/local/bin/sigiro
ENV SIGIRO_EXTENSION_DIR=/opt/sigiro/extensions
RUN /usr/local/bin/sigiro fetch-extensions

# Final image: distroless/cc (glibc 2.36 + libstdc++ + ca-certificates), no shell,
# runs as the built-in nonroot user.
FROM gcr.io/distroless/cc-debian12
COPY --from=builder /build/sigiro /usr/local/bin/sigiro
COPY --from=runtime-prep /opt/sigiro/extensions /opt/sigiro/extensions
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
