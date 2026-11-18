---
title: API Surface
created: 2026-05-12
type: reference
tags: ['architecture', 'api', 'http']
---

# API Surface

Every feature is an HTTP endpoint first. CLI and other agent-facing clients use the same API.

## Design Principles

1. **Bearer auth everywhere.** `Authorization: Bearer sigiro_...` — one header, every request.
2. **JSON in, JSON out.** `Content-Type: application/json` for requests. Responses always JSON (including errors).
3. **Structured errors.** Every error has `{ "error": { "code": "...", "message": "..." } }`.
4. **Idempotent where possible.** PUT for upserts, POST for actions.
5. **No pagination needed.** [[architecture/investigation.md|Investigation]] blocks are pre-aggregated (~10-50KB). Query results capped at 1000 rows.

---

## Endpoints

```
Base URL: http://localhost:9999/v1
Auth:     Authorization: Bearer sigiro_...
```

### Investigation (the product)

```
POST /v1/diagnose
Body: { "services": ["checkout", "payments", "inventory"], "from_ts": ..., "to_ts": ... }
-> 200: { "results": { "checkout": InvestigationBlock, ... }, "errors": { ... } }
```

### Query (power users)

```
POST /v1/query
Body: { "sql": "SELECT service_name, count(*) FROM sigiro_spans GROUP BY 1" }
-> 200: { "columns": ["service_name", "count_star()"], "rows": [...], "truncated": false }
```

### Status

```
GET /v1/status
-> 200: {
    "services": ["checkout", "payments"],
    "tables": { "sigiro_spans": 14523, "sigiro_logs": 8901 },
    "retention_days": 30,
    "plan": "free",
    "usage": { "spans_ingested": 450000, "limit": 1000000 }
  }
```

### Services (discovery for agents)

```
GET /v1/services
-> 200: {
    "services": [
      { "name": "checkout", "first_seen": "...", "last_seen": "...", "span_count": 1234 },
      { "name": "payments", "first_seen": "...", "last_seen": "...", "span_count": 567 }
    ]
  }
```

### OTLP Ingest

```
gRPC:  :4317 (with Authorization metadata)
HTTP:  POST /v1/traces  (Authorization header)
       POST /v1/metrics
       POST /v1/logs
```

### Health (public, no auth)

```
GET /health -> 200: "ok"
```

---

## Error Format

```json
{
  "error": {
    "code": "invalid_api_key",
    "message": "Invalid API key.",
    "request_id": "req_abc123"
  }
}
```

Error codes (stable, agents can switch on these):

| Code | HTTP | When |
|------|------|------|
| `missing_auth` | 401 | No Authorization header |
| `invalid_api_key` | 401 | Key doesn't verify |
| `rate_limited` | 429 | Rate limit exceeded |
| `query_rejected` | 400 | SQL contains blocked keyword |
| `invalid_request` | 400 | Malformed JSON, missing fields |
| `service_not_found` | 404 | No data for requested service |
| `internal_error` | 500 | DuckDB error, unexpected failure |

### Response Headers

Every response includes:

```
X-Request-Id: req_abc123
X-RateLimit-Limit: 500
X-RateLimit-Remaining: 487
X-RateLimit-Reset: 1711100960
```
