.PHONY: collector test

collector:
	go install go.opentelemetry.io/collector/cmd/builder@v0.146.1
	builder --config=otelcol-builder.yaml

test:
	go test ./...
