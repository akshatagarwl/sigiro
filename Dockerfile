# syntax=docker/dockerfile:1

# Stage 1: cargo-chef planner — extracts dependency recipe
FROM rust:slim AS chef
RUN cargo install cargo-chef
WORKDIR /build

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
RUN cargo chef prepare --recipe-path recipe.json

# Stage 2: cook dependencies (cached until Cargo.lock changes)
FROM chef AS deps
RUN apt-get update && apt-get install -y pkg-config libssl-dev g++ && rm -rf /var/lib/apt/lists/*
COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Stage 3: build sigiro (only recompiles src/ changes)
FROM deps AS builder
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
RUN cargo build --release

# Stage 4: minimal runtime
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates curl && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/sigiro /usr/local/bin/sigiro
EXPOSE 4317 4318 9999
ENV SIGIRO_DATA_DIR=/var/lib/sigiro
CMD ["sigiro", "serve"]
