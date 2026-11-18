---
title: "Plan: AXI Compliance for Sigiro CLI"
created: 2026-04-09
updated: 2026-04-15
type: summary
tags: ['ai-agent', 'auth', 'data-model', 'duckdb', 'investigation', 'logs']
---

# Plan: AXI Compliance for Sigiro CLI

**Date:** 2026-04-09
**Reference:** [AXI Audit](../[[axi-audit.md|axi-audit]].md)

---

## Phase 1: High-Impact Fixes (Principles 1, 6, 8)

### 1.1 Add TOON output format (Principle 1)

**Goal:** Replace `serde_json::to_string_pretty` with TOON format for all CLI output.

**Changes:**
- Add `toon` dependency to `Cargo.toml`
- Create `src/toon.rs` — TOON serializer for Sigiro types
- Update `main.rs` to use TOON for `query`, `status`, `investigate` commands
- Keep JSON as fallback with `--json` flag

**TOON format for Sigiro types:**
```text
# sigiro status
services[2]{name,spans,logs,metrics}:
  checkout,1500,300,45
  payments,800,200,30

# sigiro diagnose checkout
investigate{service,window,ops,spans,logs}:
  checkout,1711000000-1711001000,3,50,20
ops[3]{op,err%,p95,exemplar}:
  POST /checkout,5%,1200ms,abc123
  GET /cart,0%,50ms,def456
  POST /pay,2%,800ms,ghi789
```

**Files to change:**
- `Cargo.toml` — add `toon` crate
- `src/toon.rs` — new file, TOON serializers
- `src/main.rs` — update output format

### 1.2 Structured error format (Principle 6)

**Goal:** Replace anyhow/Context with structured error output on stdout.

**Changes:**
- Create `src/errors.rs` — structured error types
- Update error handling in `main.rs` to use structured format
- Define exit codes: 0 = success, 1 = error, 2 = usage error

**Error format:**
```
error: no hosted API key configured
help: set SIGIRO_ACCESS_TOKEN or pass --token <token>
```

**Files to change:**
- `src/errors.rs` — new file, error types and formatting
- `src/main.rs` — update error handling
- `src/client.rs` — update HTTP error translation

### 1.3 Content-first default command (Principle 8)

**Goal:** `sigiro` with no args shows live data, not starts server.

**Changes:**
- Make `sigiro` (no args) show a dashboard of:
  - Active services and row counts
  - Recent anomalies
  - DB file size
  - Connection [[plans/STATUS.md|status]]
- Move server start to `sigiro serve` (already exists, but currently default)

**Dashboard format:**
```text
sigiro[1.0.0]:
  data_dir: ~/.sigiro
  db_size: 45MB
  status: connected
services[2]{name,spans,logs}:
  checkout,1500,300
  payments,800,200
anomalies[1]{type,service,severity}:
  latency_spike,checkout,warning
help[2]:
  Run `sigiro diagnose <service>` for detailed analysis
  Run `sigiro query "SELECT ..."` for custom queries
```

**Files to change:**
- `src/main.rs` — change default command behavior
- `src/dashboard.rs` — new file, dashboard data collection

---

## Phase 2: Medium-Impact Fixes (Principles 2, 7, 9)

### 2.1 Minimal default schemas (Principle 2)

**Goal:** Reduce default fields in list views to 3-4 items.

**Changes:**
- Add `--fields` flag to `sigiro diagnose` and `sigiro query`
- Create "lite" versions of SpanRow, LogTemplate, MetricBucket
- Default to identifier + [[plans/STATUS.md|status]] + key metric

**Files to change:**
- `src/investigate.rs` — add SpanRowLite, LogTemplateLite structs
- `src/main.rs` — add --fields flag parsing

### 2.2 Session hooks (Principle 7)

**Goal:** Self-installing hooks for Claude Code and Codex.

**Changes:**
- Add `sigiro install` command
- Create `src/hooks.rs` — hook installation logic
- Register `SessionStart` hooks in Claude Code/Codex config
- Self-heal on every invocation

**Files to change:**
- `src/hooks.rs` — new file, hook installation
- `src/main.rs` — add install command

### 2.3 Contextual disclosure (Principle 9)

**Goal:** Append next-step suggestions after each output.

**Changes:**
- Create `src/suggestions.rs` — contextual help generation
- Append `help[N]` section after each command output
- Suggestions based on output type and content

**Files to change:**
- `src/suggestions.rs` — new file, suggestion generation
- `src/main.rs` — integrate suggestions into output

---

## Phase 3: Low-Impact Fixes (Principles 5, 10)

### 3.1 Definitive empty states (Principle 5)

**Goal:** Explicit "0 results" messages.

**Changes:**
- Update `sigiro status` to show meaningful status (not just "ok")
- Update `sigiro diagnose` to show explicit zero-counts
- Update `sigiro query` to show "0 rows returned"

**Files to change:**
- `src/main.rs` — update [[plans/STATUS.md|status]] output
- `src/investigate.rs` — update empty state handling

### 3.2 Enhanced help text (Principle 10)

**Goal:** Better Clap help with examples.

**Changes:**
- Update Clap attributes with examples
- Add TOON format examples to help text
- Add AXI-specific guidance

**Files to change:**
- `src/main.rs` — update Clap attributes

---

## Implementation Order

1. **Phase 1.3** — Content-first default (smallest change, biggest UX win)
2. **Phase 1.2** — Structured errors (affects all commands)
3. **Phase 1.1** — TOON output (largest change, most impactful)
4. **Phase 2** — Medium fixes (can be done in parallel)
5. **Phase 3** — Low fixes (can be done last)

---

## Testing

- Unit tests for TOON serialization
- Integration tests for error formatting
- Manual testing of `sigiro` (no args) dashboard
- Test with Claude Code/Codex hooks

## Dependencies

- `toon` crate for TOON format
- No other new dependencies needed

## Future Considerations

- **[[architecture/data-model.md|DuckDB]] native TOON support**: There's an open discussion about adding TOON format support to DuckDB: https://github.com/duckdb/duckdb/discussions/21349. If implemented, we could potentially use DuckDB's native TOON export for preparing prompts directly in the database, which could be more efficient than our current approach of converting JSON to TOON in Rust.
