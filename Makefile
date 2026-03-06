.PHONY: collector test e2e e2e-real sdk-validate

collector:
	go install go.opentelemetry.io/collector/cmd/builder@v0.147.0
	builder --config=otelcol-builder.yaml

test:
	go test ./...

e2e:
	DOCKER_BUILDKIT=1 bash test/e2e/run.sh

e2e-real:
	DOCKER_BUILDKIT=1 bash test/e2e/run-real.sh

sdk-validate:
	DOCKER_BUILDKIT=1 bash test/sdk-validation/run.sh
