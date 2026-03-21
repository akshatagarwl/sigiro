# syntax=docker/dockerfile:1

# Stage 1: cargo-chef planner — extracts dependency recipe
FROM --platform=$BUILDPLATFORM lukemathwalker/cargo-chef:latest-rust-slim AS chef
WORKDIR /build

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
COPY benches/ benches/
RUN cargo chef prepare --recipe-path recipe.json

# Stage 2: cook dependencies (cached until Cargo.lock changes)
# Cache mounts keyed by TARGETARCH to avoid mixing amd64/arm64 artifacts.
# DuckDB bundled C++ compile is the bottleneck (~3-5 min); cache mounts
# persist it across layer invalidations so it only compiles once per arch.
FROM lukemathwalker/cargo-chef:latest-rust-slim AS deps
WORKDIR /build
ARG TARGETARCH
RUN apt-get update && apt-get install -y pkg-config libssl-dev g++ && rm -rf /var/lib/apt/lists/*
COPY --from=planner /build/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-${TARGETARCH} \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git-${TARGETARCH} \
    --mount=type=cache,target=/build/target,id=cargo-target-${TARGETARCH} \
    cargo chef cook --release --recipe-path recipe.json

# Stage 3: build the sigiro binary only (no benches, no tests)
FROM deps AS builder
ARG TARGETARCH
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
COPY benches/ benches/
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-${TARGETARCH} \
    --mount=type=cache,target=/usr/local/cargo/git,id=cargo-git-${TARGETARCH} \
    --mount=type=cache,target=/build/target,id=cargo-target-${TARGETARCH} \
    cargo build --release --bin sigiro && \
    cp /build/target/release/sigiro /build/sigiro

# Stage 4: minimal runtime
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y ca-certificates curl && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/sigiro /usr/local/bin/sigiro
EXPOSE 4317 4318 9999
ENV SIGIRO_DATA_DIR=/var/lib/sigiro
CMD ["sigiro", "serve"]
