---
title: Getting Started
created: 2026-05-17
type: guide
tags: ['deployment', 'quickstart']
---

# Getting Started

Run Sigiro locally in under 5 minutes.

---

## 1. Install

```bash
curl -sSf https://sigiro.com/install | sh
```

Requires macOS (Intel or Apple Silicon) or Linux (x86_64, ARM64).

---

## 2. Configure

```bash
export SIGIRO_DATA_DIR=~/.sigiro/data
```

This is the only required configuration. Everything else defaults to sensible values.

---

## 3. Start

```bash
sigiro serve
```

Starts OTLP HTTP on `:4318` and the query API on `:9999`.

---

## 4. Send telemetry

Point your OpenTelemetry SDK at Sigiro:

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318
export OTEL_SERVICE_NAME=my-service
```

No collector needed. Sigiro accepts OTLP directly.

---

## 5. Investigate

```bash
sigiro diagnose my-service
```

Returns a structured investigation block: metrics, spans, logs, and linked traces.

---

## Next Steps

- **Agent integration**: use `sigiro diagnose`, `sigiro query`, or the `/v1` HTTP API from your coding agent
- **Cloud deploy**: See `deploy/` for Fly.io deployment
