# syntax=docker/dockerfile:1

# Toolchain image, bookworm/glibc 2.36 to match the distroless runtime. Pinned to
# $BUILDPLATFORM so rustc runs NATIVELY and cross-links to the target arch (no
# QEMU). Carries both GNU cross toolchains + cargo-chef for dep-layer caching.
FROM --platform=$BUILDPLATFORM rust:1.99.1-slim-bookworm@sha256:24707e70a4f1e23d9a7b25202ec43126d28a52294b96aad5c670df1fd1f1d400 AS chef
ARG TARGETARCH
RUN apt-get update && apt-get install -y --no-install-recommends \
      pkg-config protobuf-compiler git binutils libssl-dev make mold \
      gcc-x86-64-linux-gnu g++-x86-64-linux-gnu \
      gcc-aarch64-linux-gnu g++-aarch64-linux-gnu \
    && dpkg --add-architecture "$TARGETARCH" \
    && apt-get update \
    && apt-get install -y --no-install-recommends "libssl-dev:$TARGETARCH" \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef --locked && \
    rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu
WORKDIR /build

# Prebuilt DuckDB static libs for the TARGET arch: download + ar-merge, NO
# compile. All build surfaces use the same pinned preparation script.
FROM --platform=$BUILDPLATFORM debian:bookworm-slim AS duckdb-static
ARG TARGETARCH
COPY scripts/prepare-duckdb-static.sh /usr/local/bin/prepare-duckdb-static
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl binutils && rm -rf /var/lib/apt/lists/* && \
    case "$TARGETARCH" in amd64) T=x86_64-unknown-linux-gnu ;; arm64) T=aarch64-unknown-linux-gnu ;; *) echo "unsupported TARGETARCH=$TARGETARCH" >&2; exit 1 ;; esac && \
    eval "$(DUCKDB_STATIC_CACHE=/opt/duckdb-cache /usr/local/bin/prepare-duckdb-static "$T")" && \
    mkdir -p /opt/duckdb && cp "$DUCKDB_LIB_DIR/libduckdb_static.a" "$DUCKDB_INCLUDE_DIR/duckdb.h" /opt/duckdb/

# Node and aube for the two static exports the binary embeds, the dashboard
# (src/dashboard.rs) and the docs (src/docs.rs). Platform-independent output,
# so both build once on $BUILDPLATFORM. Aube is the version and checksum pinned
# in mise.lock.
FROM --platform=$BUILDPLATFORM node:24.20.0-bookworm-slim@sha256:ba849c60be29959425b8734d57b8b4b7d56f98edd9504c9af091d5281095a71e AS node-aube
ARG BUILDARCH
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/* && \
    set -eux; \
    case "$BUILDARCH" in \
      amd64) A=x86_64; SUM=7cdc99ea7a4ceced9ee174c2f2406abf6ed252b192b222a8a5ecb386b2dc2fe1 ;; \
      arm64) A=aarch64; SUM=7cf94b73080d6604bf93e3e8d129c35264a851572a09a2b1236af75f592d87b5 ;; \
      *) echo "unsupported BUILDARCH=$BUILDARCH" >&2; exit 1 ;; \
    esac; \
    curl -fsSL -o /tmp/aube.tgz "https://github.com/aubepkg/aube/releases/download/v2.8.4/aube-v2.8.4-${A}-unknown-linux-gnu.tar.gz"; \
    echo "${SUM}  /tmp/aube.tgz" | sha256sum -c -; \
    tar -xzf /tmp/aube.tgz -C /usr/local/bin aube; \
    rm /tmp/aube.tgz

FROM node-aube AS dashboard
WORKDIR /build/dashboard
COPY dashboard/package.json dashboard/aube-lock.yaml ./
RUN aube install --frozen-lockfile
COPY dashboard/ ./
RUN aube run build && test -f dist/dashboard/index.html

# The docs (sigiro.com/docs) as a static export. The build reads the API
# reference from https://sigiro.com/openapi.json, as the sigiro.com build does.
FROM node-aube AS docs
WORKDIR /build
COPY www/package.json www/aube-lock.yaml www/
COPY www/patches/ www/patches/
RUN aube -C www install --frozen-lockfile
COPY www/ www/
COPY scripts/build-docs-export.sh scripts/
RUN scripts/build-docs-export.sh && test -f www/dist-binary/index.html

FROM chef AS planner
COPY Cargo.toml Cargo.lock build.rs ./
COPY src/ src/
COPY crates/ crates/
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
    export CARGO_TARGET_${V}_LINKER="${P}-gcc" CC_${Tu}="${P}-gcc" CXX_${Tu}="${P}-g++" AR_${Tu}="${P}-ar" \
      PKG_CONFIG_ALLOW_CROSS=1 PKG_CONFIG_SYSROOT_DIR=/ \
      PKG_CONFIG_PATH="/usr/lib/${P}/pkgconfig:/usr/share/pkgconfig" \
      OPENSSL_LIB_DIR="/usr/lib/${P}" OPENSSL_INCLUDE_DIR="/usr/include/${P}"; \
    mold -run cargo chef cook --release --target "$T" --recipe-path recipe.json
COPY Cargo.toml Cargo.lock build.rs ./
COPY src/ src/
COPY crates/ crates/
COPY --from=dashboard /build/dashboard/dist/dashboard dashboard/dist/dashboard
COPY --from=docs /build/www/dist-binary www/dist-binary
ENV SIGIRO_REQUIRE_DASHBOARD=1 SIGIRO_REQUIRE_DOCS=1
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-$TARGETARCH \
    set -eux; \
    case "$TARGETARCH" in \
      amd64) T=x86_64-unknown-linux-gnu; P=x86_64-linux-gnu ;; \
      arm64) T=aarch64-unknown-linux-gnu; P=aarch64-linux-gnu ;; \
    esac; \
    V=$(echo "$T" | tr 'a-z-' 'A-Z_'); Tu=$(echo "$T" | tr '-' '_'); \
    export CARGO_TARGET_${V}_LINKER="${P}-gcc" CC_${Tu}="${P}-gcc" CXX_${Tu}="${P}-g++" AR_${Tu}="${P}-ar" \
      PKG_CONFIG_ALLOW_CROSS=1 PKG_CONFIG_SYSROOT_DIR=/ \
      PKG_CONFIG_PATH="/usr/lib/${P}/pkgconfig:/usr/share/pkgconfig" \
      OPENSSL_LIB_DIR="/usr/lib/${P}" OPENSSL_INCLUDE_DIR="/usr/include/${P}"; \
    mold -run cargo build --release --target "$T" --bin sigiro && \
    cp "target/${T}/release/sigiro" /build/sigiro && \
    mkdir -p /build/data-dir

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

# Final image: distroless/cc (glibc 2.36 + libstdc++ + ca-certificates), no shell,
# runs as the built-in nonroot user.
FROM gcr.io/distroless/cc-debian12
COPY --from=builder /build/sigiro /usr/local/bin/sigiro
COPY --from=builder --chown=nonroot:nonroot /build/data-dir /var/lib/sigiro
ENV SIGIRO_DATA_DIR=/var/lib/sigiro
USER nonroot
WORKDIR /var/lib/sigiro
EXPOSE 4317 4318 9999
# Shell-free healthcheck: distroless has no curl/sh, so the usual
# `curl -f /health` can't run. `sigiro healthcheck` exits 0 only when Ready.
# Deployment health checks must use this exec form too, not a curl string, or
# the container is wrongly reported unhealthy.
HEALTHCHECK --interval=10s --timeout=5s --start-period=120s --retries=6 \
  CMD ["/usr/local/bin/sigiro", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/sigiro"]
CMD ["serve"]
