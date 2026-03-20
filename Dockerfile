# syntax=docker/dockerfile:1
FROM rust:slim AS builder
RUN apt-get update && apt-get install -y pkg-config libssl-dev g++ && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
RUN --mount=type=cache,target=/build/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo build --release && cp target/release/sigiro /sigiro

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates curl && rm -rf /var/lib/apt/lists/*
COPY --from=builder /sigiro /usr/local/bin/sigiro
EXPOSE 4317 4318 9999
ENV SIGIRO_DATA_DIR=/var/lib/sigiro
CMD ["sigiro", "serve"]
