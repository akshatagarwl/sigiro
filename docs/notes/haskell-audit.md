---
title: Haskell Programming Principles Audit — Sigiro
created: 2026-04-28
updated: 2026-04-28
type: concept
tags: ['audit', 'haskell', 'principles', 'rust', 'architecture', 'code-quality']
---

# Haskell Programming Principles Audit — Sigiro

**Date:** 2026-04-28
**Approach:** Adapt core Haskell programming principles to evaluate Rust codebase quality.
**Reference:** Haskell's emphasis on purity, type safety, composition, and equational reasoning.
**Method:** 4 parallel Oracle audit swarms, each covering 3 principles.

---

## Summary

| # | Principle | Status | Key Finding | Priority |
|---|-----------|--------|-------------|----------|
| 1 | Purity & Referential Transparency | ⚠️ PARTIAL | SQL construction entangled with I/O; hidden clock dependency | HIGH |
| 2 | Immutability by Default | ⚠️ PARTIAL | 8× boilerplate `mut results` pattern (justified by DuckDB API) | LOW |
| 3 | Type-Driven Design | ⚠️ PARTIAL | `InvestigateRequest` allows invalid state; 3 stringly-typed fields | HIGH |
| 4 | Exhaustive Pattern Matching | ⚠️ PARTIAL | 3× `.expect()` in flusher can panic; CHECKPOINT failure silenced | **CRITICAL** |
| 5 | Composition over Inheritance | ✅ COMPLIANT | Clean middleware chain; flat trait implementations | — |
| 6 | Monadic Error Handling | ⚠️ PARTIAL | Same `.expect()` + silenced checkpoint | **CRITICAL** |
| 7 | Pure Core / Impure Edges | ⚠️ PARTIAL | SQL+IO entangled; `QueryCtx` couples pure+impure | MEDIUM |
| 8 | Equational Reasoning | ⚠️ PARTIAL | 5 functions exceed 50 lines; best-in-class comments | LOW |
| 9 | Parametricity | ⚠️ PARTIAL | 9× copy-paste prepare→query→collect; 8× mutex lock boilerplate | HIGH |
| 10 | Avoid Primitive Obsession | ⚠️ PARTIAL | `status_code` is magic `i32`; timestamps are unitless `i64` | HIGH |
| 11 | Laziness — On-Demand Computation | ✅ COMPLIANT | Correct query gating, short-circuits, explicit column lists | — |
| 12 | IO Monad Pattern — Explicit Effects | ⚠️ PARTIAL | 6+ pure functions in IO module | LOW |

**Score: 2/12 fully compliant, 10/12 partially compliant, 0/12 in full violation.**

---

## Principle 1: Purity & Referential Transparency — ⚠️ PARTIAL

**Strengths:** `compute_coverage()`, `adaptive_config()`, `build_span_trees()`, `validate_read_only()`, `resolve_services()` are all genuinely pure — depend only on parameters, zero I/O.

**Violations:**
1. **`chrono::Utc::now()` hidden in domain logic** (`investigate.rs:513`) — `investigate_one()` reads the system clock, making it non-deterministic. The same `InvestigateRequest` with `to_ts: None` produces different results depending on when it's called.
2. **SQL construction entangled with I/O execution** — Every `query_*` function interleaves `format!()` SQL building with `conn.prepare()` + `stmt.query()`. The 400+ lines of SQL logic in `investigate.rs` are untestable without a live DuckDB.
3. **Tracing side effects** in domain functions (`investigate.rs:469`) — minor but present.

**Fix:** Inject `now_us` as parameter. Extract SQL builders into pure functions.

---

## Principle 2: Immutability by Default — ⚠️ PARTIAL

**Strengths:** `&str` used consistently for borrowed params. `QueryCtx` carries `&'a str` references. Iterator chains used for linked trace extraction (`filter → filter_map → take(5) → collect`). `Arc<Mutex<Connection>>` is justified by DuckDB's single-writer model.

