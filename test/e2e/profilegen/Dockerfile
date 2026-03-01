# syntax=docker/dockerfile:1
FROM golang:1.25 AS builder
WORKDIR /src
COPY go.mod go.sum ./
RUN --mount=type=cache,target=/go/pkg/mod go mod download
COPY main.go .
RUN --mount=type=cache,target=/go/pkg/mod \
    --mount=type=cache,target=/root/.cache/go-build \
    CGO_ENABLED=0 go build -trimpath -o /profilegen .

FROM gcr.io/distroless/static-debian12
COPY --from=builder /profilegen /profilegen
ENTRYPOINT ["/profilegen"]
