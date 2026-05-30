# Federation E2E Test — Agent Playbook

> **Purpose:** One canonical real-world telemetry validation test.
> **Target:** macOS ARM64 (Apple Silicon) with Docker (OrbStack/Desktop).

## Canonical E2E Test

```bash
cd tests/e2e/federation
./run-e2e.sh
```

This is the **only** microservices-based E2E test. It runs the OpenTelemetry Demo
(Astronomy Shop) inside Docker with sigiro-central + sigiro-collector in true
federation mode. ~90s end-to-end.

## Structure

```
tests/
├── e2e/federation/           # <-- YOU ARE HERE
│   ├── docker-compose.yml    # sigiro + OTel Demo services
│   ├── run-e2e.sh            # Orchestration + verification
│   ├── Dockerfile.sigiro     # sigiro image from local binary
│   ├── otelcol-config.yml    # OTel collector → sigiro-collector
│   ├── postgresql/init.sql   # Catalog schema
│   ├── flagd/flags.json      # Feature flags
│   └── .env                  # Environment variables
└── fixtures/
    └── opentelemetry-demo/   # Git submodule (v2.2.0)
```
