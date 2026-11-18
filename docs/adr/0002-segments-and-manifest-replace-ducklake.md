# Parquet segments and a manifest replace DuckLake everywhere

Both hosted Sigiro and the self-hosted binary store telemetry as immutable Parquet segments in a bucket or on local disk, listed by a Sigiro-owned manifest. A shared Rust crate defines OTLP decoding, the table schema, the segment writer, and the manifest format. It compiles natively into the binary and to wasm for ingest cells. DuckLake is retired: its PostgreSQL catalog cannot be reached from Workers, and its signed-extension and prepared-statement problems blocked production. One format also lets the binary and a hosted tenant read each other's data.

## Considered Options

- DuckLake with a per-tenant SQLite catalog file in R2: keeps the format but needs a writable catalog that DuckDB-wasm in Workers cannot share safely.
- Separate TypeScript ingest with only shared contract tests: two writers of one format would drift.
