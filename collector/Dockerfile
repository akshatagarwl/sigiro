# syntax=docker/dockerfile:1
FROM golang:1.25 AS builder
WORKDIR /src

# Install OCB (cached across builds)
RUN --mount=type=cache,target=/go/pkg/mod \
    --mount=type=cache,target=/root/.cache/go-build \
    go install go.opentelemetry.io/collector/cmd/builder@v0.147.0

# TODO: copy go.mod/go.sum before COPY . . for dep-only layer caching.
# Currently limited because OCB generates its own go.mod in collector/cmd/collector/,
# so the real dep download happens during `builder` and `go build`, not here.
# The cache mounts below are the primary win.
COPY . .

# Generate collector code and compile
# OCB output_path is ./collector/cmd/collector — parent must exist.
RUN --mount=type=cache,target=/go/pkg/mod \
    --mount=type=cache,target=/root/.cache/go-build \
    mkdir -p collector/cmd/collector && builder --config=otelcol-builder.yaml

RUN --mount=type=cache,target=/go/pkg/mod \
    --mount=type=cache,target=/root/.cache/go-build \
    cd collector/cmd/collector && CGO_ENABLED=0 go build -trimpath -o /observe-collector .

# Stage 2: Minimal runtime image
FROM gcr.io/distroless/static-debian12
COPY --from=builder /observe-collector /observe-collector
COPY --from=builder /src/collector/otelcol.yaml /etc/otelcol/otelcol.yaml
VOLUME /var/lib/observe/queue
EXPOSE 4317 4318 4319
ENTRYPOINT ["/observe-collector"]
CMD ["--config", "/etc/otelcol/otelcol.yaml"]
