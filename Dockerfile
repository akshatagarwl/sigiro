# syntax=docker/dockerfile:1

FROM --platform=$BUILDPLATFORM lukemathwalker/cargo-chef:latest-rust-slim AS chef
WORKDIR /build

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
COPY benches/ benches/
RUN cargo chef prepare --recipe-path recipe.json

FROM lukemathwalker/cargo-chef:latest-rust-slim AS deps
WORKDIR /build
ARG TARGETARCH
RUN apt-get update && apt-get install -y pkg-config libssl-dev g++ git curl clang lld protobuf-compiler ccache && rm -rf /var/lib/apt/lists/*
COPY --from=planner /build/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-${TARGETARCH} \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git-shared,sharing=shared \
    --mount=type=cache,target=/build/target,id=cargo-target-${TARGETARCH} \
    --mount=type=cache,target=/root/.ccache,id=ccache-${TARGETARCH} \
    CC="ccache clang" CXX="ccache clang++" \
    CARGO_NET_GIT_FETCH_WITH_CLI=true \
    cargo chef cook --release --recipe-path recipe.json

FROM deps AS builder
ARG TARGETARCH
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
COPY benches/ benches/
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-${TARGETARCH} \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git-shared,sharing=shared \
    --mount=type=cache,target=/build/target,id=cargo-target-${TARGETARCH} \
    --mount=type=cache,target=/root/.ccache,id=ccache-${TARGETARCH} \
    CC="ccache clang" CXX="ccache clang++" \
    CARGO_NET_GIT_FETCH_WITH_CLI=true \
    cargo build --release --bin sigiro && \
    cp /build/target/release/sigiro /build/sigiro

FROM debian:trixie-slim
RUN apt-get update && apt-get install -y ca-certificates curl && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/sigiro /usr/local/bin/sigiro
# Pre-download DuckDB extensions so the first `sigiro serve` doesn't hang
# on a 30-90s blocking download. Both the pre-warm CLI and `sigiro serve`
# use SIGIRO_EXTENSION_DIR (default: $SIGIRO_DATA_DIR/extensions) — set it
# explicitly here so pre-warm and serve write to the same location.
ENV SIGIRO_DATA_DIR=/var/lib/sigiro \
    SIGIRO_EXTENSION_DIR=/var/lib/sigiro/extensions
RUN sigiro fetch-extensions
EXPOSE 4317 4318 9999
CMD ["sigiro", "serve"]