**Violations:**
1. **`compute_coverage()` gap construction** (`investigate.rs:47-62`) — imperative `mut Vec` with conditional pushes. Could use `then_some` + iterator chain.
2. **8× boilerplate `mut results` + `while let` pattern** — justified by DuckDB's `Result<Option>` API (doesn't implement `Iterator`), but a `collect_rows<T>` helper would eliminate the duplication.
3. **`query_derived_events()`** — imperative event collection could use `Iterator::chain()`.

**Fix:** Add `collect_rows<T>` helper. Refactor gap construction to functional style.

---

## Principle 3: Type-Driven Design — ⚠️ PARTIAL

**Strengths:** `SignalTable` enum (7 variants, exhaustive match) prevents invalid table names. `IngestMsg` covers all signal types. Typed axum paths (`TypedPath`) provide compile-time route verification. No bare `unwrap()` in production — all uses are `unwrap_or`, `unwrap_or_default`, or `expect` on compile-time-known values.

**Violations:**
1. **`InvestigateRequest` allows invalid state** (`investigate.rs:270-279`) — both `service` and `services` can be `Some` simultaneously. Should use `enum ServiceSelector { Single(String), Multiple(Vec<String>) }`.
2. **`DerivedEvent.event_type: String`** — only ever `"deploy"` or `"k8s_event"`. Should be an enum.
3. **`CreateKeyRequest.plan: String`** — valid values are `free/builder/team/pro` but not enforced by types. Should be `enum Plan`.
4. **`AdaptiveConfig.bucket_interval: &'static str`** — 3 possible values, could be an enum.
5. **Sentinel string `"__missing_auth__"`** in `BearerKeyExtractor` (`server.rs:39`).

**Fix:** `ServiceSelector` enum (highest impact). `EventType` and `Plan` enums (quick wins).

---

## Principle 4: Exhaustive Pattern Matching — ⚠️ PARTIAL

**Strengths:** All `match` arms are exhaustive (`IngestMsg::project_id`, `SignalTable::table_suffix`, `resolve_services`, `buffer_msg`, `value_ref_to_json` with documented catch-all).

**Violations:**
1. **3× `.expect()` in ingest flusher** (`inserter.rs:191,196,224`) — can panic and kill the per-project flusher task, silently stopping data ingestion with no recovery. `classify_logs` should return `Result`.
2. **`let _ =` discarding `Result`** (`types.rs:31`) — cleanup file deletion failure silently swallowed.
3. **Critical: CHECKPOINT failure silenced** (`pool.rs:32`) — `let _result = guard.execute_batch("CHECKPOINT lake;")` means data may not persist to Parquet before connection eviction. This is the durability path.
4. **`if let Ok(s) = row.get(...)` silently discards errors** (`investigate.rs:186`).
5. **`.expect()` in startup code** (`server.rs:112,178`, `main.rs:119,130`) — acceptable but improvable.

**Fix (URGENT):** Make `classify_logs` return `Result`. Log checkpoint errors instead of discarding.

---

## Principle 5: Composition over Inheritance — ✅ COMPLIANT

**Strengths:** Clean middleware chain (`GovernorLayer → auth_middleware → handler`). `investigate_one` decomposes into 10+ focused sub-query functions. `MsgRouter` composes `ProjectPool` + `DashMap` + `mpsc::channel` + `tokio::spawn` — flat composition, no inheritance. All trait implementations are flat (`KeyExtractor`, `TypedPath`, `Drop`).

**Minor concern:** `run_serve` in `main.rs` is 120 lines of application wiring. Could extract `setup_ebpf()` and `setup_federation()` sub-functions for testability.

---

## Principle 6: Monadic Error Handling — ⚠️ PARTIAL

