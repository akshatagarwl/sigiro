# Sigiro

Observability platform with DuckDB-based storage, OTLP ingestion, and federated queries.

## Quick Start

Download the latest binary for your platform from the releases page, then run:

```bash
sigiro serve
```

The server starts on:
- HTTP API: `http://localhost:9999`
- OTLP gRPC: `localhost:4317`
- OTLP HTTP: `localhost:4318`

No configuration required. Data is stored in the platform data directory (`~/.local/share/sigiro` on Unix, `%LOCALAPPDATA%\sigiro` on Windows). Override with `SIGIRO_DATA_DIR`. The [local data layout](docs/notes/architecture/ducklake-integration.md#local-mode) identifies the DuckLake catalog, Parquet files, and separate administrative database.

## Authentication

Sigiro has two server authentication modes:

* **Open** (default): self-hosted servers and edge/query-surface nodes run with
  no per-request authentication. Secure the network perimeter with a tailnet,
  private network, or reverse proxy.
* **TenantKeys** (hosted): set `SIGIRO_CATALOG_URL` to use the hosted control
  plane. Requests authenticate with per-tenant bearer keys minted and managed
  through that control plane.

`SIGIRO_API_KEY` is **client-only** configuration for calling a hosted endpoint:

```bash
SIGIRO_API_KEY=<tenant-bearer-key> sigiro status
# or: sigiro --key <tenant-bearer-key> status
```

## End-to-End Test

Verify the full stack (OpenTelemetry Demo + Sigiro) with one command:

```bash
cd tests/e2e/k8s
./run.sh
```

This starts the complete demo environment and verifies telemetry flows from collectors through to queries.

## Output

Every command prints pretty JSON. One format, so a human and a pipeline read the
same bytes:

```bash
sigiro status
sigiro anomalies | jq '.anomalies[] | select(.signal == "latency_p95")'
sigiro query "SELECT count(*) FROM sigiro_spans" > report.json
```

## Documentation

Full documentation is available at [docs.sigiro.com](https://docs.sigiro.com) (or in `docs-site/docs/`).

## Development

```bash
# Build
cargo build --release

# Run with custom data directory
SIGIRO_DATA_DIR=./data cargo run -- serve

# Run tests
cargo test --release

# Run E2E test
cd tests/e2e/k8s && ./run.sh
```
