.PHONY: collector test e2e

collector:
	go install go.opentelemetry.io/collector/cmd/builder@v0.146.1
	builder --config=otelcol-builder.yaml

test:
	go test ./...

e2e:
	DOCKER_BUILDKIT=1 bash test/e2e/run.sh
