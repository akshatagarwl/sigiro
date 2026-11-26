---
"sigiro": minor
---

Ingest is acknowledged only after its rows are committed. A `200` (OTLP/HTTP) or `OK` (gRPC) now means the rows are stored and survive a restart; a request that cannot be committed gets `503` with `Retry-After: 1` (gRPC `UNAVAILABLE`) and nothing from it is stored. Small commits are inlined into the local DuckLake catalog instead of written as one Parquet file each, so a local install no longer piles up thousands of tiny files.

A failing DuckLake `CHECKPOINT` no longer stops ingest. It is retried after 30 seconds and then less often, falls back to bounded steps when it keeps failing, and runs with at most 2 threads. `/health` reports each project's last checkpoint, failures, data files and inlined rows under `storage`, also exported as `sigiro.storage.*` metrics.

Queued ingest is bounded in bytes (`SIGIRO_INGEST_QUEUE_BYTES`, `SIGIRO_INGEST_PROJECT_QUEUE_BYTES`) and checkpoints run every `SIGIRO_CHECKPOINT_INTERVAL_SECS`. Removed: `SIGIRO_FLUSH_INTERVAL_SECS`, `SIGIRO_FLUSH_ROW_THRESHOLD`, `SIGIRO_FLUSH_BYTE_THRESHOLD`, `SIGIRO_BACKPRESSURE_POLICY` (no more `drop-newest`) and `SIGIRO_MAX_CHECKPOINT_FAILURES`. Log ingest no longer gets `429` while alerts are being evaluated. Idle project writers now release their connection, and shutdown stops every writer within one `SIGIRO_SHUTDOWN_TIMEOUT_SECS` deadline.