**Strengths:** `?` used consistently throughout. 30+ `.context()` calls with descriptive messages. `RouterError` is a typed enum in `inserter.rs`. `axum_anyhow` provides structured HTTP error responses (`context_bad_request`, `context_internal`, `context_not_found`). Error wrapping preserves service context.

**Violations:**
1. **Same 3× `.expect()` in flusher** — bypasses `Result` propagation entirely (see P4).
2. **`anyhow` everywhere** — error types not visible in signatures. Acceptable for an application, but callers cannot pattern-match on specific error variants.
3. **CHECKPOINT failure silently discarded** (`pool.rs:32`) — durability violation.

**Fix:** Same as P4 — make `classify_logs` return `Result`. Log checkpoint errors.

---

## Principle 7: Separation of Pure Core from Impure Edges — ⚠️ PARTIAL

**Strengths:** `DrainTree` (`drain.rs`) is entirely pure — `classify()` takes `&str`, returns `(String, u64)`. `compute_coverage()` and `adaptive_config()` are unit-testable without I/O. `investigate_handler` is a thin HTTP wrapper.

**Violations:**
1. **SQL construction entangled with `Connection` execution** — cannot test SQL correctness without DuckDB.
2. **`QueryCtx` couples pure params with impure conn** — can't construct for SQL-building tests.
3. **`query_handler` and `services_handler`** are not thin wrappers — SQL + execution + JSON assembly inline.

**Fix:** Extract SQL builders as pure functions. Split `QueryCtx` into `QueryParams` + executor.

---

## Principle 8: Equational Reasoning — ⚠️ PARTIAL

**Strengths:** Best-in-class "why" comments throughout the codebase:
- *"Without this, an agent might conclude 'no errors in logs' when there ARE no logs"* (`investigate.rs:19-20`)
- *"ANY_VALUE because resource_attributes varies per span"* (`investigate.rs:97-98`)
- *"TRADEOFF: root spans have NULL parent — empty string is correct"* (`investigate.rs:797`)
- *"Uses a CTE to extract version first — avoids DuckDB casting issues"* (`investigate.rs:209-210`)

Variable names self-documenting. Good function decomposition for sub-queries.

**Violations:**
1. **5 functions exceed 50 lines:** `investigate_one` (84L), `query_all_metrics` (67L), `query_operation_summaries` (76L), `query_service_context` (76L), `services_handler` (53L).
2. **`query_all_metrics` has 35-line inline SQL** — requires understanding the entire UNION ALL mentally.
3. **`investigate_one` final struct assembly** is 28 lines of field assignment alone.

**Fix:** Extract `InvestigationBlock` assembly into a helper. Break `query_service_context` into 3 focused functions.

---

## Principle 9: Parametricity — ⚠️ PARTIAL

**Strengths:** `&str` used consistently for borrowed string parameters. `SignalTable::table_suffix()` returns `&'static str` (zero allocation). `IngestMsg::project_id()` returns `&str` (borrows, no clone). `DashMap<String, ...>` properly typed.

**Violations:**
1. **9× copy-paste of prepare→query→iterate→collect pattern** — appears across all `query_*` functions:
   ```rust
   let mut stmt = ctx.conn.prepare(&sql)?;
   let mut rows = stmt.query(duckdb::params![...])?;
   let mut results = Vec::new();
   while let Some(row) = rows.next()? {
       results.push(/* row parsing */);
   }
   Ok(results)
   ```
   Should be a single generic `query_collect<T>(conn, sql, params, parse_fn)`. This is the biggest DRY violation in the codebase (~100 lines of boilerplate).

2. **8× `conn.lock().map_err(...)` boilerplate** — should be a `lock_conn()` helper.

**Fix:** Introduce `query_collect<T>` generic. Add `lock_conn()` helper.

---

## Principle 10: Avoid Primitive Obsession — ⚠️ PARTIAL

**Strengths:** `SignalTable` and `IngestMsg` are proper typed enums. Typed axum paths. `OutputFormat` is an enum.

