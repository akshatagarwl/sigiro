# syntax=docker/dockerfile:1
FROM golang:1.25-bookworm AS builder
# TODO: Build with Zig (musl) for fully static, zero-dependency binaries to
# avoid runtime glibc mismatches across base images/hosts.
WORKDIR /src
COPY go.mod go.sum ./
RUN --mount=type=cache,target=/go/pkg/mod go mod download
COPY . .
RUN --mount=type=cache,target=/go/pkg/mod \
    --mount=type=cache,target=/root/.cache/go-build \
    go build -trimpath -o /query-server ./cmd/query-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /query-server /query-server
EXPOSE 8080
ENTRYPOINT ["/query-server"]
