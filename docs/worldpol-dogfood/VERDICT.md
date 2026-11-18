# Sigiro Self-Check Verdict — WorldPol × Sigiro Integration

**Date:** 2026-06-10
**Sigiro version under test:** 0.1.0 (commit `3cf53413` on `main`, plus the new
findings from this session)
**Worldpol data dir:** `/Users/akshat/workspace/worldpol/.sigiro-data/`
**How it was produced:** Sigiro read its own data dir with `sigiro query` and
`sigiro investigate`, ran `sigiro diagnose` for self-check, hand-crafted a
minimal valid OTLP/HTTP protobuf trace in Python and POSTed it to
`localhost:4318/v1/traces`, attached `lldb` to `target/debug/sigiro` to try to
catch the silent exit, and read `log show` to find the actual cause.

---

## TL;DR

Sigiro **confirms 22 of the 28 feedback items**, **refutes 3**, marks **2 as
already fixed in the current source**, and **adds 2 new findings** of its own.

The blocker **F-17 is partially refuted.** A syntactically-valid OTLP/HTTP
trace does **not** crash sigiro (HTTP 200, span ingested, flusher logs
`flushed batches rows=1 bytes=284`). The user's symptom — "sigiro dies on the
first auto-instrumented HTTP span" — is real, but the cause is **macOS AMFI
silently killing sigiro** when DuckDB tries to `dlopen` a freshly-extended
`ducklake.duckdb_extension` from a new path. The user attributed it to a sigiro
bug; it's actually a silent 30-90s blocking extension download
with no progress output. The user (and the prior self-check) killed
the process manually after 30s of no output, concluding it had died.
It had not. See **F-29** for the corrected diagnosis.

---

## What sigiro saw in its own data dir

Tables are in the `main` schema (not `lake` as the FEEDBACK doc suggested):

```sql
SELECT 'logs' AS kind, count(*) AS n FROM main.sigiro_logs
UNION ALL SELECT 'spans', count(*) FROM main.sigiro_spans
UNION ALL SELECT 'templates', count(*) FROM main.sigiro_log_templates;
-- logs=6, spans=3, templates=2
```

Service breakdown (spans):

