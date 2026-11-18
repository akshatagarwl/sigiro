---
title: Collector Config Validation at Deploy Time
created: 2026-04-08
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'competitor', 'decision', 'deployment', 'design', 'logs']
---

# Collector Config Validation at Deploy Time

## Problem

User's OTel Collector config can transform data in ways that break Sigiro's storage contract. Required NOT NULL fields (service_name, timestamp, severity_number, severity_text, etc.) could be deleted, renamed, or dropped by processors. Without validation, data is silently lost.

## Approach

Reject bad configs before they deploy. Parse the YAML, walk the pipeline, flag anything that violates the field contract. User must fix and resubmit.

## Field Contract

These must survive the pipeline:

### Resource attributes
- `service.name` → sigiro_logs.service_name (NOT NULL)
- `service.namespace` → sigiro_logs.service_namespace (nullable)

### Log record fields
- `timestamp` → sigiro_logs.timestamp (NOT NULL)
- `observedTimestamp` → sigiro_logs.observed_timestamp (NOT NULL)
- `severityNumber` → sigiro_logs.severity_number (NOT NULL)
- `severityText` → sigiro_logs.severity_text (NOT NULL)
- `body` → sigiro_logs.body (nullable)

### Span fields
- `timestamp`, `end_timestamp`, `duration` (NOT NULL)
- `service_name`, `span_name` (NOT NULL)
- `span_kind`, `status_code` (NOT NULL)

### Metric fields
- `timestamp`, `metric_name`, `value`, `service_name` (NOT NULL)

## What Gets Rejected

```yaml
# REJECTED — deletes service.name
processors:
  transform:
    log_statements:
      - context: resource
        statements:
          - delete_key(attributes, "service.name")
```

```yaml
# REJECTED — drops all logs
processors:
  filter:
    logs:
      log_record:
        - 'severity_number < 9'
```

```yaml
# ACCEPTED — renames but preserves
processors:
  transform:
    resource_statements:
      - statements:
          - set(attributes["app.name"], attributes["service.name"])
```

## API

```
POST /collectors/validate
Content-Type: application/yaml
Body: <raw config.yaml>

200 { "valid": false, "errors": [...] }
200 { "valid": true, "errors": [] }
```

`POST /collectors/{project_id}/deploy` calls validate first. Rejects with 400 if invalid.

## Implementation

- `validator.rs` — ~300 lines
- Parse config YAML → CollectorConfig struct
- Walk each pipeline's processors in order
- Track surviving required fields through the pipeline
- `analyze_transform()` — pattern match on known OTTL statement forms:
  - `delete_key(attributes, "X")` — removes field
  - `replace_all_patterns` — could match required fields, flag as warning
  - `set()` with `??` fallback — field survives
- `analyze_filter()` — check if filter could drop all records
- `analyze_attributes()` — check for delete actions on required keys
- Unknown processors — warn but don't reject

## UI (future)

```
✗ Config rejected — 1 error

  logs pipeline
  └─ transform processor
     └─ service.name
        ├─ delete_key removes required resource attribute
        └─ Fix: remove the delete_key, or add fallback:
           set(attributes["service.name"], attributes["service.name"] ?? "unknown")
```

## Open Questions

- OTTL statement parser scope: full OTTL grammar vs known pattern forms? Start with known forms (delete_key, set, replace_all_patterns, replace_match). Full parser later if needed.
- Should we allow configs that might be risky (replace_all_patterns) with a warning, or strict rejection only?
- Version pinning: which OTel Collector version's OTTL grammar do we validate against?
