# Microservices Demo — Agent Playbook

> **Purpose:** Real-world telemetry validation with the OpenTelemetry Demo.
> **Target:** macOS ARM64 (Apple Silicon) with Docker (OrbStack/Desktop).

## Quick Start

Run the fast federation E2E test (~90s):

```bash
cd tests/microservices-demo/otel-demo-federation
./run-e2e.sh
```

## Structure

```
tests/microservices-demo/
├── AGENTS.md                          # <-- YOU ARE HERE
├── opentelemetry-demo/                # Git submodule (v2.2.0)
├── online-boutique/                   # Git submodule (Google repo)
└── otel-demo-federation/              # Fast Docker-based federation E2E
    ├── docker-compose.yml
    ├── run-e2e.sh
    ├── Dockerfile.sigiro
    ├── otelcol-config.yml
    ├── postgresql/init.sql
    ├── flagd/flags.json
    └── .env
```

## For Future Agents

The old dated run directories (`runs/2026-05-*/`) have been removed.
The canonical fast test is always `otel-demo-federation/`.