**Violations:**
1. **`status_code: i32`** — magic number `2` used 5+ times for "error" (`investigate.rs:544,669,708,912`). Should be `#[repr(i32)] enum StatusCode { Unset=0, Ok=1, Error=2 }`.
2. **`from_ts`/`to_ts: i64`** — unitless timestamps. `main.rs:170` multiplies by `1_000_000` (secs→µs) but nothing enforces the unit. Should be `TimestampMicros(i64)` newtype.
3. **`project_id`, `trace_id`, `span_id`** — all bare `String`. `TraceId` should be a newtype with hex validation.
4. **`plan: String`** — should be `enum Plan { Free, Builder, Team, Pro }`.
5. **`duration_ns: i64`** — comment is the only enforcement of "nanoseconds".

**Fix (quick win):** `StatusCode` enum (~1h). `TimestampMicros` newtype (~2-4h). `Plan` enum (~30min).

---

## Principle 11: Laziness — On-Demand Computation — ✅ COMPLIANT

**Strengths:**
- `adaptive_config()` correctly gates expensive queries (0 span limit for >2h windows).
- `query_spans()` short-circuits on `span_limit == 0`.
- Linked traces short-circuit on empty input (`investigate.rs:923`).
- All SQL uses explicit column lists (no `SELECT *`).
- Flusher short-circuits on empty buffer (`inserter.rs:251`).
- Auth cache avoids DB hits on repeat requests.
- Iterator `filter_map + take(5)` limits before collecting.

**No violations found.** The codebase demonstrates good laziness discipline throughout.

---

## Principle 12: IO Monad Pattern — Explicit Effects — ⚠️ PARTIAL

**Strengths:** Query functions clearly named (`query_*`). Write functions clearly named (`buffer_*`, `flush`, `send`). `run_flusher` is clearly an async IO boundary. Module structure partially separates IO (`inserter.rs` = writes, `investigate.rs` = reads, `server.rs` = HTTP adapter).

**Violations:**
1. **6+ pure functions live in IO-heavy `investigate.rs`:** `compute_coverage`, `build_span_trees`, `adaptive_config`, `us_to_iso`, `resolve_services`, `parse_span_row` — all pure, mixed with `query_*` functions.
2. **2 pure functions in `server.rs`:** `validate_read_only`, `value_ref_to_json` — pure string analysis among async handlers.

**Fix:** Extract pure functions into `investigate/analysis.rs` and `server/pure.rs`.

---

## Priority Fix Roadmap

### 🔴 Critical (data loss / crash risk)
1. **`classify_logs` → return `Result`** (`inserter.rs:191,196,224`) — prevents flusher panic killing ingestion
2. **Log CHECKPOINT errors** (`pool.rs:32`) — prevents silent data loss on eviction

### 🟠 High (type safety, DRY, testability)
3. **`ServiceSelector` enum** — eliminates invalid `InvestigateRequest` states (~3h)
4. **`query_collect<T>` generic** — eliminates 9× copy-paste (~100 lines, ~2-3h)
5. **`StatusCode` enum** — replaces 5+ magic number comparisons (~1h)
6. **Extract SQL builders as pure functions** — enables unit testing without DuckDB (~2-4h)
7. **`TimestampMicros` newtype** — prevents sec/µs confusion (~2-4h)

### 🟡 Medium (code quality)
8. **Split `QueryCtx`** into `QueryParams` + executor (~2h)
9. **`Plan` + `EventType` enums** — replaces stringly-typed fields (~1h)
10. **Extract pure functions** from `investigate.rs` into separate module (~2-4h)

### 🟢 Low (nice to have)
11. **`collect_rows<T>` helper** — reduces `while let` boilerplate (~1h)
12. **Functional `compute_coverage` gaps** — replace imperative `mut Vec` (~30min)
13. **Decompose `investigate_one` block assembly** — extract struct construction (~1h)
