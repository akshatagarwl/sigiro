---
title: AXI Audit — Sigiro CLI
created: 2026-04-09
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'auth', 'competitor', 'data-model', 'duckdb']
---

# AXI Audit — Sigiro CLI

**Date:** 2026-04-09
**Reference:** [AXI (Agent eXperience Interface)](https://github.com/kunchenguid/axi)
**Target:** Sigiro CLI (`sigiro` binary)

---

## Principle 1: Token-efficient output

**[[plans/STATUS.md|Status]]: ❌ VIOLATION**

All CLI commands output `serde_json::to_string_pretty` — verbose JSON with full field names, whitespace, and nesting.

**Affected commands:**
- `sigiro query` → pretty JSON array
- `sigiro status` → pretty JSON
- `sigiro investigate` → pretty JSON [[architecture/investigation.md|InvestigationBlock]]

**Fix:** Convert to [TOON](https://toonformat.dev/) format at the output boundary. Keep internal logic on JSON, convert to TOON in `main.rs` before `println!`.

**Note:** [[architecture/data-model.md|DuckDB]] is considering native TOON support: https://github.com/duckdb/duckdb/discussions/21349. If implemented, we could potentially use DuckDB's native TOON export for preparing prompts directly in the database.

**Example:**
```text
# Current (JSON, ~200 tokens)
{
  "service": "checkout",
  "window": { "from_ts": 1711000000, "to_ts": 1711001000 },
  "operation_summaries": [
    { "operation": "POST /checkout", "error_rate": 0.05, "p95_ms": 1200 }
  ]
}

# AXI (TOON, ~80 tokens)
investigate{service,window,ops}:
  checkout,1711000000-1711001000,1
ops[1]{op,err%,p95}:
  POST /checkout,5%,1200ms
```

---

## Principle 2: Minimal default schemas

**[[plans/STATUS.md|Status]]: ⚠️ PARTIAL VIOLATION**

`sigiro investigate` returns ALL fields — spans include `span_attributes`, `events_json`, `links_json` (JSON blobs), `resource_attributes`. An agent rarely needs these in the default view.

**Affected:**
- `InvestigationBlock.spans` — returns full SpanRow with all 15+ fields
- `InvestigationBlock.logs` — returns full LogTemplate with all fields
- `InvestigationBlock.metrics` — returns full MetricBucket

**Fix:** Default to 3-4 fields per item. Add `--fields` flag for expansion. Detail views (`sigiro span <id>`, `sigiro log <id>`) for full data.

---

## Principle 3: Content truncation

**[[plans/STATUS.md|Status]]: ✅ MOSTLY COMPLIANT**

Sigiro already has `Truncation` struct tracking `spans_total` vs `spans_returned`, `logs_total` vs `logs_returned`, etc. This is good.

**Minor issue:** No `--full` escape hatch in CLI. Agent can't request full data without changing the query window.

**Fix:** Add `--full` flag to `sigiro investigate` that increases caps. Or add `--limit N` per signal type.

---

## Principle 4: Pre-computed aggregates

**[[plans/STATUS.md|Status]]: ✅ COMPLIANT**

Sigiro does this well:
- `OperationSummary` includes `error_rate`, `p50_ms`, `p95_ms`, `p99_ms`, `span_count`
- `Coverage` pre-computes signal presence and gaps
- `ServiceContext` includes `pod_count`, `dependencies`, `dependents`
- `Truncation` pre-computes totals vs returned

Good work here.

---

## Principle 5: Definitive empty states

**[[plans/STATUS.md|Status]]: ⚠️ PARTIAL VIOLATION**

- `sigiro status` returns `{"status": "ok"}` — not informative
- `sigiro investigate` with no data returns empty arrays, not explicit "0 results" messages
- `sigiro query` with no results returns `[]` — ambiguous

**Fix:** Return explicit zero-count messages:
```
spans: 0 spans found for 'checkout' in last 15m
logs: 0 logs found for 'checkout' in last 15m
```

---

## Principle 6: Structured errors & exit codes

**[[plans/STATUS.md|Status]]: ❌ VIOLATION**

- Errors use `anyhow::Context` and `bail!` — human-readable but not structured
- Error messages leak dependency names (`"failed to connect to sigiro server"`)
- No structured error format on stdout
- Exit codes not explicitly managed (inherits from `?` propagation)

**Affected:**
- Missing hosted-key errors should direct users to set `SIGIRO_API_KEY` or pass `--key <token>`; local Open servers need neither.
- HTTP errors → `"Server error (401): ..."` — leaks raw server response

**Fix:** Create structured error format:
```
error: no hosted API key configured
help: set SIGIRO_API_KEY or pass --key <token>
```
Use exit codes: 0 = success, 1 = error, 2 = usage error.

---

## Principle 7: Ambient context via session hooks

**[[plans/STATUS.md|Status]]: ❌ NOT IMPLEMENTED**

Sigiro has no self-installing hooks. An agent must explicitly invoke `sigiro status` or `sigiro investigate` to see state.

**Fix:** Add `sigiro install` command that:
1. Registers `SessionStart` hooks in Claude Code (`~/.claude/settings.json`) and Codex (`~/.codex/hooks.json`)
2. Hook runs `sigiro status` on session start — shows active services, row counts, anomalies
3. Self-heals on every invocation (updates absolute path if binary moved)

---

## Principle 8: Content first

**[[plans/STATUS.md|Status]]: ❌ VIOLATION**

`sigiro` with no args → starts the server (`Serve` command). An agent running `sigiro` doesn't get data, it gets a long-running process.

`sigiro status` requires a server running; a hosted endpoint also requires a tenant key.

**Fix:** `sigiro` with no args should show:
- Active services and their row counts
- Recent anomalies (if any)
- DB file size
- Connection [[plans/STATUS.md|status]]

Like a dashboard, not a server process.

---

## Principle 9: Contextual disclosure

**[[plans/STATUS.md|Status]]: ❌ NOT IMPLEMENTED**

No output includes next-step suggestions. After `sigiro investigate`, the agent gets raw data but no hints about what to do next.

**Fix:** Append contextual help after each output:
```
help[3]:
  Run `sigiro span <trace_id>` for full trace
  Run `sigiro investigate <other_service>` to check dependencies
  Run `sigiro query "SELECT ..."` for custom analysis
```

---

## Principle 10: Consistent way to get help

**[[plans/STATUS.md|Status]]: ⚠️ PARTIAL**

Clap provides `--help` on all commands, but:
- Help text is generic (`"Run SQL query"`, `"Show server health"`)
- No per-subcommand examples
- No TOON format examples in help

**Fix:** Enhance Clap help text with examples and AXI-specific guidance.

---

## Summary

| # | Principle | Status | Priority |
|---|-----------|--------|----------|
| 1 | Token-efficient output | ❌ | HIGH |
| 2 | Minimal default schemas | ⚠️ | MEDIUM |
| 3 | Content truncation | ✅ | — |
| 4 | Pre-computed aggregates | ✅ | — |
| 5 | Definitive empty states | ⚠️ | LOW |
| 6 | Structured errors | ❌ | HIGH |
| 7 | Ambient context | ❌ | MEDIUM |
| 8 | Content first | ❌ | HIGH |
| 9 | Contextual disclosure | ❌ | MEDIUM |
| 10 | Consistent help | ⚠️ | LOW |

**High priority fixes:** Principles 1, 6, 8 — these have the most impact on agent token [[company/cost.md|cost]] and usability.
