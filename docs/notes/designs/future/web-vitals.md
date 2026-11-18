---
title: Web Vitals + Client Errors — Design
created: 2026-03-28
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'data-model', 'design', 'logs', 'market']
---

# Web Vitals + Client Errors — Design

Dependency: Vercel Marketplace blocked on this. Ship Web Vitals BEFORE Vercel integration.

## What

CLS/LCP/INP as metrics, JS exceptions as logs, browser→server span linking.

## Web Vitals

- CLS (Cumulative Layout Shift): visual stability, good < 0.1
- LCP (Largest Contentful Paint): load speed, good < 2.5s
- INP (Interaction to Next Paint): responsiveness, good < 200ms

## Ingestion Path

```
Browser:
  web-vitals (Google lib) → OTel browser SDK → OTLP HTTP JSON → :4318 → sigiro
  JS exceptions → OTel browser SDK → OTLP HTTP JSON → :4318 → sigiro
```

Zero server changes needed — :4318 already accepts [[architecture/write-path.md|OTLP]] HTTP (protobuf + JSON).

Web Vitals arrive as OTel metrics (gauge: CLS, histogram: LCP/INP).
JS exceptions arrive as OTel logs (severity=ERROR, body=stack trace).
Browser spans arrive as OTel traces (linked to server spans via traceparent header).

## Browser → Server Span Linking

Next.js/Vercel propagates W3C traceparent header from browser fetch() → server.
Browser span (parent) → server span (child) → same trace_id.
Sigiro already stores trace_id — correlation is automatic.

## SDK: @sigiro/browser or @sigiro/nextjs

Thin wrapper around:
- web-vitals (Google, ~1.5KB gzip)
- @opentelemetry/sdk-trace-web (~15KB gzip)
- Auto-configure [[architecture/write-path.md|OTLP]] exporter to point at sigiro endpoint

```js
// User code:
import { initSigiro } from '@sigiro/nextjs'
initSigiro({ endpoint: 'https://your-sigiro:4318' })
// Done. Web Vitals + errors + browser traces flow automatically.
```

## Storage

Web Vitals → sigiro_metrics_gauge / sigiro_metrics_histogram (existing tables)
JS exceptions → sigiro_logs (existing table, severity=ERROR)
Browser spans → sigiro_spans (existing table, span_kind=CLIENT)

No new tables needed.

## Correlation (the sigiro story)

"LCP is 4s because the /api/checkout call behind it takes 3.2s,
and the profile shows 2.8s in serde::to_string"

Web Vital → browser span → server span → profile → root cause.

## Files to Change

Server-side: nothing ([[architecture/write-path.md|OTLP]] HTTP already works)
New: npm package @sigiro/nextjs (or @sigiro/browser)
  - src/index.ts — init, web-vitals hook, OTel browser SDK config
  - ~200 lines total

## Estimate: ~200 lines (all client-side JS/TS)