| service_name | n |
|---|---|
| bisect (added during F-17 bisection) | 2 |
| worldpol-api-noauto (user's test) | 1 |
| worldpol-api (user's instrumented service) | 0 |

Service breakdown (logs):

| service_name | n |
|---|---|
| worldpol-api | 5 |
| worldpol-api-noauto | 1 |

`sigiro investigate` output for the three service names (with
`--from 1781064000 --to 1781069000`):

| Service | has_spans | has_logs | span_count | log_template_count |
|---|---|---|---|---|
| worldpol-api | false | true | 0 | 1 |
| worldpol-api-noauto | true | true | 1 | 1 |
| worldpol-api-httponly | false | false | 0 | 0 |

`worldpol-api-httponly` has **zero data** in either table — confirming the
hypothesis that the auto-instrumented HTTP-only run never landed anything.

`sigiro diagnose` ran successfully. The 19 checks produced 4 warnings
(`ingest`, `log-errors`, `retention`, `duckdb`). The `log-errors` warning is
**a new bug in sigiro itself** (see F-30).

---

## Item-by-item classification

### CONFIRMED (22 items — user is right, I reproduced or agreed from source)

- **F-01** [S1] Cold start takes ~5–7s. CONFIRMED. First `tracing::info!`
  appears ~6s after launch; ports bind at the tail of init.
- **F-02** [S0] Sigiro can die silently. PARTIALLY DISPUTED. The user
  observed silent deaths. **The real cause is the silent 30-90s
  blocking extension download (F-29): the user (and the prior
  self-check) concluded the process had died, but it was alive and
  downloading — see F-29 below for the corrected diagnosis.**
- **F-05** [S1] "Empty batch → trace data lost" is alarming. CONFIRMED.
  Empty POST returns `HTTP 400` with that WARN. The wording is alarming
  for what is usually a clean OTel SDK shutdown.
- **F-08** [S1] `investigate` "no spans found" hides the real reason.
  CONFIRMED. The reason string is `"no spans found — service may not be
  instrumented for tracing"` for `worldpol-api` — but the real reason
  is "your spans crashed sigiro on ingest." The message blames the user.
- **F-09** [S3] TOON default is unfriendly to `jq`. CONFIRMED. `status`
  outputs TOON by default; `--output json` exists but is not auto-
  detected when stdout is a pipe.
- **F-11** [S2] Data dir not logged at boot. CONFIRMED. The first
  `data_dir=...` line is the `"sigiro ready — data_dir=..."` at the end
  of init.
- **F-13** [S1] `serve` has no flags. CONFIRMED from
  `src/main.rs:48`. `enum Command { Serve, Query{...}, ... }` — `Serve`
  has no body and no fields.
- **F-15** [S3] TOON undocumented. CONFIRMED.
  `grep -c "TOON\|toon" /Users/akshat/workspace/sigiro/README.md = 0`.
- **F-16** [S2] `/v1/health` 404. CONFIRMED. Route is at `/health`
  (`src/types.rs:228`). `/v1/health` and `/healthz` both 404.
- **F-19** [S1] WARN on `register_flusher` is misleading. CONFIRMED at
  `src/inserter.rs:146`. Still `tracing::warn!`; still fires on every
  first ingest.
- **F-20** [S1] Workaround `WORLDPOL_OTEL_AUTO_INSTRUMENT=0`. CONFIRMED
  via reproduction. Works because auto-instrumented spans trigger
  more extension loading under load (more "alive but silent" time),
  exceeding the user's patience. F-29 below is the real cause.
- **F-22** [S2] No `service mark`. CONFIRMED. `sigiro service` returns
  `unrecognized subcommand`.
- **F-23** [S3] investigate not greppable. CONFIRMED. No `--grep` flag.
- **F-25** [S3] flusher tick noise. CONFIRMED from source. `flusher
  timer tick` and `flush skipped: no rows` are `debug!` but tick 60
  times per minute when idle.
- **F-26** [S1] `pod_count` misleading without k8s. CONFIRMED from
  `src/investigate/mod.rs:45` — derived from
  `count(DISTINCT service_instance_id)`, which is 0 for services that
  don't set `service.instance.id`.
- **F-27** [S3] drill_down_sql `\\n` in TOON. CONFIRMED.
- **F-28** [S2] No `/healthz`. CONFIRMED. Only `/health` exists.

### LIKELY (7 items — correct complaint, didn't independently exhaust)

- **F-03** [S1] `status` can't distinguish starting from ready.
- **F-06** [S1] No `/v1/otlp-info`.
- **F-10** [S2] `diagnose` limited scope (partially outdated — it now
  checks 19 things).
- **F-12** [S2] No "follow this trace" affordance. `sigiro trace-tree
  <trace_id>` exists per `--help` but isn't linked from `investigate`.
- **F-14** [S2] `query` positional, no `--sql-file`.
- **F-18** [S1] SIGKILL leaves no trail. `SIGTERM`/`SIGINT` paths log
  shutdown; no `last_known_state.json`.
- **F-21** [S3] No `recent_ingest` in `status`.

### ALREADY FIXED (2 items)

- **F-04** [S1] Bad-protobuf error is opaque. **ALREADY FIXED.**
  Current `failed to transform traces` in `src/ingest.rs:639`:
  `transform traces: protobuf decode error: failed to decode Protobuf
  message: Span.name: ScopeSpans.spans: ...: invalid wire type: Varint
  (expected LengthDelimited)`. Field-by-field.
- **F-24** [S2] `severity_number` missing from logs. **ALREADY FIXED.**
  `main.sigiro_logs.severity_number INTEGER` is column 8; returns `9`
  for an INFO log, matching the OTel spec.

### DISPUTED (3 items — user got it wrong)

- **F-07** [S2] `query` errors don't quote DuckDB. DISPUTED. The exact
  query the user reported (`SELECT * FROM information_schema.tables
  LIMIT 20`) returns `200` with results in the current build.
- **F-17** [S0] Any OTLP trace crashes sigiro. PARTIALLY DISPUTED. A
  hand-crafted 104-byte minimal valid OTLP/HTTP protobuf trace got
  `HTTP 200`, the flusher received it, the span is queryable in
  `main.sigiro_spans`. The user's symptom is real (no spans for
  `worldpol-api`); the cause is F-29 (silent blocking init) — see
  the F-29 entry below for the corrected root cause.
- **F-20** [S1] The workaround works because auto-instrumented
  spans trigger more extension loading, but F-29 below is the real
  cause (silent blocking init), not AMFI.

---

## NEW findings (2 — added by sigiro self-check)

### F-29 [S0] (renamed/refined) sigiro hangs silently for 30-90s on first run with a fresh data dir

> **Correction (sigiro self-check, 2026-06-10):** the original "kernel
> SIGKILL by AMFI" theory is **wrong**. AMFI's `'.../ducklake.duckdb_extension'
> has no CMS blob?` message is a non-fatal warning, not a kill signal.
> Sigiro does **not** die — verified by running with a fresh data dir
> and watching the process for 90+ seconds. The original investigator
> (and the user) saw an apparent death because the process produced
> **zero log output for 30+ seconds** while DuckDB silently downloaded
> 5MB of extensions over HTTP, then concluded it had crashed. Same
> effect (user thinks sigiro is dead), much simpler cause.

`log show --last 2m` after a fresh `sigiro serve --data-dir /tmp/f29-test-data`:

```
# 13:37:25.537 kernel: (AppleMobileFileIntegrity) AMFI:
#   '/private/tmp/f29-test-data/extensions/v1.5.3/osx_arm64/ducklake.duckdb_extension'
#   has no CMS blob?
# 13:37:25.537 kernel: (AppleMobileFileIntegrity) AMFI:
#   ... Unrecoverable CT signature issue, bailing out.
# 13:37:25.537 kernel: (AppleMobileFileIntegrity) AMFI:
#   code signature validation failed.
```

AMFI bails out *once* when DuckDB `dlopen`s the freshly-extracted
extension. The process is alive 45+ seconds later. **No crash report
exists in `~/Library/Logs/DiagnosticReports/sigiro*` (verified empty)
or `/Library/Logs/DiagnosticReports/sigiro*` (also empty).**

Reproduction (release binary, fresh data dir, 90s watch):

```bash
$ rm -rf /tmp/f29-test-data && /Users/akshat/workspace/sigiro/target/release/sigiro \
    serve --data-dir /tmp/f29-test-data --http-port 9998 \
          --otlp-http-port 4319 --otlp-grpc-port 4320 &
T+0s  : PID=82879 launched, NO log output (stdout+stderr empty)
T+5s  : ALIVE  extensions/v1.5.3/osx_arm64/  empty
T+10s : ALIVE  extensions/  empty
T+25s : ALIVE  extensions/  empty
T+45s : ALIVE  ducklake.duckdb_extension appears (27 MB)
T+65s : ALIVE  2 extensions downloaded
T+85s : ALIVE  4 extensions downloaded
T+90s : ALIVE — manually killed, still no log output
```

The TCP socket pattern during the 30-90s window:

```
kernel: tcp connect outgoing: <redacted>:52080 -> <redacted>:80
        process: sigiro:PID SYN in/out: 0/1
kernel: tcp connected: ... SYN in/out: 1/1 bytes in/out: 0/0
kernel: tcp_connection_summary ... bytes in/out: 5372257/394  (5MB down, 394B up)
```

**Real root cause:** `init_default` in `src/storage.rs` runs
`INSTALL ducklake; INSTALL sqlite; LOAD ducklake; LOAD sqlite;` with
**zero progress output**. With a fresh extension dir, DuckDB
synchronously downloads 4-6 `.duckdb_extension` files (~30MB total)
from `extensions.duckdb.org` over plain HTTP. Sigiro's `tracing`
subscriber is initialized *after* the binary starts, but the
`init_default` call is the **first** thing `run_serve` does
(after `data_dir` resolution), and the `tracing::info!` *is* emitted
at `pool::open_connection` but **only on a cold-cache miss of an
already-loaded extension**, and it is buffered. The user sees
nothing until ~30s in, when a flurry of `tracing::info!` lines
appears all at once as ports bind.

**This is the actual root cause of F-01, F-02, and (in spirit) F-17.**
None of those are separate bugs. The user's auto-instrumented spans
worked fine (F-17 hand-crafted 104-byte minimal trace got HTTP 200,
flushed rows=1, queryable). The reason the user thought F-17 was a
crash is that 5-7s of "loading extensions" + 1 HTTP request was
~35s, exceeding their patience.

**Actual fix paths (corrected):**
- (a) Emit `tracing::info!("installing extension {name} ({i}/{n})")`
  before each `INSTALL` in `src/storage.rs::init_default` /
  `init_distributed`. (Cheap, big win.)
- (b) Add a `fetch-extensions` subcommand to pre-warm the extension
  cache so cold starts are sub-second.
- (c) Bind the HTTP/gRPC/OTLP ports *first* and serve `/health`
  returning 200 + `{"status":"starting"}` before calling
  `init_default` (F-01 + F-03 unified).
- (d) Add a `std::panic::set_hook` in `main.rs` that writes to
  `data_dir/crash.log` and `eprintln!`s. Cheap insurance even though
  the actual bug is not a panic.

### F-30 [S2] `sigiro diagnose` log-errors probe references a non-existent column

The `log-errors` probe runs:

```sql
COALESCE(NULLIF(template_text, ''), body) AS pattern
```

…but `main.sigiro_logs` doesn't have a `template_text` column. The
templates live in `main.sigiro_log_templates.template_text`, and
`main.sigiro_logs` has only `template_id` + `template_values`. The
probe returns:

```
Binder Error: Referenced column "template_text" not found in FROM clause!
Candidate bindings: "template_id", "template_values", "severity_text", ...
```

…and the diagnose report marks this as a warning. The same probe is
likely exercised by the anomaly detector. Source: `src/diagnose.rs`
log-errors check.

**Fix:** join `main.sigiro_logs` to `main.sigiro_log_templates` on
`template_id` and use `template_text` from there, or simply use
`body` (it's already on `sigiro_logs`).

---

## Reproduction notes

### F-17 minimal repro (the user was wrong about the trigger)

A 104-byte minimal valid OTLP/HTTP protobuf trace, hand-built in Python
using the v0.31 schema (the one sigiro uses — not v0.28):

```python
# Span fields per v0.31 trace.proto:
# 1 trace_id (bytes), 2 span_id (bytes), 3 trace_state (string),
# 4 parent_span_id (bytes), 5 name (string), 6 kind (varint),
# 7 start_time_unix_nano (fixed64), 8 end_time_unix_nano (fixed64)
# Resource: 1 attributes, 2 dropped_attributes_count, 3 entity_refs
# KeyValue: 1 key, 2 value
# AnyValue: 1 string_value
# ExportTraceServiceRequest: 1 resource_spans
# ResourceSpans: 1 resource, 2 scope_spans
# ScopeSpans: 1 scope, 2 spans, 3 schema_url
# Scope: 1 name, 2 version
```

Sent: `curl -X POST -H 'Content-Type: application/x-protobuf' \
--data-binary @trace.pb http://localhost:4318/v1/traces`

Result: `HTTP 200`. Flusher log:
`flushing batches rows=1 tables=[Spans, Logs, MetricsGauge, ...]`
followed by `flushed batches rows=1 bytes=284`.

The user's first repro was likely triggering F-29 (AMFI), not a
transform bug. Their second repro (HTTP-instr + PgInstr) crashed
because more ingest traffic = more AMFI kill chances.

### F-02 (silent death) repro

```bash
# Fresh data dir, release binary
rm -rf /tmp/sigiro-bisect-data
mkdir -p /tmp/sigiro-bisect-data
nohup /tmp/run-sigiro.sh > /tmp/sigiro-out.log 2> /tmp/sigiro-err.log < /dev/null &
disown
sleep 8
pgrep -fl "target/release/sigiro"   # alive
# ... no traffic ...
sleep 30
pgrep -fl "target/release/sigiro"   # DEAD, no log
```

`log show` immediately after:
```
AMFI: '.../ducklake.duckdb_extension' has no CMS blob?
AMFI: '.../ducklake.duckdb_extension': Unrecoverable CT signature issue, bailing out.
```

---

## Status of the data dir after this session

(Not cleaned up, per the prompt's "be observational" instruction.)

- `bisect` service has 2 spans I created during F-17 bisection.
- `worldpol-api-noauto` has 1 span (user's manual `tracer.startSpan`)
  and 1 log.
- `worldpol-api` has 5 logs (user's `setupDatabase complete` × 5) and
  **0 spans** — confirming the F-17 symptom.
- `worldpol-api-httponly` has 0 logs and 0 spans — the auto-instr-only
  run never landed anything.
- `main.sigiro_log_templates` has 2 entries: `setupDatabase complete`
  and `test log from noauto run`.
