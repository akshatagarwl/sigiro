# syntax=docker/dockerfile:1

# Stage 1: cargo-chef planner -- extracts dependency recipe
FROM --platform=$BUILDPLATFORM lukemathwalker/cargo-chef:latest-rust-slim AS chef
WORKDIR /build

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
COPY benches/ benches/
RUN cargo chef prepare --recipe-path recipe.json

# Stage 2: cook dependencies (cached until Cargo.lock changes)
FROM lukemathwalker/cargo-chef:latest-rust-slim AS deps
WORKDIR /build
ARG TARGETARCH
RUN apt-get update && apt-get install -y pkg-config libssl-dev g++ git curl clang lld protobuf-compiler && rm -rf /var/lib/apt/lists/*
COPY --from=planner /build/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-${TARGETARCH} \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git-shared,sharing=shared \
    --mount=type=cache,target=/build/target,id=cargo-target-${TARGETARCH} \
    CARGO_NET_GIT_FETCH_WITH_CLI=true \
    cargo chef cook --release --recipe-path recipe.json

# Stage 3: build the sigiro binary only
FROM deps AS builder
ARG TARGETARCH
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
COPY benches/ benches/
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-${TARGETARCH} \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git-shared,sharing=shared \
    --mount=type=cache,target=/build/target,id=cargo-target-${TARGETARCH} \
    CARGO_NET_GIT_FETCH_WITH_CLI=true \
    cargo build --release --bin sigiro && \
    cp /build/target/release/sigiro /build/sigiro

# Stage 4: build spiced from submodule (duckdb connector for federation central)
FROM deps AS spiced-builder
ARG TARGETARCH
RUN apt-get update && apt-get install -y make cmake clang lld protobuf-compiler && rm -rf /var/lib/apt/lists/*
COPY scripts/build-spiced.sh ./scripts/build-spiced.sh
COPY third_party/spiceai/ ./third_party/spiceai/
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-${TARGETARCH} \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git-shared,sharing=shared \
    --mount=type=cache,target=/build/target,id=cargo-target-${TARGETARCH} \
    --mount=type=cache,target=/build/third_party/spiceai/target,id=spice-target-shared,sharing=shared \
    CARGO_NET_GIT_FETCH_WITH_CLI=true \
    chmod +x scripts/build-spiced.sh && \
    SPICED_INSTALL=/build SPICED_FEATURES=duckdb,alloc-snmalloc scripts/build-spiced.sh

# Stage 5: minimal runtime
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y ca-certificates curl && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/sigiro /usr/local/bin/sigiro
COPY --from=spiced-builder /build/spiced /usr/local/bin/spiced
EXPOSE 4317 4318 9999
ENV SIGIRO_DATA_DIR=/var/lib/sigiro
CMD ["sigiro", "serve"]
