# Stage 1: Build the collector binary using OCB
FROM golang:1.25 AS builder

WORKDIR /src

# Install OCB
RUN go install go.opentelemetry.io/collector/cmd/builder@v0.146.1

# Copy entire repo (OCB needs root module for replaces directive)
COPY . .

# Generate collector code and compile
RUN builder --config=otelcol-builder.yaml
RUN cd collector/cmd/collector && go build -trimpath -o /observe-collector .

# Stage 2: Minimal runtime image
FROM gcr.io/distroless/static-debian12

COPY --from=builder /observe-collector /observe-collector
COPY --from=builder /src/collector/otelcol.yaml /etc/otelcol/otelcol.yaml

EXPOSE 4317 4318

ENTRYPOINT ["/observe-collector"]
CMD ["--config", "/etc/otelcol/otelcol.yaml"]
