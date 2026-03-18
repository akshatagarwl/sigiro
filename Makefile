.PHONY: sigiro sigiro-all agents.txt collector test e2e e2e-real sdk-validate clean

# ── sigiro CLI ──────────────────────────────────────────────

# Native build with zig cc (drops libc++ dependency)
sigiro:
	CGO_ENABLED=1 CC="zig cc" CXX="zig c++" \
		go build -trimpath -o dist/sigiro ./cmd/sigiro/

# All supported platforms (native only — cross-compilation needs CI runners
# because DuckDB's pre-compiled static libs are glibc-bound)
sigiro-all:
	$(MAKE) sigiro
	@echo "dist/sigiro built for $$(go env GOOS)/$$(go env GOARCH)"
	@ls -lh dist/sigiro

# Generate agents.txt from schema registry
agents.txt: sigiro
	dist/sigiro schema > agents.txt

# ── collector ───────────────────────────────────────────────

collector:
	go install go.opentelemetry.io/collector/cmd/builder@v0.147.0
	builder --config=otelcol-builder.yaml

# ── tests ───────────────────────────────────────────────────

test:
	go test ./...

e2e:
	DOCKER_BUILDKIT=1 bash test/e2e/run.sh

e2e-real:
	DOCKER_BUILDKIT=1 bash test/e2e/run-real.sh

sdk-validate:
	DOCKER_BUILDKIT=1 bash test/sdk-validation/run.sh

# ── housekeeping ────────────────────────────────────────────

clean:
	rm -rf dist/
