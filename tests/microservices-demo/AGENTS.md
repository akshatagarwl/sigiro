# Microservices Demo — Agent Playbook

> **Purpose:** One canonical real-world telemetry validation test.
> **Target:** macOS ARM64 (Apple Silicon) with Docker (OrbStack/Desktop).

## Canonical E2E Test

```bash
cd tests/microservices-demo/otel-demo-federation
./run-e2e.sh
```

This is the **only** microservices-based E2E test. It runs the OpenTelemetry Demo
(Astronomy Shop) inside Docker with sigiro-central + sigiro-collector in true
federation mode. ~90s end-to-end.

## Structure

```
tests/microservices-demo/
├── AGENTS.md                          # <-- YOU ARE HERE
├── opentelemetry-demo/                # Git submodule (v2.2.0)
└── otel-demo-federation/              # Canonical fast E2E test
    ├── docker-compose.yml
    ├── run-e2e.sh
    ├── Dockerfile.sigiro
    ├── otelcol-config.yml
    ├── postgresql/init.sql
    ├── flagd/flags.json
    └── .env
```
