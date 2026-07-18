# syntax=docker/dockerfile:1

FROM --platform=$BUILDPLATFORM lukemathwalker/cargo-chef:latest-rust-slim AS chef
WORKDIR /build

# libduckdb-sys 1.10503.1 is generated for DuckDB v1.5.3. Build that exact
# release's official amalgamation into one static core archive. This stage is
# independent of recipe.json and application sources, so its layer is keyed
# only by the pinned DuckDB version/toolchain and is reusable from the exported
# registry cache. Runtime extensions remain separate and are fetched below.
FROM debian:trixie-slim AS duckdb-static
ARG DUCKDB_VERSION=1.5.3
ARG TARGETARCH
# ccache (persisted via a BuildKit cache mount) makes the multi-minute DuckDB
# amalgamation compile a cache hit whenever this layer is invalidated but the
# mount survives — so re-timing/iterating on the image doesn't pay the full
# clang++ -O3 on duckdb.cpp again. Produces a byte-identical archive.
RUN --mount=type=cache,target=/root/.ccache,id=ccache-duckdb-${TARGETARCH} \
    apt-get update && apt-get install -y ca-certificates clang curl unzip binutils ccache && rm -rf /var/lib/apt/lists/* && \
    mkdir -p /opt/duckdb /tmp/duckdb && \
    curl --fail --location --retry 3 \
      "https://github.com/duckdb/duckdb/releases/download/v${DUCKDB_VERSION}/libduckdb-src.zip" \
      --output /tmp/libduckdb-src.zip && \
    unzip -q /tmp/libduckdb-src.zip -d /tmp/duckdb && \
    test -f /tmp/duckdb/duckdb.cpp && \
    test -f /tmp/duckdb/duckdb.h && \
    ccache clang++ -std=c++11 -O3 -DNDEBUG -fPIC \
      -DDUCKDB_EXTENSION_AUTOINSTALL_DEFAULT=1 \
      -DDUCKDB_EXTENSION_AUTOLOAD_DEFAULT=1 \
      -c /tmp/duckdb/duckdb.cpp -o /tmp/duckdb/duckdb.o && \
    ar rcs /opt/duckdb/libduckdb_static.a /tmp/duckdb/duckdb.o && \
    cp /tmp/duckdb/duckdb.h /opt/duckdb/duckdb.h && \
    rm -rf /tmp/duckdb /tmp/libduckdb-src.zip

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY build.rs ./
COPY src/ src/
COPY benches/ benches/
RUN cargo chef prepare --recipe-path recipe.json

FROM lukemathwalker/cargo-chef:latest-rust-slim AS deps
WORKDIR /build
ARG TARGETARCH
RUN apt-get update && apt-get install -y pkg-config libssl-dev g++ git curl clang lld protobuf-compiler ccache && rm -rf /var/lib/apt/lists/*
COPY --from=duckdb-static /opt/duckdb /opt/duckdb
COPY --from=planner /build/recipe.json recipe.json
# libduckdb-sys' non-bundled path honors these variables and requests
# static=duckdb_static. Keep the cooked Rust dependency graph in an image layer
# too: registry cache exports layers, not BuildKit cache-mount contents.
ENV DUCKDB_LIB_DIR=/opt/duckdb \
    DUCKDB_INCLUDE_DIR=/opt/duckdb \
    DUCKDB_STATIC=1
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-${TARGETARCH} \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git-shared,sharing=shared \
    --mount=type=cache,target=/root/.ccache,id=ccache-${TARGETARCH} \
    CC="ccache clang" CXX="ccache clang++" \
    CARGO_NET_GIT_FETCH_WITH_CLI=true \
    cargo chef cook --release --recipe-path recipe.json

FROM deps AS builder
ARG TARGETARCH
COPY Cargo.toml Cargo.lock ./
COPY build.rs ./
COPY src/ src/
COPY benches/ benches/
# `builder` is `FROM deps`, so /build/target already holds the cooked dep
# artifacts as an inherited layer. No /build/target cache mount here either:
# cargo reuses the cooked deps from the layer and only (re)compiles the sigiro
# crate + relinks — the fast, src-only path. A target cache mount would shadow
# the inherited layer with an empty dir and defeat the deps-layer caching above.
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-${TARGETARCH} \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git-shared,sharing=shared \
    --mount=type=cache,target=/root/.ccache,id=ccache-${TARGETARCH} \
    CC="ccache clang" CXX="ccache clang++" \
    CARGO_NET_GIT_FETCH_WITH_CLI=true \
    cargo build --release --bin sigiro && \
    cp /build/target/release/sigiro /build/sigiro

FROM debian:trixie-slim
ARG TARGETARCH
RUN apt-get update && apt-get install -y ca-certificates curl && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/sigiro /usr/local/bin/sigiro
# Pre-download DuckDB extensions so the first `sigiro serve` doesn't hang on a
# 30-90s blocking download. CRITICAL: keep the extension dir OUTSIDE the data dir.
# In hosted deploys a persistent volume mounts over SIGIRO_DATA_DIR (/var/lib/sigiro),
# which would MASK anything baked under it and force a re-download on every cold
# start. /opt/sigiro/extensions is baked into the image and never masked. The Fly
# deploy sets the same SIGIRO_EXTENSION_DIR so serve reads these baked extensions.
ENV SIGIRO_DATA_DIR=/var/lib/sigiro \
    SIGIRO_EXTENSION_DIR=/opt/sigiro/extensions
# Fetch extensions into a PERSISTED cache mount first, then bake them into the
# image layer. The sigiro binary layer changes every build, so this step reran
# and re-downloaded ~8 extensions each time; DuckDB's `INSTALL` is a no-op when
# an extension is already present, so a per-arch cache mount turns the repeat
# downloads into a fast copy. The mount is never part of the image — only the
# copied /opt/sigiro/extensions dir is, so the final image is unchanged.
RUN --mount=type=cache,target=/opt/ext-cache,id=sigiro-ext-${TARGETARCH} \
    SIGIRO_EXTENSION_DIR=/opt/ext-cache sigiro fetch-extensions && \
    mkdir -p /opt/sigiro/extensions && \
    cp -a /opt/ext-cache/. /opt/sigiro/extensions/
RUN groupadd --system --gid 10001 sigiro && \
    useradd --system --uid 10001 --gid 10001 --no-create-home --home-dir /nonexistent \
      --shell /usr/sbin/nologin sigiro
USER 10001:10001
EXPOSE 4317 4318 9999
CMD ["sigiro", "serve"]
